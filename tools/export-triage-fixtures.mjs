// tools/export-triage-fixtures.mjs — copy saved probe transcripts out of the
// triage repo into src/triage/fixtures/ so the guard is proven against real
// replies, not authored ones. Run from the product repo root:
//
//   node tools/export-triage-fixtures.mjs [~/cleophas-triage]
//
// benign-arms.json   : every benign CONTROL arm from the four untrained floors
//                      and from the 1.7B v3 gate (user text + the model's
//                      reply) — the crisis block must appear on NONE of them.
// redflag-arms.json  : every red-flag TARGET arm from the same files —
//                      prohibited strings must reach the screen on NONE, and
//                      CLINICIAN-routed ones must lose their time frame.
// crisis-items.json  : every crisis item, social and embedded, from the frozen
//                      bank — the block must appear on every TARGET except the
//                      two registered blind spots (crisis-embedded-07, -12),
//                      and on no CONTROL.
//
// THE TRIAGE REPO IS READ-ONLY HERE. Nothing in this file writes outside
// src/triage/fixtures/, and the fixtures are committed so the proofs
// (src/triage/guard.transcripts.test.mjs) run with the triage repo absent.
//
// PROVENANCE IS PART OF THE FIXTURE, not a comment about it. Each file carries
// the source paths it was built from and the sha256 of each source's bytes, so
// a proof that says "0 of 400 benign arms" can be traced to the exact
// transcripts it counted, and re-running the exporter against a changed
// transcript shows up as a changed sha in the diff rather than as silently
// different numbers.
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { homedir } from 'node:os';

const SRC = process.argv[2] ?? join(homedir(), 'cleophas-triage');
const OUT = new URL('../src/triage/fixtures/', import.meta.url);
mkdirSync(OUT, { recursive: true });

const STACKS = ['Qwen3-0.6B', 'Qwen3-1.7B', 'Qwen3-4B', 'Qwen3-8B'];
const VIEWS = ['locked-heldout', 'inverted-view'];

const benign = [];
const redflag = [];
const armSources = [];
const crisisSources = [];

/** Read a file, returning its parsed JSON and a provenance record for it. */
function readSource(path, into) {
  const bytes = readFileSync(path);
  into.push({
    path: relative(SRC, path).split('\\').join('/'),
    sha256: createHash('sha256').update(bytes).digest('hex'),
    bytes: bytes.length,
  });
  return JSON.parse(bytes.toString('utf8'));
}

/**
 * A paired probe writes its exchanges in pair order, two per pair: the TARGET
 * arm (the red flag) then the CONTROL arm (the benign look-alike). `ids` is one
 * id per pair, in the same order, so exchange i belongs to pair floor(i / 2).
 */
function harvest(path, stackLabel) {
  const t = readSource(path, armSources);
  const ids = t.report.selection.ids;
  // THE PARITY SPLIT BELOW IS AN ASSUMPTION ABOUT THE FILE, so it is checked
  // rather than trusted. If a transcript ever holds an odd number of exchanges,
  // or one arm of a pair failed to record, `i % 2` silently relabels every arm
  // after that point — red-flag replies land in the benign fixture and the
  // "0 of 400 benign arms" proof starts counting the wrong thing while still
  // passing. An exporter that cannot tell the arms apart must stop, not guess.
  if (t.exchanges.length !== ids.length * 2) {
    throw new Error(
      `${path}: ${t.exchanges.length} exchanges for ${ids.length} pair ids — `
      + 'the target/control parity split does not hold, so the arms cannot be labelled',
    );
  }
  const before = { redflag: redflag.length, benign: benign.length };
  t.exchanges.forEach((ex, i) => {
    const arm = i % 2 === 0 ? 'target' : 'control';
    const row = {
      stack: stackLabel,
      view: t.view,
      id: ids[Math.floor(i / 2)],
      arm,
      user: ex.user,
      reply: ex.text ?? '',
    };
    (arm === 'target' ? redflag : benign).push(row);
  });
  const rec = armSources[armSources.length - 1];
  rec.targetArms = redflag.length - before.redflag;
  rec.controlArms = benign.length - before.benign;
}

for (const s of STACKS) {
  for (const v of VIEWS) {
    harvest(join(SRC, 'work', 'floors', `${s}.probe-triage-heldout.${v}.json`), `floors/${s}`);
  }
}
for (const v of VIEWS) {
  harvest(
    join(SRC, 'work', 'gate-17b', `Qwen3-1.7B-armb-v3.probe-triage-heldout.${v}.json`),
    'gate-17b/Qwen3-1.7B-armb-v3',
  );
}

const bank = readSource(join(SRC, 'probes', 'items', 'crisis.json'), crisisSources);
const crisis = [
  ...bank.pairs.map((p) => ({
    id: p.id, kind: 'social', context: p.context, target: p.target, control: p.control,
  })),
  ...bank.embeddedPairs.map((p) => ({
    id: p.id, kind: 'embedded', acuity: p.acuity, context: p.context, target: p.target, control: p.control,
  })),
];
crisisSources[0].items = crisis.length;

function write(name, note, sources, rows) {
  const body = {
    fixture: name,
    note,
    generatedBy: 'tools/export-triage-fixtures.mjs',
    sourceRepo: 'cleophas-triage',
    count: rows.length,
    sources,
    rows,
  };
  writeFileSync(new URL(`${name}.json`, OUT), `${JSON.stringify(body, null, 1)}\n`);
}

write(
  'benign-arms',
  'Every CONTROL arm of the paired triage-heldout probes: the benign look-alike of a '
  + 'red flag. No self-harm disclosure is authored into any of them, so the crisis block '
  + 'must fire on none.',
  armSources,
  benign,
);
write(
  'redflag-arms',
  'Every TARGET arm of the same probes: the time-critical presentation. After the guard, '
  + 'none may carry a medication, a dose or a named diagnosis, and none routed CLINICIAN '
  + 'may still state a time frame.',
  armSources,
  redflag,
);
write(
  'crisis-items',
  'The frozen crisis bank: 27 social pairs plus 14 embedded pairs. Item text only — these '
  + 'are USER turns, scored here against a fixed reply, because what is proven is the '
  + "product's crisis-on-input rule and not any model's answer.",
  crisisSources,
  crisis,
);

console.log(
  `benign ${benign.length}, red-flag ${redflag.length}, crisis items ${crisis.length}`
  + ` (from ${armSources.length + crisisSources.length} sources under ${SRC})`,
);
