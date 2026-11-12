import { test } from 'node:test';
import assert from 'node:assert/strict';

import { pickHeroArtifacts, heroVariantFor } from './dist-pick.js';

// 64-hex shas, distinct per artifact.
const sha = (c) => c.repeat(64);

const BASE_Q4 = { path: 'models/Qwen3-1.7B-Q4_K_M.gguf', sha256: sha('a'), size: 1282439008, kind: 'base', base_model: 'Qwen3-1.7B' };
const BASE_Q6 = { path: 'models/Qwen3-1.7B-Q6_K.gguf', sha256: sha('b'), size: 1673006944, kind: 'base', base_model: 'Qwen3-1.7B' };
const TUTOR_ADAPTER = { path: 'adapters/behavioral-v1-Qwen3-1.7B.gguf', sha256: sha('c'), size: 34000000, kind: 'adapter', base_model: 'Qwen3-1.7B' };
const TRIAGE_ADAPTER = { path: 'adapters/triage-v3-Qwen3-1.7B.gguf', sha256: sha('d'), size: 35000000, kind: 'adapter', base_model: 'Qwen3-1.7B' };
const CONTRACT = { path: 'adapters/contract-v2-Qwen3-1.7B.gguf', sha256: sha('e'), size: 20000000, kind: 'contract-adapter', base_model: 'Qwen3-1.7B' };
const OTHER_BASE = { path: 'models/Qwen3-4B-Q4_K_M.gguf', sha256: sha('f'), size: 2497280384, kind: 'base', base_model: 'Qwen3-4B' };

// Two bases and two adapters for ONE base_model: `kind + base_model` is
// ambiguous here by construction, and the old selector took whichever came
// first.
const AMBIGUOUS = [BASE_Q4, TUTOR_ADAPTER, OTHER_BASE, BASE_Q6, TRIAGE_ADAPTER, CONTRACT];

const TRIAGE_VARIANT = { baseModel: 'Qwen3-1.7B', sha256: sha('b'), adapterSha256: sha('d'), contractAdapterSha256: null };

// ---------------------------------------------------------------- selection

test('picks the pinned base and adapter out of an ambiguous catalog, not the first by kind', () => {
  const got = pickHeroArtifacts(AMBIGUOUS, TRIAGE_VARIANT);
  assert.equal(got.base, BASE_Q6);
  assert.equal(got.adapter, TRIAGE_ADAPTER);
  assert.equal(got.contract, null);
});

test('the other pinned pair picks the other artifacts from the same catalog', () => {
  const got = pickHeroArtifacts(AMBIGUOUS, { ...TRIAGE_VARIANT, sha256: sha('a'), adapterSha256: sha('c') });
  assert.equal(got.base, BASE_Q4);
  assert.equal(got.adapter, TUTOR_ADAPTER);
});

test('sha comparison ignores hex case', () => {
  const got = pickHeroArtifacts(AMBIGUOUS, { ...TRIAGE_VARIANT, sha256: 'B'.repeat(64) });
  assert.equal(got.base, BASE_Q6);
});

// ---------------------------------------------------------------- missing pins

test('a pinned base sha absent from the signed catalog refuses as not published', () => {
  const arts = AMBIGUOUS.filter((a) => a !== BASE_Q6);
  assert.throws(() => pickHeroArtifacts(arts, TRIAGE_VARIANT), (e) => {
    assert.match(e.message, /This build's model isn't published yet/);
    assert.match(e.message, /base/);
    return true;
  });
});

test('a pinned adapter sha absent from the signed catalog refuses and names the adapter', () => {
  const arts = AMBIGUOUS.filter((a) => a !== TRIAGE_ADAPTER);
  assert.throws(() => pickHeroArtifacts(arts, TRIAGE_VARIANT), (e) => {
    assert.match(e.message, /This build's model isn't published yet/);
    assert.match(e.message, /adapter/);
    return true;
  });
});

test('a variant that pins no base sha refuses rather than guessing', () => {
  assert.throws(() => pickHeroArtifacts(AMBIGUOUS, { ...TRIAGE_VARIANT, sha256: '' }), /base/);
  assert.throws(() => pickHeroArtifacts(AMBIGUOUS, { ...TRIAGE_VARIANT, adapterSha256: undefined }), /adapter/);
});

test('an empty or missing artifact list refuses as not published', () => {
  assert.throws(() => pickHeroArtifacts([], TRIAGE_VARIANT), /isn't published yet/);
  assert.throws(() => pickHeroArtifacts(undefined, TRIAGE_VARIANT), /isn't published yet/);
});

// ---------------------------------------------------------------- disagreement

test('a sha match whose kind disagrees refuses', () => {
  // The pinned base sha is published, but as an adapter record.
  const wrongKind = { ...BASE_Q6, kind: 'adapter' };
  const arts = [BASE_Q4, wrongKind, TRIAGE_ADAPTER];
  assert.throws(() => pickHeroArtifacts(arts, TRIAGE_VARIANT), (e) => {
    assert.match(e.message, /kind/);
    return true;
  });
});

test('a sha match whose base_model disagrees refuses', () => {
  const wrongBase = { ...TRIAGE_ADAPTER, base_model: 'Qwen3-4B' };
  const arts = [BASE_Q6, wrongBase];
  assert.throws(() => pickHeroArtifacts(arts, TRIAGE_VARIANT), /base model/);
});

test('an agreeing record wins over a disagreeing duplicate of the same sha', () => {
  const dupWrong = { ...BASE_Q6, path: 'models/elsewhere.gguf', kind: 'adapter' };
  const got = pickHeroArtifacts([dupWrong, BASE_Q6, TRIAGE_ADAPTER], TRIAGE_VARIANT);
  assert.equal(got.base, BASE_Q6);
});

// ---------------------------------------------------------------- contract adapter

test('a declared contract adapter present in the catalog is picked', () => {
  const got = pickHeroArtifacts(AMBIGUOUS, { ...TRIAGE_VARIANT, contractAdapterSha256: sha('e') });
  assert.equal(got.contract, CONTRACT);
});

test('a declared contract adapter absent from the catalog refuses', () => {
  const arts = AMBIGUOUS.filter((a) => a !== CONTRACT);
  assert.throws(
    () => pickHeroArtifacts(arts, { ...TRIAGE_VARIANT, contractAdapterSha256: sha('e') }),
    /contract adapter/,
  );
});

test('no declared contract adapter means none is picked even when one is published', () => {
  assert.equal(pickHeroArtifacts(AMBIGUOUS, TRIAGE_VARIANT).contract, null);
});

// ---------------------------------------------------------------- heroVariantFor

const FLAT_HERO = {
  id: 'med-triage', real: true, baseModel: 'Qwen3-1.7B',
  sha256: sha('b'), adapterSha256: sha('d'),
};
const TIERED_HERO = {
  id: 'socratic-tutor', real: true, sha256: sha('f'), adapterSha256: sha('c'),
  tiers: {
    low: { baseModel: 'Llama-3.2-1B', sha256: sha('1'), adapterSha256: sha('2') },
    mid: { baseModel: 'Qwen3-4B', sha256: sha('f'), adapterSha256: sha('c'), contractAdapterSha256: sha('e') },
    high: { baseModel: 'Qwen3-8B', sha256: sha('3'), adapterSha256: sha('4') },
  },
};

test('a flat hero resolves its own pinned pair for its baseModel', () => {
  assert.deepEqual(heroVariantFor(FLAT_HERO, 'Qwen3-1.7B'), {
    baseModel: 'Qwen3-1.7B', sha256: sha('b'), adapterSha256: sha('d'), contractAdapterSha256: null,
  });
});

test('a tiered hero resolves the tier whose baseModel matches', () => {
  assert.deepEqual(heroVariantFor(TIERED_HERO, 'Qwen3-4B'), {
    baseModel: 'Qwen3-4B', sha256: sha('f'), adapterSha256: sha('c'), contractAdapterSha256: sha('e'),
  });
  assert.equal(heroVariantFor(TIERED_HERO, 'Qwen3-8B').sha256, sha('3'));
});

test('an empty, unknown or undeclared baseModel resolves to nothing', () => {
  assert.equal(heroVariantFor(FLAT_HERO, ''), null);
  assert.equal(heroVariantFor(FLAT_HERO, undefined), null);
  assert.equal(heroVariantFor(FLAT_HERO, 'Qwen3-4B'), null);
  assert.equal(heroVariantFor(TIERED_HERO, 'Qwen3-1.7B'), null);
  assert.equal(heroVariantFor({ ...FLAT_HERO, baseModel: undefined }, 'Qwen3-1.7B'), null);
  assert.equal(heroVariantFor(null, 'Qwen3-1.7B'), null);
});
