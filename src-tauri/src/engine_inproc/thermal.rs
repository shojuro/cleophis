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
//! `SessionTurns::turn` is called once per token by `EngineSession::stream`,
//! before any of that, and it is the only place the real cadence is visible.
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
}

#[cfg(test)]
mod tests {
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
