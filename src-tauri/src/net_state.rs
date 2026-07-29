//! Parsing the network/battery snapshot Android reports, for the download
//! policy (spec §2.2).
//!
//! Declared in `lib.rs` on **every** platform via `#[path]`, exactly as
//! `engine_tools`/`engine_serve` are, and for the same reason (**decision
//! D-3**): the JNI call is platform-bound, the parse is not, and the parse's
//! failure mode is silent and expensive. A field misread as `metered=0` starts
//! a multi-gigabyte download on someone's cellular plan and nothing anywhere
//! reports an error. The desktop suite is the only place on this project where
//! tests execute, so the parse lives here.
//!
//! # The safe direction, and why it is not symmetric
//!
//! **Every unknown resolves to metered.** Guessing "unmetered" when the truth
//! is metered spends the user's money and cannot be undone; guessing "metered"
//! when the truth is unmetered costs one extra tap on a prompt. Those are not
//! close, so absence, malformity, an empty string, an unparseable number and a
//! missing key all land on the same conservative answer.
//!
//! This mirrors `tier_for_mobile`, where every detection failure degrades to
//! the floor tier: guessing low costs a smaller model, guessing high costs a
//! device that swaps or is OOM-killed. Same shape of asymmetry, same rule —
//! **when a probe fails, fail toward the cheaper mistake**, and write down
//! which one that is.

use serde::Serialize;

/// What Android told us, after parsing. Serialized to the frontend, where
/// `download-policy.js` turns it into a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetState {
    /// True unless Android positively said otherwise. See the module doc: the
    /// default is the safe one, not the common one.
    pub metered: bool,
    /// `None` when the platform did not report it — deliberately not `false`,
    /// so "we don't know whether it's charging" and "it isn't charging" stay
    /// distinguishable. A charge *recommendation* should not be issued on a
    /// guess.
    pub charging: Option<bool>,
    /// Percent 0..=100, `None` when unreported or out of range.
    pub battery_percent: Option<u8>,
}

impl Default for NetState {
    fn default() -> Self {
        NetState {
            metered: true,
            charging: None,
            battery_percent: None,
        }
    }
}

/// Parse `NetworkPolicy.describe`'s `key=value` snapshot.
///
/// Unknown keys are ignored so a future field cannot break an older parser,
/// and any malformed field simply leaves its own value at the default rather
/// than failing the whole parse — one bad battery reading should not silently
/// flip `metered` back to a guess.
pub fn parse(raw: &str) -> NetState {
    let mut out = NetState::default();
    for field in raw.split_whitespace() {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            // Only an explicit "0" clears it. Anything else — "x", "", a
            // truncated read — leaves the safe default in place.
            "metered" => out.metered = value != "0",
            "charging" => {
                out.charging = match value {
                    "1" => Some(true),
                    "0" => Some(false),
                    _ => None,
                }
            }
            "battery" => out.battery_percent = value.parse::<u8>().ok().filter(|p| *p <= 100),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_snapshot() {
        let s = parse("metered=1 charging=0 battery=57");
        assert!(s.metered);
        assert_eq!(s.charging, Some(false));
        assert_eq!(s.battery_percent, Some(57));
    }

    #[test]
    fn unmetered_requires_an_explicit_zero() {
        assert!(!parse("metered=0").metered);
        assert!(parse("metered=1").metered);
    }

    /// THE asymmetry assertion. Every one of these inputs is a way the probe
    /// can fail, and every one of them must come back metered — because the
    /// alternative spends someone's money.
    #[test]
    fn every_failure_mode_falls_back_to_metered() {
        for raw in [
            "",                       // no fields at all (no ConnectivityManager)
            "   ",                    // whitespace only
            "charging=1 battery=80",  // battery reported, metered key absent
            "metered=",               // present but empty
            "metered=x",              // unparseable
            "metered",                // no '=' at all
            "meteredd=0",             // near-miss key name
            "METERED=0",              // wrong case
        ] {
            assert!(
                parse(raw).metered,
                "an unreadable snapshot must fall back to metered: {raw:?}"
            );
        }
    }

    /// `charging` stays `None` rather than collapsing to `false`, so a charge
    /// recommendation is never issued on an unknown.
    #[test]
    fn unknown_charging_is_none_not_false() {
        assert_eq!(parse("metered=0").charging, None);
        assert_eq!(parse("metered=0 charging=").charging, None);
        assert_eq!(parse("metered=0 charging=maybe").charging, None);
        assert_eq!(parse("metered=0 charging=1").charging, Some(true));
    }

    #[test]
    fn battery_rejects_out_of_range_and_garbage() {
        assert_eq!(parse("battery=0").battery_percent, Some(0));
        assert_eq!(parse("battery=100").battery_percent, Some(100));
        assert_eq!(parse("battery=101").battery_percent, None);
        assert_eq!(parse("battery=-5").battery_percent, None);
        assert_eq!(parse("battery=abc").battery_percent, None);
        assert_eq!(parse("battery=").battery_percent, None);
    }

    /// A future field must not break this parser — the reason the format is
    /// key=value rather than positional.
    #[test]
    fn unknown_keys_are_ignored() {
        let s = parse("metered=0 vpn=1 charging=1 futureField=whatever battery=42");
        assert!(!s.metered);
        assert_eq!(s.charging, Some(true));
        assert_eq!(s.battery_percent, Some(42));
    }

    /// One malformed field must not poison the others — in particular it must
    /// not flip a positively-reported `metered=0` back to a guess.
    #[test]
    fn one_bad_field_does_not_poison_the_rest() {
        let s = parse("metered=0 battery=999 charging=1");
        assert!(!s.metered);
        assert_eq!(s.battery_percent, None);
        assert_eq!(s.charging, Some(true));
    }

    #[test]
    fn default_is_the_safe_answer() {
        let d = NetState::default();
        assert!(d.metered);
        assert_eq!(d.charging, None);
        assert_eq!(d.battery_percent, None);
    }
}
