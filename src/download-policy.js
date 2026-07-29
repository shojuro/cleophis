// Should this download start right now? (spec §2.2)
//
// # Why the decision lives here rather than in Rust
//
// Same reasoning as `engine-state.js` (decision D-3): logic whose failure mode
// is silent goes where the tests run. A wrong verdict here throws nothing — it
// either spends someone's cellular data without asking, or nags for consent on
// a wifi connection until they stop reading the prompt. Both look exactly like
// working software.
//
// It belongs on this side of the seam specifically because it is a **user
// preference with an override**, not a security boundary. The user can always
// say "yes, on cellular"; nothing here protects the app from the user, it
// protects the user from a surprise on their bill. The facts it decides over
// come from Android (`network_state`, backed by ConnectivityManager) and the
// parse of those facts is Rust's `net_state.rs`, tested in the desktop suite —
// so both halves are tested, each where its language runs.
//
// # The rule
//
// Unmetered-only by default; a per-download override; and a charge
// recommendation that is advice, never a block.

/** Default cap above which a metered download is worth interrupting for. */
export const METERED_PROMPT_MIN_BYTES = 50 * 1024 * 1024; // 50 MB

/** Below this battery percentage, a large download suggests plugging in. */
export const LOW_BATTERY_PERCENT = 40;

/** Downloads at least this large are worth a charge suggestion at all. */
export const CHARGE_NOTICE_MIN_BYTES = 1024 * 1024 * 1024; // 1 GB

/**
 * Decide whether a download may start.
 *
 * @param {{metered?: boolean, charging?: boolean|null, batteryPercent?: number|null}} net
 *   as reported by the `network_state` command.
 * @param {{bytes?: number, allowMetered?: boolean}} req
 *   `bytes` is the expected size; `allowMetered` is the per-download override,
 *   set only by the user answering the prompt this function produces.
 * @returns {{allowed: boolean, reason: string, chargeNotice: string|null}}
 */
export function decideDownload(net, req) {
  const metered = net?.metered !== false; // unknown → metered; see net_state.rs
  const bytes = Number(req?.bytes) || 0;
  const override = req?.allowMetered === true;

  const chargeNotice = chargeAdvice(net, bytes);

  // A small file on a metered connection is not worth a modal. The policy
  // exists to stop a multi-gigabyte model landing on a cellular plan, and a
  // prompt that fires for a 2 MB catalog trains people to dismiss it — which
  // is how the prompt that matters gets dismissed too.
  if (metered && !override && bytes >= METERED_PROMPT_MIN_BYTES) {
    return { allowed: false, reason: 'metered', chargeNotice };
  }

  return { allowed: true, reason: metered ? 'metered-allowed' : 'unmetered', chargeNotice };
}

/**
 * Advice, never a block — which is the whole design of this one.
 *
 * A download refused for battery is a download the user cannot start at all
 * while their phone happens to be at 38 %, and they may have a charger in
 * their bag and a train to catch. Telling them is useful; deciding for them is
 * not. Returns `null` when there is nothing worth saying.
 *
 * `charging === null/undefined` means Android did not report it, and no
 * recommendation is issued on an unknown — `net_state.rs` keeps that distinct
 * from `false` precisely so this function can tell them apart.
 */
export function chargeAdvice(net, bytes) {
  if (!(Number(bytes) >= CHARGE_NOTICE_MIN_BYTES)) return null;
  if (net?.charging !== false) return null; // charging, or unknown → say nothing

  const pct = net?.batteryPercent;
  if (typeof pct !== 'number') return null;
  if (pct >= LOW_BATTERY_PERCENT) return null;

  return `This is a large download and your battery is at ${pct}%. Plugging in is recommended.`;
}

/**
 * The prompt copy for a `metered` refusal.
 *
 * Deliberately says "mobile data" rather than "metered network": the second is
 * accurate and the first is what a person recognises. It also names the size,
 * because "are you sure?" without a number is a question nobody can answer.
 */
export function meteredPromptText(bytes) {
  return `You're on mobile data. This download is ${formatBytes(bytes)} and may use your data allowance.`;
}

/** Human sizes, binary units, one decimal above MB. */
export function formatBytes(bytes) {
  const n = Number(bytes) || 0;
  if (n >= 1024 * 1024 * 1024) return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  if (n >= 1024 * 1024) return `${Math.round(n / (1024 * 1024))} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}
