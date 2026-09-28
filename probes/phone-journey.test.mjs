// probes/phone-journey.test.mjs — the founder's phone checklist cannot drift
// from the code it describes (Phase 1i TC1, controller ruling 1).
//
// `docs/superpowers/mobile-tools/phone-journey.md` tells the founder, step by
// step, the EXACT text the phone must show. A checklist that quotes a crisis
// line the product no longer shows is worse than none: the founder ticks PASS
// against the wrong words. So every quoted screen string in it is written next
// to the expression that produces it, and this test evaluates the expression
// against the shipped modules and compares byte for byte:
//
//   <!-- expect: guard.CRISIS_LINE_REPLACE -->
//   ```text
//   If you are having thoughts ...
//   ```
//
// Screen text that is a literal inside a file rather than an exported constant
// (a button label in index.html, a tooltip in app.js, the downgrade refusal in
// catalog_dist.rs) is pinned the other way round: the quoted block must occur
// verbatim in the named file.
//
//   <!-- expect-source: src/app.js -->
//   ```text
//   Signed in — offline, using saved account data
//   ```
//
// The step structure is pinned too: every step the TC1 brief lists exists,
// carries a PASS box and a FAIL box, and names its evidence.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import * as guard from '../src/triage/guard.js';
import * as lookupGuard from '../src/triage/lookup-guard.js';
import * as lookupTurn from '../src/lookup-turn.js';
import * as confirm from '../src/triage-confirm.js';
import * as triageTurn from '../src/triage-turn.js';

const ROOT = fileURLToPath(new URL('../', import.meta.url));
const CHECKLIST = new URL('../docs/superpowers/mobile-tools/phone-journey.md', import.meta.url);
const catalog = JSON.parse(readFileSync(new URL('../src-tauri/resources/catalog.triage.json', import.meta.url), 'utf8'));
const entry = catalog.find((e) => e.real);

/** The steps the TC1 brief lists, in the order the founder runs them. */
export const STEP_IDS = Object.freeze([
  'S1', 'S2', 'S3', 'S4',
  'T1', 'T2', 'T3', 'T4', 'T5', 'T6', 'T7', 'T8',
  'L1', 'L2', 'L3', 'L4', 'L5', 'L6',
  'E1', 'R1', 'C1',
]);
const EXPORT_STEPS = STEP_IDS.filter((id) => /^[TL]\d$/.test(id));

const readChecklist = () => readFileSync(CHECKLIST, 'utf8');

/** `<!-- expect: EXPR -->` followed by a ```text block. */
export function expectBlocks(md) {
  const out = [];
  const re = /<!-- expect: (.+?) -->\n```text\n([\s\S]*?)\n```/g;
  for (let m = re.exec(md); m; m = re.exec(md)) out.push({ expr: m[1], text: m[2], at: m.index });
  return out;
}

/** `<!-- expect-source: PATH -->` followed by a ```text block. */
export function sourceBlocks(md) {
  const out = [];
  const re = /<!-- expect-source: (.+?) -->\n```text\n([\s\S]*?)\n```/g;
  for (let m = re.exec(md); m; m = re.exec(md)) out.push({ path: m[1], text: m[2], at: m.index });
  return out;
}

/** `<!-- input: ID -->` followed by a ```text block: what the founder types. */
export function inputBlocks(md) {
  const out = {};
  const re = /<!-- input: ([A-Z]\d) -->\n```text\n([\s\S]*?)\n```/g;
  for (let m = re.exec(md); m; m = re.exec(md)) out[m[1]] = m[2];
  return out;
}

/** `### ID — title` sections, each to the next `### ` or `## `. */
export function steps(md) {
  const out = {};
  const re = /^### ([A-Z]\d) — (.+)$/gm;
  const heads = [];
  for (let m = re.exec(md); m; m = re.exec(md)) heads.push({ id: m[1], title: m[2], at: m.index });
  for (let i = 0; i < heads.length; i += 1) {
    const rest = md.slice(heads[i].at);
    const next = rest.slice(4).search(/^##/m);
    out[heads[i].id] = { title: heads[i].title, body: next === -1 ? rest : rest.slice(0, next + 4) };
  }
  return out;
}

function evaluate(expr) {
  // The checklist is repo-controlled text, and these expressions only ever
  // name the shipped modules' exports; `Function` keeps them to exactly those
  // names.
  // eslint-disable-next-line no-new-func
  const f = new Function('guard', 'lookupGuard', 'lookupTurn', 'confirm', 'triageTurn', 'entry',
    `"use strict"; return (${expr});`);
  return f(guard, lookupGuard, lookupTurn, confirm, triageTurn, entry);
}

test('the checklist exists where the plan names it', () => {
  assert.ok(existsSync(CHECKLIST), 'docs/superpowers/mobile-tools/phone-journey.md is missing');
});

test('every quoted screen string equals the shipped constant that produces it', () => {
  const blocks = expectBlocks(readChecklist());
  assert.ok(blocks.length >= 25, `only ${blocks.length} pinned strings; the checklist quotes more than that`);
  for (const { expr, text } of blocks) {
    const value = evaluate(expr);
    assert.equal(typeof value, 'string', `${expr} is not a string`);
    assert.equal(text, value, `the checklist's text for ${expr} is not the shipped string`);
  }
});

test('every quoted literal occurs verbatim in the file it is quoted from', () => {
  const blocks = sourceBlocks(readChecklist());
  assert.ok(blocks.length >= 5, `only ${blocks.length} source-pinned strings`);
  for (const { path, text } of blocks) {
    const body = readFileSync(new URL(path, `file://${ROOT}`), 'utf8');
    assert.ok(body.includes(text), `${path} does not contain ${JSON.stringify(text)}`);
  }
});

test('every step the brief lists is present, in order, with a PASS box, a FAIL box and its evidence', () => {
  const md = readChecklist();
  const found = steps(md);
  assert.deepEqual(Object.keys(found), STEP_IDS);
  for (const id of STEP_IDS) {
    const { body } = found[id];
    assert.match(body, /- \[ \] PASS/, `${id} has no PASS box`);
    assert.match(body, /- \[ \] FAIL/, `${id} has no FAIL box`);
    assert.match(body, /\*\*Evidence:\*\*/, `${id} names no evidence`);
  }
});

test('every conversation and lookup step names its input, its export row and at least one pinned string', () => {
  const md = readChecklist();
  const found = steps(md);
  const inputs = inputBlocks(md);
  for (const id of EXPORT_STEPS) {
    const { body } = found[id];
    assert.ok(typeof inputs[id] === 'string' && inputs[id].length > 0, `${id} has no input block`);
    assert.ok(body.includes(`<!-- input: ${id} -->`), `${id}'s input block is outside its section`);
    const kind = id.startsWith('T') ? 'triage' : 'lookup';
    assert.match(body, new RegExp(`\\*\\*Evidence:\\*\\* export row \`kind: ${kind}\``),
      `${id} does not name its export row as kind ${kind}`);
    assert.ok(expectBlocks(body).length + sourceBlocks(body).length > 0, `${id} pins no screen text`);
  }
});

test('the parser refuses what it should: a wrong string is caught, not skipped', () => {
  const md = '<!-- expect: guard.CRISIS_LINE_REPLACE -->\n```text\nCall the wrong number.\n```\n';
  const [b] = expectBlocks(md);
  assert.equal(b.expr, 'guard.CRISIS_LINE_REPLACE');
  assert.notEqual(b.text, evaluate(b.expr));
});
