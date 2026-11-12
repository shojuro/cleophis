import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { belowMinTier, minTierNotice, tierSelectorApplies, TIER_RANK } from './min-tier.js';

const catalog = (name) =>
  JSON.parse(readFileSync(new URL(`../src-tauri/resources/${name}`, import.meta.url)));
// The FRONTEND's hero predicate, not Rust's. `app.js`'s `heroEntry()` is
// `state.catalog.find((m) => m.real)` — no `modelFile` requirement — and the
// frontend is the code under test here. The two agree on both shipped
// catalogs; pinning the wrong one would test agreement that nothing relies on.
const heroOf = (entries) => entries.find((e) => e.real);

// ------------------------------------------------------------- the rule

test('a device at the floor is not below it', () => {
  assert.equal(belowMinTier('low', 'low'), false);
  assert.equal(belowMinTier('mid', 'mid'), false);
  assert.equal(belowMinTier('high', 'high'), false);
});

test('a device above the floor is not below it', () => {
  assert.equal(belowMinTier('mid', 'low'), false);
  assert.equal(belowMinTier('high', 'mid'), false);
});

test('a device below the floor is gated', () => {
  assert.equal(belowMinTier('low', 'mid'), true);
  assert.equal(belowMinTier('low', 'high'), true);
  assert.equal(belowMinTier('mid', 'high'), true);
});

// --------------------------------------------------------- failing open

test('an entry that declares no minTier is never gated', () => {
  for (const min of [undefined, null, '']) {
    assert.equal(belowMinTier('low', min), false);
  }
});

// A catalog typo must not be able to hide the Get button on every device.
test('an unrecognised minTier or device tier fails open', () => {
  assert.equal(belowMinTier('low', 'ultra'), false);
  assert.equal(belowMinTier('banana', 'high'), false);
  assert.equal(belowMinTier(undefined, 'high'), false);
});

// ------------------------------------------------------------ the copy

test('the notice names the tier the entry asked for', () => {
  assert.equal(
    minTierNotice('mid'),
    'This model needs a faster phone (minimum tier: mid).',
  );
});

// ----------------------------------------------- against the real catalogs

// The FE reads `minTier` off the catalog entry; a rename on either side is a
// tile whose gate silently never fires, so the field name is pinned here.
test('the triage entry declares a minTier the rule understands', () => {
  const hero = heroOf(catalog('catalog.triage.json'));
  assert.equal(hero.id, 'med-triage');
  assert.ok(hero.minTier in TIER_RANK, `minTier ${hero.minTier} is a known tier`);
  // minTier "low" is the Phase 2 value; Phase 3 P3.3 sets the final one. At
  // "low" nothing is gated, which is the point — the mechanism ships before
  // the number it will carry.
  assert.equal(belowMinTier('low', hero.minTier), false);
});

test('the tutor hero declares no minTier, so its tile is unchanged', () => {
  const hero = heroOf(catalog('catalog.json'));
  assert.equal(hero.id, 'socratic-tutor');
  assert.equal(hero.minTier, undefined);
  assert.equal(belowMinTier('low', hero.minTier), false);
});

// ------------------------------------------- the "pick your engine size" gate

test('the tutor hero still offers the three tiers', () => {
  const hero = heroOf(catalog('catalog.json'));
  assert.equal(hero.id, 'socratic-tutor');
  assert.deepEqual(Object.keys(hero.tiers).sort(), ['high', 'low', 'mid']);
  assert.equal(tierSelectorApplies(hero, { owned: true }), true);
  assert.equal(tierSelectorApplies(hero, { installed: true }), true);
});

// The triage hero is one pinned base+LoRA pair with no `tiers` (spec A15).
// Rendering the selector for it would offer 1B / 4B / 8B variants it does not
// have, and heroBaseModel would answer 'Qwen3-4B' for a 1.7B entry.
test('an entry without a tiers block yields an empty selector', () => {
  const hero = heroOf(catalog('catalog.triage.json'));
  assert.equal(hero.id, 'med-triage');
  assert.equal(hero.tiers, undefined);
  assert.equal(tierSelectorApplies(hero, { owned: true, installed: true }), false);
});

test('a partial tiers block does not render either', () => {
  const partial = { real: true, tiers: { low: {}, mid: {} } };
  assert.equal(tierSelectorApplies(partial, { owned: true }), false);
});

// The two conditions that were already there before P2.9 must still hold.
test('an unowned or non-real entry yields an empty selector as before', () => {
  const hero = heroOf(catalog('catalog.json'));
  assert.equal(tierSelectorApplies(hero, { owned: false, installed: false }), false);
  assert.equal(tierSelectorApplies({ ...hero, real: false }, { owned: true }), false);
  assert.equal(tierSelectorApplies(null, { owned: true }), false);
  assert.equal(tierSelectorApplies(undefined), false);
});
