// The vision probe scorer: turns the bench's JSONL records into pass/fail
// against the Stage 3 gates. Pure except for the CLI at the bottom, which only
// runs when this file is the entry point.
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { normalise } from './normalise.mjs';

export const GATES = {
  exactMin: 17,
  exactOf: 18,
  acceptedSignErrorsMax: 0,
  refusalsMin: 2,
  floorP50Ms: 8000,
  floorP95Ms: 12000,
  floorVmHwmKb: 1_258_291, // 1.2 GiB in KiB
};

const SIGN_WEIGHT = 5;
const SIGNS = new Set(['+', '-']);
const RELATIONS = new Set(['=', '<', '>', '\\le', '\\ge', '\\ne']);

export class RecordsError extends Error {
  constructor(message) {
    super(message);
    this.name = 'RecordsError';
  }
}

// Tokens: a known control word, any other control word, a control symbol, or a
// single character. Normalisation removes whitespace, so `2\\times a` reads
// `2\\timesa`; the known words are split off first for that reason.
function tokens(s) {
  return s.match(/\\(?:times|div|le|ge|ne|frac|sqrt|pm)|\\[A-Za-z]+|\\.|[\s\S]/g) ?? [];
}

// Levenshtein alignment over arrays. Returns the distance and the aligned
// substitution pairs [expected, got]. O(n*m); the strings here are <= ~200.
function align(a, b) {
  const n = a.length;
  const m = b.length;
  const d = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = 0; i <= n; i++) d[i][0] = i;
  for (let j = 0; j <= m; j++) d[0][j] = j;
  for (let i = 1; i <= n; i++) {
    for (let j = 1; j <= m; j++) {
      d[i][j] = Math.min(
        d[i - 1][j] + 1,
        d[i][j - 1] + 1,
        d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1),
      );
    }
  }
  const subs = [];
  let i = n;
  let j = m;
  while (i > 0 && j > 0) {
    const cost = a[i - 1] === b[j - 1] ? 0 : 1;
    if (d[i][j] === d[i - 1][j - 1] + cost) {
      if (cost) subs.push([a[i - 1], b[j - 1]]);
      i--;
      j--;
    } else if (d[i][j] === d[i - 1][j] + 1) {
      i--;
    } else {
      j--;
    }
  }
  return { distance: d[n][m], subs };
}

function isSignSub(e, g) {
  if (SIGNS.has(e) && SIGNS.has(g)) return true;
  const times = (t) => t === '\\times';
  const other = (t) => SIGNS.has(t) || t === '\\div';
  return (times(e) && other(g)) || (times(g) && other(e));
}

function isOperatorSub(e, g) {
  return RELATIONS.has(e) && RELATIONS.has(g);
}

export function recordLatex(record) {
  return (record.blocks ?? [])
    .filter((b) => b.kind === 'latex')
    .map((b) => b.content)
    .join('\\\\');
}

export function scoreItem(item, record) {
  const errored = record.error != null;
  const verdictGot = errored ? 'error' : record.verdict;
  const verdictExpected = item.verdict;
  const base = { id: item.id, verdictExpected, verdictGot };

  if (item.latex == null) {
    // A refusal item: correct iff the product refused.
    const exact = !errored && record.verdict === 'refuse';
    return {
      ...base, exact, cer: exact ? 0 : 1,
      signSubstitutions: 0, operatorSubstitutions: 0, weightedErrors: 0,
      verdictOk: exact, survivesToTutor: false,
    };
  }

  const expected = normalise(item.latex);
  // An accept item is satisfied by accept or review; refuse and error are not.
  const verdictOk = !errored && (record.verdict === 'accept' || record.verdict === 'review');
  if (errored) {
    return {
      ...base, exact: false, cer: expected.length === 0 ? 0 : 1,
      signSubstitutions: 0, operatorSubstitutions: 0,
      weightedErrors: tokens(expected).length, verdictOk, survivesToTutor: false,
    };
  }

  const got = normalise(recordLatex(record));
  const exact = expected === got;
  const chars = align([...expected], [...got]).distance;
  const cer = expected.length === 0 ? (got.length === 0 ? 0 : 1) : chars / expected.length;
  const { distance, subs } = align(tokens(expected), tokens(got));
  let signSubstitutions = 0;
  let operatorSubstitutions = 0;
  for (const [e, g] of subs) {
    if (isSignSub(e, g)) signSubstitutions++;
    else if (isOperatorSub(e, g)) operatorSubstitutions++;
  }
  const otherEdits = distance - signSubstitutions - operatorSubstitutions;
  return {
    ...base, exact, cer, signSubstitutions, operatorSubstitutions,
    weightedErrors: SIGN_WEIGHT * (signSubstitutions + operatorSubstitutions) + otherEdits,
    verdictOk,
    survivesToTutor: record.verdict === 'accept' && signSubstitutions > 0,
  };
}

// Nearest-rank percentile over a numeric array.
function percentile(values, p) {
  const sorted = [...values].sort((x, y) => x - y);
  return sorted[Math.max(0, Math.ceil((p / 100) * sorted.length) - 1)];
}

// Group records by id and repeat; every item needs exactly one record per
// repeat seen anywhere in the file. Returns the last repeat's record per id.
function lastRepeat(items, records) {
  const byKey = new Map();
  const reps = new Set();
  for (const r of records) {
    const rep = r.rep ?? 0;
    reps.add(rep);
    const key = `${r.id}\u0000${rep}`;
    if (byKey.has(key)) throw new RecordsError(`duplicate record for id ${r.id} rep ${rep}`);
    byKey.set(key, r);
  }
  const repList = [...reps].sort((a, b) => a - b);
  if (repList.length === 0) throw new RecordsError('no records');
  const missing = [];
  for (const it of items) {
    for (const rep of repList) {
      if (!byKey.has(`${it.id}\u0000${rep}`)) missing.push(`${it.id} (rep ${rep})`);
    }
  }
  if (missing.length) throw new RecordsError(`missing records: ${missing.join(', ')}`);
  const last = repList[repList.length - 1];
  return new Map(items.map((it) => [it.id, byKey.get(`${it.id}\u0000${last}`)]));
}

export function scoreRun(items, records, { device = false } = {}) {
  const chosen = lastRepeat(items, records);
  const rows = items.map((it) => scoreItem(it, chosen.get(it.id)));
  const total = rows.length;
  const exact = rows.filter((r) => r.exact).length;
  const refuseIds = new Set(items.filter((it) => it.latex == null).map((it) => it.id));
  const refusalsOk = rows.filter((r) => refuseIds.has(r.id) && r.verdictOk).length;
  const survivesToTutor = rows.filter((r) => r.survivesToTutor).length;
  const exactRequired = total === GATES.exactOf
    ? GATES.exactMin
    : Math.ceil((GATES.exactMin / GATES.exactOf) * total);

  const failures = [];
  if (exact < exactRequired) failures.push(`exact ${exact}/${total} is below the gate of ${exactRequired}/${total}`);
  if (survivesToTutor > GATES.acceptedSignErrorsMax) {
    failures.push(`${survivesToTutor} accepted transcription(s) with a sign error reach the tutor (max ${GATES.acceptedSignErrorsMax})`);
  }
  if (refusalsOk < GATES.refusalsMin) failures.push(`refusals ${refusalsOk} is below the gate of ${GATES.refusalsMin}`);

  let latency = null;
  let vmHwmKbMax = null;
  if (device) {
    const totals = [...chosen.values()].map((r) => r.total_ms);
    const hwms = [...chosen.values()].map((r) => r.vmhwm_kb);
    if (totals.some((v) => typeof v !== 'number') || hwms.some((v) => typeof v !== 'number')) {
      failures.push('device mode needs numeric total_ms and vmhwm_kb on every record');
    } else {
      latency = { p50: percentile(totals, 50), p95: percentile(totals, 95) };
      vmHwmKbMax = Math.max(...hwms);
      if (latency.p50 > GATES.floorP50Ms) failures.push(`p50 ${latency.p50} ms is above the floor of ${GATES.floorP50Ms} ms`);
      if (latency.p95 > GATES.floorP95Ms) failures.push(`p95 ${latency.p95} ms is above the floor of ${GATES.floorP95Ms} ms`);
      if (vmHwmKbMax > GATES.floorVmHwmKb) failures.push(`VmHWM ${vmHwmKbMax} KiB is above the floor of ${GATES.floorVmHwmKb} KiB`);
    }
  }
  return { rows, exact, total, exactRequired, refusalsOk, survivesToTutor, pass: failures.length === 0, latency, vmHwmKbMax, failures };
}

export function toMarkdown(result) {
  const L = [];
  L.push(`# Vision probe score: ${result.pass ? 'PASS' : 'FAIL'}`, '');
  L.push('| Gate | Value | Requirement |', '|---|---|---|');
  L.push(`| exact | ${result.exact}/${result.total} | >= ${result.exactRequired} |`);
  L.push(`| accepted sign errors | ${result.survivesToTutor} | <= ${GATES.acceptedSignErrorsMax} |`);
  L.push(`| refusals correct | ${result.refusalsOk} | >= ${GATES.refusalsMin} |`);
  if (result.latency) {
    L.push(`| p50 total_ms | ${result.latency.p50} | <= ${GATES.floorP50Ms} |`);
    L.push(`| p95 total_ms | ${result.latency.p95} | <= ${GATES.floorP95Ms} |`);
    L.push(`| VmHWM max (KiB) | ${result.vmHwmKbMax} | <= ${GATES.floorVmHwmKb} |`);
  }
  L.push('');
  if (result.failures.length) {
    L.push('## Failures', '', ...result.failures.map((f) => `- ${f}`), '');
  }
  L.push('## Items', '', '| id | exact | CER | sign | operator | weighted | expected | got | tutor |', '|---|---|---|---|---|---|---|---|---|');
  for (const r of result.rows) {
    L.push(`| ${r.id} | ${r.exact ? 'yes' : 'NO'} | ${r.cer.toFixed(3)} | ${r.signSubstitutions} | ${r.operatorSubstitutions} | ${r.weightedErrors} | ${r.verdictExpected} | ${r.verdictGot} | ${r.survivesToTutor ? 'YES' : 'no'} |`);
  }
  return L.join('\n') + '\n';
}

export function toJson(result) {
  return JSON.stringify(result, null, 2) + '\n';
}

function parseArgs(argv) {
  const o = { device: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--device') o.device = true;
    else if (a === '--items') o.items = argv[++i];
    else if (a === '--records') o.records = argv[++i];
    else if (a === '--out-md') o.outMd = argv[++i];
    else if (a === '--out-json') o.outJson = argv[++i];
    else throw new Error(`unknown argument ${a}`);
  }
  if (!o.items || !o.records) throw new Error('usage: score.mjs --items <items.json> --records <jsonl> [--device] --out-md <path> --out-json <path>');
  return o;
}

export function main(argv) {
  let o;
  try {
    o = parseArgs(argv);
  } catch (e) {
    process.stderr.write(`${e.message}\n`);
    return 2;
  }
  let result;
  try {
    const items = JSON.parse(readFileSync(o.items, 'utf8')).items;
    const records = readFileSync(o.records, 'utf8').split('\n').filter((l) => l.trim()).map((l) => JSON.parse(l));
    result = scoreRun(items, records, { device: o.device });
  } catch (e) {
    process.stderr.write(`${e.message}\n`);
    return 2;
  }
  const md = toMarkdown(result);
  if (o.outMd) writeFileSync(o.outMd, md);
  if (o.outJson) writeFileSync(o.outJson, toJson(result));
  process.stdout.write(md);
  return result.pass ? 0 : 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exitCode = main(process.argv.slice(2));
}
