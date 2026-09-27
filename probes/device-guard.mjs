// probes/device-guard.mjs — the product half of the device-fidelity bar.
//
// `probe --json` records what the ENGINE produced. This records what the
// PATIENT reads, by running every device reply through the same
// `applyGuard` the app runs it through before it reaches a screen.
//
// ── WHY BOTH NUMBERS EXIST (plan review, risk R2) ───────────────────────────
//
// `applyGuard` is not a formatter. It removes sentences that name a medication
// or a diagnosis; on a CLINICIAN route it removes every stated time frame and,
// if one survives, it discards the model's text ENTIRELY and shows a fixed
// note; it appends the product's crisis block when the patient's own words
// disclose self-harm and the reply does not signpost. So a device number
// computed on raw engine text can be green while the text on the screen says
// something else — and the disqualifying release bar is about the screen.
//
// The plan review's table is the short version of what this file measures:
//
//   omission      (route right, crisis line absent)  the guard COMPLETES it
//   displacement  (crisis line instead of a route)   the guard CANNOT fix it
//   scope refusal (OUT_OF_SCOPE where EMERGENCY due) the guard CANNOT fix it,
//                                                    and shows a confident
//                                                    out-of-scope banner
//
// Two of those three are worse after the guard than they look before it. That
// is why the post-guard number is the disqualifying one and the raw number is
// the diagnostic.
//
// ── WHAT IT REFUSES, AND WHY IT STILL WRITES A LINE ─────────────────────────
//
// A record with no `user`, no `text`, or one the harness marked
// `truncated_in_think`, is not guarded. `applyGuard('')` hands back the out-of-scope banner line and a
// route — a display indistinguishable from a reply the model actually gave —
// and a truncation's `text` is the model's own reasoning, which would become a
// banner and a route out of a sentence it was writing to itself.
//
// But the file STILL CARRIES A LINE for that id, empty. Task A4 counts
// `scanned` from the entries present, so a missing id lowers the denominator
// and turns the bar NOT MEASURED: a device failure would make the bar
// unmeasurable instead of failed. An empty display fails the item, which is
// what a device failure should do.
//
// Usage:
//   node probes/device-guard.mjs --in work/device-probes/m7.R58N.json \
//                                --out work/device-probes/m7.R58N.guard.json
//   [--catalog src-tauri/resources/catalog.triage.json] [--catalog-id med-triage]
//   [--crisis-rule append|replace] [--keep-routes EMERGENCY[,CLINICIAN]]
//   [--dedupe-signposts on|off] [--strip-doses on|off] [--strip-disclaimers on|off]
//
// THE RULE DEFAULTS TO THE CATALOG ENTRY'S (Phase 1i MA1 fix round 1), read
// through `crisisRuleFor` — the same sanitised reader the app uses — so a run
// with no flags measures what the phone runs. The registered rule is `replace`
// with keep routes EMERGENCY,CLINICIAN: on a disclosure the product's
// acknowledgement + signpost block leads the display and the model's reply is
// shown under it only when its route is kept. `append` is the REPORTED twin,
// measured with `--crisis-rule append`. `--crisis-rule` and `--keep-routes` are
// explicit overrides. What ran is echoed in the header beside `detectors_sha`
// (with `crisis_rule_source`: catalog or flag) and on every record, so a scorer
// can never read a file without knowing which rule produced it.
//
// `--dedupe-signposts` (Phase 1i MA1) — when the product's crisis block is on
// screen, cut every sentence of the kept reply that the frozen detector reads
// as a crisis signpost, so the block is the only crisis line: no number inside
// a sentence the frozen detector reads as a crisis signpost is displayed. A
// number in any other sentence is not touched (the census's wrong-number floor
// uses the same definition).
// The default follows the rule: on under `replace`, off under `append`. The
// value is echoed as `dedupe_signposts` in the header (the triage census reads
// it there) and on every record, with `signposts_removed` per record.
//
// `--strip-doses` and `--strip-disclaimers` (Phase 1i MA6) — the product's two
// owned floors on a kept reply: every sentence carrying a dose token (the
// lookup guard's normaliser and DOSE_UNITS) is removed and the fixed
// "A clinician can advise on treatment." note shown once; on a reply whose
// direction is EMERGENCY or CLINICIAN every scope-disclaimer sentence is
// removed. Both default ON under every rule. Echoed as `strip_doses` and
// `strip_disclaimers` in the header (after `dedupe_signposts`) and on every
// record, with `doses_removed` and `disclaimers_removed` per record.
//
// Output: a header object on line 1, then EXACTLY one object per input record:
//   {"id", "display", "route_banner", "removed": [...], ...}
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { CRISIS_RULES, REPLACE_KEEP_ROUTES_DEFAULT, applyGuard } from '../src/triage/guard.js';
import { ROUTE } from '../src/triage/detectors.mjs';
import { crisisRuleFor } from '../src/triage-turn.js';
import pin from '../src/triage/detectors.pin.js';

const DEFAULT_CATALOG = new URL('../src-tauri/resources/catalog.triage.json', import.meta.url);

/** Parse JSON lines, naming the line that failed rather than the whole file. */
export function parseJsonl(text) {
  const out = [];
  const lines = String(text).split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i].trim();
    if (!line) continue;
    try {
      out.push(JSON.parse(line));
    } catch (e) {
      throw new Error(`line ${i + 1}: ${e.message}`);
    }
  }
  return out;
}

/** The catalog entry this run is answerable to. No fallback, deliberately. */
export function catalogEntry(path = DEFAULT_CATALOG, id = null) {
  const cat = JSON.parse(readFileSync(path, 'utf8'));
  const hit = id ? cat.find((e) => e.id === id) : cat.find((e) => e.real);
  if (!hit) {
    throw new Error(id
      ? `catalog has no entry with id ${JSON.stringify(id)}`
      : 'catalog has no entry marked "real": true');
  }
  if (!hit.crisisLine) throw new Error(`catalog entry ${hit.id} has no crisisLine`);
  return hit;
}

/**
 * The crisis-rule flags, parsed and validated BEFORE any record is guarded.
 *
 * `guardAll` turns a record's error into a refused entry, so a bad flag
 * discovered per record would write a file of 260 "device failures" — a failed
 * bar produced by a typo. Refused here, the run fails with nothing written.
 *
 * With an `entry`, an absent flag takes the ENTRY's rule and keep set through
 * `crisisRuleFor` (the app's reader); an entry with no rule means append.
 * Without one (unit callers), an absent rule means append and an absent keep
 * set REPLACE_KEEP_ROUTES_DEFAULT, as before.
 *
 * @returns {{crisisRule: 'append'|'replace', keepRoutes: string[]|null, dedupeSignposts: boolean,
 *   crisisRuleSource: 'catalog'|'flag'|'default'}}
 *   `keepRoutes` is null under `append`, which reads no keep set.
 *   `dedupeSignposts` defaults to the rule's own default (on under replace).
 */
export function crisisOptions({
  crisisRule = null, keepRoutes = null, dedupeSignposts = null, entry = null,
} = {}) {
  const fromEntry = entry ? crisisRuleFor(entry) : {};
  const source = crisisRule != null ? 'flag' : (entry ? 'catalog' : 'default');
  const rule = crisisRule ?? fromEntry.crisisRule ?? 'append';
  if (!CRISIS_RULES.includes(rule)) {
    throw new Error(`--crisis-rule must be one of ${CRISIS_RULES.join('|')}, got ${JSON.stringify(crisisRule)}`);
  }
  const dedupe = parseDedupe(dedupeSignposts, rule);
  const out = (keep) => ({
    crisisRule: rule, keepRoutes: keep, dedupeSignposts: dedupe, ...(entry ? { crisisRuleSource: source } : {}),
  });
  if (rule !== 'replace') {
    if (keepRoutes != null) throw new Error('--keep-routes is read only with --crisis-rule replace');
    return out(null);
  }
  if (keepRoutes == null) return out([...(fromEntry.replaceKeepRoutes ?? REPLACE_KEEP_ROUTES_DEFAULT)]);
  const list = Array.isArray(keepRoutes)
    ? [...keepRoutes]
    : String(keepRoutes).split(',').map((r) => r.trim()).filter(Boolean);
  const known = Object.values(ROUTE);
  if (!list.length || list.some((r) => !known.includes(r))) {
    throw new Error(`--keep-routes must be a comma list of ${known.join('|')}, got ${JSON.stringify(keepRoutes)}`);
  }
  return out(list);
}

/** `on|off` (or a boolean) to a boolean; absent follows the rule. Refused otherwise. */
function parseDedupe(value, rule) {
  if (value == null) return rule === 'replace';
  if (value === true || value === 'on') return true;
  if (value === false || value === 'off') return false;
  throw new Error(`--dedupe-signposts must be on|off, got ${JSON.stringify(value)}`);
}

/** `on|off` (or a boolean) to a boolean; absent is ON. Refused otherwise. */
function parseOnOff(value, flag) {
  if (value == null) return true;
  if (value === true || value === 'on') return true;
  if (value === false || value === 'off') return false;
  throw new Error(`--${flag} must be on|off, got ${JSON.stringify(value)}`);
}

/**
 * The MA6 switches, parsed and validated BEFORE any record is guarded (for the
 * same reason as `crisisOptions`). Both default on, under every rule.
 *
 * @returns {{stripDoses: boolean, stripDisclaimers: boolean}}
 */
export function stripOptions({ stripDoses = null, stripDisclaimers = null } = {}) {
  return {
    stripDoses: parseOnOff(stripDoses, 'strip-doses'),
    stripDisclaimers: parseOnOff(stripDisclaimers, 'strip-disclaimers'),
  };
}

/** The effective switch for a guard call: explicit boolean, else on. */
const onUnlessOff = (v) => v !== false;

/** The effective de-duplication for a guard call: explicit, else the rule's default. */
const dedupeFor = (dedupeSignposts, crisisRule) => (typeof dedupeSignposts === 'boolean'
  ? dedupeSignposts : crisisRule === 'replace');

const sha256 = (s) => createHash('sha256').update(String(s), 'utf8').digest('hex');

/**
 * The header the scorer reads first.
 *
 * `detectors_sha` is the pin `applyGuard` ITSELF reports on every verdict, not
 * a second read of the pin file — a guard run scored against a detector set
 * other than the one it used is the failure this line exists to make loud.
 * `crisis_line_sha256` is here because the crisis block is the one piece of
 * text the product ADDS, so a change to it changes what the bar measured.
 */
export function headerRecord({
  source, catalogId, crisisLine, records, skipped = [], crisisRule = 'append', keepRoutes = null,
  dedupeSignposts = null, crisisRuleSource = null, stripDoses = null, stripDisclaimers = null,
}) {
  const probe = applyGuard({ userText: '', replyText: 'Call 999 now.' });
  return {
    header: 'device-guard',
    source,
    catalog_id: catalogId,
    // ENTRIES, not successes. There is one line per harness record, always, and
    // this is that count — so `records` is the scorer's denominator directly
    // and nothing has to be added back to it.
    records,
    // The subset of those entries that are empty because the guard refused
    // them, with the reason. They are device failures: present, scorable as
    // failures, and named.
    skipped,
    detectors_sha: probe.detectorsSha,
    // Beside the detector pin because it is the same kind of fact: which rule
    // decided every display below. `keep_routes` is null under `append`.
    crisis_rule: crisisRule,
    keep_routes: crisisRule === 'replace' ? [...keepRoutes] : null,
    // Phase 1i MA1: whether a kept reply's crisis signposts were cut beside the
    // product's block. The triage census reads this line.
    dedupe_signposts: dedupeFor(dedupeSignposts, crisisRule),
    // Phase 1i MA6: the product's dose and red-flag disclaimer strips. The
    // triage census reads these lines beside the rule and the dedupe flag.
    strip_doses: onUnlessOff(stripDoses),
    strip_disclaimers: onUnlessOff(stripDisclaimers),
    // Where the rule came from: 'catalog' (the entry's registered rule, no
    // flag), 'flag' (an explicit --crisis-rule), or null for a unit caller.
    crisis_rule_source: crisisRuleSource,
    detectors_pin_file_sha: pin.sha256,
    crisis_line_sha256: sha256(crisisLine),
  };
}

/**
 * One harness record through the product's contract.
 *
 * The guarded text is the record's `text` — post-think-strip — because that is
 * what the app hands `applyGuard`. `raw` stays in the harness file as the
 * diagnostic; it is never what a patient reads and must never be scored as if
 * it were.
 */
export function guardRecord(record, {
  crisisLine, crisisRule = 'append', keepRoutes = null, dedupeSignposts = null,
  stripDoses = null, stripDisclaimers = null,
}) {
  const { id, user, text, state } = record ?? {};
  // The PATIENT'S WORDS, refused the same way the reply is. `String(user ?? '')`
  // would have guarded the record against an empty patient turn, and an empty
  // patient turn cannot disclose self-harm — so `detectCrisisStatement` finds
  // nothing, the crisis block is not appended, and a display that should have
  // carried it does not. That is the same failure class as a blank reply, with
  // no refusal attached to it, and it is silent in exactly the direction R15
  // says it must not be.
  if (typeof user !== 'string') {
    throw new Error(
      `record ${JSON.stringify(id ?? '?')}: "user" is ${typeof user}, not a string. `
      + 'Guarding against an empty patient turn disables crisis-on-input detection, '
      + 'so a reply that needed the crisis block would be shown without it.',
    );
  }
  if (typeof text !== 'string' || text.length === 0) {
    throw new Error(
      `record ${JSON.stringify(id ?? '?')}: no "text" to guard `
      + `(state=${JSON.stringify(state ?? null)}). A blank reply guarded is a `
      + 'display carrying a banner and a route, which reads exactly like a reply the '
      + 'model gave. Score the harness state instead.',
    );
  }
  // The generation died inside `<think>`. The harness returns the partial
  // think content rather than an empty string — that is right, because the
  // alternative is a record that cannot be told apart from a reply the model
  // never gave. But that content is REASONING, and guarding it would produce a
  // display, a banner and a route out of a sentence the model was writing to
  // itself. Counted as a device failure, never scored as a reply.
  if (state === 'truncated_in_think') {
    throw new Error(
      `record ${JSON.stringify(id ?? '?')}: state is truncated_in_think, so "text" is `
      + 'partial reasoning, not a reply. Count it as a device failure; a banner and a '
      + 'route built out of it would read exactly like a reply the model gave.',
    );
  }
  const replace = crisisRule === 'replace';
  const dedupe = dedupeFor(dedupeSignposts, crisisRule);
  const v = applyGuard({
    userText: user,
    replyText: text,
    crisisLine,
    crisisRule,
    ...(replace && keepRoutes ? { replaceKeepRoutes: keepRoutes } : {}),
    dedupeSignposts: dedupe,
    stripDoses: onUnlessOff(stripDoses),
    stripScopeDisclaimers: onUnlessOff(stripDisclaimers),
  });
  return {
    id,
    display: v.displayText,
    route_banner: v.banner,
    removed: v.prohibitedRemoved,
    // Diagnostics past the fixed shape. Every one of them is a reason the
    // display differs from `text`, so a reader never has to guess which rule
    // moved it.
    route: v.route,
    why: v.why,
    suite: record.suite,
    state: record.state,
    crisis_on_input: v.crisisOnInput,
    crisis_line_appended: v.crisisLineAppended,
    timeframe_stripped: v.timeframeStripped,
    timeframe_unlocated: v.timeframeUnlocated,
    prohibited: v.prohibited,
    changed: v.displayText !== text,
    // The crisis rule (Phase 1h M2), on every record whichever rule ran, so a
    // file has one shape. Under `append` the rule cannot replace anything:
    // `crisis_replaced` is false and `reply_shown` true by construction.
    crisis_rule: crisisRule,
    keep_routes: replace ? v.keepRoutes : null,
    crisis_replaced: replace ? v.crisisReplaced : false,
    reply_shown: replace ? v.replyShown : true,
    route_detected: replace ? v.routeDetected : v.route,
    // Phase 1i MA1: on every record, whichever rule ran. The removed sentences
    // are never display text; they remain only in the harness file's `text`.
    dedupe_signposts: dedupe,
    signposts_removed: v.signpostsRemoved ?? 0,
    // Phase 1i MA6: counts only; the removed sentences stay in `text`.
    strip_doses: onUnlessOff(stripDoses),
    doses_removed: v.dosesRemoved ?? 0,
    strip_disclaimers: onUnlessOff(stripDisclaimers),
    disclaimers_removed: v.disclaimersRemoved ?? 0,
  };
}

/**
 * The entry for a record the guard refuses to guard.
 *
 * THE FILE CARRIES A LINE FOR EVERY ID, and that is Task A4's requirement, not
 * a preference: the scorer's `scanned` count comes from the entries present, so
 * an id with no entry lowers the denominator and turns the bar NOT MEASURED —
 * a device failure would make the bar *unmeasurable* instead of *failed*, which
 * is the wrong direction for every bar in this programme.
 *
 * So the entry exists and is EMPTY. `display` is the empty string and
 * `route_banner` is null, because there is nothing a patient would have read
 * and no banner the product would have shown; anything else here would be this
 * file inventing a reply. An empty display fails the item, which is what a
 * device failure should do.
 */
function refusedEntry(record, reason, {
  crisisRule = 'append', keepRoutes = null, dedupeSignposts = null, stripDoses = null, stripDisclaimers = null,
} = {}) {
  return {
    id: record?.id ?? null,
    display: '',
    route_banner: null,
    removed: [],
    route: null,
    why: null,
    suite: record?.suite ?? null,
    state: record?.state ?? null,
    refused: true,
    refused_reason: reason,
    crisis_on_input: null,
    crisis_line_appended: false,
    timeframe_stripped: [],
    timeframe_unlocated: false,
    prohibited: { medication: [], diagnosis: [] },
    changed: false,
    crisis_rule: crisisRule,
    keep_routes: crisisRule === 'replace' ? keepRoutes : null,
    crisis_replaced: false,
    reply_shown: false,
    route_detected: null,
    dedupe_signposts: dedupeFor(dedupeSignposts, crisisRule),
    signposts_removed: 0,
    strip_doses: onUnlessOff(stripDoses),
    doses_removed: 0,
    strip_disclaimers: onUnlessOff(stripDisclaimers),
    disclaimers_removed: 0,
  };
}

/**
 * Every record, in the order the harness wrote them, one entry each.
 *
 * A record the guard refuses does not abort the file and does not vanish from
 * it: it gets an empty entry and a line in `skipped`, so the refusal is both
 * scorable and countable. One device failure on item 140 must not cost the
 * other 259 their scores, and it must not quietly shrink the denominator
 * either.
 */
export function guardAll(records, opts) {
  const guarded = [];
  const skipped = [];
  for (const r of records) {
    try {
      guarded.push(guardRecord(r, opts));
    } catch (e) {
      guarded.push(refusedEntry(r, e.message, opts));
      skipped.push({ id: r?.id ?? null, state: r?.state ?? null, reason: e.message });
    }
  }
  return { guarded, skipped };
}

/** A one-line count of what the guard did, for the run log. */
export function summarise(guarded) {
  const banners = {};
  let changed = 0;
  let appended = 0;
  let removedAny = 0;
  let unlocated = 0;
  let refused = 0;
  let replaced = 0;
  let hidden = 0;
  let signpostsRemoved = 0;
  let dosesRemoved = 0;
  let disclaimersRemoved = 0;
  for (const g of guarded) {
    if (g.refused) { refused += 1; continue; }
    banners[g.route_banner] = (banners[g.route_banner] ?? 0) + 1;
    if (g.changed) changed += 1;
    if (g.crisis_line_appended) appended += 1;
    if (g.removed.length) removedAny += 1;
    if (g.timeframe_unlocated) unlocated += 1;
    if (g.crisis_replaced) replaced += 1;
    if (g.crisis_replaced && !g.reply_shown) hidden += 1;
    signpostsRemoved += g.signposts_removed ?? 0;
    dosesRemoved += g.doses_removed ?? 0;
    disclaimersRemoved += g.disclaimers_removed ?? 0;
  }
  return {
    records: guarded.length,
    refused,
    banners,
    changed,
    crisis_line_appended: appended,
    prohibited_removed: removedAny,
    timeframe_unlocated: unlocated,
    crisis_replaced: replaced,
    reply_hidden: hidden,
    signposts_removed: signpostsRemoved,
    doses_removed: dosesRemoved,
    disclaimers_removed: disclaimersRemoved,
  };
}

export function run({
  inPath, outPath, catalogPath, catalogId, crisisRule = null, keepRoutes = null, dedupeSignposts = null,
  stripDoses = null, stripDisclaimers = null,
}) {
  const entry = catalogEntry(catalogPath ?? DEFAULT_CATALOG, catalogId ?? null);
  const { crisisRuleSource, ...crisis } = crisisOptions({ crisisRule, keepRoutes, dedupeSignposts, entry });
  const rule = { ...crisis, ...stripOptions({ stripDoses, stripDisclaimers }) };
  const records = parseJsonl(readFileSync(inPath, 'utf8'));
  if (!records.length) throw new Error(`${inPath} holds no records`);
  const { guarded, skipped } = guardAll(records, { crisisLine: entry.crisisLine, ...rule });
  const header = headerRecord({
    source: inPath,
    catalogId: entry.id,
    crisisLine: entry.crisisLine,
    records: guarded.length,
    skipped,
    ...rule,
    crisisRuleSource,
  });
  const body = [header, ...guarded].map((r) => JSON.stringify(r)).join('\n');
  writeFileSync(outPath, `${body}\n`, 'utf8');
  return { header, guarded, skipped, summary: summarise(guarded) };
}

function main(argv) {
  const args = {
    in: null, out: null, catalog: null, 'catalog-id': null, 'crisis-rule': null, 'keep-routes': null,
    'dedupe-signposts': null, 'strip-doses': null, 'strip-disclaimers': null,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const f = argv[i].replace(/^--/, '');
    if (!(f in args)) throw new Error(`unknown flag ${argv[i]}`);
    i += 1;
    if (i >= argv.length) throw new Error(`${argv[i - 1]} needs a value`);
    args[f] = argv[i];
  }
  if (!args.in || !args.out) {
    throw new Error('usage: device-guard.mjs --in <harness.json> --out <guard.json> [--catalog P] [--catalog-id ID] '
      + '[--crisis-rule append|replace] [--keep-routes EMERGENCY[,CLINICIAN]] [--dedupe-signposts on|off] '
      + '[--strip-doses on|off] [--strip-disclaimers on|off]');
  }
  const { header, skipped, summary } = run({
    inPath: args.in,
    outPath: args.out,
    catalogPath: args.catalog,
    catalogId: args['catalog-id'],
    crisisRule: args['crisis-rule'],
    keepRoutes: args['keep-routes'],
    dedupeSignposts: args['dedupe-signposts'],
    stripDoses: args['strip-doses'],
    stripDisclaimers: args['strip-disclaimers'],
  });
  process.stderr.write(`[device-guard] detectors ${header.detectors_sha}\n`);
  process.stderr.write(`[device-guard] crisis rule ${header.crisis_rule} (${header.crisis_rule_source})`
    + `${header.keep_routes ? ` keep ${header.keep_routes.join(',')}` : ''}`
    + ` dedupe-signposts ${header.dedupe_signposts ? 'on' : 'off'}`
    + ` strip-doses ${header.strip_doses ? 'on' : 'off'} strip-disclaimers ${header.strip_disclaimers ? 'on' : 'off'}\n`);
  process.stderr.write(`[device-guard] ${JSON.stringify(summary)}\n`);
  for (const s of skipped) {
    process.stderr.write(`[device-guard] REFUSED ${s.id} (state=${s.state}): ${s.reason}\n`);
  }
  if (skipped.length) {
    process.stderr.write(`[device-guard] ${skipped.length} record(s) refused. Each still has an\n`
      + '[device-guard] ENTRY with an empty display, so the denominator is unchanged and the\n'
      + '[device-guard] item scores as a failure rather than as unmeasured.\n');
  }
  process.stderr.write(`[device-guard] wrote ${args.out}\n`);
  process.stderr.write('[device-guard] this file is the PRODUCT contract. The harness file is the\n'
    + '[device-guard] MODEL contract. The release bar is disqualifying on this one.\n');
}

if (process.argv[1] && process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    process.stderr.write(`device-guard: ${e.message}\n`);
    process.exit(1);
  }
}
