// probes/vision/manifest.test.mjs — node --test probes
//
// The licence-clean vision probe set (fixtures/v1): 16 typeset maths problems,
// one diagram, one page of prose. gen.py writes the PNGs, items.json and
// manifest.json; this test pins every file by sha256 and the item schema.
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
  assert.equal(manifest.count, 18);
  assert.equal(Object.keys(manifest.files).length, 18);
  for (const [name, want] of Object.entries(manifest.files)) {
    assert.equal(createHash('sha256').update(bytes(name)).digest('hex'), want, `${name} bytes changed: rerun gen.py and review`);
  }
});

test('items.json lists 18 well-formed items, each naming a pinned file', () => {
  const { version, items } = json('items.json');
  const manifest = json('manifest.json');
  assert.equal(version, 1);
  assert.equal(items.length, 18);
  const ids = new Set();
  for (const it of items) {
    for (const k of ['id', 'file', 'kind', 'latex', 'plain', 'verdict', 'notes']) assert.ok(k in it, `${it.id} lacks ${k}`);
    assert.ok(!ids.has(it.id), `duplicate id ${it.id}`);
    ids.add(it.id);
    assert.ok(['printed', 'diagram', 'prose'].includes(it.kind), `${it.id} kind`);
    assert.ok(['accept', 'refuse'].includes(it.verdict), `${it.id} verdict`);
    assert.ok(existsSync(new URL(it.file, DIR)), `${it.id} file missing`);
    assert.ok(it.file in manifest.files, `${it.id} not in manifest`);
    assert.equal(typeof it.plain, 'string');
    if (it.verdict === 'accept') assert.equal(typeof it.latex, 'string', `${it.id} latex`);
    else assert.equal(it.latex, null, `${it.id} refuse has null latex`);
  }
  assert.deepEqual(items.map((i) => i.verdict), [...Array(16).fill('accept'), 'refuse', 'refuse']);
  assert.deepEqual(items.map((i) => i.kind), [...Array(16).fill('printed'), 'diagram', 'prose']);
});
