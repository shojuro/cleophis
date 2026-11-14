// probes/vision/manifest.test.mjs — node --test probes
//
// The licence-clean vision probe set (fixtures/v1): 16 typeset maths problems,
// two photographs of the printed sheet (q19 upright, q20 EXIF-rotated), and four
// typeset refusal probes (q17/q21 diagrams, q18/q22 prose). gen.py writes the PNGs,
// items.json and manifest.json and hashes the photographs without rendering them;
// this test pins every file by sha256 and the item schema.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';

const DIR = new URL('./fixtures/v1/', import.meta.url);
const bytes = (name) => readFileSync(new URL(name, DIR));
const json = (name) => JSON.parse(bytes(name).toString('utf8'));

test('manifest.json pins every fixture file by sha256 and count', () => {
  const manifest = json('manifest.json');
  assert.equal(manifest.version, 1);
  assert.equal(manifest.count, 22);
  assert.equal(Object.keys(manifest.files).length, 22);
  for (const [name, want] of Object.entries(manifest.files)) {
    assert.equal(createHash('sha256').update(bytes(name)).digest('hex'), want, `${name} bytes changed: rerun gen.py and review`);
  }
});

test('items.json lists 22 well-formed items, each naming a pinned file', () => {
  const { version, items } = json('items.json');
  const manifest = json('manifest.json');
  assert.equal(version, 1);
  assert.equal(items.length, 22);
  const ids = new Set();
  for (const it of items) {
    for (const k of ['id', 'file', 'kind', 'latex', 'plain', 'verdict', 'notes']) assert.ok(k in it, `${it.id} lacks ${k}`);
    assert.ok(!ids.has(it.id), `duplicate id ${it.id}`);
    ids.add(it.id);
    assert.ok(['printed', 'photo', 'diagram', 'prose'].includes(it.kind), `${it.id} kind`);
    assert.ok(['accept', 'refuse'].includes(it.verdict), `${it.id} verdict`);
    assert.ok(existsSync(new URL(it.file, DIR)), `${it.id} file missing`);
    assert.ok(it.file in manifest.files, `${it.id} not in manifest`);
    assert.equal(typeof it.plain, 'string');
    if (it.verdict === 'accept') assert.equal(typeof it.latex, 'string', `${it.id} latex`);
    else assert.equal(it.latex, null, `${it.id} refuse has null latex`);
  }
  assert.deepEqual(items.map((i) => i.id), Array.from({ length: 22 }, (_, i) => `q${String(i + 1).padStart(2, '0')}`));
  assert.deepEqual(items.map((i) => i.verdict), [...Array(16).fill('accept'), 'refuse', 'refuse', 'accept', 'accept', 'refuse', 'refuse']);
  assert.deepEqual(items.map((i) => i.kind), [...Array(16).fill('printed'), 'diagram', 'prose', 'photo', 'photo', 'diagram', 'prose']);
  // The photographs: kept as the founder's camera JPEGs, orientation 1 and 6, and never rewritten.
  for (const [id, file] of [['q19', 'photos/q19-photo-upright.jpg'], ['q20', 'photos/q20-photo-rotated.jpg']]) {
    const it = items.find((i) => i.id === id);
    assert.equal(it.file, file);
    assert.equal(it.latex, 'x^2 - 5x + 6 = 0');
  }
});
