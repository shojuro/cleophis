#!/usr/bin/env python3
"""clusters.py — Phase 1h Task M4a: entity clusters over the NHS reference corpus.

Reads the committed corpus (`tools/reference/corpus/`), groups its entries
into ENTITY CLUSTERS, splits the clusters into disjoint sets, and writes
`tools/reference/reference-clusters.json` — the file the triage repo copies
(with its sha) to cut training rows (A33) and the reference probe bank from
DISJOINT clusters.

WHAT A CLUSTER IS. One entity and every page about it:
  1. an entry's slug and its variants — `ibuprofen-for-adults` and
     `ibuprofen-for-children` share the core `ibuprofen` (medicine qualifiers
     "for ..." and form words "tablets/cream/inhaler/..." are stripped;
     "in children/adults/babies/pregnancy" is stripped in both sections);
  2. every other name the NHS gives the entry — its title, the title's
     parenthetical ("Aciclovir (Zovirax)") and the index's "X, see Y"
     aliases (kept whole: "Eczema (atopic)" is not "Eczema"). The "Other
     common brands" list is recorded as names but does NOT merge: umbrella
     brands span different medicines;
  3. any page whose TITLE names the same entity — a medicine's core name
     inside another title ("Low-dose aspirin" joins "Aspirin for pain
     relief"), and a MULTI-word condition name inside another title. A
     single-word condition name ("Cancer", "Diabetes", "Stroke") does NOT
     absorb every title containing it: that would make one giant cluster
     of every cancer and wreck the split, and those pages are distinct
     entities that merely share a head noun.
Entries sharing any name key (length >= 3) are merged (union-find).

THE SPLIT. Each cluster is assigned by a SEEDED HASH of its id (the
smallest `section/slug` in it) to `rows` / `probe` / `calibration` at
70/15/15. The seed is `SEED` below and is written into the output. A
cluster's set depends only on its own id, so adding pages to the corpus
never reshuffles existing clusters.

THE HELD-OUT SETS.
  * `fabrication-heldout` — every cluster containing a REAL entity of the
    triage repo's fabrication bank (`probes/items/prohibitions.json`,
    `probe-fabrication`, read-only) is forced here and excluded from all
    three sets. A real name matches an entry whose title/alias/brand
    contains it as a whole phrase (apostrophes and case ignored), or equals
    it once a trailing "syndrome/disease/phenomenon/disorder" is dropped
    ("Raynaud's syndrome" = the page "Raynaud's"). A real entity with NO
    page of its own holds out every cluster whose text MENTIONS it; failing
    that, the committed `FABRICATION_FALLBACKS` table maps it to a covering
    page or declares it ABSENT (re-verified on every build), with the
    reason recorded. A real entity resolved by none of these is an ERROR.
  * Every FAKE name of the bank must be absent from the whole corpus
    (titles, aliases, brands and bodies) — an ERROR otherwise, because a
    "fake" the reference pack can retrieve is not a fabrication probe.
  * `held_out_family: true` — a cluster whose pages read as one of the
    triage partition's HELD-OUT families (`probes/items/partition.json`
    is the authority: whichever families it marks `heldOut`). The row
    builder excludes these. The tag comes from a page-level keyword hint
    (`PAGE_LEXICON`: title/alias hits weigh 2, lede/indication hits 1, a
    family counts at >= 2) and is deliberately CONSERVATIVE — any held-out
    family over threshold tags the cluster, even when another family
    scores higher. It is a hint, not a clinical classification; the row
    builder still sweeps row text with the triage taxonomy.
  * `fabrication_mentions` — per cluster, which REAL bank names occur
    anywhere in its text, for the row builder's bank sweep (A28/A33): a
    row mentioning a held-out entity is dropped even when its page is not
    held out.

Usage:
    python3 tools/reference/clusters.py            # write reference-clusters.json
    python3 tools/reference/clusters.py --check    # exit 1 if the committed file is stale
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import unicodedata
from pathlib import Path

REFERENCE_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(REFERENCE_ROOT))

import fetch_nhs as fn  # noqa: E402

DEFAULT_CORPUS = REFERENCE_ROOT / "corpus"
DEFAULT_OUT = REFERENCE_ROOT / "reference-clusters.json"
DEFAULT_PROHIBITIONS = Path("/home/penguinzyue/cleophas-triage/probes/items/prohibitions.json")
DEFAULT_PARTITION = Path("/home/penguinzyue/cleophas-triage/probes/items/partition.json")

SCHEMA = "cleophis/reference-clusters/v1"
SEED = "cleophis-m4a-reference-clusters-2026-09-27"
RATIOS = (("rows", 0.70), ("probe", 0.15), ("calibration", 0.15))
HELDOUT = "fabrication-heldout"
MIN_KEY_LEN = 3

# Real fabrication-bank entities with no NHS page of their own, mapped to the
# page(s) that cover them. Every target must exist in the corpus.
# Resolution order for each real name: (1) an entry NAMED for it; else (2)
# every entry whose text MENTIONS it; else (3) this table. A table value of
# [] declares the entity ABSENT from the corpus — accepted only while it is
# neither named nor mentioned anywhere (re-checked on every build). A real
# name resolved by none of these is an error.
FABRICATION_FALLBACKS: dict[str, list[str]] = {
    "guttate psoriasis": ["conditions/psoriasis"],
    "benzocaine": [],
    "farmer's lung": [],
    "interstitial nephritis": [],
}
FABRICATION_FALLBACK_REASONS: dict[str, str] = {
    "guttate psoriasis": "No NHS page and no mention; guttate psoriasis is a type of psoriasis, so the "
                         "parent Psoriasis page is held out (conservative).",
    "benzocaine": "Not in the Medicines A-Z and mentioned nowhere in the corpus (checked 2026-09-27).",
    "farmer's lung": "No Health A-Z page and mentioned nowhere in the corpus (checked 2026-09-27).",
    "interstitial nephritis": "No Health A-Z page and mentioned nowhere in the corpus (checked 2026-09-27); "
                              "glomerulonephritis is a different condition and is not held out.",
}


class ClusterError(RuntimeError):
    pass


# ---------------------------------------------------------------------------
# Names and keys.
# ---------------------------------------------------------------------------


def norm(s: str) -> str:
    s = unicodedata.normalize("NFKD", s)
    s = "".join(ch for ch in s if not unicodedata.combining(ch)).lower()
    s = s.replace("’", "").replace("'", "").replace("‘", "")
    return re.sub(r"[^a-z0-9]+", " ", s).strip()


_PAREN = re.compile(r"\(([^)]*)\)")
_FORM = re.compile(
    r"\s+(?:tablets?|capsules?|creams?|gels?|ointments?|eye drops|ear drops|drops|inhalers?|injections?|"
    r"nasal sprays?|sprays?|skin treatments?|skin creams?|liquid|patches|suppositories|mouthwash|shampoo|"
    r"lozenges|granules|medicines?)$"
)
_FOR = re.compile(r"\s+for\s+.*$")
_IN_GROUP = re.compile(r"\s+(?:in|during)\s+(?:children|adults|babies|pregnancy|older people|men|women|teenagers|young people)$")
_GENERIC_TAIL = re.compile(r"\s+(?:syndrome|disease|phenomenon|disorder)$")


def core(name: str, section: str) -> str:
    n = norm(_PAREN.sub(" ", name))
    if section == "medicines":
        n = _FOR.sub("", n)
        prev = None
        while prev != n:
            prev, n = n, _FORM.sub("", n)
    return _IN_GROUP.sub("", n).strip()


def entry_names(fm: dict) -> list[str]:
    """Display names of an entry: title, its parenthetical(s), aliases, brands, slug."""
    names = [fm["title"]]
    names += [p.strip() for p in _PAREN.findall(fm["title"]) if p.strip()]
    names += list(fm.get("aliases") or []) + list(fm.get("brands") or [])
    names.append(fm["slug"].replace("--", " ").replace("-", " "))
    out = []
    for n in names:
        if n and n not in out:
            out.append(n)
    return out


def merge_keys(fm: dict, section: str) -> set[str]:
    """The keys that MERGE entries. Title, the title's parenthetical(s) and
    the slug go through `core` (variant qualifiers stripped). Index aliases
    keep their qualifier — the Health A-Z writes inverted names like "Eczema
    (atopic)" and stripping the parenthetical would collapse every eczema
    onto "eczema". Brands are NOT merge keys: umbrella brands (Pollenase,
    Nurofen, Sudafed...) span different active ingredients."""
    keys = {core(fm["title"], section), core(fm["slug"].replace("--", " ").replace("-", " "), section)}
    keys |= {core(p, section) for p in _PAREN.findall(fm["title"]) if p.strip()}
    keys |= {norm(a) for a in (fm.get("aliases") or [])}
    return keys


def _phrase_in(needle: str, hay: str) -> bool:
    return bool(needle) and f" {needle} " in f" {hay} "


# ---------------------------------------------------------------------------
# Family hint.
# ---------------------------------------------------------------------------

PAGE_LEXICON: dict[str, list[str]] = {
    "cardiac": [
        r"heart", r"cardiac", r"cardio\w*", r"angina", r"arrhythmias?", r"atrial fibrillation", r"palpitations?",
        r"high blood pressure", r"hypertension", r"aortic", r"aneurysms?", r"raynauds?", r"endocarditis",
        r"pericarditis", r"myocarditis", r"coronary", r"chest pain", r"varicose veins", r"deep vein thrombosis",
        r"tachycardia", r"bradycardia", r"peripheral arterial disease", r"heart failure", r"heart attack",
    ],
    "respiratory": [
        r"lungs?", r"asthma", r"copd", r"pneumonia", r"bronchitis", r"bronchiectasis", r"bronchiolitis", r"cough\w*",
        r"shortness of breath", r"breathing", r"breathless\w*", r"tuberculosis", r"pleurisy", r"emphysema",
        r"pulmonary", r"respiratory", r"croup", r"sarcoidosis", r"pneumothorax", r"cystic fibrosis", r"sleep apnoea",
        r"wheez\w*", r"inhalers?", r"mesothelioma", r"chest infections?",
    ],
    "neurological": [
        r"brain", r"nerves?", r"neuralgia", r"palsy", r"stroke", r"epilepsy", r"seizures?", r"migraines?",
        r"headaches?", r"multiple sclerosis", r"parkinsons", r"dementia", r"alzheimers", r"neuropathy",
        r"meningitis", r"encephalitis", r"neuritis", r"motor neurone", r"spinal cord", r"concussion",
        r"head injury", r"tremors?", r"ataxia", r"guillain barre", r"hydrocephalus", r"transient ischaemic attack",
        r"narcolepsy", r"restless legs", r"neurolog\w*", r"numbness", r"tingling",
    ],
    "abdominal": [
        r"stomach", r"bowel", r"abdominal", r"abdomen", r"tummy", r"gut", r"liver", r"hepatitis", r"cirrhosis",
        r"gallstones?", r"gallbladder", r"crohns", r"colitis", r"ibs", r"coeliac", r"appendicitis", r"hernias?",
        r"reflux", r"heartburn", r"indigestion", r"constipation", r"diarrhoea", r"vomiting", r"nausea",
        r"pancreatitis", r"pancreas", r"diverticul\w*", r"haemorrhoids", r"piles", r"oesophag\w*", r"gastro\w*",
        r"anal", r"rectal", r"colon", r"bloating", r"food poisoning", r"norovirus",
    ],
    "dermatological": [
        r"skin", r"rash\w*", r"eczema", r"dermatitis", r"psoriasis", r"acne", r"rosacea", r"urticaria", r"hives",
        r"warts?", r"verruca\w*", r"moles?", r"melanoma", r"impetigo", r"cellulitis", r"ringworm", r"athletes foot",
        r"scabies", r"head lice", r"boils?", r"blisters?", r"sunburn", r"itch\w*", r"hair loss", r"alopecia",
        r"nails?", r"vitiligo", r"cold sores?", r"shingles", r"lichen", r"keratosis", r"intertrigo",
        r"prickly heat", r"pemphig\w*", r"hyperhidrosis", r"birthmarks?", r"corns", r"calluses", r"chilblains",
        r"stretch marks", r"scars?", r"keloids?", r"molluscum", r"pityriasis", r"erythema", r"dandruff",
        r"seborrhoeic", r"burns", r"insect bites", r"stings", r"lipomas?", r"hidradenitis", r"fungal",
    ],
    "musculoskeletal": [
        r"arthritis", r"osteoarthritis", r"back pain", r"neck pain", r"joint pain", r"joints?", r"sprains?",
        r"strains?", r"fractures?", r"broken (?:arm|leg|wrist|ankle|toe|finger|collarbone|hip|rib|bone)s?",
        r"carpal tunnel", r"tendon\w*", r"tendin\w*", r"bursitis", r"gout", r"osteoporosis", r"sciatica",
        r"frozen shoulder", r"tennis elbow", r"golfers elbow", r"plantar fasciitis", r"bunions?", r"scoliosis",
        r"fibromyalgia", r"muscles?", r"bones?", r"ligament\w*", r"cartilage", r"polymyalgia", r"dupuytren\w*",
        r"hypermobility", r"whiplash", r"slipped disc", r"leg cramps", r"osteomyelitis", r"ankylosing spondylitis",
        r"rheumat\w*", r"spondyl\w*", r"trigger finger", r"flat feet", r"heel pain", r"foot pain", r"elbow",
        r"wrist", r"ankle", r"knee", r"hip", r"shoulder",
    ],
    "urinary": [
        r"urinary", r"urine", r"bladder", r"cystitis", r"utis?", r"kidney infections?", r"kidney stones?",
        r"kidneys?", r"nephritis", r"incontinence", r"prostat\w*", r"bedwetting", r"enuresis", r"peeing", r"pee",
        r"haematuria", r"urethr\w*", r"pyelonephritis", r"catheters?", r"renal", r"nephrotic", r"testic\w*",
        r"scrot\w*",
    ],
    "ENT": [
        r"ears?", r"earache", r"earwax", r"hearing", r"deaf\w*", r"tinnitus", r"tonsil\w*", r"throat", r"sinus\w*",
        r"nose", r"nasal", r"nosebleeds?", r"labyrinthitis", r"vertigo", r"menieres", r"glandular fever",
        r"laryng\w*", r"catarrh", r"snoring", r"adenoids?", r"glue ear", r"voice", r"hoarse\w*", r"quinsy",
        r"rhinitis", r"hay fever", r"cholesteatoma", r"otitis", r"acoustic neuroma", r"vestibular", r"pharyng\w*",
        r"mastoiditis", r"eardrum",
    ],
}
_COMPILED = {f: [re.compile(rf"\b(?:{p})\b") for p in pats] for f, pats in PAGE_LEXICON.items()}
FAMILY_MIN_SCORE = 2


def _lede(body: str) -> str:
    """The first prose paragraph plus the first bullet list (for a medicine,
    the "what it is for" indications), with headings and the brand caption skipped."""
    paras, got_list = [], False
    for block in body.split("\n\n"):
        b = block.strip()
        if not b or b.startswith("#") or b.lower().startswith("other common brands"):
            continue
        if b.startswith(("- ", "1. ")):
            if not got_list:
                paras.append(b)
                got_list = True
        elif not [p for p in paras if not p.startswith(("- ", "1. "))]:
            paras.append(b)
        if got_list and len(paras) >= 2:
            break
    return " ".join(paras)


def family_scores(title_text: str, lede_text: str) -> dict[str, dict]:
    t, l_ = norm(title_text), norm(lede_text)
    out = {}
    for fam, pats in _COMPILED.items():
        title_hits = [rx.pattern for rx in pats if rx.search(t)]
        lede_hits = [rx.pattern for rx in pats if rx.search(l_) and rx.pattern not in title_hits]
        score = 2 * len(title_hits) + len(lede_hits)
        if score:
            out[fam] = {"score": score, "title": title_hits, "lede": lede_hits}
    return out


# ---------------------------------------------------------------------------
# Corpus, bank, partition.
# ---------------------------------------------------------------------------


def sha256_file(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def load_corpus(corpus: Path) -> dict[str, dict]:
    idx = json.loads((corpus / "index.json").read_text(encoding="utf-8"))
    out = {}
    for key in sorted(idx["entries"]):
        rec = idx["entries"][key]
        text = (corpus / rec["section"] / f"{rec['slug']}.md").read_text(encoding="utf-8")
        fm, body = fn.split_front_matter(text)
        out[key] = {"fm": fm, "body": body, "section": rec["section"]}
    return out


def load_bank(path: Path) -> list[dict]:
    d = json.loads(path.read_text(encoding="utf-8"))
    pairs = d["probe-fabrication"]["pairs"]
    for p in pairs:
        if not (p.get("real") and p.get("fake")):
            raise ClusterError(f"fabrication pair {p.get('id')} lacks a real or fake name")
    return pairs


def load_held_out_families(path: Path) -> list[str]:
    d = json.loads(path.read_text(encoding="utf-8"))
    return sorted(f for f, a in d["assignment"].items() if a == "heldOut")


# ---------------------------------------------------------------------------
# Build.
# ---------------------------------------------------------------------------


class _UF:
    def __init__(self, keys):
        self.p = {k: k for k in keys}

    def find(self, k):
        while self.p[k] != k:
            self.p[k] = self.p[self.p[k]]
            k = self.p[k]
        return k

    def union(self, a, b):
        ra, rb = self.find(a), self.find(b)
        if ra != rb:
            lo, hi = sorted((ra, rb))
            self.p[hi] = lo


def assign_split(cluster_id: str, seed: str) -> str:
    h = int.from_bytes(hashlib.sha256(f"{seed}\x00{cluster_id}".encode("utf-8")).digest()[:8], "big")
    x = h / 2**64
    acc = 0.0
    for name, share in RATIOS:
        acc += share
        if x < acc:
            return name
    return RATIOS[-1][0]


def build_clusters(corpus: Path, prohibitions: Path, partition: Path, seed: str = SEED,
                   fallbacks: dict[str, list[str]] | None = None) -> dict:
    fallbacks = FABRICATION_FALLBACKS if fallbacks is None else fallbacks
    entries = load_corpus(corpus)
    pairs = load_bank(prohibitions)
    held_out_families = load_held_out_families(partition)

    names = {k: entry_names(e["fm"]) for k, e in entries.items()}
    name_norms = {k: [norm(n) for n in ns] for k, ns in names.items()}
    title_norm = {k: norm(e["fm"]["title"]) for k, e in entries.items()}
    full_text = {
        k: norm(" ".join(names[k]) + " " + e["body"]) for k, e in entries.items()
    }

    # 1+2: shared name keys.
    uf = _UF(entries)
    key_owner: dict[str, str] = {}
    entry_keys: dict[str, set[str]] = {}
    for k, e in entries.items():
        ks = {x for x in merge_keys(e["fm"], e["section"]) if len(x) >= MIN_KEY_LEN}
        entry_keys[k] = ks
        for x in sorted(ks):
            if x in key_owner:
                uf.union(k, key_owner[x])
            else:
                key_owner[x] = k

    # 3: a title naming the same entity.
    for k, e in entries.items():
        section = e["section"]
        own = {core(e["fm"]["title"], section), core(e["fm"]["slug"].replace("-", " "), section)}
        for x in sorted(own):
            if len(x) < MIN_KEY_LEN:
                continue
            if section == "conditions" and len(x.split()) < 2:
                continue  # single-word condition names absorb nothing (see module doc)
            for other, t in title_norm.items():
                if other != k and _phrase_in(x, t):
                    uf.union(k, other)

    groups: dict[str, list[str]] = {}
    for k in entries:
        groups.setdefault(uf.find(k), []).append(k)

    # Fakes must be absent.
    fake_hits: dict[str, list[str]] = {}
    for p in pairs:
        fk = norm(p["fake"])
        hits = [k for k, t in full_text.items() if _phrase_in(fk, t)]
        if hits:
            fake_hits[p["fake"]] = hits
    if fake_hits:
        raise ClusterError(f"fabrication-bank FAKE names found in the corpus: {fake_hits}")

    # Reals: match by name, else fallback, else error.
    real_entries: dict[str, list[str]] = {}
    matched_by: dict[str, str] = {}
    for p in pairs:
        real = p["real"]
        rk = norm(real)
        rloose = _GENERIC_TAIL.sub("", rk)
        hit = sorted(
            k for k in entries
            if any(_phrase_in(rk, n) for n in name_norms[k])
            or any(_GENERIC_TAIL.sub("", n) == rloose for n in name_norms[k])
        )
        mentioned = sorted(k for k in entries if _phrase_in(rk, full_text[k]))
        declared = real in fallbacks
        fb = list(fallbacks.get(real, []))
        for t in fb:
            if t not in entries:
                raise ClusterError(f"fallback for {real!r} names {t!r}, which is not in the corpus")
        if hit:
            chosen, how = set(hit) | set(fb), ("name+fallback" if fb else "name")
        elif mentioned:
            # no page of its own: every page that mentions it is held out
            chosen, how = set(mentioned) | set(fb), ("mention+fallback" if fb else "mention")
        elif fb:
            chosen, how = set(fb), "fallback"
        elif declared:
            chosen, how = set(), "absent"  # declared absent AND verified: no page, no mention
        else:
            raise ClusterError(
                f"fabrication-bank REAL entity {real!r} matches no corpus entry, is mentioned nowhere, "
                "and is not declared in FABRICATION_FALLBACKS"
            )
        real_entries[real] = sorted(chosen)
        matched_by[real] = how

    heldout_roots = {uf.find(k) for ks in real_entries.values() for k in ks}
    real_keys = {p["real"]: norm(p["real"]) for p in pairs}

    clusters = []
    for root, members in groups.items():
        members = sorted(members)
        cid = members[0]
        split = HELDOUT if root in heldout_roots else assign_split(cid, seed)
        fams: dict[str, int] = {}
        hint, hint_score = None, 0
        held_out = False
        for k in members:
            e = entries[k]
            title_text = " ".join([e["fm"]["title"], *(e["fm"].get("aliases") or [])])
            sc = family_scores(title_text, _lede(e["body"]))
            for f, s in sc.items():
                if s["score"] >= FAMILY_MIN_SCORE:
                    fams[f] = max(fams.get(f, 0), s["score"])
                    if f in held_out_families:
                        held_out = True
        if fams:
            hint, hint_score = sorted(fams.items(), key=lambda kv: (-kv[1], kv[0]))[0]
        mentions = sorted(r for r, rk in real_keys.items() if any(_phrase_in(rk, full_text[k]) for k in members))
        clusters.append({
            "id": cid,
            "split": split,
            "entries": members,
            "names": sorted({n for k in members for n in names[k]}),
            "family_hint": hint,
            "family_candidates": dict(sorted(fams.items())),
            "held_out_family": held_out,
            "fabrication_real": sorted(r for r, ks in real_entries.items() if set(ks) & set(members)),
            "fabrication_mentions": mentions,
        })
    clusters.sort(key=lambda c: c["id"])
    entry_to_cluster = {k: c["id"] for c in clusters for k in c["entries"]}

    sets = {name: [] for name, _ in RATIOS}
    sets[HELDOUT] = []
    for c in clusters:
        sets[c["split"]].append(c["id"])
    counts = {}
    for s, ids in sets.items():
        cs = [c for c in clusters if c["split"] == s]
        counts[s] = {
            "clusters": len(ids),
            "entries": sum(len(c["entries"]) for c in cs),
            "held_out_family_clusters": sum(1 for c in cs if c["held_out_family"]),
        }
    return {
        "schema": SCHEMA,
        "seed": seed,
        "ratios": dict(RATIOS),
        "inputs": {
            "corpus_index_sha256": sha256_file(corpus / "index.json"),
            "prohibitions_sha256": sha256_file(prohibitions),
            "partition_sha256": sha256_file(partition),
            "prohibitions": "cleophas-triage probes/items/prohibitions.json (probe-fabrication)",
            "partition": "cleophas-triage probes/items/partition.json",
        },
        "held_out_families": held_out_families,
        "counts": counts,
        "fabrication": {
            "real_to_clusters": {r: sorted({entry_to_cluster[k] for k in ks}) for r, ks in sorted(real_entries.items())},
            "real_to_entries": dict(sorted(real_entries.items())),
            "matched_by": dict(sorted(matched_by.items())),
            "fallback_reasons": {r: FABRICATION_FALLBACK_REASONS.get(r, "") for r in sorted(fallbacks) if r in real_entries},
            "fakes_checked": sorted(p["fake"] for p in pairs),
            "fake_hits": fake_hits,
        },
        "sets": sets,
        "clusters": clusters,
        "entry_to_cluster": dict(sorted(entry_to_cluster.items())),
    }


def render(result: dict) -> str:
    return json.dumps(result, indent=1, sort_keys=True, ensure_ascii=False) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Build tools/reference/reference-clusters.json from the committed corpus.")
    ap.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    ap.add_argument("--prohibitions", type=Path, default=DEFAULT_PROHIBITIONS)
    ap.add_argument("--partition", type=Path, default=DEFAULT_PARTITION)
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--check", action="store_true", help="exit 1 if --out differs from a fresh build")
    args = ap.parse_args(argv)
    try:
        text = render(build_clusters(args.corpus, args.prohibitions, args.partition))
    except ClusterError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    if args.check:
        same = args.out.is_file() and args.out.read_text(encoding="utf-8") == text
        print(f"[check] {args.out} is {'up to date' if same else 'STALE'}")
        return 0 if same else 1
    args.out.write_text(text, encoding="utf-8")
    r = json.loads(text)
    print(json.dumps({"counts": r["counts"], "matched_by": r["fabrication"]["matched_by"]}, indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
