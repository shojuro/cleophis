import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { belowMinTier, minTierNotice, TIER_RANK } from './min-tier.js';

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
  const triage = JSON.parse(
    readFileSync(new URL('../src-tauri/resources/catalog.triage.json', import.meta.url)),
  );
  const hero = triage.find((e) => e.real && e.modelFile);
  assert.equal(hero.id, 'med-triage');
  assert.ok(hero.minTier in TIER_RANK, `minTier ${hero.minTier} is a known tier`);
  // minTier "low" is the Phase 2 value; Phase 3 P3.3 sets the final one. At
  // "low" nothing is gated, which is the point — the mechanism ships before
  // the number it will carry.
  assert.equal(belowMinTier('low', hero.minTier), false);
});

test('the tutor hero declares no minTier, so its tile is unchanged', () => {
  const general = JSON.parse(
    readFileSync(new URL('../src-tauri/resources/catalog.json', import.meta.url)),
  );
  const hero = general.find((e) => e.real && e.modelFile);
  assert.equal(hero.id, 'socratic-tutor');
  assert.equal(hero.minTier, undefined);
  assert.equal(belowMinTier('low', hero.minTier), false);
});
