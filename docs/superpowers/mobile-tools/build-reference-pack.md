# Build the bundled NHS reference pack

The floor-tier triage build ships a bundled, signed knowledge pack of NHS
Medicines A-Z and Health A-Z content. The phone looks entities up in it
lexically (no embedder on Android). Training rows (A33) and the reference
probe bank are cut from disjoint entity clusters of the same corpus.

This runbook covers the whole path. Steps 1, 2, 4 and 5 exist today (Phase
1h Task M4a). Step 3, the pack build, is Task M4b. Step 6, the embed, is
Task M5.

| Step | Who | Tool | Output |
|---|---|---|---|
| 1. Fetch the corpus | agent or founder | `tools/reference/fetch_nhs.py` | `tools/reference/corpus/` (committed) |
| 2. Build the clusters | agent or founder | `tools/reference/clusters.py` | `tools/reference/reference-clusters.json` (committed) |
| 3. Build the pack | agent or founder | `crates/kpack-cli` (M4b) | `reference.kpack`, `chunks.jsonl`, `titles.json` |
| 4. Sign the pack | **founder only** | `tools/pipeline/sign_pack.py` | `reference.kpack.sig` |
| 5. Verify the signature | anyone | `tools/pipeline/sign_pack.py --verify-with` | exit 0 |
| 6. Embed the pack | agent or founder | M5's resource embed | the triage build variant |

## Licence and terms

Checked on 2026-09-27 by reading the live pages.

- **Terms.** NHS website terms and conditions,
  https://www.nhs.uk/our-policies/terms-and-conditions/. The page's own
  review dates say "last reviewed 12 January 2021" and "next review due 12
  January 2024". The review is overdue, but this is the live, current terms
  page.
- **Exclusions.** "Content not covered by our standard terms and
  conditions",
  https://www.nhs.uk/our-policies/terms-and-conditions/content-not-licensed-for-re-use/,
  last reviewed 17 April 2024.
- **Robots.** https://www.nhs.uk/robots.txt, fetched 2026-09-27.

What the terms say:

- **§3.3 and §3.4.** Copyright and database rights in NHS Website Content
  are released under the current Open Government Licence. You may copy it,
  adapt it, and use it for any purpose, including commercially, if you
  follow the terms and the OGL.
- **No bar on programmatic reuse.** Nothing in the terms forbids automated
  retrieval or requires registration. The syndication API is one route,
  not a requirement. §12.2(b) forbids use that could overburden the site,
  so the fetcher is rate-limited.
- **§3.5.** The OGL grant does not cover logos, visuals, trademarks,
  personal data, third-party material, or the site's medical devices.
  Stock-licensed images are called out. The corpus keeps no images, no
  embeds and no interactive tools.
- **Excluded sites.** Six sub-sites are not under the standard terms:
  Change4Life, Best Start in Life, Be Clear on Cancer, One You, Smokefree
  and Quit. The fetcher refuses those paths.
- **§3.6(a), unchanged copies.** Attribute "Information from the NHS
  website". If the copy is not refreshed at least every 7 days, add "as at
  DDMMYY". Where a link is possible, link each attribution to the source
  page. Also publish, somewhere prominent, the statement "Information from
  the NHS website is licensed under the Open Government Licence v3.0",
  linked to the OGL where possible.
- **§3.6(b), adapted content.** Content that is adapted, or taken out of a
  context important to its meaning, is attributed "Contains public sector
  information licensed under the Open Government Licence v3.0." It must not
  be attributed to the NHS website. The terms warn that adaptation may void
  the content's clinical approval.
- **§3.7.** The NHS recommends refreshing copies every 24 hours. A bundled
  pack cannot do that, so it is a non-refreshed copy under §3.6(a).
- **§3.10 and §3.11.** Access to NHS content may not carry its own charge.
  Nothing may imply NHS endorsement of the app.
- **Robots.** The file disallows `/Conditions/` in capitals. Robots paths
  are case-sensitive (RFC 9309), and the Health A-Z lives at lowercase
  `/conditions/`, so it is allowed. The fetcher checks every URL against
  the live robots file.

### What this means for the product

- **Displayed excerpts.** When the app shows retrieved text, it is an
  unchanged copy. Show "Information from the NHS website, as at DDMMYY"
  with a link to the source page. Each corpus file carries the page URL,
  and each sub-page's URL is in its `sources` list.
- **About screen.** Carry the §3.6(a) statement in a prominent place, with
  the OGL link.
- **Model output and training rows.** A model answer that paraphrases NHS
  text is adapted content. It carries the §3.6(b) wording and must not say
  "the NHS says". M5 and M6 own the display rule. This is flagged for the
  founder.
- **Refresh.** Re-run steps 1 to 6 to refresh the pack. The `as_at` date
  moves with the fetch.

Every corpus file's front matter carries these fields:

```yaml
title: "Ibuprofen for adults (Nurofen)"
url: "https://www.nhs.uk/medicines/ibuprofen-for-adults/"
retrieved: "2026-09-26"        # UTC date of the hub page's fetch
as_at: "260926"                # the same date as DDMMYY, for the §3.6(a) attribution
licence: "OGL v3"
attribution: "Information from the NHS website, licensed under the Open Government Licence v3.0"
section: "medicines"           # or "conditions" (the Health A-Z)
slug: "ibuprofen-for-adults"
aliases: []                    # the index's "X, see Y" names
brands: ["Brufen", "..."]      # "Other common brands"
sources: [{"url": ..., "title": ..., "retrieved": ..., "last_reviewed": ...}]  # hub, then sub-pages
```

Each value is written as JSON. JSON is valid YAML 1.2, and it parses with
`json.loads` line by line.

## 1. Fetch the corpus

```bash
python3 tools/reference/fetch_nhs.py              # network, about 1 hour at 1 request per second
python3 tools/reference/fetch_nhs.py --offline    # re-render from the cache, no network
```

The fetcher reads the Medicines A-Z and Health A-Z indexes. For each entry
it fetches the hub page, then every sub-page linked under the hub's own
path, such as a condition's "Symptoms" or "Treatment" pages. It writes one
markdown file per entry to `tools/reference/corpus/<section>/<slug>.md`.
Sub-pages become `##` sections in their hub's file. It also writes
`tools/reference/corpus/index.json`, which maps `section/slug` to the
title, URL, section and sha256 of the body.

How it behaves:

- **Politeness.** It uses one connection and waits at least 1 second
  between requests. It backs off on 429 and 5xx, honouring Retry-After. It
  sends an identifying User-Agent and requests only `https://www.nhs.uk/`.
- **Cache.** Every response, 404s included, is cached in
  `tools/reference/.cache/`, which is gitignored. A re-run costs no
  network.
- **Extraction.** It keeps the `<main>` article. Headings become markdown
  headings, lists stay lists, and tables become pipe tables. Expanders keep
  their summary in bold. Care cards keep their headings, such as "Urgent
  advice: Call NHS 111 if:". Links keep only their text.
- **What it drops.** Navigation, breadcrumbs, "More in" and related links,
  action links, images, figures, video, iframes, scripts and the "Page last
  reviewed" line. The review dates move to `sources[].last_reviewed`.
- **Determinism.** Entries follow index order and sub-pages follow
  first-link order. The body has no dates. JSON is written with sorted
  keys. An `--offline` re-render is byte-identical.
- **Aliases.** Index entries that redirect to one page collapse into one
  entry, and the extra titles become aliases.

After a full run, commit `tools/reference/corpus/`.

The committed corpus came from the fetch of 2026-09-26 (UTC). That run
made 2,693 network requests in about an hour, with no back-offs.

| Section | Index links | Entries | Pages | Bytes |
|---|---|---|---|---|
| Medicines A-Z | 262 | 259 | 1,365 | 4,446,166 |
| Health A-Z | 682 | 682 | 1,380 | 5,473,256 |
| Total | 944 | 941 | 2,745 | 9,919,422 |

Three Medicines A-Z links do not become entries:

- **Budesonide nasal spray** redirects to the "Medicine page no longer
  available" page, so it is skipped.
- **Budesonide rectal foam and enemas** redirects to the same page, so it
  is also skipped.
- **One index link** redirects to a page another link already reaches. Its
  title becomes an alias on that entry.

## 2. Build the clusters

```bash
python3 tools/reference/clusters.py            # writes tools/reference/reference-clusters.json
python3 tools/reference/clusters.py --check    # exit 1 if the committed file is stale
```

It reads two files from the triage repo, read-only, by absolute path:

- `probes/items/prohibitions.json`, the `probe-fabrication` bank of 25
  real and invented entity pairs.
- `probes/items/partition.json`, which is the authority on held-out
  families.

The rules are documented in full in `clusters.py`'s module docstring:

- **What a cluster is.** One entity: the slug and its variants, the title
  and its parenthetical, and the index's "see" aliases. Aliases are kept
  whole, so "Eczema (atopic)" is not "Eczema". A page whose title names
  the same medicine or the same multi-word condition also joins. Brands
  are recorded as names but never merge entries, because umbrella brands
  such as Pollenase span different drugs.
- **The split.** A seeded hash of the cluster id assigns `rows`, `probe` or
  `calibration` at 70/15/15. The seed is committed in the file.
- **Fabrication held-out.** Every cluster holding a real bank entity goes to
  `fabrication-heldout` and is excluded from all three sets. Each real
  name resolves in order. First, any entry named for it. Failing that,
  every entry whose text mentions it. Failing that, the committed
  `FABRICATION_FALLBACKS` table, which maps it to a covering page or
  declares it absent. An absent declaration is re-verified on every build.
  A real name that resolves nowhere is an error. So is any invented bank
  name found anywhere in the corpus.
- **Held-out families.** `held_out_family: true` marks clusters that read
  as a family the partition holds out. This is a conservative keyword
  hint. The row builder excludes these clusters.
- **Mentions.** `fabrication_mentions` lists the real bank names each
  cluster's text mentions, for the row builder's bank sweep.

The triage repo takes `reference-clusters.json` by copy and pins its sha.

The committed clusters were built from the corpus above with seed
`cleophis-m4a-reference-clusters-2026-09-27`.

| Set | Clusters | Entries | Held-out-family clusters |
|---|---|---|---|
| rows | 617 | 636 | 207 |
| probe | 134 | 140 | 48 |
| calibration | 128 | 137 | 46 |
| fabrication-heldout | 25 | 28 | 11 |

How the 25 real fabrication entities resolved:

- **18 by name.** Examples: "Raynaud's syndrome" is the page "Raynaud's".
  "Interstitial cystitis" is the page "Bladder pain syndrome" through its
  index alias. "Paget's disease" holds out both Paget's pages.
- **3 by mention.** Optic neuritis, seborrhoeic dermatitis and microscopic
  colitis.
- **1 by fallback.** Guttate psoriasis holds out Psoriasis.
- **3 absent.** Benzocaine, farmer's lung and interstitial nephritis have
  no page and are mentioned nowhere.

None of the 25 invented names occurs anywhere in the corpus, not even as a
first word.

## 3. Build the pack (Task M4b)

`crates/kpack-cli` builds a `PackTier::Curated` pack from the committed
corpus with `SOURCE_DATE_EPOCH`. It also writes `chunks.jsonl` and
`titles.json`, the title index. M4b fills in this step. The pack must
rebuild byte-identically from the committed corpus, because the signature
is over its bytes.

## 4. Sign the pack (founder)

The curator private key never leaves the founder's machine. No agent
session ever runs this step.

```bash
python3 tools/pipeline/sign_pack.py --pack path/to/reference.kpack
```

This reads `CURATOR_KEY_FILE` from `tools/pipeline/.env`, the same
contract as `sign_catalog.py`. The key file must be outside the repo and
mode `600`. It writes `path/to/reference.kpack.sig`, a raw 64-byte ed25519
signature over the pack's exact bytes. That is the form
`kpack_core::sign::verify_detached` checks with `verify_strict`.

The script refuses these inputs before reading any key:

- A file that does not end in `.kpack`.
- A missing file.
- A pack over 512 MiB.

## 5. Verify the signature

Verification needs only the public key. The production key is
`kpack_core::sign::CURATOR_PUBLIC_KEY`.

```bash
python3 tools/pipeline/sign_pack.py --pack path/to/reference.kpack \
  --verify-with 158cb99e9756e2e4d01d88b7ecfeb99a76547821ffe9f9f1985316c5451cd0c0
python3 tools/pipeline/sign_pack.py --self-test   # no key, no .env
```

The self-test signs and verifies with a throwaway key. It also checks the
RFC 8032 §7.1 TEST 1 vector byte for byte.

## 6. Embed (Task M5)

M5 embeds `reference.kpack` and `reference.kpack.sig` in the triage build
variant as a read-only bundled pack. It records `referencePack: {id,
sha256, contentSha256, version}` in `src-tauri/resources/catalog.triage.json`.
`Pack::mount` verifies the curator signature at load, as it does for every
curated pack. After signing, the founder's second step is to run M5's
embed with the signed pair. That step is documented here once M5 lands.

## Tests

```bash
python3 -m pytest -q tools/                 # fetcher, clusters, sign_pack and the pipeline tests
python3 tools/pipeline/sign_pack.py --self-test
```

The fetcher tests run on synthetic HTML with no network. The clusters
tests use a synthetic mini-corpus. They also check the committed
`reference-clusters.json` against the committed corpus, and against the
triage bank and partition when the triage repo is on the machine.
