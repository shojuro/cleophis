# The `.gate-freeze` marker — design

Proposed structural close for the three coordination races of 2026-07-29. Not
built yet; this is the design steering asked for while it was fresh.

---

## What it replaces, and why the current protocol cannot close these

The freeze protocol is **procedural**: steering sends "freeze", the agent holds
edits until "thawed". Three failures in one day, none of them a discipline
lapse:

| # | race | why the protocol didn't catch it |
|---|---|---|
| 1 | Gate requested at `b1f7b89`; agent committed `e203ec3` before the freeze message arrived. Run compiled a different tree than its label. | **A message in flight is not a freeze in effect.** Both parties were correct when they wrote. |
| 2 | Agent declared freeze, then received an approval for the *next* chunk and started it — editing Rust during a running gate. | An approval delivered *during* a freeze read as licence to act *during* it. |
| 3 | Agent reset the branch backward (on instruction) while a gate was mid-run, moving HEAD under the compiler. | The instruction and the run overlapped; "I will re-run" read as future tense. |

**A gate script that merely refuses to start would have prevented none of
them** — in every case a run had *already started*. That is the requirement the
design has to meet.

## The rule the marker encodes

Steering's paired rule, both halves required:

> **The party under freeze holds regardless of what arrives; the party granting
> approval says when the approval takes effect.**
>
> An approval granted during a freeze takes effect at **thaw**, never before.

Either half alone leaves the gap open — 2026-07-29 proved that by leaving it
open in both directions.

## Mechanism

A file, because **a file is a fact and a message is an assertion**. This is the
same conversion as the provenance sidecar (a watcher creates the opportunity to
report; a file *is* the report), applied to coordination instead of evidence.

### `.gate-freeze` (repo root, gitignored)

```
commit      <full sha the run is pinned to>
requested   <ISO-8601>
by          steering
reason      <e.g. "phase 2 closing gate">
```

### Three enforcement points

1. **The requester writes it** before launching, pinning the commit. Pinning is
   what fixes race #1: the run gates *the named commit*, and never inherits
   whatever the tip happens to be at launch.

2. **The agent's own tooling refuses writes while it exists.** This is the
   non-obvious requirement and the one today supplied: it must bind the *agent*,
   not just the runner. A pre-edit check — Claude Code hook or a wrapper — fails
   any write to a tracked file, and any `commit`/`reset`/`checkout`/`branch`
   operation, while `.gate-freeze` is present. Races #2 and #3 both required an
   *edit during a run*, and only this point stops them.

3. **The gate script refuses to start without it**, and **verifies `HEAD`
   matches the pinned commit** before compiling. That converts "the label is
   right" from an assertion into a precondition.

Thaw is deleting the file. One observable act, by one party, with no ambiguity
about when it took effect.

## What it deliberately does not do

- **It does not replace live-HEAD provenance.** The harness reading HEAD at both
  ends is what made two of today's three races *recoverable*, and it stays. The
  marker prevents; the provenance detects. Both, because prevention that fails
  silently is worse than none.
- **It does not gate reads.** Diagnosis during a freeze is not merely allowed,
  it is the useful thing to do — the `parse` warning was diagnosed read-only
  mid-freeze.
- **It does not gate the scratchpad.** Work outside the worktree during a freeze
  is established practice (gen-3's handoff, today's 3.2 design).

## Cost, honestly

One file, one hook, three lines in the gate script. The hook is the only real
work, and it is the piece without which the design is decorative — a marker
that only the runner respects is a marker that fails in exactly the scenario
that produced it.
