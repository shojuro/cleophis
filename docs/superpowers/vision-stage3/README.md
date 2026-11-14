# Vision Stage 3: bench evidence (host and device)

The JSONL files behind `docs/superpowers/verification-vision-stage3.md` (ruling S3-P13), one record
per probe item, written by `vision_bench --via-engine --json`. Every `host-*` run is on one x86-64 WSL2 host and every `device-*` run on the Galaxy A22
(`run-vision-probes.sh`), one repeat, the committed items (`probes/vision/fixtures/v1/items.json`) or a render of the same 18 typeset
items. The `host-v1-*` and `device-a22-s3-v1` runs are over the 18 items that existed then; the
`host-v2-*` and `device-a22-s3-v2` runs are over the 22 items of the photo-join round. Host timings are not a gate: other builds ran on the box throughout.

Common flags: `--family docling --max-side 512 --threads 4 --one-pass`, base
`granite-docling-258M-Q8_0.gguf` (sha256 `b687245c…`) and projector
`mmproj-granite-docling-258M-Q8_0.preproc512.gguf` (sha256 `bb75a65a…`), the two files the CI job
pins. "Parser" is the `vision_parse.rs` that judged the tokens. It was unchanged from `main`
(`4aadcc9`) until `83b8929` (S3-P9), widened by `f5bd4d3` (S3-P15).

| File | Prompt, cap | Render | Bench | Parser | Scored by | Gate evidence |
|---|---|---|---|---|---|---|
| `host-v1.jsonl` | page ("Convert this page to docling."), 256 | DejaVu Sans 28 pt (`b53ab15`) | `1ac6b5a` | `main` | — | no: page-prompt run |
| `host-v1-cm.jsonl` | page, 256 | Computer Modern, committed (`b6ad9dc`) | `1ac6b5a` | `main` | — | no: page-prompt run |
| `host-v1-product.jsonl` | formula ("Convert formula to latex."), 96, both flags explicit | DejaVu Sans 28 pt (`b53ab15`, extracted with `git show`) | `ba47bd4` (same example as `1ac6b5a`) | `main` | — | no |
| `host-v1-dejavu-fill-product.jsonl` | formula, 96, the `--via-engine` defaults | DejaVu Sans, crop-filling (`gen.py --out <dir> --fontset dejavusans`, `5fc68a4`) | `166ab9e` | `main` | — | no |
| `host-v1-cm-product.jsonl` | formula, 96, both flags explicit | Computer Modern, committed | `ba47bd4` | `main` | — | superseded by r2 |
| `host-v1-cm-product-r2.jsonl` (+ `.md`, `.json`) | formula, 96, the `--via-engine` defaults | Computer Modern, committed | `b4b4131` | `83b8929` | `b4b4131` | yes, until round 2 |
| `host-v1-cm-product-r3.jsonl` (+ `.md`, `.json`) | formula, 96, the `--via-engine` defaults | Computer Modern, committed | `f5bd4d3` | `f5bd4d3` (S3-P15) | `b4b4131` | yes, until round 3 |
| `host-v1-cm-product-r4.jsonl` (+ `.md`, `.json`) | formula, 96, the `--via-engine` defaults | Computer Modern, committed | `dccd85e` | `dccd85e` (S3-P16, the operator guard sees the Unicode operators) | `b4b4131` | **yes** |
| `device-a22-s3-v1.jsonl` (+ `.md`, `.json`, `.meminfo.txt`; the log is not committed, `*.log` is gitignored) | formula, 96 (the script's runner) | Computer Modern, committed | `vision_bench-aarch64-715b129` (engine `dccd85e`) on the Galaxy A22 | `dccd85e` | `score.mjs --device` at `b4b4131`+ | **yes: the device run — PASS 18/18, refusals 2/2, p50 8,791 ms, p95 10,604 ms, VmHWM 389,292 KiB** |
| `host-v2-cm-product.jsonl` (+ `.md`, `.json`) | formula, 96, both flags explicit | Computer Modern committed, 22 items: the 18 typeset, photographs q19/q20, refusals q21/q22 | `512fc75` | `512fc75` | the photo-join tree | **yes, the first held-out scoreboard: PASS 22/22, refusals 4/4** |
| `host-v2-cm-product-r2.jsonl` (+ `.md`, `.json`) | formula, 96, both flags explicit | Computer Modern committed, 22 items | `4844f10` | `4844f10` (S3-P17, a digit no longer makes a block maths) | the photo-join tree | **yes: PASS 22/22, refusals 4/4; raws identical to `host-v2-cm-product`, q21 now refuses by `no_math_found`** |
| `device-a22-s3-v2.jsonl` (+ `.md`, `.json`, `.meminfo.txt`; log not committed) | formula, 96 (the script's runner) | Computer Modern committed, 22 items | `vision_bench-aarch64-f998395` (engine `f998395`) on the Galaxy A22 | `f998395` (S3-P17) | `score.mjs --device` at `f998395` | **yes: the 22-item device run — PASS 22/22, refusals 4/4, p50 8,733 ms, p95 9,870 ms, VmHWM 389,188 KiB; the 18 typeset raws byte-identical to `device-a22-s3-v1`, 21 of 22 to `host-v2-cm-product-r2` (q18, the prose page at the token cap, decodes differently per architecture; both refuse)** |
| `host-v1-dejavu-fill-product-r2.jsonl` (+ `.md`, `.json`) | formula, 96, the `--via-engine` defaults | DejaVu Sans, crop-filling, byte-identical to the earlier render | `b4b4131` | `83b8929` | `b4b4131` | no |

Round 3's `host-v1-cm-product-r4` scores PASS, and its 18 raws, its records apart from the
timings, and its `.md` and `.json` are identical to r3's. Round 2's `host-v1-cm-product-r3` scores
PASS like r2. Its 18 raws, and all 18 records apart
from the timings, are identical to r2's.

The two round-1 fix-wave runs score as follows with the scorer at `b4b4131`. The CM gate run passes: exact
18/18, refusals 2/2, no accepted sign, operator or symbol error. The DejaVu crop-filling run fails:
exact 16/18, and q08 and q13 (`x` read as `X`, accepted) reach the tutor. Both runs' 18 raws are
byte-identical to the Task 6 runs on the same images. Only q17 and q18 changed, from `review` to
`refuse`.

The first five were run before the fix wave and are copied unchanged from the SDD workspace. Their
scores live in the record, because the scorer changed after them (S3-P10, S3-P11). Re-score any of
them with the current scorer:

```
node probes/vision/score.mjs --items probes/vision/fixtures/v1/items.json \
  --records docs/superpowers/vision-stage3/<file>.jsonl --out-md /tmp/out.md --out-json /tmp/out.json
```

The gate run's command:

```
cargo run -p kpack-engine --features vision --release --example vision_bench -- \
  --via-engine --family docling \
  --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf \
  --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.preproc512.gguf \
  --items probes/vision/fixtures/v1/items.json --max-side 512 --threads 4 --one-pass \
  --json docs/superpowers/vision-stage3/host-v1-cm-product-r4.jsonl
```

The host build also needs `BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/11/include`.
