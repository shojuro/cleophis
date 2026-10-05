// Which signed-catalog artifacts does this build install? (Phase 1g, M1)
//
// # Why by sha and not by kind + base_model
//
// The bundled catalog entry already pins the exact bytes it was gated on: the
// base `sha256`, the adapter `adapterSha256` and, when a tier declares one,
// the `contractAdapterSha256`. The signed dist catalog, meanwhile, can carry
// more than one base or adapter for the same `base_model` (a Q4 and a Q6_K
// base; the tutor's behavioral adapter beside the triage adapter). Selecting
// by `kind + base_model` is then ambiguous, and the ambiguity resolves to
// whichever record happens to come first — a download that lands the wrong
// adapter, which `verify_adapter_once` then refuses at launch. So the pinned
// sha is the key, and `kind` / `base_model` are only cross-checks.
//
// Pure (no DOM, no invoke) so `node --test` covers it; `app.js` wires it in.
// The Rust side refuses an unpinned sha again in `download_artifact` (defense
// in depth); this is the check that gives the user a readable reason.

const NOT_PUBLISHED = "This build's model isn't published yet — please update the app or try again later.";

const norm = (s) => (typeof s === 'string' ? s.trim().toLowerCase() : '');
const short = (s) => norm(s).slice(0, 12);

// One pinned role: find the records carrying `pinnedSha`, prefer one whose
// kind and base_model agree, and refuse with a named reason otherwise.
function pickOne(arts, pinnedSha, kind, baseModel, label) {
  const want = norm(pinnedSha);
  if (!want) {
    throw new Error(`This build doesn't pin a ${label} file — please update the app.`);
  }
  const matches = arts.filter((a) => a && norm(a.sha256) === want);
  if (matches.length === 0) {
    throw new Error(`${NOT_PUBLISHED} (the ${label} ${short(want)}… is not in the signed catalog)`);
  }
  const agreeing = matches.find((a) => a.kind === kind && a.base_model === baseModel);
  if (agreeing) return agreeing;
  const m = matches[0];
  if (m.kind !== kind) {
    throw new Error(
      `The signed catalog lists this build's ${label} (${short(want)}…) as kind '${m.kind}', not '${kind}' — please update the app.`,
    );
  }
  throw new Error(
    `The signed catalog lists this build's ${label} (${short(want)}…) for base model '${m.base_model}', not '${baseModel}' — please update the app.`,
  );
}

/**
 * Select the artifacts this build pins, out of the signed dist catalog.
 *
 * @param {Array<{path:string, sha256:string, size:number, kind:string, base_model:string}>} artifacts
 *   `fetch_dist_catalog().artifacts`.
 * @param {{baseModel:string, sha256:string, adapterSha256:string, contractAdapterSha256?:string|null}} variant
 *   The resolved hero variant from the bundled catalog (see `heroVariantFor`).
 * @returns {{base:object, adapter:object, contract:object|null}}
 *   `contract` is null when the variant declares no contract adapter.
 * @throws {Error} with a user-facing message naming what is missing or
 *   disagrees; nothing is returned partially.
 */
export function pickHeroArtifacts(artifacts, variant) {
  const arts = Array.isArray(artifacts) ? artifacts : [];
  if (arts.length === 0) throw new Error(NOT_PUBLISHED);
  const v = variant || {};
  const baseModel = v.baseModel;
  const base = pickOne(arts, v.sha256, 'base', baseModel, 'base model');
  const adapter = pickOne(arts, v.adapterSha256, 'adapter', baseModel, 'adapter');
  const contract = norm(v.contractAdapterSha256)
    ? pickOne(arts, v.contractAdapterSha256, 'contract-adapter', baseModel, 'contract adapter')
    : null;
  return { base, adapter, contract };
}

/**
 * The pinned variant the bundled hero entry declares for a dist `baseModel`:
 * the matching `tiers` entry when the hero is tiered, else the flat fields
 * when the flat entry's `baseModel` matches (mirrors Rust
 * `catalog::hero_variant`). Null when `baseModel` is empty or unknown — the
 * caller refuses to download rather than guess.
 *
 * @returns {{baseModel:string, sha256:string, adapterSha256:string, contractAdapterSha256:string|null}|null}
 */
export function heroVariantFor(hero, baseModel) {
  if (!hero || typeof baseModel !== 'string' || baseModel === '') return null;
  let src = null;
  if (hero.tiers) {
    src = Object.values(hero.tiers).find((t) => t && t.baseModel === baseModel) || null;
  } else if (hero.baseModel === baseModel) {
    src = hero;
  }
  if (!src) return null;
  return {
    baseModel,
    sha256: src.sha256,
    adapterSha256: src.adapterSha256,
    contractAdapterSha256: src.contractAdapterSha256 || null,
  };
}
