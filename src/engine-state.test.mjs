// Tests for the engine-state presentation model (task 2.2).
//
// These assert on `kind` and on properties of the copy rather than on exact
// wording, so the strings stay editable without the suite becoming a
// transcription of them — except where the wording IS the requirement (the
// "no model" case exists because a permanently-green "Local · offline" made
// the founder believe a download had happened).

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  describeEngineState,
  createReadableSequence,
  MIN_DWELL_MS,
  PREFILL_EXPLAIN_MS,
} from './engine-state.js';

/* ---------------- describeEngineState ---------------- */

test('an unsupported device says so and outranks every other state', () => {
  const s = describeEngineState({
    supported: false,
    engineStatus: 'Ready',
    installed: true,
    download: { phase: 'downloading', bytesDownloaded: 1, totalBytes: 2 },
  });
  assert.equal(s.kind, 'unsupported');
  assert.equal(s.prominent, true);
  // 2.3 produces `supported:false`; this is the screen it drives.
  assert.match(s.detail, /4 GB/);
});

test('no model installed never reads as ready — the copy defect, inverted', () => {
  const s = describeEngineState({ engineStatus: 'NoModel', installed: false });
  assert.equal(s.kind, 'no-model');
  assert.equal(s.tone, 'warn');
  // The old pill said "Local · offline" in permanent teal here, which is how
  // someone concludes a model is present and running.
  assert.doesNotMatch(s.label.toLowerCase(), /ready|local · offline/);
  assert.match(s.label.toLowerCase(), /no model/);
});

test('an engine reporting Ready but with nothing installed still says no model', () => {
  const s = describeEngineState({ engineStatus: 'Ready', installed: false });
  assert.equal(s.kind, 'no-model');
});

test('a live download outranks engine state', () => {
  const s = describeEngineState({
    engineStatus: 'NoModel',
    installed: false,
    download: { phase: 'downloading', bytesDownloaded: 2 ** 30, totalBytes: 2 ** 31 },
  });
  assert.equal(s.kind, 'downloading');
});

test('downloading reports a real percentage and real byte counts', () => {
  const s = describeEngineState({
    download: {
      phase: 'downloading',
      bytesDownloaded: 807694112 / 2,
      totalBytes: 807694112,
      bytesPerSec: 3 * 2 ** 20,
    },
  });
  assert.equal(s.kind, 'downloading');
  assert.match(s.label, /50%/);
  // The real low-tier model is 807,694,112 bytes. Both halves use one unit.
  assert.match(s.detail, /385 MB of 770 MB/);
  assert.match(s.detail, /3\.0 MB\/s/);
});

test('both halves of a progress figure use the unit of the total, not their own', () => {
  // Picking the unit per-value straddles the 1 GB boundary mid-download and
  // makes the number look like it shrank: "999 MB" then "1.00 GB".
  const s = describeEngineState({
    download: { phase: 'downloading', bytesDownloaded: 2 ** 30, totalBytes: 2497280384 },
  });
  assert.match(s.detail, /^1\.00 GB of 2\.33 GB/);
});

test('a download with no known total does not fabricate a percentage denominator', () => {
  const s = describeEngineState({ download: { phase: 'downloading', bytesDownloaded: 100, totalBytes: 0 } });
  assert.match(s.label, /0%/);
  assert.doesNotMatch(s.detail, /of 0/);
});

test('verification is named as an integrity check, not a generic spinner', () => {
  const s = describeEngineState({ download: { phase: 'verifying' } });
  assert.equal(s.kind, 'verifying');
  assert.match(s.detail.toLowerCase(), /hash|byte/);
});

test('a failed download is an error and points at resuming', () => {
  const s = describeEngineState({ download: { phase: 'failed', error: 'connection lost' } });
  assert.equal(s.kind, 'download-failed');
  assert.equal(s.tone, 'error');
  assert.equal(s.detail, 'connection lost');
});

test('a cancelled download reads as paused and says the bytes are kept', () => {
  const s = describeEngineState({ download: { phase: 'cancelled' } });
  assert.equal(s.kind, 'download-paused');
  assert.match(s.detail.toLowerCase(), /kept|resum/);
});

test('phase "done" falls through to engine state rather than latching', () => {
  const s = describeEngineState({ download: { phase: 'done' }, engineStatus: 'Starting', installed: true });
  assert.equal(s.kind, 'loading');
});

test('a download that stopped partway is visible and resumable, not "no model"', () => {
  // The bytes are on disk and `download_status` has always reported them, but
  // until now they only surfaced inside the model drawer — invisible to anyone
  // sitting in the chat view, who saw a screen identical to a fresh install.
  const s = describeEngineState({ installed: false, partBytes: 403847056, downloadActive: false });
  assert.equal(s.kind, 'download-interrupted');
  assert.equal(s.action, 'resume');
  assert.match(s.detail, /385 MB/);
  assert.match(s.detail.toLowerCase(), /resum/);
});

test('a partial download does not claim to be stopped while it is running', () => {
  const s = describeEngineState({ installed: false, partBytes: 403847056, downloadActive: true });
  assert.notEqual(s.kind, 'download-interrupted');
});

test('a live download outranks the stale partial-bytes state', () => {
  const s = describeEngineState({
    installed: false,
    partBytes: 403847056,
    downloadActive: true,
    download: { phase: 'downloading', bytesDownloaded: 5e8, totalBytes: 8e8 },
  });
  assert.equal(s.kind, 'downloading');
});

test('an installed model is never described as a stopped download', () => {
  const s = describeEngineState({ installed: true, partBytes: 999, engineStatus: 'Ready' });
  assert.equal(s.kind, 'ready');
});

test('only the states a user can act on carry an action', () => {
  const withAction = [
    describeEngineState({ installed: false }),                                     // no-model
    describeEngineState({ installed: false, partBytes: 1e8 }),                     // interrupted
    describeEngineState({ download: { phase: 'failed' } }),                        // failed
    describeEngineState({ download: { phase: 'cancelled' } }),                     // paused
  ];
  for (const s of withAction) {
    assert.ok(['download', 'resume'].includes(s.action), `${s.kind} should be actionable`);
  }
  const withoutAction = [
    describeEngineState({ installed: true, engineStatus: 'Ready' }),
    describeEngineState({ installed: true, engineStatus: 'Starting' }),
    describeEngineState({ download: { phase: 'verifying' } }),
    describeEngineState({ supported: false }),
  ];
  for (const s of withoutAction) {
    assert.equal(s.action, undefined, `${s.kind} offers an action the user cannot take`);
  }
});

test('a failed engine outranks "no model", because the user did download one', () => {
  // Failed + not-fully-installed is the missing-adapter case the milestone
  // describes. Saying "no model on this device" there would be wrong: they
  // downloaded one and a file went missing.
  const s = describeEngineState({ engineStatus: 'Failed', installed: false });
  assert.equal(s.kind, 'failed');
  assert.equal(s.tone, 'error');
});

test('a starting engine says it is loading the model into memory', () => {
  const s = describeEngineState({ engineStatus: 'Starting', installed: true });
  assert.equal(s.kind, 'loading');
  assert.equal(s.prominent, true);
});

test('an unknown engine status is treated as loading, never as ready', () => {
  const s = describeEngineState({ engineStatus: null, installed: true });
  assert.equal(s.kind, 'loading');
});

test('a restarting engine is distinguishable from a starting one', () => {
  const s = describeEngineState({ engineStatus: 'Restarting', installed: true });
  assert.equal(s.kind, 'restarting');
});

test('ready is quiet — it does not take the prominent row', () => {
  const s = describeEngineState({ engineStatus: 'Ready', installed: true });
  assert.equal(s.kind, 'ready');
  assert.equal(s.tone, 'ready');
  assert.equal(s.prominent, false);
});

test('a turn waiting briefly for its first token stays quiet', () => {
  const s = describeEngineState({
    engineStatus: 'Ready',
    installed: true,
    turn: { startedAt: 1000, firstDeltaAt: null, now: 1000 + PREFILL_EXPLAIN_MS - 1 },
  });
  assert.equal(s.kind, 'working');
  assert.equal(s.prominent, false);
});

test('a turn still waiting past the threshold explains the wait honestly', () => {
  // CP1: "first turn very slow, subsequent turns lightning fast". A user shown
  // nothing here concludes the app is broken.
  const s = describeEngineState({
    engineStatus: 'Ready',
    installed: true,
    turn: { startedAt: 1000, firstDeltaAt: null, now: 1000 + PREFILL_EXPLAIN_MS },
  });
  assert.equal(s.kind, 'preparing');
  assert.equal(s.prominent, true);
  assert.match(s.detail.toLowerCase(), /first reply/);
});

test('once the first token arrives the state stops claiming to be preparing', () => {
  const s = describeEngineState({
    engineStatus: 'Ready',
    installed: true,
    turn: { startedAt: 1000, firstDeltaAt: 9000, now: 20000 },
  });
  assert.equal(s.kind, 'generating');
});

test('every state carries a pill-sized short form as well as a row-sized label', () => {
  // Not cosmetic. Rendering `label` in the pill produced "Ready — r" at 360px,
  // and the bounds assertion still passed because the pill's RECT was inside
  // the viewport — the defect was in the pixels inside the box, which is the
  // failure mode this repo has now hit three times. The pill's budget is
  // ~170px beside a model name, so the cap is character count, not CSS.
  const cases = [
    { supported: false },
    { engineStatus: 'NoModel', installed: false },
    { installed: false, partBytes: 403847056 },
    { engineStatus: 'Failed', installed: true },
    { engineStatus: 'Starting', installed: true },
    { engineStatus: 'Restarting', installed: true },
    { engineStatus: 'Ready', installed: true },
    { download: { phase: 'requesting' } },
    { download: { phase: 'verifying' } },
    { download: { phase: 'failed' } },
    { download: { phase: 'cancelled' } },
    { download: { phase: 'downloading', bytesDownloaded: 1, totalBytes: 2 } },
  ];
  for (const c of cases) {
    const s = describeEngineState(c);
    assert.ok(s.label.length <= 42, `label too long (${s.label.length}): ${s.label}`);
    assert.ok(typeof s.short === 'string' && s.short.length > 0, `no short form for ${s.kind}`);
    assert.ok(s.short.length <= 16, `short too long (${s.short.length}): ${s.short}`);
  }
});

test('the turn states carry short forms too — they are the common case', () => {
  const busy = describeEngineState({
    engineStatus: 'Ready', installed: true,
    turn: { startedAt: 0, firstDeltaAt: null, now: 10 },
  });
  const writing = describeEngineState({
    engineStatus: 'Ready', installed: true,
    turn: { startedAt: 0, firstDeltaAt: 5, now: 10 },
  });
  const preparing = describeEngineState({
    engineStatus: 'Ready', installed: true,
    turn: { startedAt: 0, firstDeltaAt: null, now: PREFILL_EXPLAIN_MS },
  });
  for (const s of [busy, writing, preparing]) {
    assert.ok(s.short && s.short.length <= 16, `${s.kind}: ${s.short}`);
  }
});

/* ---------------- createReadableSequence ---------------- */

function fakeClock() {
  let t = 0;
  let nextId = 1;
  const timers = new Map();
  return {
    now: () => t,
    schedule: (fn, ms) => { const id = nextId++; timers.set(id, { fn, at: t + ms }); return id; },
    cancel: (id) => timers.delete(id),
    advance(ms) {
      const target = t + ms;
      for (;;) {
        let due = null;
        for (const [id, e] of timers) if (e.at <= target && (due == null || e.at < timers.get(due).at)) due = id;
        if (due == null) break;
        const e = timers.get(due);
        timers.delete(due);
        t = e.at;
        e.fn();
      }
      t = target;
    },
  };
}

function seq(overrides = {}) {
  const clock = fakeClock();
  const rendered = [];
  const s = createReadableSequence({
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    render: (p) => rendered.push(p.kind),
    ...overrides,
  });
  return { clock, rendered, s };
}

test('the first state is shown immediately — nothing to wait behind', () => {
  const { rendered, s } = seq();
  s.push({ kind: 'loading', tone: 'busy' });
  assert.deepEqual(rendered, ['loading']);
});

test('a following state waits for the dwell, so the transition can be read', () => {
  const { clock, rendered, s } = seq();
  s.push({ kind: 'loading', tone: 'busy' });
  s.push({ kind: 'ready', tone: 'ready' });
  assert.deepEqual(rendered, ['loading'], 'ready must not replace loading instantly');
  clock.advance(MIN_DWELL_MS - 1);
  assert.deepEqual(rendered, ['loading']);
  clock.advance(1);
  assert.deepEqual(rendered, ['loading', 'ready']);
});

test('same-kind updates bypass the dwell, so a progress bar is not frozen', () => {
  const { rendered, s } = seq();
  s.push({ kind: 'downloading', tone: 'busy', label: '1%' });
  s.push({ kind: 'downloading', tone: 'busy', label: '2%' });
  s.push({ kind: 'downloading', tone: 'busy', label: '3%' });
  assert.deepEqual(rendered, ['downloading', 'downloading', 'downloading']);
  assert.equal(s.pending(), 0);
  assert.equal(s.current().label, '3%');
});

test('an error preempts the queue instead of waiting behind it', () => {
  const { rendered, s } = seq();
  s.push({ kind: 'loading', tone: 'busy' });
  s.push({ kind: 'verifying', tone: 'busy' });
  s.push({ kind: 'failed', tone: 'error' });
  assert.deepEqual(rendered, ['loading', 'failed']);
  assert.equal(s.pending(), 0, 'the queued state is dropped, not shown after the error');
});

test('a backlog that would fall too far behind collapses to the newest', () => {
  const { clock, rendered, s } = seq({ minDwellMs: 700, maxLagMs: 1400 });
  s.push({ kind: 'a', tone: 'busy' });
  for (const k of ['b', 'c', 'd', 'e', 'f']) s.push({ kind: k, tone: 'busy' });
  clock.advance(700);
  // 5 queued * 700ms would put the display 3.5s behind a 1.4s bound.
  assert.deepEqual(rendered, ['a', 'f']);
  assert.equal(s.pending(), 0);
});

test('a queue within the lag bound is played in order, not collapsed', () => {
  const { clock, rendered, s } = seq({ minDwellMs: 700, maxLagMs: 2500 });
  s.push({ kind: 'verifying', tone: 'busy' });
  s.push({ kind: 'loading', tone: 'busy' });
  s.push({ kind: 'ready', tone: 'ready' });
  clock.advance(700);
  clock.advance(700);
  assert.deepEqual(rendered, ['verifying', 'loading', 'ready']);
});

test('re-offering the state already queued last does not queue it twice', () => {
  const { rendered, s } = seq();
  s.push({ kind: 'loading', tone: 'busy' });
  s.push({ kind: 'ready', tone: 'ready', label: 'first' });
  s.push({ kind: 'ready', tone: 'ready', label: 'second' });
  assert.equal(s.pending(), 1);
  assert.deepEqual(rendered, ['loading']);
});

test('reset drops the queue and cancels the pending timer', () => {
  const { clock, rendered, s } = seq();
  s.push({ kind: 'loading', tone: 'busy' });
  s.push({ kind: 'ready', tone: 'ready' });
  s.reset();
  clock.advance(10000);
  assert.deepEqual(rendered, ['loading']);
  assert.equal(s.current(), null);
});
