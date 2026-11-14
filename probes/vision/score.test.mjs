import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { GATES, scoreItem, scoreRun, toMarkdown, toJson } from './score.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const CLI = join(HERE, 'score.mjs');

const item = (id, latex, verdict = 'accept', kind = 'printed') => ({ id, file: `${id}.png`, kind, latex, plain: '', verdict, notes: '' });
const rec = (id, latex, verdict = 'accept', extra = {}) => ({
  id, file: `${id}.png`, rep: 0, verdict, warnings: [],
  blocks: latex === null ? [] : [{ kind: 'latex', content: latex, confidence: 0.99, min_operator_prob: 0.99 }],
  raw: '', n_image_tokens: 0, stop: 'eos', prep_ms: 0, prefill_ms: 0, decode_ms: 0,
  total_ms: 1000, vmrss_kb: 1, vmhwm_kb: 1000, error: null, ...extra,
});

// 16 printed + 2 refusals, every one correct.
function fixture() {
  const items = [];
  const records = [];
  for (let i = 1; i <= 16; i++) {
    const id = `q${String(i).padStart(2, '0')}`;
    items.push(item(id, `${i}x+7=2`));
    records.push(rec(id, `${i}x+7=2`));
  }
  items.push(item('q17', null, 'refuse', 'diagram'), item('q18', null, 'refuse', 'prose'));
  records.push(rec('q17', null, 'refuse'), rec('q18', null, 'refuse'));
  return { items, records };
}

test('GATES are pinned', () => {
  // The spec's scaling (S3-P11): 18 of 20 once the photographs join, 17 of 18 now.
  assert.equal(GATES.exactMin(20), 18);
  assert.equal(GATES.exactMin(18), 17);
  assert.equal(GATES.exactMin(19), 18);
  assert.equal(GATES.exactMin(22), 20);
  assert.equal(GATES.acceptedSignErrorsMax, 0);
  // The refusal gate is every refuse item in the set (computed, never a constant).
  const set = (n) => Array.from({ length: n }, (_, i) => item(`r${i}`, null, 'refuse'));
  assert.equal(GATES.refusalsMin([...set(2), item('a', 'x')]), 2);
  assert.equal(GATES.refusalsMin([...set(4), item('a', 'x'), item('b', 'y')]), 4);
  assert.equal(GATES.refusalsMin([item('a', 'x')]), 0);
  // The accepted floor regime (S3-P12).
  assert.equal(GATES.floorP50Ms, 10_000);
  assert.equal(GATES.floorP95Ms, 12_000);
  assert.equal(GATES.floorVmHwmKb, 1_258_291);
});

test('exact match survives normalisation differences', () => {
  const s = scoreItem(item('a', '\\frac{1}{2}x \\le 3'), rec('a', '\\dfrac12 x \\leq 3'));
  assert.equal(s.exact, true);
  assert.equal(s.cer, 0);
  assert.equal(s.weightedErrors, 0);
});

test('multiple latex blocks are joined with \\\\ and text blocks ignored', () => {
  const r = rec('a', 'x+y=7');
  r.blocks.push({ kind: 'text', content: 'hello', confidence: 1, min_operator_prob: null });
  r.blocks.push({ kind: 'latex', content: 'x-y=1', confidence: 1, min_operator_prob: null });
  assert.equal(scoreItem(item('a', 'x+y=7 \\\\ x-y=1'), r).exact, true);
});

test('CER arithmetic: one substitution over expected length', () => {
  // normalised expected "3x+7=2" has 6 chars; "3x+7=5" differs by one.
  const s = scoreItem(item('a', '3x + 7 = 2'), rec('a', '3x + 7 = 5'));
  assert.equal(s.exact, false);
  assert.equal(s.cer, 1 / 6);
  assert.equal(s.signSubstitutions, 0);
  assert.equal(s.weightedErrors, 1);
});

test('CER edge cases: both empty is 0, empty expected with output is 1', () => {
  assert.equal(scoreItem(item('a', ''), rec('a', '')).cer, 0);
  assert.equal(scoreItem(item('a', ''), rec('a', 'x')).cer, 1);
});

test('a sign flip counts once and is weighted x5', () => {
  const s = scoreItem(item('a', '3x+7=2x+5'), rec('a', '3x-7=2x+5'));
  assert.equal(s.signSubstitutions, 1);
  assert.equal(s.operatorSubstitutions, 0);
  assert.equal(s.weightedErrors, 5);
  assert.equal(s.survivesToTutor, true);
});

test('\\times <-> + and \\times <-> - are sign substitutions', () => {
  assert.equal(scoreItem(item('a', '2\\times a'), rec('a', '2+a')).signSubstitutions, 1);
  assert.equal(scoreItem(item('a', '2-a'), rec('a', '2\\times a')).signSubstitutions, 1);
});

test('a swapped relation is an operator substitution, weighted x5, not a sign one', () => {
  const s = scoreItem(item('a', 'a\\le b'), rec('a', 'a\\ge b'));
  assert.equal(s.operatorSubstitutions, 1);
  assert.equal(s.signSubstitutions, 0);
  assert.equal(s.weightedErrors, 5);
  const t = scoreItem(item('a', 'a=b'), rec('a', 'a<b'));
  assert.equal(t.operatorSubstitutions, 1);
});

test('an insertion beside a flipped sign is one sign error plus one plain edit', () => {
  const s = scoreItem(item('a', 'x+1=2'), rec('a', 'x-1=22'));
  assert.equal(s.signSubstitutions, 1);
  assert.equal(s.weightedErrors, 6);
});

test('survivesToTutor needs verdict accept', () => {
  assert.equal(scoreItem(item('a', '3x+7'), rec('a', '3x-7', 'review')).survivesToTutor, false);
  assert.equal(scoreItem(item('a', '3x+7'), rec('a', '3x-7', 'refuse')).survivesToTutor, false);
});

test('an accepted operator swap survives to the tutor too; in review it does not', () => {
  const acc = scoreItem(item('a', 'a\\le b'), rec('a', 'a\\ge b', 'accept'));
  assert.equal(acc.signSubstitutions, 0);
  assert.equal(acc.operatorSubstitutions, 1);
  assert.equal(acc.survivesToTutor, true);
  assert.equal(scoreItem(item('a', 'a\\le b'), rec('a', 'a\\ge b', 'review')).survivesToTutor, false);
});

test('an accepted run with an operator swap fails the gate', () => {
  const { items, records } = fixture();
  items[0] = item('q01', 'a\\le b');
  records[0] = rec('q01', 'a\\ge b', 'accept');
  const r = scoreRun(items, records);
  assert.equal(r.survivesToTutor, 1);
  assert.equal(r.pass, false);
});

// The crop-filling DejaVu render under the product prompt
// (host-v1-dejavu-fill-product.jsonl, verbatim): the italic sans `x` read as `X`
// and accepted at ~0.99, above every threshold.
const DEJAVU_Q08 = {
  item: item('q08', '\\frac{3}{4}x - \\frac{1}{2} = \\frac{5}{8}'),
  rec: {
    ...rec('q08', null, 'accept'),
    blocks: [{ kind: 'latex', content: '\\frac{3}{4}X-\\frac{1}{2}=\\frac{5}{8}', confidence: 0.99353224, min_operator_prob: 0.99804217 }],
  },
};
const DEJAVU_Q13 = {
  item: item('q13', '\\left(\\frac{1}{2}\\right)^x = 8'),
  rec: {
    ...rec('q13', null, 'accept'),
    blocks: [{ kind: 'latex', content: '\\left(\\frac{1}{2}\\right)^{X}=8', confidence: 0.9747802, min_operator_prob: 0.9933092 }],
  },
};

test('the DejaVu x read as X is a symbol substitution, weighted x5, and survives to the tutor', () => {
  for (const { item: it, rec: r } of [DEJAVU_Q08, DEJAVU_Q13]) {
    const s = scoreItem(it, r);
    assert.equal(s.exact, false, it.id);
    assert.equal(s.symbolSubstitutions, 1, it.id);
    assert.equal(s.signSubstitutions, 0, it.id);
    assert.equal(s.operatorSubstitutions, 0, it.id);
    assert.equal(s.weightedErrors, 5, it.id);
    assert.equal(s.survivesToTutor, true, it.id);
  }
  // In review the student reads it first: it does not survive.
  assert.equal(scoreItem(DEJAVU_Q08.item, { ...DEJAVU_Q08.rec, verdict: 'review' }).survivesToTutor, false);
});

test('every look-alike pair counts in both directions; any other letter swap is a plain edit', () => {
  for (const [a, b] of [['x', 'X'], ['1', 'l'], ['0', 'O'], ['5', 'S'], ['2', 'Z']]) {
    for (const [e, g] of [[a, b], [b, a]]) {
      const s = scoreItem(item('a', `${e}+y=7`), rec('a', `${g}+y=7`));
      assert.equal(s.symbolSubstitutions, 1, `${e}->${g}`);
      assert.equal(s.weightedErrors, 5, `${e}->${g}`);
      assert.equal(s.survivesToTutor, true, `${e}->${g}`);
    }
  }
  const plain = scoreItem(item('a', 'x+1=2'), rec('a', 'y+1=2'));
  assert.equal(plain.symbolSubstitutions, 0);
  assert.equal(plain.weightedErrors, 1);
  assert.equal(plain.survivesToTutor, false);
});

test('an accepted symbol substitution fails the run and is reported in its own column', () => {
  const { items, records } = fixture();
  items[7] = DEJAVU_Q08.item;
  records[7] = DEJAVU_Q08.rec;
  const r = scoreRun(items, records);
  assert.equal(r.exact, 17);
  assert.equal(r.survivesToTutor, 1);
  assert.equal(r.pass, false);
  assert.ok(r.failures.some((f) => /symbol/.test(f)), r.failures.join('; '));
  const md = toMarkdown(r);
  assert.match(md, /\| id \| exact \| CER \| sign \| operator \| symbol \| weighted \|/);
  assert.match(md, /\| q08 \| NO \| 0\.0\d\d \| 0 \| 0 \| 1 \| 5 \|/);
  assert.equal(JSON.parse(toJson(r)).rows[7].symbolSubstitutions, 1);
});

test('refuse item: exact iff the verdict is refuse', () => {
  const ok = scoreItem(item('d', null, 'refuse', 'diagram'), rec('d', null, 'refuse'));
  assert.equal(ok.exact, true);
  assert.equal(ok.verdictOk, true);
  assert.equal(ok.signSubstitutions, 0);
  const bad = scoreItem(item('d', null, 'refuse', 'diagram'), rec('d', 'x+1', 'accept'));
  assert.equal(bad.exact, false);
  assert.equal(bad.verdictOk, false);
  assert.equal(bad.operatorSubstitutions, 0);
  assert.equal(bad.survivesToTutor, false);
});

test('an errored record is never exact and never matches an expectation', () => {
  const s = scoreItem(item('a', 'x'), rec('a', 'x', 'accept', { error: 'boom' }));
  assert.equal(s.exact, false);
  assert.equal(s.verdictGot, 'error');
  assert.equal(s.verdictOk, false);
  const r = scoreItem(item('d', null, 'refuse'), rec('d', null, 'refuse', { error: 'boom' }));
  assert.equal(r.exact, false);
  assert.equal(r.verdictOk, false);
});

test('a clean run passes', () => {
  const { items, records } = fixture();
  const r = scoreRun(items, records);
  assert.equal(r.total, 18);
  assert.equal(r.exact, 18);
  assert.equal(r.refusalsOk, 2);
  assert.equal(r.refusalsRequired, 2);
  assert.match(toMarkdown(r), /refusals correct \| 2 \| >= 2 \(every/);
  assert.equal(r.survivesToTutor, 0);
  assert.equal(r.pass, true);
  assert.deepEqual(r.failures, []);
  assert.equal(r.latency, null);
  assert.equal(r.vmHwmKbMax, null);
});

test('one miss of 18 still passes (17/18), two misses fail', () => {
  const { items, records } = fixture();
  records[0] = rec('q01', '9x+9=9', 'review');
  assert.equal(scoreRun(items, records).pass, true);
  records[1] = rec('q02', '9x+9=9', 'review');
  const r = scoreRun(items, records);
  assert.equal(r.exact, 16);
  assert.equal(r.pass, false);
  assert.ok(r.failures.some((f) => /exact/i.test(f)));
});

test('a sign flip accepted fails the run; the same in review passes', () => {
  const { items, records } = fixture();
  records[0] = rec('q01', '1x-7=2', 'accept');
  const bad = scoreRun(items, records);
  assert.equal(bad.survivesToTutor, 1);
  assert.equal(bad.pass, false);
  assert.ok(bad.failures.some((f) => /sign/i.test(f)));
  records[0] = rec('q01', '1x-7=2', 'review');
  const good = scoreRun(items, records);
  assert.equal(good.survivesToTutor, 0);
  assert.equal(good.pass, true);
});

test('a missed refusal (diagram accepted) fails', () => {
  const { items, records } = fixture();
  records[16] = rec('q17', 'x+1=2', 'accept');
  const r = scoreRun(items, records);
  assert.equal(r.refusalsOk, 1);
  assert.equal(r.pass, false);
  assert.ok(r.failures.some((f) => /refus/i.test(f)));
});

test('the exact gate scales by the spec: 18 of 20 with the photographs, 17 of 18 without', () => {
  const { items, records } = fixture();
  for (const id of ['q19', 'q20']) {
    items.push(item(id, 'x+1=2'));
    records.push(rec(id, 'x+1=2'));
  }
  records[0] = rec('q01', '9x+9=9', 'review');
  records[1] = rec('q02', '9x+9=9', 'review');
  const two = scoreRun(items, records);
  assert.equal(two.exact, 18);
  assert.equal(two.exactRequired, 18);
  assert.equal(two.pass, true);
  assert.match(toMarkdown(two), /total - 2/);
  records[2] = rec('q03', '9x+9=9', 'review');
  const three = scoreRun(items, records);
  assert.equal(three.pass, false);
  assert.match(three.failures.join('\n'), /17\/20 is below the gate of 18\/20/);

  const eighteen = scoreRun(...Object.values(fixture()));
  assert.equal(eighteen.exactRequired, 17);
  assert.match(toMarkdown(eighteen), /total - 1/);
});

test('the refusal gate scales with the refuse items: all four of 22 items', () => {
  const { items, records } = fixture();
  for (const id of ['q19', 'q20']) {
    items.push(item(id, 'x+1=2', 'accept', 'photo'));
    records.push(rec(id, 'x+1=2'));
  }
  items.push(item('q21', null, 'refuse', 'diagram'), item('q22', null, 'refuse', 'prose'));
  records.push(rec('q21', null, 'refuse'), rec('q22', null, 'refuse'));
  const ok = scoreRun(items, records);
  assert.equal(ok.total, 22);
  assert.equal(ok.exactRequired, 20);
  assert.equal(ok.refusalsRequired, 4);
  assert.equal(ok.refusalsOk, 4);
  assert.equal(ok.pass, true);
  assert.match(toMarkdown(ok), /refusals correct \| 4 \| >= 4 \(every/);
  // One held-out refusal missed: 3 of 4 fails the refusal gate although exact is 21/22.
  records[21] = rec('q22', 'x+1=2', 'accept');
  const miss = scoreRun(items, records);
  assert.equal(miss.exact, 21);
  assert.equal(miss.refusalsOk, 3);
  assert.equal(miss.pass, false);
  assert.match(miss.failures.join('\n'), /refusals 3 is below the gate of 4/);
});

test('device mode: latency percentiles over the last repeat, nearest rank', () => {
  const { items, records } = fixture();
  // rep 0 slow, rep 1 fast: only rep 1 counts.
  const reps = [];
  for (const r of records) {
    reps.push({ ...r, rep: 0, total_ms: 99999 });
    reps.push({ ...r, rep: 1, total_ms: 1000 + records.indexOf(r) * 100 });
  }
  const r = scoreRun(items, reps, { device: true });
  // totals 1000..2700 step 100, n=18: p50 rank ceil(9)=9 -> 1800; p95 rank ceil(17.1)=18 -> 2700.
  assert.deepEqual(r.latency, { p50: 1800, p95: 2700 });
  assert.equal(r.pass, true);
});

test('device mode: p50 over 10000, p95 over 12000, VmHWM over the floor each fail', () => {
  const { items, records } = fixture();
  // Stage 2's A22 readings were 8,991-9,403 ms: inside the accepted floor band.
  const floor = scoreRun(items, records.map((r) => ({ ...r, total_ms: 9500 })), { device: true });
  assert.equal(floor.latency.p50, 9500);
  assert.equal(floor.pass, true, floor.failures.join('; '));
  const slow50 = records.map((r) => ({ ...r, total_ms: 10_500 }));
  const a = scoreRun(items, slow50, { device: true });
  assert.equal(a.pass, false);
  assert.ok(a.failures.some((f) => /p50 10500 ms is above the floor of 10000 ms/.test(f)), a.failures.join('; '));

  const slow95 = records.map((r, i) => ({ ...r, total_ms: i >= 16 ? 13000 : 1000 }));
  const b = scoreRun(items, slow95, { device: true });
  assert.equal(b.pass, false);
  assert.ok(b.failures.some((f) => /p95/.test(f)));
  assert.ok(!b.failures.some((f) => /p50/.test(f)));

  const fat = records.map((r, i) => ({ ...r, vmhwm_kb: i === 3 ? 1_300_000 : 1000 }));
  const c = scoreRun(items, fat, { device: true });
  assert.equal(c.vmHwmKbMax, 1_300_000);
  assert.equal(c.pass, false);
  assert.ok(c.failures.some((f) => /VmHWM/i.test(f)));
});

test('latency and memory are not gated without --device', () => {
  const { items, records } = fixture();
  const r = scoreRun(items, records.map((x) => ({ ...x, total_ms: 99999, vmhwm_kb: 9e9 })));
  assert.equal(r.pass, true);
});

test('a missing record throws a RecordsError; a duplicate (id, rep) too', () => {
  const { items, records } = fixture();
  assert.throws(() => scoreRun(items, records.slice(1)), (e) => e.name === 'RecordsError' && /q01/.test(e.message));
  assert.throws(() => scoreRun(items, [...records, records[0]]), (e) => e.name === 'RecordsError' && /duplicate/i.test(e.message));
});

test('every item needs a record for every repeat seen', () => {
  const { items, records } = fixture();
  const reps = [...records, { ...records[0], rep: 1 }];
  assert.throws(() => scoreRun(items, reps), (e) => e.name === 'RecordsError');
});

test('toMarkdown and toJson carry the verdict and the rows', () => {
  const { items, records } = fixture();
  records[0] = rec('q01', '1x-7=2', 'accept');
  const r = scoreRun(items, records, { device: true });
  const md = toMarkdown(r);
  assert.match(md, /FAIL/);
  assert.match(md, /q01/);
  assert.match(md, /p50/);
  const back = JSON.parse(toJson(r));
  assert.equal(back.pass, false);
  assert.equal(back.rows.length, 18);
});

function runCli(args) {
  return spawnSync(process.execPath, [CLI, ...args], { encoding: 'utf8' });
}

test('CLI: exit 0 on pass, 1 on a gate failure, 2 on a missing record; writes both reports', () => {
  const dir = mkdtempSync(join(tmpdir(), 'score-'));
  const { items, records } = fixture();
  const itemsPath = join(dir, 'items.json');
  writeFileSync(itemsPath, JSON.stringify({ version: 1, items }));
  const write = (name, recs) => {
    const p = join(dir, name);
    writeFileSync(p, recs.map((r) => JSON.stringify(r)).join('\n') + '\n');
    return p;
  };
  const md = join(dir, 'out.md');
  const js = join(dir, 'out.json');

  const ok = runCli(['--items', itemsPath, '--records', write('ok.jsonl', records), '--out-md', md, '--out-json', js]);
  assert.equal(ok.status, 0, ok.stderr);
  assert.match(ok.stdout, /PASS/);
  assert.equal(readFileSync(md, 'utf8'), ok.stdout);
  assert.equal(JSON.parse(readFileSync(js, 'utf8')).pass, true);

  const bad = records.slice();
  bad[0] = rec('q01', '1x-7=2', 'accept');
  const fail = runCli(['--items', itemsPath, '--records', write('bad.jsonl', bad), '--out-md', md, '--out-json', js]);
  assert.equal(fail.status, 1);

  const miss = runCli(['--items', itemsPath, '--records', write('miss.jsonl', records.slice(1)), '--out-md', md, '--out-json', js]);
  assert.equal(miss.status, 2);
  assert.match(miss.stderr, /q01/);
});

test('importing the module runs nothing', async () => {
  const m = await import('./score.mjs');
  assert.equal(typeof m.scoreRun, 'function');
});
