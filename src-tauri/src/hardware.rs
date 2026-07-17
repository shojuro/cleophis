use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct HardwareInfo {
    pub ram_gb: u64,
    pub gpu: String,
    pub vram_gb: u64,
    pub platform: String,
    pub tier: String,
}

/// Tier mapping per spec §6: high = Apple Silicon or NVIDIA with >= 8 GB VRAM;
/// mid = >= 16 GB RAM; low = everything else.
pub fn tier_for(ram_gb: u64, nvidia_vram_gb: u64, apple_silicon: bool) -> &'static str {
    if apple_silicon || nvidia_vram_gb >= 8 {
        "high"
    } else if ram_gb >= 16 {
        "mid"
    } else {
        "low"
    }
}

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

    let apple = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    HardwareInfo {
        ram_gb,
        gpu,
        vram_gb,
        platform: std::env::consts::OS.to_string(),
        tier: tier_for(ram_gb, vram_gb, apple).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
