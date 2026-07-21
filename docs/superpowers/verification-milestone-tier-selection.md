# Verification — wrapper tier-selection

Branch `feat/tier-selection`. The app picks the hero's base+adapter by device tier
(low→1B / mid→4B / high→8B), with a manual override that also serves as the
all-three-tiers test harness on one machine.

## Automated (Rust)

`cargo test -p cleophis` → **245 passed, 0 failed, 7 ignored** (the ignored are the
pre-existing real-network / real-engine integration tests). Zero new warnings.

New coverage:
- `catalog`: hero carries all three tier variants @ 64-hex hashes; `hero_variant`
  selects per tier (mid-default) and falls back to flat fields without a `tiers`
  block. (4 tests)
- `tier_select`: `effective_tier` (auto/override/garbage); `read`/`write` round-trip;
  the `switch_allowed` state machine (uncommitted-free / no-entitlement-refused /
  first-switch-this-period / second-switch-blocked / perpetual 30-day cooldown);
  `resolve_hero_entitlement` (Period / expired-None / Perpetual / prefers-latest-live-
  period / ignores-other-models); `sweep_models` keeps only the active pair and
  removes the orphan / no-ops on a missing dir. (17 tests)
- `download`: 42 tests green with the tier-aware `download_status`.

`node --check src/app.js` → syntax OK.

## Manual E2E (user, MSI) — the three-tier acceptance gate

Machine is `mid` (16 GB RAM, GTX 1650 4 GB). Downloaded models live at
`C:\Users\<you>\AppData\Roaming\com.cleophis.desktop\models\`.

1. **Auto → 4B unchanged.** Launch; the hero drawer shows "Pick your engine size"
   with **Auto (detected: mid)** active. Engine loads the 4B pair; chat works.
2. **Override → Small · 1B.** Pick it → downloads the 1B base+adapter, then **deletes
   the 4B pair and the stale `Llama-3.2-3B` orphan** (only the 1B pair remains in
   `models\`). Engine relaunches on 1B. Run the **4 Stage-5 probes on 1B**:
   fake-entity→refusal, "5+5=9"→pushback, correction→concession,
   medical-boundary→decline. Send a chat → `committed`.
3. **Second switch, same period → blocked.** Try another tier → the options are
   disabled with "One change per billing period — next change after `<date>`."
4. **After a simulated renewal → Large · 8B.** (Advance the cached entitlement
   `expires_at`.) Pick **Large · 8B** → downloads the 8B pair, deletes 1B, relaunches;
   **probes pass on 8B** (CPU-heavy on this box — expect slow tok/s).

Pass = each tier launches its own base+adapter and clears the 4 probes; the switch
limit blocks a second same-period change; only one model pair is ever on disk.

_Result: ____ (to be filled after the user runs the MSI)._
