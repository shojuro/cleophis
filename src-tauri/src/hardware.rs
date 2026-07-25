use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct HardwareInfo {
    pub ram_gb: u64,
    pub gpu: String,
    pub vram_gb: u64,
    pub platform: String,
    pub tier: String,
    /// Can this device run Cleophis at all? False only on Android below the
    /// 4 GB floor (task 2.3), where the honest answer is a "not yet" screen
    /// rather than a multi-gigabyte download that ends in an OOM kill. Always
    /// true on desktop. Additive to the serialized shape, so a frontend that
    /// does not read it is unaffected.
    pub supported: bool,
}

/// Tier mapping per spec §6: high = Apple Silicon or NVIDIA with >= 8 GB VRAM;
/// mid = >= 16 GB RAM; low = everything else.
///
/// Desktop's rule, and now desktop's only: since task 2.3 Android answers the
/// tier question through [`mobile_tier`] instead, so this is dead there by
/// platform fact rather than by oversight. The mirror image of convstore's
/// partial-row methods, which are dead on desktop for the same kind of reason.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub fn tier_for(ram_gb: u64, nvidia_vram_gb: u64, apple_silicon: bool) -> &'static str {
    if apple_silicon || nvidia_vram_gb >= 8 {
        "high"
    } else if ram_gb >= 16 {
        "mid"
    } else {
        "low"
    }
}

/// Mobile tiering (task 2.3).
///
/// Compiled on **every** platform so the desktop suite runs its tests — the
/// rule is pure, and a wrong tier is silent (a phone that loads a model it
/// cannot run at usable speed, which reads as "the app is slow"). Only the two
/// detection functions are Android-gated, because they read Linux paths and
/// can be tested nowhere but a phone. This is decision D-3 applied a third
/// time.
///
/// Nothing off Android calls into here, which is what the attribute states —
/// narrowly, and as a permanent platform fact rather than a deferred question.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod mobile_tier {

/// The SoC facts that decide whether a phone can carry the 4B hero.
///
/// RAM alone is not the question — P0 measured that directly. The A22's
/// Dimensity 700 pairs *workhorse-class RAM with floor-class CPU* and managed
/// only 1.0–1.8 tok/s on a 4B stack, which is a technically-loaded model that
/// nobody would use. So the mid tier asks about the CPU too.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SocCaps {
    /// ARM dot-product (`asimddp`). Load-bearing for llama.cpp's Q4 kernels —
    /// the vendored `llama-cpp-sys-2` patch compiles for `armv8.2-a+dotprod`
    /// precisely so this path exists.
    pub dotprod: bool,
    /// ARM Int8 Matrix Multiply. Present on Cortex-A78/X1-class cores and
    /// absent on the A55-class cores that define the floor, which is what makes
    /// it a good proxy for "this SoC is a generation above the A22".
    pub i8mm: bool,
    /// Fastest core's `cpuinfo_max_freq`, in kHz (the unit the kernel uses).
    pub max_core_khz: u64,
}

// AArch64 HWCAP bits, from the kernel's `asm/hwcap.h`, and the auxv keys that
// carry them. Named rather than inlined because a wrong bit here is silent in
// the worst direction: it would promote an incapable phone to a 4B model it
// cannot run, and the symptom is "the app is slow", not an error.
pub const AT_HWCAP: u64 = 16;
pub const AT_HWCAP2: u64 = 26;
pub const HWCAP_ASIMDDP: u64 = 1 << 20;
pub const HWCAP2_I8MM: u64 = 1 << 13;

/// 2.75 GHz, in `cpuinfo_max_freq`'s kHz.
pub const FAST_CORE_KHZ: u64 = 2_750_000;

/// Below this the device cannot carry even the floor stack, and the honest
/// answer is an onboarding screen that says so rather than a download that
/// ends in an out-of-memory kill.
///
/// **Rounding is load-bearing.** `ram_gb` is `MemTotal` rounded, not the
/// nominal spec figure: a nominally-4 GB phone reports ~3.6–3.7 GiB because
/// the kernel and reserved regions are already subtracted. Rounding puts it at
/// 4 and keeps it supported; flooring would put it at 3 and lock out the
/// reference device — the A22 — along with every other 4 GB Android phone.
pub fn mobile_supported(ram_gb: u64) -> bool {
    ram_gb >= 4
}

/// Mobile tier per the brief: **mid (4B) only if RAM >= 8 GB AND the CPU is a
/// generation above the floor** (i8mm, or a core at 2.75 GHz or better).
/// Everything else is low. The A22 — 4 GB, no i8mm — is low by design, which is
/// the case this rule was written from.
pub fn tier_for_mobile(ram_gb: u64, caps: SocCaps) -> &'static str {
    let fast_cpu = caps.i8mm || caps.max_core_khz >= FAST_CORE_KHZ;
    if ram_gb >= 8 && fast_cpu {
        "mid"
    } else {
        "low"
    }
}

/// Pull `AT_HWCAP` and `AT_HWCAP2` out of a raw `/proc/self/auxv`.
///
/// The file is a flat array of `(key, value)` pairs of `unsigned long`,
/// terminated by an `AT_NULL` (0) key. Parsed here rather than read through
/// `getauxval` so that it needs no new dependency **and** so the parse is a
/// pure function the desktop suite can test — the detection around it cannot
/// be tested anywhere but a phone (decision D-3).
///
/// Trailing bytes that do not form a whole pair are ignored rather than
/// guessed at: a truncated read should lose a capability (downgrading the
/// device to `low`) and never invent one.
pub fn hwcaps_from_auxv(bytes: &[u8]) -> (u64, u64) {
    let (mut hwcap, mut hwcap2) = (0u64, 0u64);
    for pair in bytes.chunks_exact(16) {
        let key = u64::from_ne_bytes(pair[..8].try_into().unwrap_or_default());
        let val = u64::from_ne_bytes(pair[8..].try_into().unwrap_or_default());
        match key {
            0 => break, // AT_NULL terminates the vector.
            AT_HWCAP => hwcap = val,
            AT_HWCAP2 => hwcap2 = val,
            _ => {}
        }
    }
    (hwcap, hwcap2)
}

/// The fastest core's max frequency from a set of `cpuinfo_max_freq` readings.
///
/// Unreadable or unparseable cores are skipped rather than treated as zero-
/// speed, and an empty set yields 0 — which reads as "not fast", so a phone
/// whose sysfs we cannot parse lands on the floor tier instead of being
/// promoted on a guess.
pub fn max_core_khz(readings: impl IntoIterator<Item = String>) -> u64 {
    readings
        .into_iter()
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

/// Read every CPU's `cpuinfo_max_freq`. Android-only: the path is Linux's, and
/// on desktop the tier question is answered by RAM and GPU instead.
#[cfg(target_os = "android")]
fn read_core_khz() -> u64 {
    let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu") else {
        return 0;
    };
    let readings = entries.filter_map(|e| {
        let path = e.ok()?.path();
        let name = path.file_name()?.to_str()?;
        // `cpu0`, `cpu1`, … — not `cpuidle`, `cpufreq`, `possible`.
        if !name.starts_with("cpu") || !name[3..].chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        std::fs::read_to_string(path.join("cpufreq/cpuinfo_max_freq")).ok()
    });
    max_core_khz(readings)
}

/// What this SoC can do. Every failure degrades to "no capability", which
/// degrades to the floor tier — the safe direction, since the cost of guessing
/// low is a smaller model and the cost of guessing high is a device that
/// swaps or is killed.
#[cfg(target_os = "android")]
pub fn soc_caps() -> SocCaps {
    let (hwcap, hwcap2) = std::fs::read("/proc/self/auxv")
        .map(|b| hwcaps_from_auxv(&b))
        .unwrap_or((0, 0));
    SocCaps {
        dotprod: hwcap & HWCAP_ASIMDDP != 0,
        i8mm: hwcap2 & HWCAP2_I8MM != 0,
        max_core_khz: read_core_khz(),
    }
}

} // mod mobile_tier

pub fn detect() -> HardwareInfo {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let ram_gb = ((sys.total_memory() as f64) / 1_073_741_824.0).round() as u64;

    #[cfg(windows)]
    let (gpu, vram_gb) = match nvml_wrapper::Nvml::init() {
        Ok(nvml) => match nvml.device_by_index(0) {
            Ok(d) => (
                d.name().unwrap_or_else(|_| "NVIDIA GPU".to_string()),
                d.memory_info().map(|m| m.total / 1_073_741_824).unwrap_or(0),
            ),
            Err(_) => ("No dedicated GPU detected".to_string(), 0),
        },
        Err(_) => ("No dedicated GPU detected".to_string(), 0),
    };
    #[cfg(not(windows))]
    let (gpu, vram_gb) = ("No dedicated GPU detected".to_string(), 0u64);

    // Android answers the tier question from the SoC, not from RAM alone, and
    // is the only platform that can report `supported: false` — a desktop that
    // runs this code has already met the floor by existing.
    #[cfg(target_os = "android")]
    let (tier, supported) = (
        mobile_tier::tier_for_mobile(ram_gb, mobile_tier::soc_caps()).to_string(),
        mobile_tier::mobile_supported(ram_gb),
    );
    #[cfg(not(target_os = "android"))]
    let (tier, supported) = {
        // Scoped to this branch on purpose: on Android it would be an unused
        // binding, and this phase holds a zero-warning bar.
        let apple = cfg!(all(target_os = "macos", target_arch = "aarch64"));
        (tier_for(ram_gb, vram_gb, apple).to_string(), true)
    };

    HardwareInfo {
        ram_gb,
        gpu,
        vram_gb,
        platform: std::env::consts::OS.to_string(),
        tier,
        supported,
    }
}

#[cfg(test)]
mod tests {
    use super::mobile_tier::*;
    use super::*;

    /// A phone one generation above the floor: 8 GB and i8mm.
    fn capable() -> SocCaps {
        SocCaps { dotprod: true, i8mm: true, max_core_khz: 2_400_000 }
    }

    /// The A22 5G (Dimensity 700): dotprod yes, i8mm no, 2.2 GHz, 4 GB.
    fn a22() -> SocCaps {
        SocCaps { dotprod: true, i8mm: false, max_core_khz: 2_200_000 }
    }

    #[test]
    fn the_reference_device_is_low() {
        // The case the whole rule was written from. P0 measured a 4B stack on
        // this SoC at 1.0-1.8 tok/s — loadable and unusable — so "low" here is
        // the assertion that RAM alone must never promote a device.
        assert_eq!(tier_for_mobile(4, a22()), "low");
        // Even with workhorse RAM, this CPU stays on the floor.
        assert_eq!(tier_for_mobile(8, a22()), "low");
    }

    #[test]
    fn mid_needs_both_the_ram_and_the_cpu() {
        assert_eq!(tier_for_mobile(8, capable()), "mid");
        // RAM without the CPU generation.
        assert_eq!(tier_for_mobile(12, a22()), "low");
        // CPU generation without the RAM — a 6 GB flagship-lite still cannot
        // hold a 4B stack, which P0 measured at 4.27/4.54 GB peak RSS.
        assert_eq!(tier_for_mobile(6, capable()), "low");
    }

    #[test]
    fn a_fast_core_substitutes_for_i8mm() {
        let fast = SocCaps { dotprod: true, i8mm: false, max_core_khz: 2_800_000 };
        assert_eq!(tier_for_mobile(8, fast), "mid");
        // Just under the line stays low — the boundary is asserted, not assumed.
        let nearly = SocCaps { dotprod: true, i8mm: false, max_core_khz: 2_749_999 };
        assert_eq!(tier_for_mobile(8, nearly), "low");
    }

    #[test]
    fn unknown_caps_land_on_the_floor() {
        // Every detection failure degrades to this, so it must be "low" rather
        // than a promotion on a guess.
        assert_eq!(tier_for_mobile(16, SocCaps::default()), "low");
    }

    #[test]
    fn the_support_floor_survives_ram_rounding() {
        // A nominally-4 GB phone reports ~3.6-3.7 GiB of MemTotal, which
        // rounds to 4. If this ever floors instead, the reference device and
        // every other 4 GB Android phone become "unsupported" — the exact
        // silent-boundary failure this codebase keeps finding.
        assert!(mobile_supported(4));
        assert!(!mobile_supported(3));
        assert!(mobile_supported(8));
    }

    #[test]
    fn hwcaps_are_read_out_of_an_auxv_image() {
        // AT_HWCAP=16 carrying ASIMDDP, AT_HWCAP2=26 carrying I8MM, plus an
        // unrelated key, terminated by AT_NULL.
        let mut buf = Vec::new();
        let mut pair = |k: u64, v: u64| {
            buf.extend_from_slice(&k.to_ne_bytes());
            buf.extend_from_slice(&v.to_ne_bytes());
        };
        pair(6, 4096); // AT_PAGESZ — must be ignored
        pair(16, HWCAP_ASIMDDP);
        pair(26, HWCAP2_I8MM);
        pair(0, 0); // AT_NULL

        let (hwcap, hwcap2) = hwcaps_from_auxv(&buf);
        assert!(hwcap & HWCAP_ASIMDDP != 0);
        assert!(hwcap2 & HWCAP2_I8MM != 0);
    }

    #[test]
    fn auxv_parsing_stops_at_the_terminator_and_tolerates_a_short_read() {
        let mut buf = Vec::new();
        let mut pair = |k: u64, v: u64| {
            buf.extend_from_slice(&k.to_ne_bytes());
            buf.extend_from_slice(&v.to_ne_bytes());
        };
        pair(0, 0); // AT_NULL first
        pair(16, HWCAP_ASIMDDP); // past the terminator — must not be read
        assert_eq!(hwcaps_from_auxv(&buf), (0, 0));

        // A truncated final pair is dropped, never half-parsed: losing a
        // capability downgrades the device, inventing one promotes it.
        let mut short = 16u64.to_ne_bytes().to_vec();
        short.extend_from_slice(&[0xff, 0xff, 0xff]);
        assert_eq!(hwcaps_from_auxv(&short), (0, 0));
        assert_eq!(hwcaps_from_auxv(&[]), (0, 0));
    }

    #[test]
    fn the_fastest_core_wins_and_junk_is_skipped() {
        let readings = ["1800000\n", "2208000\n", "not a number", ""]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        assert_eq!(max_core_khz(readings), 2_208_000);
        // No readable cores reads as "not fast", not as "assume fast".
        assert_eq!(max_core_khz(Vec::<String>::new()), 0);
    }

    #[test]
    fn demo_machine_is_mid() {
        // GTX 1650 (4 GB VRAM) + 16 GB RAM
        assert_eq!(tier_for(16, 4, false), "mid");
    }

    #[test]
    fn big_nvidia_is_high() {
        assert_eq!(tier_for(16, 8, false), "high");
        assert_eq!(tier_for(8, 12, false), "high");
    }

    #[test]
    fn apple_silicon_is_high() {
        assert_eq!(tier_for(8, 0, true), "high");
    }

    #[test]
    fn small_ram_no_gpu_is_low() {
        assert_eq!(tier_for(8, 0, false), "low");
        assert_eq!(tier_for(15, 0, false), "low");
    }

    #[test]
    fn ram_only_is_mid() {
        assert_eq!(tier_for(32, 0, false), "mid");
    }

    #[test]
    fn detect_returns_something_sane() {
        let h = detect();
        assert!(h.ram_gb > 0);
        assert!(["high", "mid", "low"].contains(&h.tier.as_str()));
    }
}
