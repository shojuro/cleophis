#!/usr/bin/env python3
"""Adapter-v2 contract dataset generator (S1).

Authors synthetic chat examples that teach a LoRA adapter to obey
`contracts/prompt-contract.v1.toml`: answer strictly from the numbered sources
citing `[n]`, and produce the refusal-with-offer when the sources don't cover
the question (or on `[[NO_EVIDENCE]]`). Plus non-grounded examples (tutor
prompt, no sources) so the composed adapter conditions on the system prompt
and never imposes "refuse/cite" on ordinary chat.

The system message is assembled BYTE-IDENTICALLY to the runtime's
`retrieve.rs::assemble()`:  system_contract [+ doc_context] + sources_block
(sources via a faithful single-pass render of the contract's citation_line).
So the adapter trains on exactly the format the app sends.

Env: reads DEEPSEEK_API (or DEEPSEEK_API_KEY) from tools/pipeline/.env.
Output: JSONL {messages:[system,user,assistant], source, category} to --out.

  ./.venv/bin/python3 ../dataset/gen_contract_v1.py --n 1 --out work/contract_pilot.jsonl   # pilot: 1 per cell
"""
import argparse, json, os, re, sys, time, urllib.request, urllib.error

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
ENV = os.path.join(REPO, "tools", "pipeline", ".env")
CONTRACT_TOML = os.path.join(REPO, "contracts", "prompt-contract.v1.toml")
CATALOG = os.path.join(REPO, "src-tauri", "resources", "catalog.json")
DEEPSEEK_URL = "https://api.deepseek.com/chat/completions"

DOMAINS = ["math", "history", "science", "medical-reference"]
MODES = ["cite", "summarize", "refuse_uncovered", "no_evidence", "non_grounded"]
# Weighted toward ANSWERS (the over-refusal fix): 60% cited answers
# (cite+summarize), 30% refusals (uncovered+no-evidence), 10% non-grounded.
MODE_WEIGHTS = {"cite": 0.40, "summarize": 0.20, "refuse_uncovered": 0.20,
                "no_evidence": 0.10, "non_grounded": 0.10}


def envval(*names):
    for line in open(ENV):
        line = line.strip()
        for n in names:
            if line.startswith(n + "="):
                return line.split("=", 1)[1].strip().strip('"').strip("'")
    return None


def load_contract():
    # Minimal TOML read of the fields we need (values are triple-quoted or "quoted").
    txt = open(CONTRACT_TOML, encoding="utf-8").read()
    def field(key):
        m = re.search(rf'^{key}\s*=\s*"""(.*?)"""', txt, re.S | re.M)
        if m: return m.group(1).strip()
        m = re.search(rf'^{key}\s*=\s*"(.*?)"', txt, re.M)
        if m: return m.group(1)
        raise SystemExit(f"contract: missing {key}")
    return {
        "system_contract": field("system_contract"),
        "citation_line": field("citation_line"),
        "citation_source_sep": field("citation_source_sep"),
        "no_evidence_marker": field("no_evidence_marker"),
        "refusal_with_offer": field("refusal_with_offer"),
    }


def tutor_system_prompt():
    cat = json.load(open(CATALOG, encoding="utf-8"))
    hero = next(m for m in cat if m.get("real"))
    return hero["systemPrompt"]


def substitute_line(template, n, source, text):
    """Faithful single-pass {n}/{source}/{text} substitution (matches
    kpack-core::contract::substitute_line — substituted text is never re-scanned)."""
    out, rest = [], template
    while True:
        i = rest.find("{")
        if i < 0:
            out.append(rest); break
        out.append(rest[:i]); after = rest[i + 1:]
        for ph, val in (("n}", n), ("source}", source), ("text}", text)):
            if after.startswith(ph):
                out.append(val); rest = after[len(ph):]; break
        else:
            out.append("{"); rest = after
    return "".join(out)


def render_sources(chunks, c):
    lines = []
    for i, ch in enumerate(chunks):
        source = c["citation_source_sep"].join(
            p for p in [ch.get("title", ""), ch.get("section", ""), ch.get("locator", "")] if p)
        lines.append(substitute_line(c["citation_line"], str(i + 1), source, ch["text"]))
    return "\n".join(lines)


def doc_context(chunks):
    titles = []
    for ch in chunks:
        t = (ch.get("title") or "").strip()
        if t and t not in titles:
            titles.append(t)
    return "" if not titles else "These sources are excerpts from: " + "; ".join(titles) + "."


def assemble_system(c, chunks, no_evidence):
    """Byte-identical to retrieve.rs::assemble(): system_contract [+ doc_context]
    + sources_block. On no_evidence the sources block is the marker."""
    sc = c["system_contract"].rstrip()
    block = c["no_evidence_marker"] if no_evidence else render_sources(chunks, c)
    dc = "" if no_evidence else doc_context(chunks)
    return f"{sc}\n\n{block}" if not dc else f"{sc}\n\n{dc}\n\n{block}"


def deepseek(key, system, user, temperature=0.9, max_tokens=1500, retries=3):
    body = json.dumps({"model": "deepseek-chat", "temperature": temperature,
                       "max_tokens": max_tokens, "response_format": {"type": "json_object"},
                       "messages": [{"role": "system", "content": system},
                                    {"role": "user", "content": user}]}).encode()
    req = urllib.request.Request(DEEPSEEK_URL, data=body,
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"})
    for a in range(retries):
        try:
            r = json.load(urllib.request.urlopen(req, timeout=120))
            return json.loads(r["choices"][0]["message"]["content"])
        except (urllib.error.HTTPError, urllib.error.URLError, json.JSONDecodeError, KeyError) as e:
            if a == retries - 1:
                raise
            time.sleep(2 * (a + 1))


GEN_SYS = (
    "You are a data engineer building a fine-tuning set that teaches a small model a strict "
    "retrieval-grounding CONTRACT. Output ONLY a single JSON object, no prose. Be realistic and "
    "domain-accurate; vary phrasing. The `text` fields are excerpt bodies (2-5 sentences), NOT "
    "including their titles."
)

def gen_instruction(domain, mode, c):
    common = (
        f'Domain: {domain}. Produce JSON with keys: "sources" (array of '
        '{"title","section","locator","text"}), "question" (a student/user question), '
        'and "answer" (the ideal assistant reply). '
    )
    if mode == "cite":
        return common + (
            "The sources MUST contain the information needed to answer. The answer must be a real, "
            "helpful answer drawn ONLY from the sources, and cite each supporting source inline as "
            "[1], [2], etc. (real indices). 3-5 sources. Do NOT use outside knowledge.")
    if mode == "summarize":
        return common + (
            'The question asks to "summarize" or give "the key points / bullet points" of the sources. '
            "The answer must synthesize across the sources into a concise summary, citing [n] for each "
            "point. 3-6 sources on one coherent topic.")
    if mode == "refuse_uncovered":
        return common + (
            "The sources are on a RELATED topic but genuinely DO NOT contain what the question asks "
            "(a real hard negative). The answer must DECLINE — state plainly that the provided sources "
            "don't cover it and do not guess or use outside knowledge — in the spirit of: "
            f'"{c["refusal_with_offer"].strip()}". 2-4 sources.')
    if mode == "no_evidence":
        return (
            f"Domain: {domain}. There are NO sources (the retrieval gate found nothing). Produce JSON "
            'with keys "question" (a user question) and "answer". Set "sources" to []. The answer MUST '
            f'be the refusal-with-offer, essentially: "{c["refusal_with_offer"].strip()}" (you may '
            "lightly vary wording but keep the meaning: no material on that in this pack; offer to say "
            "what it does cover or to add a source and rebuild).")
    if mode == "non_grounded":
        return (
            f"Domain: {domain}. This is an ORDINARY tutoring chat with NO attached sources. Produce JSON "
            'with keys "question" (a student question) and "answer" (a normal, helpful, honest tutor '
            'answer). Set "sources" to []. Do NOT mention sources, citations, or refusing — just teach.')
    raise ValueError(mode)


def validate(ex, mode):
    msgs = ex["messages"]
    assert len(msgs) == 3 and [m["role"] for m in msgs] == ["system", "user", "assistant"]
    a = msgs[2]["content"]
    if mode in ("cite", "summarize"):
        assert re.search(r"\[\d+\]", a), "cited answer must contain [n] citations"
    if mode == "no_evidence":
        assert msgs[0]["content"].rstrip().endswith("[[NO_EVIDENCE]]")
    return True


def gen_one(key, c, tutor, domain, mode, temperature):
    g = deepseek(key, GEN_SYS, gen_instruction(domain, mode, c), temperature)
    chunks = g.get("sources") or []
    if mode == "no_evidence":
        system = assemble_system(c, [], no_evidence=True)
    elif mode == "non_grounded":
        system = tutor
    else:
        if not chunks:
            raise ValueError("grounded modes need sources")
        system = assemble_system(c, chunks, no_evidence=False)
    ex = {"messages": [{"role": "system", "content": system},
                       {"role": "user", "content": g["question"].strip()},
                       {"role": "assistant", "content": g["answer"].strip()}],
          "source": "synthetic_deepseek", "category": f"contract_{mode}_{domain}"}
    validate(ex, mode)
    return ex


def main():
    import random
    from concurrent.futures import ThreadPoolExecutor, as_completed
    ap = argparse.ArgumentParser()
    ap.add_argument("--total", type=int, default=3000, help="target example count (weighted by mode)")
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--eval-frac", type=float, default=0.05)
    ap.add_argument("--out-dir", default="work/contract-v1")
    ap.add_argument("--temperature", type=float, default=0.9)
    ap.add_argument("--n", type=int, help="pilot: fixed count per (domain,mode) cell, one file")
    ap.add_argument("--out", help="pilot: single output file (implies --n)")
    args = ap.parse_args()

    key = envval("DEEPSEEK_API", "DEEPSEEK_API_KEY") or sys.exit("no DEEPSEEK_API in .env")
    c = load_contract()
    tutor = tutor_system_prompt()

    # Build the weighted (domain, mode) task list.
    if args.out:  # pilot mode: fixed n per cell
        n = args.n or 1
        tasks = [(d, m) for d in DOMAINS for m in MODES for _ in range(n)]
    else:
        tasks = []
        for mode, w in MODE_WEIGHTS.items():
            per_cell = max(1, round(args.total * w) // len(DOMAINS))
            for d in DOMAINS:
                tasks += [(d, mode)] * per_cell
    random.seed(7)
    random.shuffle(tasks)
    print(f"generating {len(tasks)} examples with {args.workers} workers...")

    rows, n_fail = [], 0
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        futs = {pool.submit(gen_one, key, c, tutor, d, m, args.temperature): (d, m) for d, m in tasks}
        for i, fut in enumerate(as_completed(futs), 1):
            try:
                rows.append(fut.result())
            except Exception as e:
                n_fail += 1
                if n_fail <= 20:
                    print(f"  FAIL: {type(e).__name__}: {str(e)[:120]}")
            if i % 100 == 0:
                print(f"  {i}/{len(tasks)} done, {n_fail} failed")

    # Dedup by the user question (case-insensitive).
    seen, dedup = set(), []
    for r in rows:
        q = r["messages"][1]["content"].strip().lower()
        if q not in seen:
            seen.add(q)
            dedup.append(r)
    print(f"{len(rows)} generated, {len(dedup)} after dedup, {n_fail} failed")

    if args.out:  # pilot: one file
        os.makedirs(os.path.dirname(os.path.abspath(args.out)) or ".", exist_ok=True)
        with open(args.out, "w", encoding="utf-8") as f:
            for r in dedup:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        print(f"wrote {len(dedup)} to {args.out}")
        return

    random.shuffle(dedup)
    n_eval = max(1, int(len(dedup) * args.eval_frac))
    ev, tr = dedup[:n_eval], dedup[n_eval:]
    os.makedirs(args.out_dir, exist_ok=True)
    for name, data in (("final_train.jsonl", tr), ("final_eval.jsonl", ev)):
        with open(os.path.join(args.out_dir, name), "w", encoding="utf-8") as f:
            for r in data:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
    # category histogram
    from collections import Counter
    hist = Counter(r["category"].rsplit("_", 1)[0] for r in dedup)
    print(f"wrote {len(tr)} train + {len(ev)} eval to {args.out_dir}")
    print("by mode:", dict(hist))


if __name__ == "__main__":
    main()
