//! Vision co-residency and lifetime policy — pure, so the desktop suite tests
//! the one decision that is a memory-safety question on the floor phone.
//!
//! The Qwen3-1.7B tutor is ~1.7 GB resident; the transcriber is 0.4 GB
//! (Stage 0: 389 MB peak). Together they fit an 8 GB phone with the WebView
//! and the OS; on a 4 GB phone the honest path is sequential — unload the
//! tutor, transcribe, reload — which costs one tutor load (5–25 s) and is
//! shown to the user through the existing `engine-restarting` banner.
use std::time::Duration;

/// Below this many GB of `MemTotal` (rounded, as `hardware::detect` reports
/// it) the transcriber never shares the process with a loaded tutor.
pub const CORESIDENT_MIN_RAM_GB: u64 = 6;

/// Whether the transcriber may stay loaded beside the tutor on a phone
/// reporting `ram_gb` of total memory.
pub fn coresident(ram_gb: u64) -> bool {
    ram_gb >= CORESIDENT_MIN_RAM_GB
}

/// A resident transcriber that has not been used for this long is dropped.
/// Reloading costs 0.5 s (Stage 0), so holding it longer buys nothing.
pub const VISION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eight_gb_phones_keep_the_transcriber_beside_the_tutor() { assert!(coresident(8)); assert!(coresident(6)); }
    #[test]
    fn four_gb_phones_go_sequential() { assert!(!coresident(4)); assert!(!coresident(5)); assert!(!coresident(0)); }
    #[test]
    fn the_idle_timeout_is_two_minutes() { assert_eq!(VISION_IDLE_TIMEOUT.as_secs(), 120); }
}
