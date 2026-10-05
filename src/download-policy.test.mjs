import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  decideDownload,
  chargeAdvice,
  meteredPromptText,
  formatBytes,
  METERED_PROMPT_MIN_BYTES,
  CHARGE_NOTICE_MIN_BYTES,
  LOW_BATTERY_PERCENT,
} from './download-policy.js';

const GB = 1024 * 1024 * 1024;

// ---------------------------------------------------------------- the rule

test('unmetered lets a large download straight through', () => {
  const v = decideDownload({ metered: false }, { bytes: 5 * GB });
  assert.equal(v.allowed, true);
  assert.equal(v.reason, 'unmetered');
});

test('metered blocks a large download until the user overrides', () => {
  const v = decideDownload({ metered: true }, { bytes: 5 * GB });
  assert.equal(v.allowed, false);
  assert.equal(v.reason, 'metered');
});

test('the per-download override is what unblocks it', () => {
  const v = decideDownload({ metered: true }, { bytes: 5 * GB, allowMetered: true });
  assert.equal(v.allowed, true);
  assert.equal(v.reason, 'metered-allowed');
});

// THE asymmetry, mirroring net_state.rs's equivalent assertion. Every way the
// platform can fail to tell us must behave as metered — guessing unmetered
// spends money that cannot be refunded.
test('an unknown or missing metered flag is treated as metered', () => {
  for (const net of [{}, null, undefined, { metered: undefined }, { metered: null }]) {
    const v = decideDownload(net, { bytes: 5 * GB });
    assert.equal(v.allowed, false, `must not assume unmetered for ${JSON.stringify(net)}`);
  }
});

// Only an explicit `false` means unmetered — the same "explicit zero" contract
// net_state.rs enforces, asserted on this side too so the two cannot drift.
test('only an explicit false counts as unmetered', () => {
  assert.equal(decideDownload({ metered: false }, { bytes: 5 * GB }).allowed, true);
  assert.equal(decideDownload({ metered: 'false' }, { bytes: 5 * GB }).allowed, false);
  assert.equal(decideDownload({ metered: 0 }, { bytes: 5 * GB }).allowed, false);
});

// A prompt that fires for a 2 MB catalog trains people to dismiss prompts,
// which is how the one that matters gets dismissed.
test('small downloads do not prompt even on metered', () => {
  const v = decideDownload({ metered: true }, { bytes: 2 * 1024 * 1024 });
  assert.equal(v.allowed, true);
});

test('the prompt threshold is a boundary, not an approximation', () => {
  assert.equal(
    decideDownload({ metered: true }, { bytes: METERED_PROMPT_MIN_BYTES - 1 }).allowed,
    true,
  );
  assert.equal(
    decideDownload({ metered: true }, { bytes: METERED_PROMPT_MIN_BYTES }).allowed,
    false,
  );
});

test('a missing or malformed size does not block', () => {
  for (const bytes of [undefined, null, 0, 'x', NaN]) {
    assert.equal(decideDownload({ metered: true }, { bytes }).allowed, true);
  }
});

// ------------------------------------------------------- the charge notice

test('charge advice appears for a large download on low battery, not charging', () => {
  const msg = chargeAdvice({ metered: false, charging: false, batteryPercent: 20 }, 5 * GB);
  assert.ok(msg && msg.includes('20%'));
});

test('charge advice is silent while charging', () => {
  assert.equal(chargeAdvice({ charging: true, batteryPercent: 20 }, 5 * GB), null);
});

// The distinction net_state.rs keeps `charging` as Option<bool> to preserve:
// "we don't know" must not produce advice built on a guess.
test('charge advice is silent when charging state is unknown', () => {
  assert.equal(chargeAdvice({ batteryPercent: 20 }, 5 * GB), null);
  assert.equal(chargeAdvice({ charging: null, batteryPercent: 20 }, 5 * GB), null);
});

test('charge advice is silent on a healthy battery or a small download', () => {
  assert.equal(chargeAdvice({ charging: false, batteryPercent: 90 }, 5 * GB), null);
  assert.equal(chargeAdvice({ charging: false, batteryPercent: 10 }, 10 * 1024 * 1024), null);
});

test('charge advice thresholds are boundaries', () => {
  const low = { charging: false, batteryPercent: LOW_BATTERY_PERCENT - 1 };
  const at = { charging: false, batteryPercent: LOW_BATTERY_PERCENT };
  assert.ok(chargeAdvice(low, CHARGE_NOTICE_MIN_BYTES));
  assert.equal(chargeAdvice(at, CHARGE_NOTICE_MIN_BYTES), null);
  assert.equal(chargeAdvice(low, CHARGE_NOTICE_MIN_BYTES - 1), null);
});

test('charge advice is silent when the battery level is unknown', () => {
  assert.equal(chargeAdvice({ charging: false }, 5 * GB), null);
  assert.equal(chargeAdvice({ charging: false, batteryPercent: null }, 5 * GB), null);
});

// It is ADVICE, never a block: a refusal rides on the network, and the notice
// travels alongside it rather than adding a second way to be stopped.
test('charge advice never blocks a download', () => {
  const v = decideDownload(
    { metered: false, charging: false, batteryPercent: 5 },
    { bytes: 5 * GB },
  );
  assert.equal(v.allowed, true);
  assert.ok(v.chargeNotice, 'the notice should still be delivered');
});

test('the notice accompanies a metered refusal too', () => {
  const v = decideDownload(
    { metered: true, charging: false, batteryPercent: 15 },
    { bytes: 5 * GB },
  );
  assert.equal(v.allowed, false);
  assert.ok(v.chargeNotice);
});

// ---------------------------------------------------------------- the copy

test('the metered prompt names the size, because "are you sure?" is unanswerable without one', () => {
  const text = meteredPromptText(5 * GB);
  assert.ok(text.includes('5.0 GB'));
  assert.ok(text.includes('mobile data'));
});

test('sizes render in units a person reads', () => {
  assert.equal(formatBytes(0), '0 B');
  assert.equal(formatBytes(2048), '2 KB');
  assert.equal(formatBytes(50 * 1024 * 1024), '50 MB');
  assert.equal(formatBytes(807694112), '770 MB'); // the low tier's real model
  assert.equal(formatBytes(5027783616), '4.7 GB'); // the high tier's
});
