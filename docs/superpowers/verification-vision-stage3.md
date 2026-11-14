# Vision input, Stage 3 (probe set, scorer, device run, CI, calibration): verification record

Branch `vision/stage3` from `origin/main` `4aadcc9`. Plan:
`docs/superpowers/plans/2026-11-13-vision-stage3-probes.md` (`e1a53fe`). Stage 2's record is
`docs/superpowers/verification-vision-stage2.md`; Stage 0's is
`docs/superpowers/verification-vision-spike.md`. The code under test is `dccd85e`: Task 6's
`166ab9e` plus the fix wave's parser rule (`83b8929`, widened in round 2 by `f5bd4d3`), its
operator guard (`dccd85e`, round 3) and the scorer rules (`b4b4131`). The record's own
commits change no code and no constant.

Every number below is quoted from the file named beside it, or comes from a command run for this
record on 2026-11-13 or, for the fix wave, 2026-11-14 (UTC, the WSL2 box's clock). The bench
evidence is seven JSONL files committed under `docs/superpowers/vision-stage3/` (ruling S3-P13).
That folder's README names each file's prompt, cap, render and commit. The five runs made before the
fix wave were scored by the scorer of their day, and the tables below keep those scores. The two
fix-wave runs have the current scorer's `.md` and `.json` beside them. Nothing is estimated unless
marked.

## In short

- **After the fix wave the gate run passes: exact 18/18, refusals 2/2, no accepted sign, operator
  or symbol error** (`docs/superpowers/vision-stage3/host-v1-cm-product-r4.md`, on `dccd85e`). The
  parser now refuses a LaTeX block that carries no operator, relation or structural token (S3-P9,
  tightened by S3-P17, which dropped the digit clause). Round 2 widened the operators to the commands and Unicode signs that letters-only maths is written with (S3-P15).
  Round 3 taught the operator guard those signs (S3-P16). The diagram and the prose come back
  `refuse` with `no_math_found`. The r2, r3 and r4 raws are byte-identical to Task 6's CM run, and
  only q17's and q18's verdicts changed (Fix wave). **The refusal gate was in-sample** when
  the rule was derived from these two images. The photo-join round tested it on two held-out probes: q21
  (a diagram labelled 3, 4, 5) and q22 (prose holding a date) both refuse, q21 by rule since S3-P17.
  What is still untested is prose or a diagram holding an operator-bearing fragment (What Stage 4 inherits).
- **The scorer now counts look-alike swaps.** `x`/`X`, `1`/`l`, `0`/`O`, `5`/`S` and `2`/`Z` weigh ×5
  and fail the tutor gate when accepted (S3-P10). The crop-filling DejaVu run's two accepted `X`
  readings now fail it. The exact gate is total − 2 from 20 items and total − 1 below (S3-P11). The
  device p50 gate is 10,000 ms, a regression guard, not the spec's floor gate of 8 s (S3-P12; The
  device run).
- **The brief's two host runs measured the page prompt, not the product's.** Up to `ba47bd4`,
  `vision_bench --via-engine` without `--prompt` fell back to granite-docling's page prompt, "Convert
  this page to docling.", and to a 256-token cap. No Stage 3 command passed either flag. The product
  sends "Convert formula to latex." with a cap of 96 (`src-tauri/resources/catalog.json`, every
  tier; `src-tauri/src/catalog.rs:190`). Stage 0's decisive run used the same prompt
  (`docs/superpowers/vision-spike/a22-floor-formula-1pass.jsonl`). All 36 raws of the two brief runs
  open with `<doctag>`. Stage 1's record names that as the page prompt's output shape; the formula
  prompt's is `<formula>…</formula>` alone.
- **Ruling S3-P8 fixed the regime at `166ab9e`.** `--via-engine` now defaults to the product's prompt
  and cap. The CI job and the device script pass both flags explicitly.
- **Task 6 ran the product's regime on three renders.** The committed Computer Modern set and the
  original 28 pt DejaVu set each read all 16 printed items exactly. A crop-filling DejaVu set read
  14: it took the variable `x` for a capital `X` in q08 and q13, and both misreadings were
  accepted. Before the fix wave all three renders failed the refusal gate. The diagram and the
  prose came back `review` with a LaTeX block.
- **The gate evidence is the product-regime CM run** (ruling S3-P8), on the committed fixtures,
  which stay the gate set. Task 6's run (`host-v1-cm-product`) scored FAIL: exact 16/18 and
  refusals 0/2, with every printed item exact. Its re-run on `b4b4131`
  (`host-v1-cm-product-r2`) scores PASS. The two earlier runs are page-prompt runs. Their tables
  are kept below, and they are not gate evidence.
- **Under the product's prompt, DejaVu loses by 2 of 16 at the crop-filling size and by 0 at 28 pt.**
  The original DejaVu finding (ruling S3-P6), 8/18, was the page prompt. The two remaining misses
  are `x` read as `X`, both accepted, at confidence 0.9935 and 0.9748.
- **The thresholds stay 0.90 / 0.70 / 0.97.** No sign or operator error appears in any reading of
  any run, including the two fix-wave runs. The confidence pair pushes no exact reading into
  review. The operator threshold does, in every run, but this evidence cannot set its floor (see
  Calibration). A lowered operator threshold of 0.86 is recorded as a proposal pending the device
  run. The two accepted `X` readings sit above
  every threshold, so no threshold value would have caught them. The scorer now counts them, and
  the product still accepts them.
- **The device run and the CI job have not run.**

| Run (`docs/superpowers/vision-stage3/`) | Prompt, cap | Render | Exact | Printed exact | Refusals | Sign + operator substitutions | Scorer |
|---|---|---|---|---|---|---|---|
| `host-v1` (Task 3) | page, 256 | DejaVu Sans 28 pt (`b53ab15`) | 8/18 | 6/16 | 2/2 | 0 | FAIL (exact) |
| `host-v1-cm` (Task 1 round 2) | page, 256 | Computer Modern, committed | 17/18 | 15/16 | 2/2 | 0 | PASS |
| `host-v1-product` (Task 6) | formula, 96 | DejaVu Sans 28 pt (`b53ab15`) | 16/18 | 16/16 | 0/2 | 0 | FAIL (exact, refusals) |
| `host-v1-dejavu-fill-product` (Task 6) | formula, 96 | DejaVu Sans, crop-filling (`gen.py --fontset dejavusans`) | 14/18 | 14/16 | 0/2 | 0 | FAIL (exact, refusals) |
| `host-v1-cm-product` (Task 6) | formula, 96 | Computer Modern, committed | 16/18 | 16/16 | 0/2 | 0 | FAIL (exact, refusals) |
| `host-v1-cm-product-r2` (fix wave, `b4b4131`) | formula, 96 | Computer Modern, committed | 18/18 | 16/16 | 2/2 | 0 | **PASS** |
| `host-v1-cm-product-r3` (round 2, `f5bd4d3`) | formula, 96 | Computer Modern, committed | 18/18 | 16/16 | 2/2 | 0 | **PASS** |
| `host-v1-dejavu-fill-product-r2` (fix wave, `b4b4131`) | formula, 96 | DejaVu Sans, crop-filling | 16/18 | 14/16 | 2/2 | 0, and 2 symbol | FAIL (exact, tutor: q08, q13) |

The exact count includes the two refusal items, which are exact only when refused. The first five
rows were scored before the fix wave. Re-scored with the scorer at `b4b4131`, each keeps its
verdict and its counts. The one change is that the DejaVu crop-filling run also fails the tutor
gate on q08 and q13, two accepted symbol substitutions.

- **The device run passed (2026-11-14, A22):** 18/18 exact, refusals 2/2, p50 8,791 ms, p95 10,604 ms, VmHWM 389 MB; six exact readings held in review by the operator threshold, as on the host; thresholds kept (The device run).
- **The 22-item device run passed (2026-11-14, A22, bench from `f998395`):** 22/22 exact, refusals 4/4, p50 8,733 ms, p95 9,870 ms, VmHWM 389,188 KiB. The 18 typeset raws are byte-identical to the first device run, the photographs are accepted at 0.9999 and 0.9985 with the orientation tag applied, q21 and q22 refuse by rule (Photo-join round, The 22-item device run).

## Fix wave (S3-P9 to S3-P16)

Task 6's product-regime evidence found one parser gap and four scorer and gate corrections. The
controller ruled on each, and the commits below carry them. Round 2 (S3-P15) followed the fix
wave's own concerns, and round 3 (S3-P16) the whole-branch review:

| Commit | Ruling | What |
|---|---|---|
| `83b8929` | S3-P9 | The parser refuses a "formula" with no digit and no operator. |
| `b4b4131` | S3-P10, S3-P11, S3-P12 | The scorer counts look-alike swaps, the exact gate scales by the spec, and the device latency gate is a 10 s regression guard inside lever 6's band, not the spec's 8 s floor gate. |
| `d77f541` | S3-P13 | The evidence is committed under `docs/superpowers/vision-stage3/`, and the CM gate run and the DejaVu crop-filling run are re-run on `b4b4131`. |
| `f5bd4d3` | S3-P15 | Round 2: letters-only maths with a command or a Unicode operator stays maths. |
| `bb015aa` | S3-P15 | Round 2's docs: the plan's gate line, the Stage 4 items, the photo-join checklist and the r3 re-run. |
| `dccd85e` | S3-P16 | Round 3: the operator guard sees the Unicode operators (final review I1), and the r4 re-run. |
| this commit | S3-P16 | Round 3's docs: the floor gate's reason (I2), the in-sample refusal gate and the held-out probes (I3), the review's docs minors. |

### The parser: no operator is not maths (S3-P9)

*Superseded in part by S3-P17 (photo-join round 2): the digit clause below is gone. A digit no longer
makes a block maths. The text of this section is the S3-P9 history, with the current rule stated where it differs.*

`classify` (`crates/kpack-engine/src/vision_parse.rs`) also asks an element to hold maths. Since
S3-P17 it needs one of `= < > + - ^ _`, a Unicode operator, or a maths command as a whole token
(S3-P9 also accepted a digit):

- **S3-P9 (`83b8929`):** `\le \leq \ge \geq \ne \neq \times \cdot \div \frac \sqrt`.
- **S3-P15 (`f5bd4d3`, round 2):** `\pm \mp \dfrac \tfrac \int \sum \prod \lim \log \ln \sin \cos
  \tan \in \notin \cup \cap \subset \subseteq \to \rightarrow \infty \equiv \approx \sim \propto
  \cong \angle \perp \parallel \circ \ldots \cdots`, and the Unicode operators `≤ ≥ ≠ × ÷ ± − → ∞ ∈`
  (U+2212 for the minus).

A command is the backslash and its full letter run, compared whole. `\left` is not `\le`, `\neg`
is not `\ne`, `\intertext` is not `\int` or `\in`, and `\toprule` is not `\to`. The array syntax
`&` and `\\` and the label's `\colon` are not operators. A Unicode operator also marks an element
as maths the way `\` and `=` do. Without that, `a ≤ b` would still be text by `classify`'s older
test, which needs a strong marker (and, for the digit-and-sign form, a digit). An element
without one is a text block. A result with no other LaTeX block then takes the existing no-LaTeX
path: `NoMathFound` and `refuse`. A text block was already on the wire and is never handed to the
tutor, so `vision_cmds.rs` and `src/vision-confirm.js` are unchanged.

The two contents it refuses, verbatim from `host-v1-cm-product.jsonl`:

```
q17  C&& \\ && \\ A&& \\ B&& \\                                                  (0.7543, review)
q18  &ShppinglistforSutardy\colon \\ &\,milk,eggs,bread,butter, \\ &\,applesand   (0.8497, review)
```

The rule reads the element's raw text, before the display normalisation. That matters for q18.
Its display form joins the letter-spaced words, so the word count alone would already make it
text. The raw is `S h p p i n g …`, letter by letter, so the word count saw no word, and only the
new rule refuses it. q18 also loses `invalid_latex`, because the brace check runs only on LaTeX
blocks. Its warnings are now `no_math_found, truncated`, the same shape as the page-prompt
refusals.

The four S3-P9 tests: both raws and both contents refuse; an array of letters is text; `x+y=1`,
`a_1`, `\frac{1}{2}`, `x^2`, `3x-7=2x+5` and each token of the set beside two Greek letters stay
LaTeX; a formula beside an array of letters keeps the formula and its `accept`, with no
`NoMathFound`. Three of the four failed before the change; the fourth guards what must not move.

What the rule does not do:

- **It refuses letters and bare numbers, not prose.** Since S3-P17 a diagram labelled with plain
  numbers (the sides 3, 4 and 5: q21) or prose holding a digit (q22) refuses, because nothing in it
  is an operator. Prose or a diagram with an operator-bearing fragment (a hyphen or a dash beside a
  digit, `3 + 4`) still classifies by the word count alone and could come back `review`.
- **Letters-only maths outside the widened set still refuses.** After S3-P15, `a \pm b`,
  `\dfrac{a}{b}`, `\int f(x)\,dx`, `\angle A \cong \angle B`, `x \in A`, `a ≤ b` and
  `\lim_{x \to \infty} f(x)` are maths (`f5bd4d3`'s test). Still outside: Greek letters alone
  (`\alpha \beta`), the hyperbolic functions (`\sinh`), `\vec`, `\overline`, `\triangle`, `\mid`.
  These fail closed: the student sees "I could not find any maths" and types it.
- **A bare `x + y` or `a < b` is text,** as it was before this rule, because `classify` needs a
  strong marker (`\ = ^ _` or a Unicode operator) or a digit with a sign. The ruling's example holds
  with a marker: `x+y=1` stays LaTeX.

### The scorer (S3-P10, S3-P11, S3-P12)

- **Symbol substitutions.** On the token alignment, a swap between `x`/`X`, `1`/`l`, `0`/`O`,
  `5`/`S` or `2`/`Z`, in either direction, is a symbol substitution. It has its own column, weighs
  ×5 like a sign, and counts toward "survives to the tutor" when the verdict is `accept`. Any
  other letter swap is still a plain edit. The crop-filling DejaVu run's q08 and q13 are the
  fixtures.
- **The exact gate scales by the spec.** `GATES.exactMin` is a function of the item count:
  total − 2 from 20 items, and total − 1 below. That is 17 of 18 now and 18 of 20 once the
  founder's photographs join as q19 and q20. The Markdown names the rule that applied. The old
  ceil(17/18 × n) gave 19 of 20.
- **The device latency gate is a regression guard.** `floorP50Ms` is 10,000 and `floorP95Ms` 12,000.
  Stage 0 measured 8.9 s p50 on the A22, and Stage 2's four device readings were 8,991 to 9,403 ms.
  The spec's floor gate is 8 s p50. Stage 0's GO took lever 6, which ships 8–12 s on the A22 behind
  a staged progress state. 10 s is a regression guard inside that band. The strict 8 s waits on
  runtime kernel dispatch (lever 4). The mid-tier gate is 2.0 s and has not been measured. The
  memory gate is 1,258,291 KiB, which is 1.2 GiB; the spec says 1.2 GB, immaterial at the 0.40 GB
  measured.

### The re-run (S3-P13)

The gate run's command on `b4b4131`, with no `--prompt` or `--max-tokens`:

```
cargo run -p kpack-engine --features vision --release --example vision_bench -- \
  --via-engine --family docling \
  --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf \
  --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.preproc512.gguf \
  --items probes/vision/fixtures/v1/items.json --max-side 512 --threads 4 --one-pass \
  --json docs/superpowers/vision-stage3/host-v1-cm-product-r2.jsonl
node probes/vision/score.mjs --items probes/vision/fixtures/v1/items.json \
  --records docs/superpowers/vision-stage3/host-v1-cm-product-r2.jsonl \
  --out-md docs/superpowers/vision-stage3/host-v1-cm-product-r2.md \
  --out-json docs/superpowers/vision-stage3/host-v1-cm-product-r2.json
```

The bench logged the product regime:

```
[bench] via-engine regime: prompt="Convert formula to latex." max_tokens=96
```

The two model files were checked again: `b687245c…` and `bb75a65a…`, the CI pins. Every record has
`error` null and `n_image_tokens` 64, and every raw opens with `<formula>`. The scorer's headline:

```
# Vision probe score: PASS
| exact | 18/18 | >= 17: total - 1 (fewer than 20 items) |
| accepted sign, operator or symbol errors | 0 | <= 0 |
| refusals correct | 2 | >= 2 |
```

**What changed against Task 6's CM run.** All 18 raws are byte-identical. Apart from the timings,
16 of the 18 records are identical too: q01 to q16 keep their blocks, confidences, operator
probabilities, warnings and verdicts. Only q17 and q18 changed:

| id | Task 6 (`host-v1-cm-product`) | fix wave (`host-v1-cm-product-r2`) |
|---|---|---|
| q17 | `review`, `low_confidence`, one LaTeX block at 0.7543 | `refuse`, `no_math_found`, one text block at 0.7543 |
| q18 | `review`, `invalid_latex, low_confidence, truncated`, one LaTeX block at 0.8497 | `refuse`, `no_math_found, truncated`, one text block at 0.8497 |

The verdicts are now 10 accept, 6 review and 2 refuse; they were 10, 8 and 0. The six reviews are
the exact readings that `OperatorUncertain` alone holds: q01, q03, q04, q06, q10 and q14
(Calibration). Host `total_ms` ran from 10,274 to 15,819 ms with another session's job on the box;
it is not a gate.

**The rule on the earlier evidence.** Applied to every LaTeX block of the five earlier runs, the
rule re-kinds exactly q17 and q18 in each of the three product runs. It touches no printed item
and nothing in the two page-prompt runs. This check reads the recorded display contents, which
keep every digit and operator of the raw.

**The DejaVu crop-filling set, re-run.** `gen.py --out <dir> --fontset dejavusans` rendered it
again, and the 18 PNGs and `items.json` are byte-identical to Task 6's render. Its q17 and q18 are
byte-identical to the committed ones. Same command, `--items` pointed at that render, output
`host-v1-dejavu-fill-product-r2.jsonl`:

```
# Vision probe score: FAIL
| exact | 16/18 | >= 17: total - 1 (fewer than 20 items) |
| accepted sign, operator or symbol errors | 2 | <= 0 |
| refusals correct | 2 | >= 2 |
- exact 16/18 is below the gate of 17/18
- 2 accepted transcription(s) with a sign, operator or symbol error reach the tutor (max 0)
```

The bench logged the same regime line. Its `items.json` is byte-identical to the committed one. All
18 raws are byte-identical to Task 6's run, and q01 to q16 are identical apart from the timings.
q17 and q18 now refuse, as on the CM set. q08 and q13 each count one symbol substitution, weighted
5, and both reach the tutor. The scorer now fails the run on them, which is what S3-P10 asks. This
render is not the gate set. Its failure is the finding: a sans-serif italic `x` can be read as `X`
and accepted. Host `total_ms` ran from 9,597 to 21,522 ms.

### Gates for the fix wave

`cargo test -p kpack-engine` on `83b8929` (the default features, no native build):

```
test result: ok. 170 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.32s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

170 is Task 3's 166 plus the four new parser tests. Three of them failed before the change. The
example's own tests, `cargo test --release -p kpack-engine --features vision --example
vision_bench`, on `b4b4131`:

```
test tests::record_shape_matches_the_scorer ... ok
test tests::via_engine_defaults_to_the_product_regime ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

The Android check, `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis
--all-targets --release` from `src-tauri/` after `source ../.superpowers/ndk-env.sh`, on the tree
of `83b8929`. It printed no warning:

```
    Checking kpack-engine v0.1.0 (…/crates/kpack-engine)
    Checking kpack-embed v0.1.0 (…/crates/kpack-embed)
    Checking cleophis v0.1.0 (…/src-tauri)
    Finished `release` profile [optimized] target(s) in 7m 51s
```

`npm test` on the tree of `b4b4131`. The six new or changed scorer tests failed before the change:

```
# tests 1331
# pass 1331
# fail 0
```

1331 is the earlier 1328 plus three new scorer tests. Three existing ones changed: the pinned
gates, the exact gate's scaling and the p50 gate.

### Round 2 (S3-P15)

`f5bd4d3` widens the maths-token set (The parser, above). Its one new test checks the brief's
cases. `a \pm b`, `\dfrac{a}{b}`, `\int f(x)\,dx`, `\angle A \cong \angle B`, `x \in A`, `a ≤ b`,
`\lim_{x \to \infty} f(x)` and `x \to \infty` each stay LaTeX and do not refuse. So does every
added command placed between two Greek letters, and every Unicode operator placed between two
letters. The test failed first, at `a \pm b`. The letters-array test gained two prefix guards,
`\intertext{AB}` and `\toprule A`, which stay text. It dropped `A , B , \cdots`, now maths because
`\cdots` is in the set. The S3-P9 tests are unchanged and pass, so q17's and q18's raws and
contents still refuse.

Gates on `f5bd4d3`:

```
cargo test -p kpack-engine:
test result: ok. 171 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.55s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Android check (no warning printed):
    Finished `release` profile [optimized] target(s) in 2m 16s
npm test:
# tests 1331
# pass 1331
# fail 0
```

**The r3 re-run.** The gate run's command again, output
`docs/superpowers/vision-stage3/host-v1-cm-product-r3.jsonl`, with the same regime line. It scores
PASS: exact 18/18, refusals 2/2, no accepted sign, operator or symbol error. All 18 raws are
byte-identical to r2, and all 18 records are identical apart from the timings. The widening adds
maths tokens, and none of the probe set's readings turned on one. Applied to every recorded block
of the seven runs, the widened set changes no kind. Host `total_ms` ran from 10,131 to 17,559 ms.

### Round 3 (S3-P16)

The final review (`final-review-report.md`, I1) found that round 2 opened a hole in the sign guard.
A Unicode operator made a formula LaTeX, but `is_operator_piece` knew only `+ - = < > ^ _` and its
command list. So `12x − 34 ≥ 5678`, with U+2212 and `≥`, came back `accept` with no operator
probability, while `12x - 34 \ge 5678` comes back `review` with `OperatorUncertain`. On `main` the
glyph form had been text and refused. The spec's "zero sign errors accepted" path was blind to the
glyphs.

`dccd85e` fixes it in `vision_parse.rs`:

- `is_operator_piece` now also fires on a piece holding one of `≤ ≥ ≠ × ÷ ± −`.
- That list is `OPERATOR_GLYPHS`, the one definition that `classify` and `holds_maths` share. The
  other maths glyphs, `→ ∞ ∈`, mark an element as maths, and the guard does not watch them.
- `OPERATOR_CMDS` gains `\mp \leqslant \geqslant \lt \gt`.

There are two new tests:

- **A glyph-only formula is reviewed like an ASCII one.** `12x − 34 ≥ 5678` with the minus at
  probability 0.8 comes back `review` with `OperatorUncertain` alone, the same as the ASCII form.
  The block's confidence stays above 0.90, so the operator guard alone decides. With every glyph at
  0.99 it is `accept`, and the relation alone weak is `review`.
- **Every Unicode operator carries an operator probability.** `12x − 34 ≥ 5678`, `2x ≤ 9`, `3 × 4`,
  `x ± 2`, `8 ÷ 2` and `x ≠ 2` each get a non-null `min_operator_prob` and `OperatorUncertain` at
  0.8. So do the five added commands.

Both tests failed first, and the failure reproduced the review: `accept`, `min_operator_prob`
null, with the minus at 0.8. A glyph whose bytes span two tokens reaches the second token's piece
whole, because `vision.rs` decodes with a stateful UTF-8 decoder. Only that token's probability is
checked.

Gates on `dccd85e`:

```
cargo test -p kpack-engine:
test result: ok. 173 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.73s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Android check (no warning printed):
    Finished `release` profile [optimized] target(s) in 3m 16s
```

**The r4 re-run.** The gate run's command again, output
`docs/superpowers/vision-stage3/host-v1-cm-product-r4.jsonl`, with the same regime line. It scores
PASS: exact 18/18, refusals 2/2, no accepted sign, operator or symbol error. All 18 raws are
byte-identical to r3, and all 18 records are identical apart from the timings. The `.md` and `.json`
score files are byte-identical to r3's. No product-regime raw in any of the committed runs holds a
Unicode operator; the one raw that does is the page-prompt q12's `≥` in `host-v1.jsonl`. So the
fix changes nothing in the evidence, and the risk it closes is unmeasured on photographs. Host
`total_ms` ran from 8,479 to 15,989 ms. The aarch64 bench must be rebuilt from `dccd85e` for the
device run.

**Round 3's docs** (I2, I3 and the review's docs minors). They reword the 10 s gate's reason in
`score.mjs` and here: 10 s is a regression guard inside lever 6's band, and the spec's floor gate
of 8 s is not met. They state that the refusal gate is in-sample. Two held-out refusal probes, q21
and q22, join the photo-join checklist with the counts at 22 items. The review's M2, M3, M5 and M7
are folded in, and M1, M4 and M6 are deferred with a line each (Deferred minors). `npm test` on the
docs commit's tree:

```
# tests 1331
# pass 1331
# fail 0
```

### What the fix wave did not change

- **The thresholds and the operator finding.** 0.90 / 0.70 / 0.97 stay, and 0.86 stays a proposal
  pending the device run (Calibration). q01 to q16 are identical in the re-run, so every
  calibration number on the printed items stands. q17 and q18 now have no LaTeX block. In the
  re-run no wrong reading remains, so no confidence or operator value is set against one.
- **The device run and the CI job.** Neither has run.
- **The aarch64 bench.** The controller builds it for the device run. It must be built from
  `dccd85e` or later to carry the parser rule.

## What each task built

| Task | Commit | What |
|---|---|---|
| 1 | `b53ab15`, `b6ad9dc`, `5fc68a4` | `probes/vision/fixtures/v1/gen.py` typesets 16 printed problems, a labelled triangle and a page of prose with matplotlib mathtext at 512×160 px. `items.json` carries each item's expected LaTeX and verdict. `manifest.json` pins the 18 PNGs by sha256, and `probes/vision/manifest.test.mjs` enforces the pins. `gen.py --check` re-renders to a temp dir and compares bytes. Round 2 (S3-P6) moved the maths to mathtext's Computer Modern fontset at a size that fills the crop; q17 and q18 are byte-identical to round 1. Round 3 (S3-P8) added `--out DIR --fontset NAME`, a scratch render that leaves the committed set untouched, and `--self-test`. |
| 2 | `eb11f83`, `d90b668`, `a7ba471` | `normalise.mjs` ports Stage 0's normalisation, adds `EQUIVALENCES` and Stage 0's brace canonicalisation (S3-P4). `score.mjs` scores exact match, CER, sign and operator substitutions weighted ×5, verdict correctness and "survives to the tutor" with operator swaps counted (S3-P5). It holds the gates as `GATES`, writes Markdown and JSON reports, and exits 0, 1 or 2. Round 2 anchored the equivalence rewrites at a word boundary. |
| 3 | `1ac6b5a`, `166ab9e` | `vision_bench --via-engine --json` writes one JSONL record per item and repeat: the engine's own `assess` verdict, snake_case warnings, blocks with confidence and minimum operator probability, raw text, stop reason, timings, VmRSS and VmHWM. Round 1 (S3-P8) made `--via-engine` default to the product's prompt and 96-token cap, logged the regime at start, and made `vision-regression.yml` and `run-vision-probes.sh` pass both flags. |
| 4 | `dbcec3f`, `6085ad3` | `docs/superpowers/mobile-tools/run-vision-probes.sh`: sha-verified pushes of the bench, libc++, model, projector and fixtures, the device run, the pull, `score.mjs --device`, and one `dumpsys meminfo` of the running bench. The fix round set the executable bit, verified every push and removed stale pulls and reports. |
| 5 | `ba47bd4` | `.github/workflows/vision-regression.yml`: the probe set on `ubuntu-latest` and `ubuntu-24.04-arm`, models from the public dist bucket verified by sha256 on cache hit and miss, an 18-record guard, the scorer's exit code passed through, the report uploaded. Its header says it gates transcription only. |
| 6 | `087d1d9`, `2953552` | The three product-regime host runs, the calibration decision (no constant changed) and this record. |
| Fix wave | `83b8929`, `b4b4131`, `d77f541`; round 2 `f5bd4d3`, `bb015aa`; round 3 `dccd85e` and this commit | The parser refuses a "formula" with no digit and no operator (S3-P9), widened in round 2 to the commands and Unicode operators letters-only maths uses (S3-P15). The scorer counts look-alike swaps, scales the exact gate by the spec and guards floor latency at 10 s, inside lever 6's band (S3-P10 to S3-P12). The evidence is committed and the gate run re-run (S3-P13). See Fix wave. |

Tasks 2, 3 (round 1), 4 and 5 were built in side worktrees and rebased onto `vision/stage3`; the
commits above are the rebased ones. The originals were `9ba31d7`/`8e84a72` (Task 2), `4d2ef13` (Task 3
round 1), `e8b8611`/`651cef0` (Task 4) and `9913a99` (Task 5), from the SDD ledger.

## Gates

### Engine (Task 3, on `1ac6b5a` and `166ab9e`)

After `1ac6b5a` the only Rust file that changed is the bench example, in `166ab9e`. No other file
under `crates/`, `src/` or `src-tauri/` changed, and neither did `Cargo.toml` or `Cargo.lock`. Verbatim
from `task-3-report.md`, first for `1ac6b5a`:

```
test result: ok. 166 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.44s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
android check:
    Checking kpack-engine v0.1.0 (/home/penguinzyue/cleophis-wt/vision-s3/crates/kpack-engine)
    Checking kpack-embed v0.1.0 (/home/penguinzyue/cleophis-wt/vision-s3/crates/kpack-embed)
    Finished `release` profile [optimized] target(s) in 10m 00s
release example build: Finished, no warnings outside llama-cpp-sys-2 C++ build output
```

Then for round 1 (`4d2ef13`, rebased as `166ab9e`):

```
- Gates: example test 2 passed; cargo test -p kpack-engine 166, 4, 0 passed; no kpack warnings in the build. Android check not run (host-only example). Host bench not re-run (Task 6).
```

The three `test result` lines are `cargo test -p kpack-engine`. The example's own tests run only
under `cargo test -p kpack-engine --features vision --example vision_bench`. At `1ac6b5a` that was one
test, given as "GREEN: 1 passed" after a deliberate key rename turned it red. At `166ab9e` it is two,
with `via_engine_defaults_to_the_product_regime` added. The Task 3 reviewer could not run them (no
libclang on that box). The Android check does not build the example. The aarch64 build of the
example happens before the device run, and it has not been made at `166ab9e`. The Android check is
`RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release`
from `src-tauri/` after `source ../.superpowers/ndk-env.sh`. The host build also needs
`BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/11/include` (bindgen could not find
`stdbool.h`).

The Windows desktop suite is not required: no `src-tauri` file changed on this branch.

### `npm test`

Task 1 round 2, on `b6ad9dc` (`task-1-report.md`): "full `npm test` 1328 tests, 1328 pass, 0 fail".
Run again for this record, on `ba47bd4` and then on `166ab9e`:

```
ba47bd4:  # tests 1328  # pass 1328  # fail 0
166ab9e:  # tests 1328  # pass 1328  # fail 0
```

### The host bench

Task 6's gate run's command, the product regime on the committed fixtures. The fix wave's re-run
uses the same command with the flags left to their defaults (Fix wave). The `<workspace>` JSONL is
now `docs/superpowers/vision-stage3/host-v1-cm-product.jsonl`. Its `.md` and `.json` are not
committed, because the scorer changed after them:

```
cargo run -p kpack-engine --features vision --release --example vision_bench -- \
  --via-engine --family docling \
  --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf \
  --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.preproc512.gguf \
  --items probes/vision/fixtures/v1/items.json --max-side 512 --threads 4 --one-pass \
  --prompt "Convert formula to latex." --max-tokens 96 \
  --json <workspace>/host-v1-cm-product.jsonl
node probes/vision/score.mjs --items probes/vision/fixtures/v1/items.json \
  --records <workspace>/host-v1-cm-product.jsonl \
  --out-md <workspace>/host-v1-cm-product.md --out-json <workspace>/host-v1-cm-product.json
```

It ran on the `ba47bd4` bench with both flags explicit; `166ab9e` makes them the default. The
DejaVu 28 pt run used the same command on `b53ab15`'s DejaVu renders, the images behind the
original finding, extracted with `git show`. All 18 sha256s match that commit's `manifest.json`,
and its 18 items carry the same ids, files, LaTeX and verdicts as the committed set.

The two page-prompt runs used the brief's command (plan, Task 3 Step 3) on the bench as of
`1ac6b5a`. That command is the one above without `--prompt` and `--max-tokens`, which there meant
the page prompt and 256 tokens.

The crop-filling DejaVu run (`host-v1-dejavu-fill-product`) used the `166ab9e` bench with neither
flag, on the scratch set that
`gen.py --out <dir> --fontset dejavusans` wrote (ruling S3-P8). Its 18 items match the committed
set's ids, files, LaTeX and verdicts, and its q17 and q18 are byte-identical to the committed ones.
The bench's log line confirms the default:

```
[bench] via-engine regime: prompt="Convert formula to latex." max_tokens=96
```

All 18 raws of each product run open with `<formula>`, and none with `<doctag>`.

The two model files, checked for this record:

```
b687245c212b59da938e411e2d4485596cbaeed1ae52706582ac38ed6a970f76  granite-docling-258M-Q8_0.gguf
bb75a65aa3ec14ac4a002248d5ffa075183df369030b6ca09028463ede18b307  mmproj-granite-docling-258M-Q8_0.preproc512.gguf
```

They are the two files the CI job pins. In all five runs, and in the two fix-wave runs, every record
has `error` null and `n_image_tokens` 64 (one encoder pass). Host timings are not a gate. The box
ran other builds throughout (`total_ms` 9,784–28,787 on the CM product run, 6,579–149,490 on the
DejaVu 28 pt product run, 12,369–23,856 on the crop-filling DejaVu run).

### Device script and CI

- **Task 4:** `bash -n` OK; `--dry-run` exits 0 on both adb routes and writes nothing
  (`task-4-report.md`, fix round 1). shellcheck is not installed.
- **Task 3 round 1:** "bash -n ok, YAML parses, dry-run exits 0 with and without ADB=adbw and prints
  the flags in the runner listing" (`task-3-report.md`). A `--dry-run` on `166ab9e` for this record
  exited 0 and printed the runner's lines
  `--prompt 'Convert formula to latex.' --max-tokens 96 \` and
  `--json /data/local/tmp/cleophis-vision-probes/out-20261113T173313Z.jsonl …`.
- **Task 5:** the YAML parses under `yaml.safe_load`; actionlint is not installed. All four action
  pins resolve to the commits named in their comments (`task-5-review.md`). At `166ab9e` the bench
  step passes `--prompt 'Convert formula to latex.' --max-tokens 96` (`vision-regression.yml:115`).

## The probe set

`gen.py` typesets each item from LaTeX written for this repo, so the set is licence-clean. It is
deterministic: Agg backend, `figsize` 5.12×1.6 in, 100 dpi, no PNG metadata, and `--check` reproduces
all 18 PNGs and `items.json` byte for byte (`task-1-report.md`, both rounds). The 16 printed items
are the plan's list: q01–q16. q17 is a labelled right triangle with no equation. q18 is three lines
of a shopping list. Both refusal items expect `refuse`.

**Round 1 (DejaVu Sans, `b53ab15`)** typeset at 28 pt in mathtext's default font. On the page prompt
it read 6 of the 16 printed items exactly.

**Round 2 (Computer Modern, `b6ad9dc`, committed).** Ruling S3-P6 re-rendered the same 18 items in
mathtext's `cm` fontset, scaled to about 78% of the crop width (`task-1-report.md`). The reason given
was that the transcriber's training data and Stage 0's renders are Computer Modern. On the page
prompt it read 15 of the 16 printed items exactly.

**The finding that does not stand, and the one that does.** S3-P6 kept the DejaVu scoreboard as
evidence of font sensitivity. In the product's regime the same 28 pt DejaVu images read 16 of 16
printed items exactly (table below). The 10 DejaVu misses on the page prompt were layout readings
that prompt invites: the formula split into `<text>` or `<page_header>` rows, a `<picture>`, or a loop
to the token cap. Stage 0 measured the same thing: "page, page prompt, one pass: 1/5 (host),
fractions and exponents flattened to plain text" (`verification-vision-spike.md`).

S3-P8 also asked for a DejaVu render at the committed set's crop-filling size. In the product's
regime it read 14 of 16. The two misses are one character each: q08 `\frac{3}{4}X-…` and q13
`(\frac{1}{2})^X=8`, the italic sans-serif `x` read as `X`. Both were accepted (Calibration). The
committed Computer Modern set stays. The 28 pt DejaVu renders remain at `b53ab15`, and `gen.py --out
<dir> --fontset dejavusans` reproduces the crop-filling ones.

**q15**, "Solve for x: 5x = 35", keeps its prose in DejaVu in both rounds, because mathtext sets plain
text in the default font. On the page prompt both renders read the line as a `<picture>`. On the
formula prompt all three renders read it exactly. The CM run's raw is
`S o l v e f o r x \colon 5 x = 3 5`, and the parser's label split (Stage 1's I2) puts the words in a
text block and `5x=35` in the LaTeX block.

### Scoreboard, the five runs before the fix wave

Each cell is exact, CER, the product's verdict. In the fix wave's re-runs q17 and q18 are `yes 0.000
refuse` in both product columns, and every other cell is unchanged (Fix wave).

| id | page, DejaVu 28 pt | page, CM | product, DejaVu 28 pt | product, DejaVu fill | product, CM |
|---|---|---|---|---|---|
| q01 | yes 0.000 accept | yes 0.000 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 review |
| q02 | NO 0.962 review | yes 0.000 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept |
| q03 | yes 0.000 accept | yes 0.000 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 review |
| q04 | yes 0.000 review | yes 0.000 review | yes 0.000 review | yes 0.000 review | yes 0.000 review |
| q05 | NO 1.000 refuse | yes 0.000 accept | yes 0.000 review | yes 0.000 accept | yes 0.000 accept |
| q06 | yes 0.000 accept | yes 0.000 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 review |
| q07 | yes 0.000 review | yes 0.000 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept |
| q08 | NO 0.944 refuse | yes 0.000 accept | yes 0.000 accept | NO 0.028 accept | yes 0.000 accept |
| q09 | NO 5.917 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept |
| q10 | NO 5.909 review | yes 0.000 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 review |
| q11 | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept |
| q12 | NO 1.375 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept |
| q13 | NO 1.000 refuse | yes 0.000 accept | yes 0.000 accept | NO 0.059 accept | yes 0.000 accept |
| q14 | NO 1.000 refuse | yes 0.000 review | yes 0.000 accept | yes 0.000 review | yes 0.000 review |
| q15 | NO 1.000 refuse | NO 1.000 refuse | yes 0.000 accept | yes 0.000 accept | yes 0.000 accept |
| q16 | NO 9.143 review | yes 0.000 accept | yes 0.000 accept | yes 0.000 review | yes 0.000 accept |
| q17 | yes 0.000 refuse | yes 0.000 refuse | NO 1.000 review | NO 1.000 review | NO 1.000 review |
| q18 | yes 0.000 refuse | yes 0.000 refuse | NO 1.000 review | NO 1.000 review | NO 1.000 review |

| | page, DejaVu 28 pt | page, CM | product, DejaVu 28 pt | product, DejaVu fill | product, CM |
|---|---|---|---|---|---|
| mean CER over the 16 printed items | 1.766 | 0.063 | 0.000 | 0.005 | 0.000 |
| sign and operator substitutions | 0 | 0 | 0 | 0 | 0 |
| accepted sign or operator errors (gate 0) | 0 | 0 | 0 | 0 | 0 |
| accepted wrong readings of any kind | 0 | 0 | 0 | 2 (q08, q13) | 0 |
| verdicts accept / review / refuse | 4 / 7 / 7 | 7 / 8 / 3 | 14 / 4 / 0 | 13 / 5 / 0 | 10 / 8 / 0 |
| records stopped at the token cap | 8 | 1 (q17) | 1 (q18) | 1 (q18) | 1 (q18) |

CER exceeds 1 where the page prompt looped: q09, q10 and q16 repeat a reading until the cap or the
end of the page. q17 and q18 are byte-identical across every render. Their raws and confidences
(0.7543155 and 0.849663) are identical in all three product runs, as Stage 0's determinism predicts.

## The scorer

`probes/vision/score.mjs` with `normalise.mjs`, pure apart from the CLI.

- **Normalisation**, in order: strip `<loc_N>`; drop `\begin{array}{…}` and every `\end{…}`; drop
  `\left`, `\right`, `\displaystyle` and the thin spaces; `\dfrac`/`\tfrac` become `\frac`; drop
  `\text{…}`; `\colon` becomes `:`; strip a leading label; join lines; apply `EQUIVALENCES`
  (`\cdot`→`\times`, `\leq`→`\le`, `\geq`→`\ge`, `\neq`→`\ne`, `\frac12`→`\frac{1}{2}`, each anchored
  at a word boundary); canonicalise braces as Stage 0 did (S3-P4); remove all whitespace.
- **Per item:** exact is equality after normalisation. CER is the character Levenshtein distance
  over the expected length, so a repeating reading can exceed 1. Substitutions come from a token
  alignment. `+`↔`-` and `\times` against `+`, `-` or `\div` are sign substitutions. A swap between
  `=`, `<`, `>`, `\le`, `\ge` and `\ne` is an operator substitution. Both weigh ×5, and any other edit
  weighs 1. Since `b4b4131` a swap between `x`/`X`, `1`/`l`, `0`/`O`, `5`/`S` or `2`/`Z`, either way,
  is a symbol substitution, also ×5 (S3-P10). A refusal item is exact only when refused. A printed
  item's verdict is correct on `accept` or `review`. "Survives to the tutor" is `accept` with any
  sign, operator (S3-P5) or symbol substitution. Any other accepted misreading, such as a dropped
  exponent or a letter swap outside the five pairs, reaches the tutor without failing a gate. It
  counts only as a miss against the exact gate.
- **Gates** (`GATES`): exact ≥ `GATES.exactMin(total)`, which is total − 2 from 20 items and
  total − 1 below (S3-P11): 17 of 18 now, 18 of 20 with the photographs. No accepted sign, operator
  or symbol error; both refusals correct. With `--device`: p50 ≤ 10,000 ms and p95 ≤ 12,000 ms, a
  regression guard inside lever 6's 8–12 s band, not the spec's 8 s floor gate (S3-P12, S3-P16). The
  p95 is nearest rank, so over 18 records it is the slowest record. VmHWM ≤ 1,258,291 KiB. Before
  `b4b4131` the exact gate was ceil(17/18 × n), 19 at 20 items, and the p50 gate was 8,000 ms.
- **CLI:** exit 0 on pass, 1 on a gate miss, 2 on a missing or duplicate record. The last repeat
  per id is scored.

## The bench writer

`vision_bench --via-engine --json <path>` creates the file fresh and writes one line per item and
repeat: `id`, `file`, `rep`, `verdict`, `warnings`, `blocks` (`kind`, `content`, `confidence`,
`min_operator_prob`, null when the block has no operator piece), `raw`, `n_image_tokens`, `stop`,
`prep_ms`, `prefill_ms`, `decode_ms`, `total_ms`, `vmrss_kb`, `vmhwm_kb`, `error`. The verdict is the
engine's own `kpack_engine::assess(&tr.tokens, tr.stop == StopReason::MaxTokens)`, the call the
product makes (`task-3-review.md`, against `src-tauri/src/engine_inproc.rs:574-575`). Warnings are
snake_case. The app's wire format is camelCase, so the wire must not be fed to the scorer. `stop`
is Rust `Debug` casing (`Eos`, `MaxTokens`). VmHWM is the process peak, so a run's records share it
(399,928 KiB on every page-CM record).

**Two settings diverged from the product until `166ab9e`.** At `1ac6b5a`, `--via-engine` took its
prompt from `--prompt`, else from the family default, which is "Convert this page to docling." for
docling. Its cap came from `--max-tokens`, else 256. The product passes the catalog variant's
prompt, "Convert formula to latex.", and a cap of 96 (`src-tauri/src/engine_inproc.rs:525-531`). The
thresholds, the parser and `single_formula` were shared, so `assess` was the product's, but the
tokens it judged came from another prompt.

Ruling S3-P8 changed the defaults. `--via-engine` now runs "Convert formula to latex." and 96 tokens
unless a flag overrides them (`PRODUCT_FORMULA_PROMPT`, `PRODUCT_MAX_TOKENS`, `Args::via_engine_regime`
in `vision_bench.rs`). It logs `[bench] via-engine regime: prompt=… max_tokens=…` at start. The
non-engine path keeps the page prompt and 256, which Stage 0 measured. The unit test
`via_engine_defaults_to_the_product_regime` pins both. The record does not carry the prompt; the
log line and the raw shape do.

## The device run

**Run by the founder on 2026-11-14 on the Galaxy A22 (serial R9WR50C0W1J) with the archived bench
`vision_bench-aarch64-715b129` (engine at `dccd85e`), the committed Computer Modern set, the product
regime (the script passes the formula prompt and the 96-token cap), one encoder pass (64 image tokens
on every record). Evidence: `docs/superpowers/vision-stage3/device-a22-s3-v1.{jsonl,md,json,meminfo.txt}`; the device log stays beside
the run under `~/cleophis-artifacts/vision/runs/`, since `*.log` is gitignored (an earlier version of this
sentence listed it among the committed files; it never was).**

| Gate | Device value | Requirement | Result |
|---|---|---|---|
| exact | 18/18 | ≥ 17 | pass |
| accepted sign, operator or symbol errors | 0 | ≤ 0 | pass |
| refusals correct | 2/2 | ≥ 2 | pass |
| p50 total | 8,791 ms | ≤ 10,000 | pass |
| p95 total | 10,604 ms | ≤ 12,000 | pass |
| VmHWM max | 389,292 KiB | ≤ 1,258,291 | pass |

**Scorer verdict: PASS**, exit 0. What the records show:

- Every item: prefill 7,558–7,654 ms (the encoder pass; Stage 0 measured 7,609–7,653 ms for the same
  regime), decode 851–2,959 ms, total 8,508–10,604 ms. The slowest item is q18, the page of prose,
  which runs to the 96-token cap before the parser refuses it (decode 2,959 ms); every other item
  stopped at `Eos`.
- Memory: `dumpsys meminfo` of the bench at ~20 s shows PSS 383 MB (native heap 206 MB, mmap 175 MB);
  VmHWM 389 MB on every record — Stage 0's number, and the number without a WebView.
- Verdicts: 10 accept, 6 review, 2 refuse. The six reviews are exact readings held by the operator
  threshold, the same six items as on the host: q01 (0.881), q03 (0.541), q04 (0.913), q06 (0.739),
  q10 (0.544), q14 (0.916). No exact reading's confidence fell below 0.915, so the 0.90 confidence
  threshold never fired on a correct reading. q17 and q18 refused through the parser rule (S3-P9).
- The spec's strict floor gate (8 s p50) is missed by 0.79 s, as Stage 0 (8.9 s) and Stage 2's four
  readings (8,991–9,403 ms) missed it; lever 4 (runtime kernel dispatch) remains the path to it.

**The 0.86 operator proposal, read against the device run.** Lowering to 0.86 would move q01, q04 and
q14 to accept and leave q03, q06 and q10 (an `=` or `-` token at 0.54–0.74) in review — three fewer
"Use anyway" taps in sixteen. The floor still cannot be set: across 108 readings (90 host, 18 device)
no sign or operator error was ever accepted, so there is no wrong reading to keep 0.02 below. Decision
unchanged: **keep 0.97**; revisit when the photographs (q19/q20) and the handwriting work produce
wrong readings with measured operator probabilities.

The Stage 0 and Stage 2 numbers this run is compared with:

- Stage 0, floor build, formula crop, one pass, A22: p50 8.9 s, p95 9.4 s, VmHWM 389 MB, 15/15 exact
  (`verification-vision-spike.md`).
- Stage 2, the founder's A22 run: four transcription totals of 8,991, 9,403, 9,373 and 9,258 ms. Two
  of them came back review: 11:06:31 `[LowConfidence, OperatorUncertain]` and 15:34:30
  `[OperatorUncertain]` (`verification-vision-stage2.md`). Those photos have no expected LaTeX, so
  whether those readings were exact is unknown.

The founder's command, from the script's header:

```
ADB=adbw SERIAL=R9WR50C0W1J docs/superpowers/mobile-tools/run-vision-probes.sh \
  --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf \
  --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.preproc512.gguf \
  --out ~/cleophis-artifacts/vision/runs/a22-s3-v1.jsonl
```

At `166ab9e` the script's device runner passes `--prompt 'Convert formula to latex.' --max-tokens 96`
itself, so this command measures the product's regime. The script pushes the aarch64 bench from
`${CARGO_TARGET_DIR:-$HOME/cleophis-mobile-target}/aarch64-linux-android/release/examples/vision_bench`.
Build it from this branch first, so it carries the JSONL writer and the product defaults. The
script's own error message gives the command:
`cargo ndk -t arm64-v8a -P 24 build --release -p kpack-engine --features vision --example vision_bench`.
The script writes the JSONL, a log, the device-mode score (`.md`, `.json`) and one `dumpsys meminfo`
of the running bench beside `--out`.

What the scorer will judge against: since `b4b4131`, a p50 gate of 10,000 ms and a p95 gate of
12,000 ms (S3-P12). Stage 0 measured 8.9 s p50 for this regime on this phone, and Stage 2's four
readings were 8,991 to 9,403 ms. The spec's floor gate is 8 s p50. Stage 0's GO took lever 6, which
ships 8–12 s on the A22 behind a staged progress state. 10 s is a regression guard inside that band.
The strict 8 s waits on runtime kernel dispatch (lever 4). The mid-tier gate is 2.0 s and has not
been measured. A device score that passes at 10 s therefore does not meet the spec's floor gate; one
above 10 s is a regression from Stage 0 and Stage 2. The bench pushed to the phone must be built
from `dccd85e` or later, so that it carries the parser rule (S3-P9).

## CI

`.github/workflows/vision-regression.yml` runs on pull requests and pushes to `main` that touch
`crates/kpack-engine/**`, `third_party/llama-cpp-sys-2/**`, `probes/vision/**` or the workflow,
weekly, and by hand. It builds the bench and runs the 18 items in the product's regime, with both
flags explicit since `166ab9e`. It guards the record count, scores without `--device` and uploads
the report. Before the fix wave the host runs of that regime failed the scorer's refusal gate
(Calibration, last section). Since `b4b4131` the host gate run passes (Fix wave), so the job is
expected to pass on x86-64. Whether arm64 reads the same tokens is what its first run shows. The
refusal gate was in-sample: the parser rule was derived from these two images. The photo-join round's
two held-out probes (q21 a diagram labelled with numbers, q22 prose holding a date) both refuse, so
the gate is now tested on images the rule was not derived from. The 22-item device run passed on
2026-11-14 (Photo-join round, The 22-item device run).

First run: on the PR.

## Calibration

### The thresholds as they stand

`crates/kpack-engine/src/vision_parse.rs:52-54` holds `ACCEPT_MIN_CONFIDENCE` 0.90,
`REVIEW_MIN_CONFIDENCE` 0.70 and `OPERATOR_MIN_PROB` 0.97. For each LaTeX block, `assess`
(`vision_parse.rs:611`) does three checks:

- a confidence below 0.70 refuses;
- a confidence below 0.90 adds `LowConfidence`;
- a minimum operator probability below 0.97 adds `OperatorUncertain`.

A cut at the token cap adds `Truncated`. Any warning turns `accept` into `review`. Text blocks never
move the verdict. Confidence is exp(mean log-probability) over the block's tokens. The operator
probability is the lowest over pieces holding any of `+ - = < > ^ _`, a Unicode operator
`≤ ≥ ≠ × ÷ ± −` (since round 3, S3-P16) or an operator command, so `^` and `_` count as operators.

`src/vision-confirm.js:28-31` holds the amber flag's copies, `CONFIDENCE_FLAG` 0.9 and `OPERATOR_FLAG`
0.97. A node test ties the two homes: "the gate thresholds equal the engine constants in
vision_parse.rs" (`src/vision-confirm.test.mjs:414`). It reads the Rust source and asserts both
pairs equal. It came with Stage 2's fix wave (`3cd142d`, S2-P16) and is on `main` and on this branch.
`REVIEW_MIN_CONFIDENCE` has no JavaScript twin.

### What is compared

A reading clears a threshold only if its weakest LaTeX block does, because `assess` flags the reading
when any one block is below. So a reading's values here are the minima over its LaTeX blocks. A
reading is wrong when it is not exact: a printed item misread, or a refusal item not refused.
Re-deriving every verdict and warning set from the recorded blocks with `assess`'s rule reproduces
all 90 records of the five runs at the current values. That re-derivation is what the "at 0.86"
columns below use.

### Evidence: the gate run, product regime, Computer Modern (`host-v1-cm-product.jsonl`)

| id | expected | got | exact | verdict | warnings | confidence | min op prob | stop | image tokens |
|---|---|---|---|---|---|---|---|---|---|
| q01 | `3x-7=2x+5` | `3x-7=2x+5` | yes | review | operator_uncertain | 0.9836 | 0.8292 | Eos | 64 |
| q02 | `\frac{2x}{3}+\frac{x}{6}=5` | `\frac{2x}{3}+\frac{x}{6}=5` | yes | accept | — | 0.9864 | 0.9835 | Eos | 64 |
| q03 | `x^2-5x+6=0` | `x^2-5x+6=0` | yes | review | operator_uncertain | 0.9624 | 0.5484 | Eos | 64 |
| q04 | `2^{n+1}\timesa_n\le10` | `2^{n+1}\timesa_n\le10` | yes | review | operator_uncertain | 0.9924 | 0.9152 | Eos | 64 |
| q05 | `x+y=7\\x-y=1` | `x+y=7\\x-y=1` | yes | accept | — | 0.9533 | 0.9996 | Eos | 64 |
| q06 | `\sqrt{x+1}=3` | `\sqrt{x+1}=3` | yes | review | operator_uncertain | 0.9022 | 0.7492 | Eos | 64 |
| q07 | `-4x+9=-7` | `-4x+9=-7` | yes | accept | — | 0.9981 | 0.9839 | Eos | 64 |
| q08 | `\frac{3}{4}x-\frac{1}{2}=\frac{5}{8}` | `\frac{3}{4}x-\frac{1}{2}=\frac{5}{8}` | yes | accept | — | 0.9893 | 0.9958 | Eos | 64 |
| q09 | `a_1+a_2=2a_3` | `a_1+a_2=2a_3` | yes | accept | — | 0.9997 | 0.9968 | Eos | 64 |
| q10 | `0.5x+1.25=3` | `0.5x+1.25=3` | yes | review | operator_uncertain | 0.9574 | 0.5043 | Eos | 64 |
| q11 | `2(x-3)=4(x+1)` | `2(x-3)=4(x+1)` | yes | accept | — | 0.9858 | 0.9857 | Eos | 64 |
| q12 | `3x-2\ge7` | `3x-2\ge7` | yes | accept | — | 0.9974 | 0.9988 | Eos | 64 |
| q13 | `(\frac{1}{2})^x=8` | `(\frac{1}{2})^x=8` | yes | accept | — | 0.9833 | 0.9709 | Eos | 64 |
| q14 | `15\%\times80=x` | `15\%\times80=x` | yes | review | operator_uncertain | 0.9916 | 0.9193 | Eos | 64 |
| q15 | `5x=35` | `5x=35` | yes | accept | — | 0.9980 | 0.9960 | Eos | 64 |
| q16 | `x^3-x=0` | `x^3-x=0` | yes | accept | — | 0.9987 | 0.9916 | Eos | 64 |
| q17 | (refuse) | `C&&\\&&\\A&&\\B&&\\` | NO | review | low_confidence | 0.7543 | — | Eos | 64 |
| q18 | (refuse) | `&ShppinglistforSutardy:\\&milk,eggs,brea…` | NO | review | invalid_latex, low_confidence, truncated | 0.8497 | — | MaxTokens | 64 |

Expected and got are both normalised by `normalise.mjs`. q15 also has a text block, "S o l v e f o r x"
(0.9674), which does not count.

### Evidence: the product's regime, DejaVu Sans crop-filling (`host-v1-dejavu-fill-product.jsonl`)

| id | got (first 40 characters) | exact | verdict | warnings | LaTeX blocks | min confidence | min op prob | stop |
|---|---|---|---|---|---|---|---|---|
| q01 | `3x-7=2x+5` | yes | accept | — | 1 | 0.9992 | 0.9999 | Eos |
| q02 | `\frac{2x}{3}+\frac{x}{6}=5` | yes | accept | — | 1 | 0.9865 | 0.9839 | Eos |
| q03 | `x^2-5x+6=0` | yes | accept | — | 1 | 0.9999 | 0.9997 | Eos |
| q04 | `2^{n+1}\timesa_n\le10` | yes | review | operator_uncertain | 1 | 0.9898 | 0.8272 | Eos |
| q05 | `x+y=7\\x-y=1` | yes | accept | — | 1 | 0.9283 | 0.9991 | Eos |
| q06 | `\sqrt{x+1}=3` | yes | accept | — | 1 | 0.9567 | 0.9875 | Eos |
| q07 | `-4x+9=-7` | yes | accept | — | 1 | 0.9983 | 0.9862 | Eos |
| q08 | `\frac{3}{4}X-\frac{1}{2}=\frac{5}{8}` | NO | accept | — | 1 | 0.9935 | 0.9980 | Eos |
| q09 | `a_1+a_2=2a_3` | yes | accept | — | 1 | 0.9997 | 0.9999 | Eos |
| q10 | `0.5x+1.25=3` | yes | accept | — | 1 | 0.9991 | 0.9998 | Eos |
| q11 | `2(x-3)=4(x+1)` | yes | accept | — | 1 | 0.9996 | 0.9999 | Eos |
| q12 | `3x-2\ge7` | yes | accept | — | 1 | 0.9920 | 0.9998 | Eos |
| q13 | `(\frac{1}{2})^X=8` | NO | accept | — | 1 | 0.9748 | 0.9933 | Eos |
| q14 | `15\%\times80=x` | yes | review | operator_uncertain | 1 | 0.9948 | 0.9636 | Eos |
| q15 | `5x=35` | yes | accept | — | 1 | 0.9990 | 0.9996 | Eos |
| q16 | `x^3-x=0` | yes | review | operator_uncertain | 1 | 0.9684 | 0.9663 | Eos |
| q17 | `C&&\\&&\\A&&\\B&&\\` | NO | review | low_confidence | 1 | 0.7543 | — | Eos |
| q18 | `&ShppinglistforSutardy:\\&milk,eggs,brea…` | NO | review | invalid_latex, low_confidence, truncated | 1 | 0.8497 | — | MaxTokens |

### Evidence: the product's regime, DejaVu Sans 28 pt (`host-v1-product.jsonl`)

| id | got (first 40 characters) | exact | verdict | warnings | LaTeX blocks | min confidence | min op prob | stop |
|---|---|---|---|---|---|---|---|---|
| q01 | `3x-7=2x+5` | yes | accept | — | 1 | 0.9988 | 0.9999 | Eos |
| q02 | `\frac{2x}{3}+\frac{x}{6}=5` | yes | accept | — | 1 | 0.9739 | 0.9974 | Eos |
| q03 | `x^2-5x+6=0` | yes | accept | — | 1 | 0.9999 | 0.9999 | Eos |
| q04 | `2^{n+1}\timesa_n\le10` | yes | review | operator_uncertain | 1 | 0.9859 | 0.7570 | Eos |
| q05 | `x+y=7\\x-y=1` | yes | review | operator_uncertain | 1 | 0.9809 | 0.9119 | Eos |
| q06 | `\sqrt{x+1}=3` | yes | accept | — | 1 | 0.9970 | 0.9988 | Eos |
| q07 | `-4x+9=-7` | yes | accept | — | 1 | 0.9985 | 0.9885 | Eos |
| q08 | `\frac{3}{4}x-\frac{1}{2}=\frac{5}{8}` | yes | accept | — | 1 | 0.9936 | 0.9886 | Eos |
| q09 | `a_1+a_2=2a_3` | yes | accept | — | 1 | 0.9993 | 0.9997 | Eos |
| q10 | `0.5x+1.25=3` | yes | accept | — | 1 | 0.9991 | 0.9999 | Eos |
| q11 | `2(x-3)=4(x+1)` | yes | accept | — | 1 | 0.9997 | 0.9999 | Eos |
| q12 | `3x-2\ge7` | yes | accept | — | 1 | 0.9975 | 0.9996 | Eos |
| q13 | `(\frac{1}{2})^x=8` | yes | accept | — | 1 | 0.9647 | 0.9962 | Eos |
| q14 | `15\%\times80=x` | yes | accept | — | 1 | 0.9976 | 0.9995 | Eos |
| q15 | `5x=35` | yes | accept | — | 1 | 0.9989 | 0.9997 | Eos |
| q16 | `x^3-x=0` | yes | accept | — | 1 | 0.9996 | 0.9996 | Eos |
| q17 | `C&&\\&&\\A&&\\B&&\\` | NO | review | low_confidence | 1 | 0.7543 | — | Eos |
| q18 | `&ShppinglistforSutardy:\\&milk,eggs,brea…` | NO | review | invalid_latex, low_confidence, truncated | 1 | 0.8497 | — | MaxTokens |

### Evidence: page-prompt run, Computer Modern (`host-v1-cm.jsonl`, not gate evidence)

| id | expected | got | exact | verdict | warnings | confidence | min op prob | stop | image tokens |
|---|---|---|---|---|---|---|---|---|---|
| q01 | `3x-7=2x+5` | `3x-7=2x+5` | yes | review | operator_uncertain | 0.9887 | 0.8808 | Eos | 64 |
| q02 | `\frac{2x}{3}+\frac{x}{6}=5` | `\frac{2x}{3}+\frac{x}{6}=5` | yes | review | operator_uncertain | 0.9904 | 0.8776 | Eos | 64 |
| q03 | `x^2-5x+6=0` | `x^2-5x+6=0` | yes | review | operator_uncertain | 0.9717 | 0.6378 | Eos | 64 |
| q04 | `2^{n+1}\timesa_n\le10` | `2^{n+1}\timesa_n\le10` | yes | review | operator_uncertain | 0.9877 | 0.9364 | Eos | 64 |
| q05 | `x+y=7\\x-y=1` | `x+y=7\\x-y=1` | yes | accept | — | 0.9483 | 0.9954 | Eos | 64 |
| q06 | `\sqrt{x+1}=3` | `\sqrt{x+1}=3` | yes | review | operator_uncertain | 0.9601 | 0.8998 | Eos | 64 |
| q07 | `-4x+9=-7` | `-4x+9=-7` | yes | review | operator_uncertain | 0.9867 | 0.8699 | Eos | 64 |
| q08 | `\frac{3}{4}x-\frac{1}{2}=\frac{5}{8}` | `\frac{3}{4}x-\frac{1}{2}=\frac{5}{8}` | yes | accept | — | 0.9928 | 0.9785 | Eos | 64 |
| q09 | `a_1+a_2=2a_3` | `a_1+a_2=2a_3` | yes | accept | — | 0.9995 | 0.9960 | Eos | 64 |
| q10 | `0.5x+1.25=3` | `0.5x+1.25=3` | yes | review | operator_uncertain | 0.9801 | 0.7338 | Eos | 64 |
| q11 | `2(x-3)=4(x+1)` | `2(x-3)=4(x+1)` | yes | accept | — | 0.9775 | 0.9862 | Eos | 64 |
| q12 | `3x-2\ge7` | `3x-2\ge7` | yes | accept | — | 0.9980 | 0.9984 | Eos | 64 |
| q13 | `(\frac{1}{2})^x=8` | `(\frac{1}{2})^x=8` | yes | accept | — | 0.9763 | 0.9881 | Eos | 64 |
| q14 | `15\%\times80=x` | `15\%\times80=x` | yes | review | operator_uncertain | 0.9923 | 0.9283 | Eos | 64 |
| q15 | `5x=35` | (empty) | NO | refuse | no_math_found | no block | — | Eos | 64 |
| q16 | `x^3-x=0` | `x^3-x=0` | yes | accept | — | 0.9988 | 0.9923 | Eos | 64 |
| q17 | (refuse) | (no LaTeX) | yes | refuse | no_math_found, truncated | no LaTeX block (1 text) | — | MaxTokens | 64 |
| q18 | (refuse) | (no LaTeX) | yes | refuse | no_math_found | no LaTeX block (2 text) | — | Eos | 64 |

### Evidence: page-prompt run, DejaVu Sans 28 pt (`host-v1.jsonl`, not gate evidence)

| id | got (first 40 characters) | exact | verdict | warnings | LaTeX blocks | min confidence | min op prob | stop |
|---|---|---|---|---|---|---|---|---|
| q01 | `3x-7=2x+5` | yes | accept | — | 1 | 0.9979 | 0.9945 | Eos |
| q02 | `<loc_49` | NO | review | truncated | 1 | 0.9986 | 1.0000 | MaxTokens |
| q03 | `x^2-5x+6=0` | yes | accept | — | 1 | 0.9994 | 0.9958 | Eos |
| q04 | `2^{n+1}\timesa_n\le10` | yes | review | operator_uncertain | 1 | 0.9951 | 0.9389 | Eos |
| q05 | (no LaTeX) | NO | refuse | no_math_found, truncated | 0 | — | — | MaxTokens |
| q06 | `\sqrt{x+1}=3` | yes | accept | — | 1 | 0.9935 | 0.9790 | Eos |
| q07 | `-4x+9=-7` | yes | review | operator_uncertain | 1 | 0.9938 | 0.9430 | Eos |
| q08 | `=\\=` | NO | refuse | low_confidence, operator_uncertain, truncated | 2 | 0.5282 | 0.5282 | MaxTokens |
| q09 | `a$_1$\\a$_2$\\a$_3$\\a$_4$\\a$_5$\\a$_6$…` | NO | review | low_confidence, operator_uncertain, truncated | 11 | 0.7527 | 0.6484 | MaxTokens |
| q10 | `0.5x+1.25=3\\0.5x+1.25=3\\0.5x+1.25=3\\0…` | NO | review | low_confidence, operator_uncertain | 6 | 0.8130 | 0.8311 | Eos |
| q11 | `2(x-3)=4(x+1)` | yes | accept | — | 1 | 0.9993 | 0.9956 | Eos |
| q12 | `3x-2≥7\\<loc_441` | NO | review | low_confidence, operator_uncertain, truncated | 2 | 0.8065 | 0.6663 | MaxTokens |
| q13 | (no LaTeX) | NO | refuse | no_math_found, truncated | 0 | — | — | MaxTokens |
| q14 | (no LaTeX) | NO | refuse | no_math_found | 0 | — | — | Eos |
| q15 | (no LaTeX) | NO | refuse | no_math_found | 0 | — | — | Eos |
| q16 | `x3-x=0\\x3-x=0\\x3-x=0\\x3-x=0\\x3-x=0\\…` | NO | review | low_confidence, operator_uncertain, truncated | 9 | 0.7396 | 0.7016 | MaxTokens |
| q17 | (no LaTeX) | yes | refuse | no_math_found, truncated | 0 | — | — | MaxTokens |
| q18 | (no LaTeX) | yes | refuse | no_math_found | 0 | — | — | Eos |

Two of these wrong readings are fragments of a tag cut at the 256-token cap: q02's `<loc_49`, and
q12's second block `<loc_441` (0.9102, 0.9999). The parser classifies them as LaTeX.

### The calibration numbers

| | product, CM | product, DejaVu fill | product, DejaVu 28 pt | page, CM | page, DejaVu 28 pt |
|---|---|---|---|---|---|
| lowest confidence, exact printed reading | 0.9022 (q06) | 0.9283 (q05) | 0.9647 (q13) | 0.9483 (q05) | 0.9935 (q06) |
| highest confidence, wrong reading | 0.8497 (q18) | 0.9935 (q08, accepted); then 0.9748 (q13, accepted) | 0.8497 (q18) | none: q15 has no LaTeX block | 0.9986 (q02, the `<loc_49` fragment); next 0.8130 (q10) |
| lowest min op prob, exact printed reading | 0.5043 (q10) | 0.8272 (q04) | 0.7570 (q04) | 0.6378 (q03) | 0.9389 (q04) |
| highest min op prob, wrong reading | none: q17 and q18 have no operator piece | 0.9980 (q08, accepted); then 0.9933 (q13) | none | none | 1.0000 (q02, 0.99999994); next 0.8311 (q10) |
| sign or operator errors | 0 | 0 | 0 | 0 | 0 |
| exact printed readings below 0.90 confidence | 0 | 0 | 0 | 0 | 0 |
| exact printed readings below 0.70 confidence | 0 | 0 | 0 | 0 | 0 |
| exact printed readings below 0.97 min op prob | 6 | 3 | 2 | 8 | 2 |

There are no sign or operator errors to compare `min_operator_prob` against, accepted or not. The
lowest operator probability on an exact reading is 0.5043 (q10, product regime, CM). The same item's
weakest operator token moves a long way while the reading stays exact. q10 runs from 0.5043 (CM) to
0.9999 (DejaVu 28 pt) under the product's prompt. q03 runs from 0.9958 (page, DejaVu) to 0.5484
(product, CM). The record carries only the minimum, so which piece is weak is not known; it may be a
`^` as easily as a `-`.

The two accepted `X` readings sit above every exact reading's minimum in their own run, on both
confidence (0.9935 and 0.9748, against 0.9283) and operator probability (0.9980 and 0.9933, against
0.8272). A threshold high enough to send them to review would send most of that run's correct
readings there too.

### Which threshold put each exact reading in review

Every exact printed reading in review, in every run, is there for `OperatorUncertain` alone. Its
confidence is at least 0.90, and it has no other warning.

| run | id | confidence | min op prob |
|---|---|---|---|
| product, CM | q01 | 0.9836 | 0.8292 |
| product, CM | q03 | 0.9624 | 0.5484 |
| product, CM | q04 | 0.9924 | 0.9152 |
| product, CM | q06 | 0.9022 | 0.7492 |
| product, CM | q10 | 0.9574 | 0.5043 |
| product, CM | q14 | 0.9916 | 0.9193 |
| product, DejaVu fill | q04 | 0.9898 | 0.8272 |
| product, DejaVu fill | q14 | 0.9948 | 0.9636 |
| product, DejaVu fill | q16 | 0.9684 | 0.9663 |
| product, DejaVu 28 pt | q04 | 0.9859 | 0.7570 |
| product, DejaVu 28 pt | q05 | 0.9809 | 0.9119 |
| page, CM | q01 | 0.9887 | 0.8808 |
| page, CM | q02 | 0.9904 | 0.8776 |
| page, CM | q03 | 0.9717 | 0.6378 |
| page, CM | q04 | 0.9877 | 0.9364 |
| page, CM | q06 | 0.9601 | 0.8998 |
| page, CM | q07 | 0.9867 | 0.8699 |
| page, CM | q10 | 0.9801 | 0.7338 |
| page, CM | q14 | 0.9923 | 0.9283 |
| page, DejaVu | q04 | 0.9951 | 0.9389 |
| page, DejaVu | q07 | 0.9938 | 0.9430 |

### What holds each wrong reading out of `accept`

| run | id | what the model wrote | min confidence | min op prob | held by |
|---|---|---|---|---|---|
| product, DejaVu fill | q08 | `\frac{3}{4}X-\frac{1}{2}=\frac{5}{8}` | 0.9935 | 0.9980 | nothing: accepted |
| product, DejaVu fill | q13 | `\left(\frac{1}{2}\right)^{X}=8` | 0.9748 | 0.9933 | nothing: accepted |
| product, all three | q17 | the triangle's labels as a LaTeX array | 0.7543 | — | `low_confidence` |
| product, all three | q18 | the prose letter by letter in a LaTeX array, cut at 96 tokens | 0.8497 | — | `low_confidence`, `invalid_latex`, `truncated` |
| page, CM | q15 | a `<picture>` | — | — | no maths (refuse) |
| page, DejaVu | q02 | eleven `n x` text rows, then `<loc_49` at the cap | 0.9986 | 1.0000 | `truncated` |
| page, DejaVu | q05, q13 | no LaTeX block, cut at the cap | — | — | no maths (refuse), `truncated` |
| page, DejaVu | q08 | two `=` fragments among the fraction's pieces | 0.5282 | 0.5282 | below 0.70 (refuse), `low_confidence`, `operator_uncertain`, `truncated` |
| page, DejaVu | q09 | `a$_1$` … `a$_{10}$`, a counting loop | 0.7527 | 0.6484 | `low_confidence`, `operator_uncertain`, `truncated` |
| page, DejaVu | q10 | the right formula six times | 0.8130 | 0.8311 | `low_confidence`, `operator_uncertain` |
| page, DejaVu | q12 | `3x-2≥7`, page-number rows, `<loc_441` | 0.8065 | 0.6663 | `low_confidence`, `operator_uncertain`, `truncated` |
| page, DejaVu | q14, q15 | a `<picture>` | — | — | no maths (refuse) |
| page, DejaVu | q16 | `x3-x=0` nine times, the exponent lost | 0.7396 | 0.7016 | `low_confidence`, `operator_uncertain`, `truncated` |

No wrong reading in any run is held by the operator threshold alone. Two are held by nothing: the
`X` readings. In each, `X` is the problem's only unknown, so the tutor would get the same problem
under another name. The same failure on a problem with both `x` and `X`, or on a digit, would teach
a different one. The page prompt's highest-confidence wrong reading, q02's `<loc_49` at 0.9986 and
1.0000, is held only by `truncated`.

### The decision, by the brief's rule

The rule: keep 0.90 / 0.70 / 0.97 unless (a) a threshold would let a sign error through as `accept`
(then raise), or (b) a threshold pushes at least two exact readings into `review` with no wrong
reading between (then consider lowering, never below the highest wrong reading + 0.02).

- **(a) does not fire.** There is no sign or operator substitution in any of the 90 readings, so no
  threshold lets one through. Nothing is raised. The two accepted `X` readings are not sign errors,
  and raising a threshold would not catch them (above).
- **(b), `ACCEPT_MIN_CONFIDENCE` 0.90: does not fire.** No exact printed reading in any run is below
  0.90. In the product's CM and 28 pt DejaVu runs the highest wrong reading is q18 at 0.8497, and the
  lowest exact readings are 0.9022 (CM q06) and 0.9647 (DejaVu q13), so 0.90 sits between them. In
  the crop-filling run the `X` readings sit above every exact reading. Kept.
- **`REVIEW_MIN_CONFIDENCE` 0.70: does not fire.** No exact reading in any run is below 0.70. The one
  reading it refuses is page-DejaVu q08 (0.5282). Kept.
- **(b), `OPERATOR_MIN_PROB` 0.97: fires on the count, but the floor cannot be set.** It puts 6, 3, 2,
  8 and 2 exact readings into review in the five runs (product CM, product DejaVu fill, product
  DejaVu 28 pt, page CM, page DejaVu). The rule allows considering a lower value, never below the
  highest wrong reading + 0.02. This evidence cannot fix that floor, for three reasons:
  1. The threshold exists to catch sign and operator errors, and none occurs in any run.
  2. In the product's regime, the only wrong readings with an operator probability are the
     accepted `X` readings, at 0.9980 and 0.9933, above the current threshold. q17's and q18's
     LaTeX blocks hold no operator piece. A floor set against q08's 0.9980 would be 1.0180, which is
     not a probability.
  3. Across all runs the highest wrong reading is page-DejaVu q02 at 1.0000. No value can sit 0.02
     above every wrong reading, so the condition for making the change in this task fails.

  **Kept at 0.97. No constant or test changes.** The host bench was not re-run, because nothing
  changed.

### The proposal, pending the device run

Set aside the wrong readings that sit above 0.97, because no lower threshold changes them: q02's
fragment, held by `truncated`, and the two `X` readings, already accepted. The highest remaining wrong
reading with an operator probability is page-DejaVu q10 at 0.8311. Adding 0.02 gives
0.8511, and the constants' two-decimal grid makes that 0.86. That is the only floor this evidence can
compute, and it comes from the page prompt, a regime the product does not run.

**Proposed: `OPERATOR_MIN_PROB` and `OPERATOR_FLAG` 0.97 → 0.86, pending the device run.** Adopt it
only if three conditions hold:

- The device JSONL, run in the product's regime with the founder's two photographs added, goes
  through this same rule.
- 0.86 still stays at least 0.02 above every wrong reading there whose weakest operator token is
  below 0.97.
- The change lands as one commit with both homes and their tests.

What 0.86 would do here, re-derived from the recorded blocks:

| run | exact readings in review at 0.97 | at 0.86 | still in review at 0.86 | wrong readings that become `accept` |
|---|---|---|---|---|
| product, CM | 6 | 4 | q01 (0.8292), q03, q06, q10 | none |
| product, DejaVu fill | 3 | 1 | q04 (0.8272) | none (q08 and q13 are accepted already) |
| product, DejaVu 28 pt | 2 | 1 | q04 (0.7570) | none |
| page, CM | 8 | 2 | q03, q10 | none |
| page, DejaVu | 2 | 0 | — | none |

**The trade.** A lower threshold saves the student a tap. Each exact reading the operator threshold
sends to review costs a "Use anyway", or an edit, before the tutor starts. The cost is the risk that
a wrong problem gets taught. A sign or operator error whose weakest operator token sits between
0.86 and 0.97 would pass this guard. If its confidence were at least 0.90 and it were not truncated,
it would be accepted, and the tutor would teach the wrong problem. This evidence contains no sign
error, so that risk is unmeasured, not shown to be zero. The `X` readings show that a confident
misreading can come with a strong operator probability. In the product's CM run, the gap between the
exact readings that would come out of review (0.9152, 0.9193) and those that stay (0.8292 and
below) is wide. The page-DejaVu wrong reading that sets the floor sits only 0.029 below it.

**The sample.** 18 typeset items, one x86-64 host, one repeat per run, three renders, two prompts.
No photograph, no device run. (The photo-join round later added the photographs and ran the A22 with all 22 items; both passed.) The device run should use the product's prompt and cap.

### Outside the rule: the refusals fail in the product's regime

In all three product runs the diagram and the prose come back `review`, not `refuse`. The formula prompt
makes the model write them as LaTeX arrays, and `classify` accepts both arrays as LaTeX. Their
confidences, 0.7543 (q17) and 0.8497 (q18), clear the 0.70 floor. The spec's "correct refusal" gate
fails: the scorer reports refusals 0 of 2. The student would see amber maths with "Use anyway"
offered, which Stage 2 allows whenever a non-blank LaTeX block exists (S2-P10).

The brief's rule covers only raising for a sign error and lowering for exact readings in review, so
this task changes nothing for it. Two directions are open, and both are for the controller or the
founder:

- **A higher refuse floor.** On the three product runs, a floor of 0.87 or 0.88 would refuse both
  items and keep every exact printed reading with a margin of at least 0.02. The bounds are
  0.8497 + 0.02 = 0.8697 and 0.9022 − 0.02 = 0.8822. That leaves `LowConfidence` a review band of
  only 0.88–0.90, and it rests on two items.
- **A parser check.** An array whose cells are single letters or letter-spaced words is not maths.

The Stage 2 device run refused one non-maths frame (15:33:38, `Refuse [NoMathFound]`), so the
failure depends on the image.

**Resolved by the parser (S3-P9, tightened by S3-P17).** The controller took the second direction: a
LaTeX block with no operator is text (S3-P9 also let a digit rescue it; S3-P17 removed that). The refuse floor stays 0.70. On the re-run both items refuse with
`no_math_found` (Fix wave). The parser check is wider than "an array of single letters". It
refuses any block that lacks an operator token, whatever its layout.

## Photo-join round (2026-11-14)

**What joined.** Four items, taking the set from 18 to 22 (`c5b3908`, branch `vision/stage3-photos`):

- `q19-photo-upright` and `q20-photo-rotated`, kind `photo`, expected `accept`, the formula of q03
  (`x^2 - 5x + 6 = 0`). Each is a crop of one of the founder's two photographs of the sheet printed
  from our own typeset items, one photograph per orientation. q19 is 480x140 with EXIF orientation 1.
  q20 is 170x520 raw pixels with EXIF orientation 6, so it is upright only after the orientation is
  applied, which is the point of the probe. The EXIF holds only the orientation tag. The full
  photographs stay outside the repo. The crops are CC0 and are never re-encoded: `gen.py` hashes them
  into `manifest.json` and lists them in `items.json`, and its `--check` compares both.
- `q21-diagram-345` (a right triangle drawn with lines, its sides labelled 3, 4 and 5, no equation)
  and `q22-dated-prose` (one line, "Dentist on 14 November 2026 at 3 pm"), typeset like q17 and q18,
  expected `refuse`. They are held-out: the refusal rule was derived from q17 and q18 only.

**The counts that moved.** 22 items: 16 printed, 2 photographs, 4 refusals. The exact gate is
`GATES.exactMin(22)`, 20 of 22. `GATES.refusalsMin` is now a function of the items, the number whose
expected verdict is `refuse`, so 4 here, and the Markdown prints the applied value. The manifest test
counts 22 files and 22 items, adds `photo` to the kind domain and pins the id, verdict and kind order.
The CI guard reads the record count from `items.json` instead of a constant. Thresholds, parser and
scorer logic are unchanged.

**The run.** The host bench in the product's regime (`host-v2-cm-product.jsonl`, engine and parser
at `512fc75`, formula prompt, cap 96, the explicit `--via-engine` flags, one pass). Scorer at the
round's tree. Timings are host timings and not a gate.

**Scoreboard: PASS.**

| Gate | Value | Requirement |
|---|---|---|
| exact | 22/22 | >= 20: total - 2 (20 or more items) |
| accepted sign, operator or symbol errors | 0 | <= 0 |
| refusals correct | 4 | >= 4 (every refuse item) |

Items q01 to q18 read as in round 4: all exact, verdicts unchanged (accept for q02, q05, q07 to q09,
q11 to q13, q15, q16, and review for q01, q03, q04, q06, q10, q14; q17 and q18 refuse).

| Item | Verdict | Block, confidence, min operator prob | Raw head |
|---|---|---|---|
| q19 photo upright | accept (expected accept), exact | 1 latex block `x^{2}-5x+6=0`, 0.9999, 0.9988 | `<formula>...x ^ { 2 } - 5 x + 6 = 0</formula>` |
| q20 photo rotated | accept (expected accept), exact | 1 latex block `x^{2}-5x+6=0`, 0.9983, 0.9980 | `<formula>...x ^ { 2 } - 5 x + 6 = 0</formula>` |
| q21 diagram 3-4-5 | refuse (expected refuse) | 1 latex block `3`, 0.6945, none; warning `low_confidence` | `<formula>...\begin{array} { l } { 3 } \end{array}</formula>` |
| q22 dated prose | refuse (expected refuse) | 1 text block (the array-wrapped words), 0.8869, none; warning `no_math_found` | `<formula>...\begin{array} { l } { D e n t i s t o n \, 1 4 \, November \, 2 0 2 6 \, a t \, 3 \, p m } \end{array}</formula>` |

**Reading it.** Every gate passes and nothing was changed to make it so. Three observations:

- The photographs were read better than their typeset source. q03 sits in review on this run
  (operator probability 0.548, `operator_uncertain`), while q19 and q20 are accepted with a minimum
  operator probability above 0.99. The orientation tag is applied: q20 reads the same as q19.
  Two photographs of one formula are a thin sample, not a claim about photographs in general.
- q21 is refused only by the confidence floor. Its block confidence is 0.6945 and the review floor
  (`REVIEW_MIN_CONFIDENCE`) is 0.70, a margin of 0.006. The model emitted a one-cell array holding
  `3`, not nothing. A re-render with a different label size could land on the other side. The refusal
  gate is still in-sample for the parser's wording, and q21 is the weakest of the four.
- q22 is refused through `no_math_found`: the words came back as a text block inside a formula tag,
  which the parser's prose guard catches. The date digits did not turn it into maths.

**Round 2 (S3-P17): q21 refuses by rule.** The first run exposed a gap. The model wrote q21's triangle
as a one-cell array holding `3`, and the S3-P9 rule ("no digit and no operator is not maths") let it
through because of the digit. Only the 0.70 confidence floor refused it, by 0.006. A bare number is not
a problem to teach, so `4844f10` drops the digit clause: a block is maths only if it holds an operator,
a relation or a structural token (`= < > + - ^ _`, the maths commands, the Unicode operators). `3`,
`42` and `3x` are text; `x^2`, `a_1`, `\frac{1}{2}`, `3x-7=2x+5` and `a ≤ b` stay maths, and a formula
beside a bare `3` keeps the formula. The digit had also been carrying `x \leqslant 2`, which the
operator guard already watched but the maths list lacked, so `\leqslant \geqslant \lt \gt` joined the list.
`cargo test -p kpack-engine` passes (177) and the Android `-D warnings` check finishes clean.

Re-run on the same 22 items (`host-v2-cm-product-r2`, same command and models): **PASS, exact 22/22,
refusals 4/4, no accepted sign or operator error.** All 22 raws are byte-identical to the first run.
Only q21 changed: it is still `refuse`, but now with `no_math_found` and a text block (confidence
0.6945, unchanged) instead of `low_confidence` and a latex block. Every other record's verdict, blocks and
warnings are unchanged. The refusal no longer depends on the confidence floor for q21.

**The 22-item device run (2026-11-14, 12:53–12:56 +07:00).** Run from this branch on the Galaxy A22
(serial R9WR50C0W1J) with `run-vision-probes.sh`, the bench rebuilt from `f998395` (archived as
`~/cleophis-artifacts/vision/vision_bench-aarch64-f998395`, sha256 `c6d274ce…`), the committed 22 items,
the product regime (the script passes the formula prompt and the 96-token cap), one encoder pass (64
image tokens on every record). Scorer at `f998395` with `--device`. Evidence:
`docs/superpowers/vision-stage3/device-a22-s3-v2.{jsonl,md,json,meminfo.txt}`; the device log stays
beside the run under `~/cleophis-artifacts/vision/runs/`, since `*.log` is gitignored.

**Scoreboard: PASS**, exit 0.

| Gate | Device value | Requirement | Result |
|---|---|---|---|
| exact | 22/22 | ≥ 20 | pass |
| accepted sign, operator or symbol errors | 0 | ≤ 0 | pass |
| refusals correct | 4/4 | ≥ 4 | pass |
| p50 total | 8,733 ms | ≤ 10,000 | pass |
| p95 total | 9,870 ms | ≤ 12,000 | pass |
| VmHWM max | 389,188 KiB | ≤ 1,258,291 | pass |

What the records show:

- The 18 typeset items read exactly as in the first device run: all 18 raws are byte-identical to
  `device-a22-s3-v1`, with the same verdicts (accept for q02, q05, q07–q09, q11–q13, q15, q16; review
  for q01, q03, q04, q06, q10, q14, held by the operator threshold at minimum operator probabilities
  0.881, 0.541, 0.913, 0.739, 0.544, 0.916; refuse for q17 and q18). Over all 22: 12 accept, 6 review,
  4 refuse.
- The photographs are accepted on the phone as on the host: q19 at confidence 0.9999 and minimum
  operator probability 0.999; q20, the EXIF-orientation-6 crop, at 0.9985 and 0.9981, and its raw
  equals q19's. The phone applies the orientation tag as the host does.
- q21 and q22 refuse with `no_math_found`: q21 by the S3-P17 rule (a text block at 0.6949, so the 0.70
  floor would still have caught it), q22 through the prose guard (a text block at 0.883). Confidences
  differ from the host's in the third decimal (host 0.6945 and 0.8869); the verdicts and warnings match.
- Timings: prefill 7,580–7,625 ms (the encoder pass), decode 865–2,942 ms, total 8,510–10,613 ms. The
  slowest item is again q18, the only one to run to the 96-token cap (10,613 ms). Over 22 records the
  nearest-rank p95 is the 21st value, q22 at 9,870 ms, so q18 no longer sets p95 as it did over 18.
  p50 8,733 ms is 58 ms under the first run's 8,791 and 0.73 s over the spec's strict 8 s floor; lever 4
  (runtime kernel dispatch) is unchanged as the path to it.
- Memory: VmHWM 389,100–389,188 KiB across the records (the first run: 389,292). `dumpsys meminfo` at
  ~20 s shows PSS 383 MB (native heap 206 MB, mmap 175 MB): the bench alone, without a WebView.
- Across architectures: 21 of 22 raws are byte-identical to the host r2 run. The one that differs is
  q18, the page of prose, where greedy decoding diverges before the 96-token cap (the phone emits a
  two-column array of letters, the host the shopping-list words). Both refuse with `no_math_found`; the
  two device runs agree with each other and the two host runs with each other, and the first device run
  differed from `host-v1-cm-product-r4` on q18 alone in the same way. The scorer never compares raws
  between runs and a refuse item has no expected LaTeX, so this costs nothing today. A per-architecture
  expectation would be needed only if a raw were ever pinned; the CI section's arm64 question ("whether
  arm64 reads the same tokens") is answered: the same tokens on 21 of 22 items, and never on the gate's terms.
- The 0.86 operator proposal gains no evidence: the 22 device readings add no wrong reading with a
  measured operator probability, so a floor still cannot be set and 0.97 stays (The device run).

The command, from the `s3-photos` checkout at `f998395` (the bench was built from the same tree with
`cargo ndk -t arm64-v8a -P 24 build -p kpack-engine --features vision --release --example vision_bench`):

```
ADB=adbw SERIAL=R9WR50C0W1J docs/superpowers/mobile-tools/run-vision-probes.sh \
  --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf \
  --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.preproc512.gguf \
  --bench ~/cleophis-artifacts/vision/vision_bench-aarch64-f998395 \
  --out ~/cleophis-artifacts/vision/runs/a22-s3-v2.jsonl
```

No Rust changed in this round, so no Android or Windows run was made.

## Rulings

From the SDD ledger, one line each.

| Ruling | What was decided |
|---|---|
| S3-P1 | Stage 3 executes after the copy pass and the dependency PRs are opened. It was amended three times so that Tasks 1–2, then 3, then 4 started early, each sharing no surface with the other branches. |
| S3-P2 | 18 items now, with the exact gate scaled to ≥ 17/18. The founder's two photographs join later, with the gate restored to ≥ 18/20. |
| S3-P3 | Task 2 ran in a side worktree in parallel with Task 1, on disjoint files, and was rebased and fast-forwarded after both landed. |
| S3-P4 | The scorer ports Stage 0's brace canonicalisation (`x^{2}` ≡ `x^2`, `\frac12` ≡ `\frac{1}{2}`, grouping braces unwrapped, argument braces kept). |
| S3-P5 | Operator substitutions count toward "survives to the tutor" (fail closed: an accepted `\le`→`\ge` teaches the wrong problem). |
| S3-P6 | Re-render the same 18 items in Computer Modern at a crop-filling size, and keep the DejaVu scoreboard as a font-sensitivity finding. The product-regime runs show the 8/18 was the page prompt; a crop-filling DejaVu render still misreads two items, `x` as `X`. |
| S3-P7 | Task 6 starts on the host evidence. The device run is the founder's and is reported as not run. The CI job's first run is on the PR. |
| S3-P8 | After Task 6's finding: `--via-engine` defaults to the product's regime ("Convert formula to latex.", 96 tokens) with a unit test, and both runners pass the flags explicitly. The two earlier scoreboards are page-prompt runs and are not the gate evidence; the product-regime CM run is. The font-sensitivity claim stands only if a DejaVu render reproduces it under the product's prompt, and `gen.py` gains `--out`/`--fontset` to render one. |
| S3-P9 | A LaTeX block with no digit and no operator or relation token (`= < > \le \leq \ge \geq \ne \neq + - \times \cdot \div \frac \sqrt ^ _`) is not maths. A result with no other LaTeX block refuses with `NoMathFound`. |
| S3-P10 | The scorer counts look-alike swaps (`x`/`X`, `1`/`l`, `0`/`O`, `5`/`S`, `2`/`Z`) as symbol substitutions, weighted ×5, and as surviving to the tutor when accepted. |
| S3-P11 | The exact gate scales by the spec: total − 2 from 20 items (18/20), total − 1 below (17/18). The photographs join as q19 and q20, and the plan's q17/q18 ids are corrected. |
| S3-P12 | The device gates: p50 ≤ 10,000 ms, p95 ≤ 12,000 ms. Reworded by S3-P16: The spec's floor gate is 8 s p50. Stage 0's GO took lever 6, which ships 8–12 s on the A22 behind a staged progress state. 10 s is a regression guard inside that band. The strict 8 s waits on runtime kernel dispatch (lever 4). The mid-tier gate is 2.0 s and has not been measured. |
| S3-P13 | The JSONL evidence is committed under `docs/superpowers/vision-stage3/`, the page-prompt runs labelled as such. |
| S3-P14 | No separate task review for Task 6's record; the whole-branch review verifies it after the fix wave. |
| S3-P15 | The maths-token set widens to the commands and Unicode operators letters-only maths is written with (`\pm`, `\dfrac`, `\int`, `\lim`, `\in`, `\cong`, `\angle`, `≤`, …), compared as whole tokens. |
| S3-P16 | From the final review: the operator guard sees the Unicode operators `≤ ≥ ≠ × ÷ ± −` and the commands `\mp \leqslant \geqslant \lt \gt` (I1). The floor gate's reason is reworded (I2). The refusal gate is stated as in-sample, and two held-out probes join with the photographs (I3). |

## Deferred minors

From the SDD ledger and the reviews, per task.

- **Task 1:** the PNGs are RGBA, which is harmless because the engine composites alpha over white;
  changing it would re-pin 18 hashes. The manifest test pins the files, not the 16 LaTeX strings,
  which only `gen.py --check` guards. q17 carries a right-angle mark, and q18's prose is 24 pt to fit.
  Round 1's small mathtext fractions were replaced by round 2's crop-filling size. Rounds 2 and 3
  were approved on re-review. Round 3: an unknown `--fontset` fails with a raw traceback.
- **Task 2:** the refusal gate needs at least two refusal items, so a hand-made set needs two to exit
  0. Refusal items count toward the exact total, and operator swaps weigh ×5 like signs. Alignment
  ties follow a fixed backtrace order, so an equal-cost alignment could count substitutions
  differently. `\nu` and `\neg` tokenise as `\ne` plus a letter, which is harmless to the score.
- **Task 3:** the unit test pins the field mapping, not the whole 16-key record (a Python check over
  `host-v1.jsonl` did). An error mid-transcription leaves `verdict` null and `warnings` empty, and the
  scorer keys off `error`. `stop` is `Debug` casing. `--repeat 0` writes an empty file. The ledger asked
  for the token cap to be re-judged on the CM run. On the page-prompt CM run only q17 reached the cap,
  and in the product's regime only q18 reached the product's 96. The product's cap stands, and since
  `166ab9e` the bench uses it. Round 1's test was written with the code, with no separate red run.
- **Task 4:** the default `STAGE` hard-codes one Windows profile path; the override is documented.
  Untested on hardware: `pidof`, `kill -0` and `dumpsys` as the shell user, and `nohup` under
  `adb.exe`. If the bench finishes in under about 20 s, the meminfo snapshot is skipped and VmHWM is
  the fallback.
- **Task 5:** a guard failure writes no step summary, though the artifact still uploads. The cache key
  uses 12-character sha prefixes, which is safe because verification is separate. The model cache is
  saved only on job success. The path filters omit `Cargo.lock` and `Cargo.toml`. The 18-line guard
  does not read per-record `error`; the scorer does. The arm64 build time inside the 90-minute
  timeout is unverified, and `BINDGEN_EXTRA_CLANG_ARGS` is not set, which the host build needed.
- **Task 6:** the record has no per-piece operator probabilities. The five JSONL files are now
  committed (S3-P13).
- **Fix wave:** the S3-P9 rule reads the element's raw text, so an array column spec such as
  `p{2cm}` is read with the rest. Since S3-P17 its digit no longer counts as maths; only an operator does. The symbol pairs are
  the five the ruling names. Other case swaps (`a`/`A`, `n`/`N`) and look-alikes (`1`/`I`, `0`/`o`)
  are still plain edits. Round 2 corrected the plan's Task 2 interface line to the code's `GATES`.
- **Final review, deferred** (`final-review-report.md`, Minor list):
  - M1: the further letters-only commands that refuse are listed under What Stage 4 inherits.
  - M4: the scorer does not count `\times` read as the letter `x` (`15\%x80=x` scores weight 1,
    tutor false), nor `\cdot` against `.` or `\div`/`\pm` against `+`/`−`. One `LOOK_ALIKES` pair and
    two sign pairs, with tests.
  - M6: the bench's product prompt and cap (`vision_bench.rs`, `run-vision-probes.sh`,
    `vision-regression.yml`) are not tied to `catalog.json` by a test. A node test in the style of
    the thresholds test would keep S3-P8's trap shut if the catalog changes.
  - M7: the ledger's S3-P9 text says `x+y` stays maths. It is text, as it was before this branch;
    this record gives the measured behaviour.

## What Stage 4 inherits

- **A prose-likeness heuristic.** The parser rule (S3-P9, tightened by S3-P17) refuses an array of
  letters and a block with no operator, so a bare number or a diagram labelled with plain numbers
  refuses (q21). Prose or a diagram holding an operator-bearing fragment (a hyphen or a dash, a Unicode
  sign) still classifies by the word count alone and lands in `review`, with "Use anyway" offered. The Unicode markers (S3-P15) also promote short prose to LaTeX: `pages 3−5`,
  `Temperature −5 °C`, `→ next page` and `Total × 2 items` were text on `main` and are LaTeX now.
  The refusal gate was in-sample; the photo-join round's held-out probes q21 and q22 tested it (both refuse).
- **Drop re-kinded junk rather than show it.** A block the rule re-kinds keeps its raw text, for
  example `\begin{array} { c c c } C & & …`, because text blocks are shown trimmed, not normalised.
  The app shows no block on a refusal, so this appears only when a real formula sits beside the
  junk, as a read-only text block. Dropping the block, or showing it normalised, is the fix.
- **Letters-only maths outside the widened token set** (S3-P15) still refuses: Greek letters alone,
  `\sinh`, `\vec`, `\overline`, `\triangle`, `\mid`. The review measured more: `\sec`, `\csc`,
  `\cot`, `\arcsin`, `\exp`, `\max`, `\binom`, `\leqslant`, `\geqslant`, `\lt`, `\Rightarrow`,
  `\iff`, `\setminus`, `\lfloor`, `\overrightarrow`, `\pi r` (review M1, deferred). Each is one name
  in `MATHS_CMDS` (`vision_parse.rs`) if it should count. The handwriting stage will meet them.
- **Letters-only expressions with only `+` or `−` are text,** on `main` and here: `(a+b)(a-b)`,
  `ab+ac`, `xy - xz`, `a+b+c`. `classify` needs `\ = ^ _` or a Unicode glyph (or a digit with a sign). "Expand
  (a+b)(a−b)" is a staple printed exercise (review M2).
- **Confident misreadings that reach the tutor.** The crop-filling DejaVu render's `x` read as `X`
  was accepted twice, above every threshold. The confirmation step, where the student reads the
  maths before using it, is the only guard in the product. Since `b4b4131` the scorer's tutor gate
  counts the five look-alike pairs, so that run now fails it. A letter swap outside the pairs, a
  digit misread as another digit, or a dropped exponent still fails no gate except the exact count.
- **The device run** (The device run), and then the operator proposal of 0.86. It needs at least one
  observed sign or operator error, or a deliberate check that the guard catches one, before that
  guard is lowered. The aarch64 bench is the controller's build, from `dccd85e` or later.
- **Which operator piece is weak.** The record keeps only the minimum, and `^` and `_` count as
  operators. A per-piece field in the bench's record would let the sign guard be calibrated against
  signs.
- **The founder's photographs: the join checklist.** They joined on 2026-11-14 as `q19-photo-upright` and
  `q20-photo-rotated` (S3-P11; see the Photo-join round).
  - [x] The photographs go under `probes/vision/fixtures/v1/photos/`, CC0, a sheet printed from our
    own typeset items, never a textbook page. Keep the camera JPEG if q20's rotation is its EXIF
    Orientation tag (`vision_prep.rs` applies it; a PNG drops it). Strip GPS and device EXIF and
    keep Orientation, because the repo is public.
  - [x] `gen.py` owns `items.json` and `manifest.json`: a plain run rewrites both without the
    photos, and `gen.py --check` compares `items.json` with its own items. It needs a list of
    photos that it hashes but does not render.
  - [x] `probes/vision/manifest.test.mjs`: its counts (`manifest.count`, the file count and the
    `items.length` of 18) move, and so do its ordered verdict and kind arrays (16 accepts, then two
    refusals). A photo's kind is `printed` or a new `photo`.
  - [x] `.github/workflows/vision-regression.yml`: the 18-record guard moves.
  - [x] **Two held-out refusal probes join as q21 and q22** (review I3): a triangle labelled 3, 4
    and 5, and a prose line holding a date. Both expect `refuse`, and `gen.py` typesets them like
    q17 and q18. They test the refusal rule on images it was not derived from.
  - [x] **The counts with all four:** 22 items, of which 16 printed, 2 photographs and 4 refusals.
    The exact gate is `GATES.exactMin(22)`, 20 of 22. `GATES.refusalsMin` is a fixed 2, so it moves
    to 4, all four refusal items. The manifest test and the CI guard count 22.
  - [x] The host gate run with the 22 items: PASS, 22/22 and 4 of 4 (Photo-join round, 2026-11-14).
  - [x] The device run with the 22 items: PASS, 22/22, 4 of 4, p50 8,733 ms, p95 9,870 ms, VmHWM 389,188 KiB
    (Photo-join round, The 22-item device run, 2026-11-14).
- **Operator-free formulas now refuse (S3-P17).** A printed formula with no operator, relation or
  structural token is text: a numeric matrix with positive entries (`&` and `\\` are not operators),
  `\binom{n}{k}`, `5!`, `f(x)`, a lone Greek letter (`\pi`). They fail closed ("I could not find any
  maths"). If matrices matter for the product, `pmatrix`, `bmatrix` and `binom` are `MATHS_CMDS` candidates.
- **The remaining refusal gap (S3-P17).** A bare number now refuses by rule. What still reaches the
  gate is prose or a diagram that contains an operator-bearing fragment, such as a date written
  `2026-11-14`, a diagram label `3 + 4`, or a caption with a hyphen and a digit. None of the four
  refusal probes has one, so the gate is unmeasured there.
- **Mixed prose-and-formula lines (q15).** They pass in the product's regime through the parser's
  label split. They are still unproven on photographs, where the prose and the maths share a font.
- **The page regime's parser questions.** None reaches the product's crop flow today; all come from the
  page-prompt DejaVu run.
  - `classify` promotes a `<text>` element's content to LaTeX by its content (q10's six copies of the
    right formula were all LaTeX blocks). A repetition guard is still missing.
  - A tag cut at the cap becomes a LaTeX block at high confidence (q02 `<loc_49`, q12 `<loc_441`).
  - The scorer has no equivalence for Unicode relations: q12 wrote `≥`.
  - A dropped exponent (q16, `x^3` read as `x3`) changes the problem but is not a sign or operator
    error to the scorer.
- **HEIC input**, unprobed since Stage 2.
- **Handwriting and the two-photo case**, the spec's v0.2: ≥ 80% line-level exact on the handwritten
  items, line order preserved, crossed-out lines excluded or flagged
  (`docs/superpowers/specs/2026-10-31-vision-input-spec-v0.1.md`).

## What did not run

- **The device run.** The founder has not run `run-vision-probes.sh`.
- **The CI job.** Its first run is on the PR.
- **The aarch64 build of the bench at `dccd85e` or later.** It is the controller's, and it comes
  before the device run. A build from before `dccd85e` lacks the operator guard on the Unicode
  operators, one from before `f5bd4d3` the widened token set, and one from before `83b8929` may lack
  the rule altogether.
- **The host bench after a calibration change.** No threshold changed.
- **The Windows desktop suite.** No `src-tauri` file changed on this branch.
- **A repeat of any host run.** Each of the seven runs is one repeat. q17 and q18 are byte-identical
  across the renders, and they gave identical raws in every product run. The fix-wave CM run's 18
  raws are byte-identical to Task 6's.
- **The page-prompt runs on the new parser.** They are not the product's regime. Applied to their
  recorded blocks, the rule changes none of them (Fix wave).
