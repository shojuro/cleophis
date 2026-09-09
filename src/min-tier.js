// The drawer's two device-tier decisions for a catalog entry (spec P2.9 /
// Phase 3 P3.3): is this entry's model too big for the device it is being
// offered on, and does this entry have per-tier variants to offer at all?
//
// # Why the rule lives here rather than in Rust
//
// The same reasoning as `download-policy.js` and `engine-state.js` (decision
// D-3): the failure mode is silent. A wrong verdict throws nothing — it either
// hides the only control on a tile from someone whose phone is fine, or invites
// a download of a model their phone cannot serve at a usable rate. Both look
// exactly like working software, so the rule goes where the tests run.
//
// The FACT it decides over is Rust's: `minTier` is a catalog field
// (`CatalogEntry::min_tier`) and the effective tier comes from
// `get_tier_selection`. Only the comparison is here.

/** The device tiers, ordered. Mirrors `hardware::tier_for`'s "low"/"mid"/"high". */
export const TIER_RANK = { low: 0, mid: 1, high: 2 };

/**
 * Is `effectiveTier` below the entry's declared `minTier`?
 *
 * Fail-OPEN by construction: an entry that declares no `minTier`, an
 * unrecognised `minTier`, or an unrecognised device tier all answer `false`.
 * A typo in a catalog field must not be able to hide the Get button on every
 * device — the cost of the wrong answer in that direction is a tile nobody can
 * ever act on, against a download that the size line already warns about.
 *
 * @param {string|null|undefined} effectiveTier the device's effective tier.
 * @param {string|null|undefined} minTier the entry's `minTier`, if it declares one.
 * @returns {boolean}
 */
export function belowMinTier(effectiveTier, minTier) {
  if (!minTier) return false;
  const need = TIER_RANK[minTier];
  const have = TIER_RANK[effectiveTier];
  if (need === undefined || have === undefined) return false;
  return have < need;
}

/**
 * The line shown in place of the Get / Download button when the device is
 * below the entry's floor.
 *
 * @param {string} minTier
 * @returns {string}
 */
export function minTierNotice(minTier) {
  return `This model needs a faster phone (minimum tier: ${minTier}).`;
}

/**
 * Should the drawer render the "Pick your engine size" selector for this entry?
 *
 * The selector's options ARE the entry's `tiers` block — Small · 1B,
 * Balanced · 4B, Large · 8B are the tutor hero's three variants. An entry
 * without that block has no variants to switch between, so rendering the
 * selector would offer three models that do not exist for it, and
 * `heroBaseModel` would answer 'Qwen3-4B' for an entry that is 1.7B. The
 * supervised triage hero is exactly that shape: one pinned base+LoRA pair and
 * no `tiers` (spec A15), which is why this guard ships alongside it.
 *
 * All three tiers are required, mirroring Rust's `catalog::Tiers` whose
 * `low`/`mid`/`high` are not optional — a partial block does not parse there,
 * so it must not render here.
 *
 * @param {object|null|undefined} entry the catalog entry.
 * @param {{owned?: boolean, installed?: boolean}} state ownership as the drawer knows it.
 * @returns {boolean}
 */
export function tierSelectorApplies(entry, { owned = false, installed = false } = {}) {
  if (!entry || !entry.real) return false;
  if (!owned && !installed) return false;
  const t = entry.tiers;
  return Boolean(t && t.low && t.mid && t.high);
}
