//! Thermal-throttle detection: the decision half (spec §8, hazard **H6**,
//! acceptance **A7**).
//!
//! H6: *"sustained inference halves speed on hot phones"*. §8 asks for
//! "detection + honest UI notice" so the user meets an explanation rather than
//! mysterious degradation. This module is the detection; the notice's wording
//! is `engine-state.js`, and the plumbing that feeds this is
//! `engine_inproc`'s per-token sink.
//!
//! # Why it is here and not behind the `cfg` (decision D-3)
//!
//! Every failure mode of this code is silent. A detector that never fires
//! compiles, runs, and looks exactly like a phone that never got hot. A
//! detector that fires on the wrong thing produces a *reassuring* sentence at
//! the wrong moment, which is worse than saying nothing. Neither raises an
//! error, and nothing on Android runs until a founder device checkpoint — so
//! behind the cfg is the same as untested for as long as it takes to get a
//! device. The arithmetic is platform-neutral, so it goes where the tests run
//! and the cfg keeps only the clock and the `emit`.
//!
//! (The dead-code allow lives on this module's `mod` declaration in `lib.rs`,
//! canonical placement per D-3's amendment, cfg'd to mirror the production
//! caller — `engine_inproc` is `cfg(mobile)`.)
//!
//! # What is measured, and why it is measured THERE
//!
//! **Inter-token gaps from the raw sampler sink, not from `on_delta`.** These
//! look interchangeable and are not. `on_delta` is downstream of
//! [`crate::engine_tools::ToolStream`], which *withholds* text that might turn
//! out to be tool syntax and releases it in a burst once it decides. Timing
//! that measures the suppressor's release schedule, and would read a held-back
//! stretch as a stall and its flush as a speed-up — a throughput signal
//! reconstructed from the wrong side of a buffer. The sink in
//! `SessionTurns::turn` is upstream of all of that, and it is the closest to
//! the real cadence anything in this process can see.
//!
//! ## ⚠ CORRECTION (gen-9): it is once per VISIBLE token, not once per token
//!
//! The paragraph above originally ended "is called once per token by
//! `EngineSession::stream`". **That is false**, and the way it is false is the
//! same mechanism the paragraph rejects `on_delta` for. `llama.rs`'s generation
//! loop calls the sink only when the piece survives two filters:
//!
//! ```text
//! let visible = stripper.push(&piece);
//! if !visible.is_empty() { sink.on_token(&visible) }   // llama.rs
//! ```
//!
//! 1. **Empty pieces, on every model.** `token_to_piece` shares one
//!    `encoding_rs` decoder across the turn so a multi-byte character may
//!    straddle two tokens; the first of the pair decodes to `""` and is never
//!    sunk. So `sink_calls <= generated_tokens` *always*, and the two tokens'
//!    time arrives as one gap of roughly double the length.
//! 2. **`ThinkStripper`, on ChatML/Qwen only** (`strips_think()` is
//!    `ChatMl`-only). A `<think>` block produces **no** sink calls at all, then
//!    the first visible token after `</think>` carries the whole block as one
//!    gap. This is a withhold-and-burst suppressor — categorically the same
//!    thing as `ToolStream`, one layer further up than gen-7 looked.
//!
//! **Why the detector survives this, and where it does not.** Both filters
//! inflate gaps, but the verdict is a *ratio* of a recent rate to a baseline
//! measured through the identical filters, so a **constant** discrepancy
//! cancels and the detector is unaffected. What does not cancel is a
//! discrepancy that *changes* between baseline and window: a reply that turns
//! from ASCII to CJK/emoji, or a model that starts emitting `<think>` blocks
//! mid-session, looks like a slowdown with no thermal cause — a **false
//! positive**, the outcome §8 most wants to avoid. `ThinkStripper`'s case is
//! partly self-limiting: a block long enough to matter usually exceeds
//! [`MAX_GAP_MS`] and is dropped as a stall.
//!
//! This is **measured rather than modelled** by the A7 soak: `ThermalProbe`
//! logs `sink_calls` beside `GenStats::generated_tokens`, which the engine
//! counts independently, so the transcript states the real divergence on real
//! hardware instead of leaving it to this comment. See [`ThermalTrace`].
//!
//! # Three things that are NOT throttling, each excluded deliberately
//!
//! 1. **Prefill.** The wait before a turn's first token is the model reading
//!    the conversation, and on the floor device it dominates everything (CP1:
//!    "first turn very slow, subsequent turns lightning fast"). It is excluded
//!    structurally rather than by a threshold: a gap needs two tokens, and
//!    [`ThermalWatch::turn_ended`] drops the previous one, so the prefill wait
//!    is never a gap in the first place.
//! 2. **Idle between turns.** Same mechanism. The minutes a user spends
//!    reading and typing would otherwise be the slowest "token" ever recorded.
//! 3. **A stall.** A gap over [`MAX_GAP_MS`] is not a slow token — it is the
//!    app backgrounded, the thread descheduled, or a tool round-trip. It is
//!    dropped rather than counted, because averaging one four-second gap into
//!    a sixteen-sample window drags the mean under any threshold on its own.
//!
//! # The baseline outlives the turn, and that is the whole design
//!
//! Throttling is *persistent*: the SoC stays hot across several turns, which
//! is exactly the A7 soak scenario. So the baseline is established once per
//! watch — per inference thread — and deliberately survives
//! [`ThermalWatch::turn_ended`].
//!
//! A per-turn baseline is the obvious implementation and it **can never
//! fire**: each turn would re-baseline against the already-throttled rate,
//! measure it as normal, and report a healthy device forever. It compiles, it
//! passes any single-turn test, and it is wrong in the only scenario the
//! feature exists for. `the_baseline_survives_a_turn_boundary` is the test
//! that pins it.
//!
//! # Biased toward false negatives, on purpose
//!
//! Every constant below errs toward staying quiet. A notice that fires wrongly
//! teaches the user to ignore the one that fires rightly; a notice that fires
//! late costs only the mystery it was meant to remove, and the degradation is
//! self-evident by then anyway.
//!
//! # ⚠ What this cannot know
//!
//! Tokens/sec collapsing says throughput fell. It does **not** say the cause
//! was heat — another app taking the CPU looks identical from here. The copy
//! §8 specifies ("your phone is warming up") therefore asserts slightly more
//! than the signal supports. It is kept because sustained inference on a
//! passively-cooled phone is overwhelmingly the cause in practice, and because
//! the alternative reading ("something is slowing your phone down") is less
//! actionable. `PowerManager.getCurrentThermalStatus()` would corroborate it,
//! and is **surfaced, not attempted**: it needs API 29+, a new Kotlin class
//! over the JNI bridge and a device checkpoint to verify, and on mid-range
//! hardware — precisely the floor devices this matters on — OEMs frequently
//! leave the thermal HAL unwired to that API, so it returns `NONE` on a phone
//! that is visibly throttling. It could confirm this signal; it could not
//! replace it.

use std::collections::VecDeque;

use serde::Serialize;

/// Gaps discarded before the baseline sample starts.
///
/// The first token pair after a load carries one-time cost — the sampler's
/// first call, page faults on a freshly mapped model. Two is enough to be past
/// it and small enough not to delay the baseline noticeably.
pub(crate) const WARMUP_SKIP_GAPS: usize = 2;

/// Gaps averaged into the baseline rate.
///
/// At the floor tier's ~5 tok/s this is roughly five seconds of generation —
/// comfortably inside one ordinary reply, so a baseline exists before the
/// first turn is over, and long enough that per-token jitter averages out.
pub(crate) const BASELINE_GAPS: usize = 24;

/// The rolling comparison window.
///
/// Shorter than the baseline on purpose: the baseline should be stable and the
/// comparison should be responsive. Sixteen gaps is ~3 s at the floor rate.
pub(crate) const WINDOW_GAPS: usize = 16;

/// How far the rate must fall to count as a collapse.
///
/// H6's own number is a halving. The threshold sits *above* it (0.6 rather
/// than 0.5) so a real halving clears it with margin, and far enough below 1.0
/// that ordinary jitter, a longer sampled token, or a background app's brief
/// interference cannot reach it.
pub(crate) const COLLAPSE_RATIO: f64 = 0.6;

/// How far it must come back before the notice is withdrawn.
///
/// The gap between this and [`COLLAPSE_RATIO`] is hysteresis, and it is not
/// decoration: a device sitting exactly at the threshold would otherwise flap
/// the banner on and off every few tokens, which is a worse experience than
/// either state.
pub(crate) const RECOVERY_RATIO: f64 = 0.85;

/// How many consecutive collapsed observations make it "sustained" (§8's word).
///
/// Two full windows. One window below the line is a dip — a garbage collection,
/// another app waking up — and the point of "sustained" is that we do not
/// report those.
pub(crate) const SUSTAINED_GAPS: usize = 32;

/// Above this, a gap is a stall and not a measurement.
///
/// Four seconds between tokens is not a slow phone; it is a descheduled
/// thread, a backgrounded app, or a tool round-trip. Recording it would put a
/// single outlier into the window with enough weight to trip the threshold by
/// itself.
pub(crate) const MAX_GAP_MS: u64 = 4_000;

/// The `"thermal-notice"` payload.
///
/// One event carries both directions — `throttled` distinguishes onset from
/// recovery — so the frontend keeps one listener and one piece of state, and
/// cannot end up showing a warning nothing will ever clear.
///
/// The rates ride along because this is the number a bug report needs and the
/// number the A7 soak run has to write down. "The notice fired" is not a
/// measurement; "it fired at 2.1 tok/s against a 6.4 tok/s baseline" is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalNotice {
    pub throttled: bool,
    pub baseline_tps: f64,
    pub recent_tps: f64,
}

/// Has the rate collapsed relative to the baseline?
///
/// A zero or absent baseline is never a collapse: with nothing to compare
/// against, the honest answer is "no finding", not "yes".
pub(crate) fn detect_thermal_collapse(baseline_tps: f64, recent_tps: f64) -> bool {
    baseline_tps > 0.0 && recent_tps <= baseline_tps * COLLAPSE_RATIO
}

/// Has it come back far enough to withdraw the notice?
pub(crate) fn thermal_recovered(baseline_tps: f64, recent_tps: f64) -> bool {
    baseline_tps > 0.0 && recent_tps >= baseline_tps * RECOVERY_RATIO
}

/// Tokens per second implied by a run of inter-token gaps.
///
/// The `max(1)` floor is not defensive noise: a sum of zero would otherwise
/// divide by zero and, worse, a naive `0.0` fallback would report *infinitely
/// fast generation as a total collapse*. Sub-millisecond gaps are impossible
/// on the hardware this ships to, so clamping costs nothing real and removes
/// the one input that could invert the verdict.
fn rate(gaps: impl Iterator<Item = u64>) -> f64 {
    let mut n = 0usize;
    let mut total = 0u64;
    for g in gaps {
        n += 1;
        total += g;
    }
    if n == 0 {
        return 0.0;
    }
    1000.0 * n as f64 / total.max(1) as f64
}

/// Watches one inference thread's token cadence for a sustained collapse.
///
/// Fed one call per generated token; returns `Some` only at a *transition*, so
/// a caller can wire it straight to an `emit` without tracking whether it has
/// already said this.
#[derive(Debug, Default)]
pub(crate) struct ThermalWatch {
    /// The healthy rate, once known. Established once per watch, never per
    /// turn — see the module doc.
    baseline_tps: Option<f64>,
    baseline_gaps: Vec<u64>,
    skipped: usize,
    recent: VecDeque<u64>,
    /// Previous token's timestamp *within the current turn*. `None` between
    /// turns, which is what excludes prefill and idle time.
    last_at: Option<u64>,
    slow_run: usize,
    notified: bool,
    /// Gaps that reached the measurement path — i.e. that survived the
    /// [`MAX_GAP_MS`] stall filter. Counted for [`ThermalTrace`] only; nothing
    /// in the verdict reads it.
    gaps_seen: usize,
}

/// A read-only snapshot of what the watch currently believes, for the A7 soak
/// transcript.
///
/// **This exists because of the pre-registered soak's own reading table.** Four
/// of its five rows are verdicts about the *input* — "tok/s visibly collapses
/// and no notice" is BROKEN, "tok/s stays flat and no notice" is correctly
/// quiet — and the fifth, "silence with no trace", is the uninterpretable
/// outcome the trace exists to eliminate. Without a logged rate the soak cannot
/// distinguish any of them, so the notice alone is not a result.
///
/// The numbers are the ones the detector *actually decided on*, not a
/// re-derivation: `recent_tps` is `None` until the window is full, which is
/// exactly when [`ThermalWatch::on_token`] starts consulting it. A trace that
/// computed a rate the detector was not yet using would show a collapse the
/// verdict is not entitled to see, and the reader would call it a bug.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ThermalTrace {
    pub gaps: usize,
    pub baseline_tps: Option<f64>,
    /// `None` until [`WINDOW_GAPS`] gaps are held — the detector's own gate.
    pub recent_tps: Option<f64>,
    pub slow_run: usize,
    pub notified: bool,
}

impl ThermalWatch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether the notice is currently up.
    ///
    /// **Test-only, and deliberately marked so rather than left ambiguous.**
    /// This accessor was written for the webview re-assert, but that path
    /// publishes the whole [`ThermalNotice`] to `chat_cmds::ThermalState`
    /// instead: the rates are what a bug report and the A7 soak run need, and
    /// a bare bit would throw them away. Production therefore has no caller
    /// for this, and saying that with a `cfg` is better than leaving a method
    /// whose doc comment promises a caller nobody built — the small form of
    /// the asserted-but-unbuilt disease this phase exists to catch.
    ///
    /// The tests keep it because they assert internal state that no notice is
    /// emitted for — the hysteresis band in particular, where the whole point
    /// is that nothing is emitted and the flag must still be right.
    #[cfg(test)]
    pub(crate) fn throttled(&self) -> bool {
        self.notified
    }

    /// Record one generated token, observed `at_ms` milliseconds after this
    /// watch was created.
    ///
    /// Returns `Some` exactly when the verdict *changes*: once when a collapse
    /// becomes sustained, and once again if the rate recovers.
    pub(crate) fn on_token(&mut self, at_ms: u64) -> Option<ThermalNotice> {
        let Some(prev) = self.last_at.replace(at_ms) else {
            // First token of a turn. The wait before it is prefill, and prefill
            // is not slow generation — it is the model reading the
            // conversation. Excluded by construction rather than by threshold.
            return None;
        };
        let gap = at_ms.saturating_sub(prev);

        // A stall, not a measurement.
        if gap > MAX_GAP_MS {
            return None;
        }
        self.gaps_seen += 1;

        if self.skipped < WARMUP_SKIP_GAPS {
            self.skipped += 1;
            return None;
        }

        let Some(baseline) = self.baseline_tps else {
            self.baseline_gaps.push(gap);
            if self.baseline_gaps.len() >= BASELINE_GAPS {
                self.baseline_tps = Some(rate(self.baseline_gaps.iter().copied()));
            }
            // No baseline means no comparison. A device that is slow from the
            // very first token is a slow device, not a throttling one, and this
            // must not report it as one.
            return None;
        };

        self.recent.push_back(gap);
        if self.recent.len() > WINDOW_GAPS {
            self.recent.pop_front();
        }
        if self.recent.len() < WINDOW_GAPS {
            return None;
        }
        let recent = rate(self.recent.iter().copied());

        if detect_thermal_collapse(baseline, recent) {
            self.slow_run += 1;
            if self.slow_run >= SUSTAINED_GAPS && !self.notified {
                self.notified = true;
                return Some(ThermalNotice {
                    throttled: true,
                    baseline_tps: baseline,
                    recent_tps: recent,
                });
            }
            return None;
        }

        self.slow_run = 0;
        if self.notified && thermal_recovered(baseline, recent) {
            self.notified = false;
            return Some(ThermalNotice {
                throttled: false,
                baseline_tps: baseline,
                recent_tps: recent,
            });
        }
        None
    }

    /// A turn finished.
    ///
    /// Drops only the previous token's timestamp, so the next turn's prefill
    /// wait and the user's thinking time cannot become a gap. Everything
    /// else — baseline, window, run length, whether the notice is up —
    /// **survives on purpose**: the phone does not cool down because the reply
    /// ended, and a watch that reset here could never detect the sustained
    /// case it exists for.
    pub(crate) fn turn_ended(&mut self) {
        self.last_at = None;
    }

    /// What this watch currently believes — for the soak transcript, never for
    /// a decision. See [`ThermalTrace`].
    pub(crate) fn trace(&self) -> ThermalTrace {
        ThermalTrace {
            gaps: self.gaps_seen,
            baseline_tps: self.baseline_tps,
            recent_tps: (self.recent.len() >= WINDOW_GAPS)
                .then(|| rate(self.recent.iter().copied())),
            slow_run: self.slow_run,
            notified: self.notified,
        }
    }
}

/// The Q1 debug affordance's decision half.
///
/// # GATED, NOT SUPPRESSED — and the gate had to be found by building release
///
/// Everything in here exists only where its caller does:
/// `chat_cmds::chat_thermal_selftest`'s body is `cfg(all(mobile,
/// debug_assertions))`, so in a **release** build these six items have no
/// caller at all. `cargo ndk check` (dev profile) is blind to that and reported
/// exit 0 with zero warnings; the same check with `--release` produced six
/// dead-code warnings — **in the configuration that actually ships.**
///
/// The tempting fix is to widen this module's `cfg_attr(desktop,
/// allow(dead_code))` to cover mobile-release. That is precisely the bare-allow
/// disease D-3's amendment documents: it would switch dead-code checking off
/// for the *whole* thermal module on the shipping platform, which is what hid
/// two genuinely dead items in `tools.rs` and `tool_loop.rs` for an entire
/// phase. An allow scoped to these six would be sound but is six copies of a
/// subtle cfg, and a rule with several encodings cannot be checked by looking.
///
/// So the affordance is **compiled out** instead, one gate in canonical
/// placement on the `mod` declaration — the same call D-3's table already made
/// for `ToolOutcome::is_err` and `LoopMessage::user`. A debug-only affordance
/// that is absent from release is also the stronger property: the shipped
/// binary cannot contain a path that fakes a thermal notice.
///
/// `test` is in the gate beside `debug_assertions` so the suite still runs
/// these under `cargo test --release`, where `debug_assertions` is off.
#[cfg(any(debug_assertions, test))]
pub(crate) mod selftest {
        use super::*;

    /// The healthy cadence the Q1 selftest replays. 200 ms/token = 5 tok/s, which
    /// is the floor device's measured rate (CP1), so the baseline in the emitted
    /// payload is a number the founder recognises rather than an invented one.
    pub(super) const SELFTEST_HEALTHY_GAP_MS: u64 = 200;

    /// The throttled cadence: a third of healthy. H6's own number is a halving, so
    /// this clears [`COLLAPSE_RATIO`] with margin and the script does not sit on
    /// the threshold it is trying to demonstrate.
    pub(super) const SELFTEST_SLOW_GAP_MS: u64 = 600;

    /// Cap on the slow phase, so a script that can no longer fire terminates and
    /// says so instead of looping.
    pub(super) const SELFTEST_MAX_SLOW_GAPS: usize = 400;

    /// Replay a synthetic healthy→throttled cadence and return the onset notice.
    ///
    /// **This is the Q1 affordance's decision half, and it is here rather than in
    /// the command for the same reason the detector is (D-3): its failure mode is
    /// silent.** A script that quietly stops firing — because a constant moved, or
    /// because someone retuned [`COLLAPSE_RATIO`] — would turn the founder's
    /// five-second pipeline check into a five-second nothing, and "no notice
    /// appeared" is precisely the reading Q1 exists to make unambiguous. So it runs
    /// in the desktop suite on every gate, and it returns `Err` rather than `None`
    /// so the two ways of producing no notice cannot be confused.
    ///
    /// The clock is virtual: `at_ms` values are handed in, nothing sleeps, and the
    /// whole replay costs microseconds. The founder's phone does not have to be hot,
    /// or even warm.
    ///
    /// ⚠ **Evidence scope — what a green from this does NOT cover.** It exercises
    /// [`ThermalWatch`] and everything downstream of it (payload → managed state →
    /// `"thermal-notice"` → the frontend's §8 copy). It says **nothing** about
    /// whether `ThermalProbe::on_token` is wired to the generation loop at all,
    /// nor about the `Instant` clock that feeds it in production — this function
    /// supplies its own timestamps precisely so it need not run one. That wiring is
    /// only exercised by real generation, and it is what the `[thermal]` trace and
    /// Q2's soak cover. **A green Q1 must never be read as covering Q2.**
    pub(crate) fn synthetic_collapse() -> Result<ThermalNotice, String> {
        replay_to_onset(
            SELFTEST_HEALTHY_GAP_MS,
            SELFTEST_SLOW_GAP_MS,
            SELFTEST_MAX_SLOW_GAPS,
        )
        .map(|(_, _, notice)| notice)
    }

    /// The matching *recovery*: collapse first, then speed back up until the notice
    /// is withdrawn.
    ///
    /// **This is not a nice-to-have, it is what stops the affordance from lying.**
    /// [`ThermalWatch`] emits only on transitions, so an onset with no way back
    /// leaves `ThermalState` holding `throttled: true` — and `chat_thermal_state`
    /// re-asserts that on every webview reload, so a founder who tapped the button
    /// once would see "Your phone is warming up" for the rest of the install, on a
    /// cold phone, with no way to clear it short of reinstalling. A debug
    /// affordance that permanently falsifies the UI it was built to verify is
    /// worse than no affordance.
    ///
    /// It also earns its keep as evidence: the withdrawal edge is the half of §8
    /// that has never been seen on a device, and a soak cannot demonstrate it on
    /// demand — the phone has to actually cool down.
    pub(crate) fn synthetic_recovery() -> Result<ThermalNotice, String> {
        let (mut watch, mut clock, onset) = replay_to_onset(
            SELFTEST_HEALTHY_GAP_MS,
            SELFTEST_SLOW_GAP_MS,
            SELFTEST_MAX_SLOW_GAPS,
        )?;
        debug_assert!(onset.throttled);

        for _ in 0..SELFTEST_MAX_SLOW_GAPS {
            clock += SELFTEST_HEALTHY_GAP_MS;
            if let Some(n) = watch.on_token(clock) {
                return Ok(n);
            }
        }

        Err(format!(
            "selftest: the notice never withdrew after {SELFTEST_MAX_SLOW_GAPS} \
             gaps back at {SELFTEST_HEALTHY_GAP_MS} ms/token — a notice that cannot \
             be withdrawn stays up for the rest of the session"
        ))
    }

    /// Drive a fresh watch from healthy to a sustained collapse, handing back the
    /// watch and clock so a caller can keep going.
    ///
    /// The cadences are parameters **only** so the suite can run it with
    /// `slow == healthy` and prove the no-notice branch is reachable. Without that,
    /// every assertion about this script would be an assertion that it fires, and a
    /// script that fired unconditionally — the one failure that would make Q1 a
    /// check that cannot fail — would pass all of them.
    pub(super) fn replay_to_onset(
        healthy_gap_ms: u64,
        slow_gap_ms: u64,
        max_slow: usize,
    ) -> Result<(ThermalWatch, u64, ThermalNotice), String> {
        let mut watch = ThermalWatch::new();
        let mut clock = 0u64;

        // Past the skipped gaps, a full baseline, and a full window — all healthy,
        // so the baseline the collapse is judged against is the healthy rate. This
        // mirrors the tests' `warmed()` helper deliberately: the script and the
        // suite exercise the same entry state.
        for _ in 0..(WARMUP_SKIP_GAPS + BASELINE_GAPS + WINDOW_GAPS + 1) {
            clock += healthy_gap_ms;
            if let Some(n) = watch.on_token(clock) {
                return Err(format!(
                    "selftest: the healthy phase fired a notice ({n:?}) — a steady \
                     {healthy_gap_ms} ms/token cadence is being read as a collapse, \
                     so the constants no longer describe a healthy device"
                ));
            }
        }

        for _ in 0..max_slow {
            clock += slow_gap_ms;
            if let Some(n) = watch.on_token(clock) {
                return Ok((watch, clock, n));
            }
        }

        Err(format!(
            "selftest: no notice after {max_slow} gaps at {slow_gap_ms} ms/token \
             against a {healthy_gap_ms} ms/token baseline — the detector did not \
             fire on a collapse it should have caught"
        ))
    }

} // mod selftest

#[cfg(test)]
mod tests {
    use super::selftest::*;
    use super::*;

    /// Feed `n` tokens spaced `gap_ms` apart, collecting every notice.
    fn feed(
        w: &mut ThermalWatch,
        clock: &mut u64,
        n: usize,
        gap_ms: u64,
    ) -> Vec<ThermalNotice> {
        let mut out = Vec::new();
        for _ in 0..n {
            *clock += gap_ms;
            if let Some(notice) = w.on_token(*clock) {
                out.push(notice);
            }
        }
        out
    }

    /// Enough tokens at a healthy rate to establish the baseline and fill the
    /// window, so a test can get to the interesting part.
    fn warmed(gap_ms: u64) -> (ThermalWatch, u64) {
        let mut w = ThermalWatch::new();
        let mut clock = 0u64;
        let notices = feed(&mut w, &mut clock, WARMUP_SKIP_GAPS + BASELINE_GAPS + WINDOW_GAPS + 1, gap_ms);
        assert!(notices.is_empty(), "warm-up must be silent, got {notices:?}");
        (w, clock)
    }

    #[test]
    fn a_steady_rate_never_fires() {
        let (mut w, mut clock) = warmed(200);
        // Ten minutes of perfectly healthy generation.
        let notices = feed(&mut w, &mut clock, 3000, 200);
        assert!(notices.is_empty(), "steady generation must stay quiet, got {notices:?}");
    }

    /// Jitter is not a collapse. ±40% per-token variation around the same mean
    /// must not trip a threshold set at 60% of the mean.
    #[test]
    fn ordinary_jitter_does_not_fire() {
        let (mut w, mut clock) = warmed(200);
        let mut notices = Vec::new();
        for i in 0..600 {
            let gap = [120u64, 200, 280, 160, 240][i % 5];
            clock += gap;
            if let Some(n) = w.on_token(clock) {
                notices.push(n);
            }
        }
        assert!(notices.is_empty(), "jitter must not fire, got {notices:?}");
    }

    #[test]
    fn a_sustained_collapse_fires_exactly_once() {
        let (mut w, mut clock) = warmed(200);
        // A third of the original speed, held.
        let notices = feed(&mut w, &mut clock, 400, 600);

        assert_eq!(notices.len(), 1, "expected one transition, got {notices:?}");
        assert!(notices[0].throttled);
        assert!(w.throttled());
    }

    /// The notice must describe the measurement it was made from, not just
    /// announce itself — this is what the A7 soak run records.
    #[test]
    fn the_notice_carries_the_rates_it_decided_on() {
        let (mut w, mut clock) = warmed(200);
        let notices = feed(&mut w, &mut clock, 400, 600);
        let n = notices[0];

        // 200 ms/token = 5 tok/s baseline; 600 ms/token ≈ 1.67 tok/s.
        assert!((n.baseline_tps - 5.0).abs() < 0.2, "baseline {n:?}");
        assert!((n.recent_tps - 1.667).abs() < 0.2, "recent {n:?}");
        assert!(n.recent_tps <= n.baseline_tps * COLLAPSE_RATIO);
    }

    /// "Sustained" is the requirement. A dip shorter than the run length is
    /// exactly what this must not report.
    #[test]
    fn a_brief_dip_does_not_fire() {
        let (mut w, mut clock) = warmed(200);
        let slow = feed(&mut w, &mut clock, SUSTAINED_GAPS / 2, 600);
        let back = feed(&mut w, &mut clock, 200, 200);

        assert!(slow.is_empty(), "a dip must not fire, got {slow:?}");
        assert!(back.is_empty(), "recovery from a dip is not news, got {back:?}");
    }

    #[test]
    fn recovery_withdraws_the_notice() {
        let (mut w, mut clock) = warmed(200);
        let onset = feed(&mut w, &mut clock, 400, 600);
        assert_eq!(onset.len(), 1);

        let recovery = feed(&mut w, &mut clock, 400, 200);
        assert_eq!(recovery.len(), 1, "expected one recovery, got {recovery:?}");
        assert!(!recovery[0].throttled);
        assert!(!w.throttled());
    }

    /// A rate sitting between the two ratios must not flap the banner.
    #[test]
    fn the_hysteresis_band_does_not_flap() {
        let (mut w, mut clock) = warmed(200);
        feed(&mut w, &mut clock, 400, 600);
        assert!(w.throttled());

        // ~70% of baseline: above COLLAPSE_RATIO, below RECOVERY_RATIO.
        let band = feed(&mut w, &mut clock, 400, 285);
        assert!(band.is_empty(), "the band must be silent, got {band:?}");
        assert!(w.throttled(), "and must not silently clear the state");
    }

    /// **The test this module exists for.** A per-turn baseline re-measures
    /// against the throttled rate, calls it normal, and can never fire — while
    /// compiling cleanly and passing every single-turn test.
    #[test]
    fn the_baseline_survives_a_turn_boundary() {
        let (mut w, mut clock) = warmed(200);

        // The healthy turn ends; the user reads it and types for two minutes.
        w.turn_ended();
        clock += 120_000;

        // The next turn is throttled from its first token.
        let notices = feed(&mut w, &mut clock, 400, 600);
        assert_eq!(notices.len(), 1, "must still fire across turns, got {notices:?}");
        assert!((notices[0].baseline_tps - 5.0).abs() < 0.2,
                "the baseline must be the HEALTHY rate, not the throttled one: {notices:?}");
    }

    /// Prefill and idle time must never be measured as generation. Without
    /// `turn_ended` clearing the previous timestamp, the two-minute pause below
    /// is a single 120-second "gap".
    #[test]
    fn prefill_and_idle_time_are_not_gaps() {
        let (mut w, mut clock) = warmed(200);

        for _ in 0..5 {
            w.turn_ended();
            // A long pause, then a slow first token (prefill), then healthy
            // generation — the ordinary shape of every turn on the floor device.
            clock += 90_000;
            let notices = feed(&mut w, &mut clock, 60, 200);
            assert!(notices.is_empty(), "an ordinary turn must be silent, got {notices:?}");
        }
        assert!(!w.throttled());
    }

    /// Stalls are dropped, not averaged in.
    ///
    /// **The spacing is the test, and the first version of this test had it
    /// wrong.** A lone stall followed by thirty healthy tokens proves nothing:
    /// two full windows flush it, so the run counter never reaches
    /// [`SUSTAINED_GAPS`] and the test passes *with the exclusion deleted* —
    /// [`SUSTAINED_GAPS`] was silently doing the work the assertion claimed to
    /// check. Caught by deleting the exclusion and watching this stay green.
    ///
    /// So the stalls here recur **closer together than [`WINDOW_GAPS`]**, which
    /// is the case the exclusion actually exists for: a backgrounded app, or a
    /// tool-heavy conversation, where the window never contains a clean sample.
    /// Without the exclusion the mean sits under the threshold permanently and
    /// the notice fires on a phone that is merely being interrupted.
    #[test]
    fn recurring_stalls_are_not_slow_tokens() {
        let (mut w, mut clock) = warmed(200);

        for _ in 0..40 {
            clock += MAX_GAP_MS + 5_000;
            assert!(w.on_token(clock).is_none(), "a stall must not fire");
            // Fewer than WINDOW_GAPS, so a recorded stall would never leave the
            // window before the next one arrived.
            let notices = feed(&mut w, &mut clock, WINDOW_GAPS / 2, 200);
            assert!(notices.is_empty(), "healthy tokens around a stall, got {notices:?}");
        }
        assert!(!w.throttled(), "interruption is not throttling");
    }

    /// A device that is slow from the very first token is a slow device, not a
    /// throttling one. With no baseline there is nothing to compare against and
    /// the honest output is silence.
    #[test]
    fn no_verdict_before_a_baseline_exists() {
        let mut w = ThermalWatch::new();
        let mut clock = 0u64;
        let notices = feed(&mut w, &mut clock, WARMUP_SKIP_GAPS + BASELINE_GAPS - 1, 3_000);
        assert!(notices.is_empty(), "no baseline means no verdict, got {notices:?}");
    }

    #[test]
    fn the_ratio_predicates_are_a_band_with_a_gap() {
        // Collapse.
        assert!(detect_thermal_collapse(10.0, 6.0));
        assert!(detect_thermal_collapse(10.0, 1.0));
        assert!(!detect_thermal_collapse(10.0, 6.1));

        // Recovery.
        assert!(thermal_recovered(10.0, 8.5));
        assert!(thermal_recovered(10.0, 12.0));
        assert!(!thermal_recovered(10.0, 8.4));

        // The band between them is neither, which is what stops the flapping.
        for recent in [6.5_f64, 7.0, 8.0] {
            assert!(!detect_thermal_collapse(10.0, recent));
            assert!(!thermal_recovered(10.0, recent));
        }

        // No baseline is never a finding in either direction.
        assert!(!detect_thermal_collapse(0.0, 0.0));
        assert!(!thermal_recovered(0.0, 100.0));
    }

    // ---- The Q1 selftest script -------------------------------------------

    /// The affordance's whole promise: tap it on a cold phone, get an onset.
    #[test]
    fn the_selftest_script_produces_an_onset() {
        let n = synthetic_collapse().expect("the script must fire");
        assert!(n.throttled, "Q1 must produce an ONSET, not a recovery: {n:?}");
        // The rates must be the scripted ones, or the notice the founder sees
        // describes something other than what the script did.
        assert!((n.baseline_tps - 5.0).abs() < 0.2, "baseline {n:?}");
        assert!((n.recent_tps - 1.667).abs() < 0.2, "recent {n:?}");
        // And they must be a collapse by the same predicate production uses.
        assert!(detect_thermal_collapse(n.baseline_tps, n.recent_tps));
    }

    /// **The negative control for the script itself.** A cadence that never
    /// slows must reach the no-notice branch. Without this the suite only ever
    /// proves the script *can* fire, which a script that fires unconditionally
    /// would also satisfy — and that script is exactly the broken Q1: a check
    /// that cannot fail, reporting a healthy pipeline on a device where the
    /// detector is dead.
    #[test]
    fn the_selftest_script_stays_silent_when_nothing_slows_down() {
        let err = replay_to_onset(200, 200, SELFTEST_MAX_SLOW_GAPS)
            .err()
            .expect("a flat cadence must not produce a notice");
        assert!(err.contains("did not fire"), "wrong branch: {err}");
    }

    /// The two no-notice branches must be distinguishable. An affordance whose
    /// failures all read alike sends the founder back with "it didn't work".
    #[test]
    fn the_selftests_two_failures_say_different_things() {
        let flat = replay_to_onset(200, 200, 64).err().expect("flat must not fire");
        let slow = replay_to_onset(3_000, 3_000, 64).err().expect("also must not fire");
        assert!(flat.contains("200 ms/token"), "{flat}");
        assert!(slow.contains("3000 ms/token"), "{slow}");
    }

    /// **The affordance must clean up after itself.** An onset with no
    /// withdrawal leaves `ThermalState` holding `throttled: true`, which
    /// `chat_thermal_state` re-asserts on every reload — one tap would pin
    /// "your phone is warming up" to a cold device permanently.
    #[test]
    fn the_selftest_can_withdraw_the_notice_it_raised() {
        let n = synthetic_recovery().expect("the recovery script must fire");
        assert!(!n.throttled, "the second edge must be a WITHDRAWAL: {n:?}");
        assert!(thermal_recovered(n.baseline_tps, n.recent_tps), "{n:?}");
    }

    /// The pair must be a round trip: raise, then clear, with the same baseline
    /// in both payloads. A recovery quoting a different baseline would mean the
    /// two edges came from different watches, and the UI would be withdrawing a
    /// notice it never raised.
    #[test]
    fn the_selftests_two_edges_agree_on_the_baseline() {
        let onset = synthetic_collapse().expect("onset");
        let recovery = synthetic_recovery().expect("recovery");
        assert!(onset.throttled && !recovery.throttled);
        assert!(
            (onset.baseline_tps - recovery.baseline_tps).abs() < f64::EPSILON,
            "onset {onset:?} vs recovery {recovery:?}"
        );
    }

    // ---- The soak trace ---------------------------------------------------

    /// The trace must report the number the DETECTOR used, which means no
    /// recent rate until the window is full.
    #[test]
    fn the_trace_withholds_a_rate_the_detector_is_not_using_yet() {
        let mut w = ThermalWatch::new();
        let mut clock = 0u64;

        assert_eq!(w.trace().gaps, 0);
        assert_eq!(w.trace().baseline_tps, None);
        assert_eq!(w.trace().recent_tps, None);

        // Enough for the baseline but not to fill the window.
        feed(&mut w, &mut clock, WARMUP_SKIP_GAPS + BASELINE_GAPS + 1, 200);
        let t = w.trace();
        assert!(t.baseline_tps.is_some(), "baseline should exist by now: {t:?}");
        assert_eq!(t.recent_tps, None, "the window is not full yet: {t:?}");

        feed(&mut w, &mut clock, WINDOW_GAPS, 200);
        let t = w.trace();
        let recent = t.recent_tps.expect("a full window must report a rate");
        assert!((recent - 5.0).abs() < 0.2, "{t:?}");
        assert!(!t.notified);
    }

    /// Stalls are excluded from the verdict, so they must be excluded from the
    /// count too — a trace that counted them would show gaps the rates were
    /// never computed from, and the soak reader would find the arithmetic
    /// impossible to reconcile.
    #[test]
    fn the_traces_gap_count_excludes_stalls() {
        let mut w = ThermalWatch::new();
        let mut clock = 0u64;

        // A gap needs two tokens, so n tokens produce n-1 gaps. Written as
        // `- 1` rather than as the literal 9 because this off-by-one is the
        // whole reason prefill is excluded structurally, and a bare number
        // here would read as arbitrary.
        feed(&mut w, &mut clock, 10, 200);
        assert_eq!(w.trace().gaps, 10 - 1, "the first token has no predecessor");

        clock += MAX_GAP_MS + 5_000;
        assert!(w.on_token(clock).is_none());
        assert_eq!(w.trace().gaps, 9, "a stall is not a measured gap");

        // The first token of a turn is not a gap either.
        w.turn_ended();
        clock += 1_000;
        assert!(w.on_token(clock).is_none());
        assert_eq!(w.trace().gaps, 9, "a turn's first token is not a gap");
    }

    /// Guards the `max(1)` floor in [`rate`]. A naive `0.0` fallback would
    /// report instantaneous generation as a total collapse — the verdict
    /// inverted by its own division guard.
    #[test]
    fn instantaneous_tokens_are_not_a_collapse() {
        assert!(rate([0u64; 16].into_iter()) > 0.0);
        assert!(!detect_thermal_collapse(5.0, rate([0u64; 16].into_iter())));
        assert_eq!(rate(std::iter::empty()), 0.0);
    }
}
