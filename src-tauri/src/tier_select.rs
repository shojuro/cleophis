//! Wrapper tier-selection: pick the hero's base+adapter by device hardware
//! tier, with a manual override, an entitlement-tied switch limit, and a
//! one-active-model-on-disk cleanup.
//!
//! The launch/verify path (`inference::resolve_launch`/`hero_hash`) and the
//! FE hero-download both resolve the base+adapter through
//! [`crate::catalog::hero_variant`] keyed off [`effective_tier`]. This module
//! owns:
//!
//! - **Selection state** ([`TierSelection`], persisted at
//!   `app_data/tier_selection.json`) — the override `mode`, the last
//!   installed `active_tier`, whether the user has `committed` (chatted) on
//!   it, and the switch-limit bookkeeping.
//! - **Effective tier** ([`effective_tier`]) — override if set, else
//!   `hardware::detect().tier`.
//! - **The switch limit** ([`switch_allowed`]) — free until the first chat,
//!   then ≤1 change per billing period, keyed off the active hero
//!   entitlement's `expires_at` (perpetual grants fall back to a rolling
//!   30-day cooldown). Enforced here against locally-cached entitlements, so
//!   it works offline.
//! - **Cleanup** ([`sweep_models`]) — after a successful switch, delete every
//!   model file that isn't the active pair (one model on disk; also clears
//!   pre-tiers orphans).
//!
//! The pure functions in this module (everything not taking an `AppHandle`)
//! are unit-tested below; the `#[tauri::command]` wrappers live in A2.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The hero catalog id every tier variant belongs to — the entitlement the
/// switch limit is keyed off, and the only `real` entry `catalog::hero`
/// returns.
pub const HERO_MODEL_ID: &str = "socratic-tutor";

/// Perpetual-grant fallback cooldown: when the active entitlement has no
/// `expires_at` (a free/library grant, no billing period to anchor to), a
/// committed switch is allowed at most once every 30 days.
pub const SWITCH_COOLDOWN_SECS: i64 = 30 * 24 * 60 * 60;

/// Persisted tier-selection state (`app_data/tier_selection.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TierSelection {
    /// `"auto"` (follow `hardware::detect`) or an explicit `"low"`/`"mid"`/
    /// `"high"` override.
    pub mode: String,
    /// The tier whose base+adapter is currently installed/running. Empty
    /// until the first model lands.
    pub active_tier: String,
    /// Has the user sent a chat on the active model? Until they have, tier
    /// changes are free (setup); after, the per-period limit applies.
    pub committed: bool,
    /// The active hero entitlement's `expires_at` recorded at the last
    /// switch — the billing-period key. `None` before any limited switch.
    pub period_key: Option<String>,
    /// Epoch seconds of the last switch — drives the perpetual-grant 30-day
    /// cooldown only.
    pub switched_at: Option<i64>,
}

impl Default for TierSelection {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
            active_tier: String::new(),
            committed: false,
            period_key: None,
            switched_at: None,
        }
    }
}

/// Read the persisted selection, or [`TierSelection::default`] on any
/// missing/corrupt/unparseable file (fail-open to auto — never blocks a
/// launch).
pub fn read_selection(path: &Path) -> TierSelection {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the selection atomically (temp file + rename), mirroring
/// `catalog_dist::write_highest_version`.
pub fn write_selection(path: &Path, sel: &TierSelection) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("tier_selection dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(sel).map_err(|e| format!("tier_selection serialize: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| format!("tier_selection write: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("tier_selection rename: {e}"))?;
    Ok(())
}

/// The effective device tier from a selection + a detected tier: the override
/// `mode` when it is a valid tier, else the detected tier (auto).
pub fn effective_tier_from(sel: &TierSelection, detected: &str) -> String {
    match sel.mode.as_str() {
        "low" | "mid" | "high" => sel.mode.clone(),
        _ => detected.to_string(),
    }
}

/// The current access state for the hero product, as far as the switch limit
/// is concerned.
#[derive(Debug, Clone, PartialEq)]
pub enum HeroEntitlement {
    /// No active (non-expired) entitlement → switching is refused (the same
    /// state that pauses downloads until renewal).
    None,
    /// Active but with no billing period (`expires_at: null`) — a free/library
    /// grant. Falls back to the 30-day cooldown.
    Perpetual,
    /// Active with a billing period; the `String` is `expires_at`, used as the
    /// period key.
    Period(String),
}

/// Why a switch was refused — mapped to user-facing copy at the command layer.
#[derive(Debug, Clone, PartialEq)]
pub enum SwitchDenied {
    /// No active hero entitlement — renew to switch.
    NoEntitlement,
    /// The one change for this billing period is already spent; unlocks at
    /// renewal (the current period's `expires_at`).
    AlreadySwitchedThisPeriod { available_at: Option<String> },
    /// Perpetual-grant cooldown not yet elapsed; `available_at` is epoch secs.
    CooldownActive { available_at: i64 },
}

/// The switch-limit state machine (pure). Free until the user has `committed`
/// (chatted) on the active model; after that, one change per billing period
/// (period key = the entitlement `expires_at`), or one per 30 days for a
/// perpetual grant. No active entitlement always refuses.
pub fn switch_allowed(
    committed: bool,
    ent: &HeroEntitlement,
    recorded_period_key: Option<&str>,
    switched_at: Option<i64>,
    now: i64,
) -> Result<(), SwitchDenied> {
    if let HeroEntitlement::None = ent {
        return Err(SwitchDenied::NoEntitlement);
    }
    if !committed {
        // Setup / pre-first-chat: changing the tier is free.
        return Ok(());
    }
    match ent {
        HeroEntitlement::None => unreachable!("handled above"),
        HeroEntitlement::Period(expires_at) => {
            if recorded_period_key == Some(expires_at.as_str()) {
                Err(SwitchDenied::AlreadySwitchedThisPeriod {
                    available_at: Some(expires_at.clone()),
                })
            } else {
                Ok(())
            }
        }
        HeroEntitlement::Perpetual => match switched_at {
            Some(t) if now < t + SWITCH_COOLDOWN_SECS => Err(SwitchDenied::CooldownActive {
                available_at: t + SWITCH_COOLDOWN_SECS,
            }),
            _ => Ok(()),
        },
    }
}

/// Delete every `*.gguf` directly in `models_dir` whose filename is not in
/// `keep` (the active tier's base + adapter basenames). Returns the names
/// removed. Only touches whole `.gguf` files inside `models_dir` — never a
/// `.part` in-flight download, never a subdirectory. Must be called only
/// after the active model has relaunched, so the in-use pair is safe.
pub fn sweep_models(models_dir: &Path, keep: &[String]) -> Vec<String> {
    let mut removed = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(models_dir) else {
        return removed;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_gguf = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("gguf"))
            .unwrap_or(false);
        if !is_gguf {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if keep.iter().any(|k| k == &name) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed.push(name);
        }
    }
    removed
}

// ---------------------------------------------------------------------------
// AppHandle-bound helpers (thin wrappers over the pure core above).
// ---------------------------------------------------------------------------

/// `app_data/tier_selection.json`, or `None` if the app-data dir can't be
/// resolved.
pub fn selection_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("tier_selection.json"))
}

/// The persisted selection for this install (default `auto` when absent).
pub fn read_app_selection(app: &tauri::AppHandle) -> TierSelection {
    selection_path(app).map(|p| read_selection(&p)).unwrap_or_default()
}

/// The effective device tier for this install: override if set, else
/// `hardware::detect().tier`. This is the single tier the launch, verify, and
/// download paths all key off, so they agree.
pub fn effective_tier(app: &tauri::AppHandle) -> String {
    let sel = read_app_selection(app);
    effective_tier_from(&sel, &crate::hardware::detect().tier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_tier_auto_follows_detected() {
        let sel = TierSelection::default(); // mode "auto"
        assert_eq!(effective_tier_from(&sel, "low"), "low");
        assert_eq!(effective_tier_from(&sel, "mid"), "mid");
        assert_eq!(effective_tier_from(&sel, "high"), "high");
    }

    #[test]
    fn effective_tier_override_wins_over_detected() {
        for over in ["low", "mid", "high"] {
            let sel = TierSelection {
                mode: over.to_string(),
                ..Default::default()
            };
            assert_eq!(effective_tier_from(&sel, "high"), over);
        }
    }

    #[test]
    fn effective_tier_garbage_mode_falls_back_to_detected() {
        let sel = TierSelection {
            mode: "banana".to_string(),
            ..Default::default()
        };
        assert_eq!(effective_tier_from(&sel, "mid"), "mid");
    }

    #[test]
    fn read_selection_missing_file_is_default_auto() {
        let p = std::env::temp_dir().join("cleophis-tiersel-missing-xyz.json");
        let _ = std::fs::remove_file(&p);
        assert_eq!(read_selection(&p), TierSelection::default());
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = std::env::temp_dir().join(format!("cleophis-tiersel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tier_selection.json");
        let sel = TierSelection {
            mode: "high".to_string(),
            active_tier: "high".to_string(),
            committed: true,
            period_key: Some("2026-08-21T00:00:00Z".to_string()),
            switched_at: Some(1_700_000_000),
        };
        write_selection(&p, &sel).unwrap();
        assert_eq!(read_selection(&p), sel);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- switch_allowed state machine ---------------------------------------

    #[test]
    fn uncommitted_switch_is_always_free_when_entitled() {
        // Setup phase: no chat yet → free change regardless of period state.
        let ent = HeroEntitlement::Period("2026-08-21T00:00:00Z".to_string());
        assert_eq!(
            switch_allowed(false, &ent, Some("2026-08-21T00:00:00Z"), Some(0), 0),
            Ok(())
        );
        assert_eq!(switch_allowed(false, &HeroEntitlement::Perpetual, None, Some(0), 0), Ok(()));
    }

    #[test]
    fn no_entitlement_always_refuses() {
        assert_eq!(
            switch_allowed(false, &HeroEntitlement::None, None, None, 0),
            Err(SwitchDenied::NoEntitlement)
        );
        assert_eq!(
            switch_allowed(true, &HeroEntitlement::None, None, None, 0),
            Err(SwitchDenied::NoEntitlement)
        );
    }

    #[test]
    fn committed_first_switch_this_period_is_allowed() {
        let ent = HeroEntitlement::Period("2026-08-21T00:00:00Z".to_string());
        // period_key not yet recorded for this period → allowed.
        assert_eq!(switch_allowed(true, &ent, None, None, 0), Ok(()));
        // recorded key is a DIFFERENT (older) period → the month rolled → allowed.
        assert_eq!(
            switch_allowed(true, &ent, Some("2026-07-21T00:00:00Z"), Some(0), 0),
            Ok(())
        );
    }

    #[test]
    fn committed_second_switch_same_period_is_blocked() {
        let ent = HeroEntitlement::Period("2026-08-21T00:00:00Z".to_string());
        assert_eq!(
            switch_allowed(true, &ent, Some("2026-08-21T00:00:00Z"), Some(0), 0),
            Err(SwitchDenied::AlreadySwitchedThisPeriod {
                available_at: Some("2026-08-21T00:00:00Z".to_string())
            })
        );
    }

    #[test]
    fn perpetual_grant_uses_30_day_cooldown() {
        let ent = HeroEntitlement::Perpetual;
        let switched = 1_700_000_000_i64;
        // one day later → still cooling down.
        assert_eq!(
            switch_allowed(true, &ent, None, Some(switched), switched + 86_400),
            Err(SwitchDenied::CooldownActive {
                available_at: switched + SWITCH_COOLDOWN_SECS
            })
        );
        // 30 days + 1s later → allowed.
        assert_eq!(
            switch_allowed(true, &ent, None, Some(switched), switched + SWITCH_COOLDOWN_SECS + 1),
            Ok(())
        );
        // never switched before → allowed.
        assert_eq!(switch_allowed(true, &ent, None, None, switched), Ok(()));
    }

    // --- sweep_models -------------------------------------------------------

    #[test]
    fn sweep_keeps_active_pair_and_removes_the_rest() {
        let dir = std::env::temp_dir().join(format!("cleophis-sweep-{}", std::process::id()));
        let models = dir.join("models");
        std::fs::create_dir_all(&models).unwrap();
        for f in [
            "Qwen3-4B-Instruct-Q4_K_M.gguf",
            "behavioral-v1-Qwen3-4B.gguf",
            "Llama-3.2-3B-Instruct-Q4_K_M.gguf", // stale orphan
            "notes.txt",                          // non-gguf, must survive
        ] {
            std::fs::write(models.join(f), b"x").unwrap();
        }
        let keep = vec![
            "Qwen3-4B-Instruct-Q4_K_M.gguf".to_string(),
            "behavioral-v1-Qwen3-4B.gguf".to_string(),
        ];
        let mut removed = sweep_models(&models, &keep);
        removed.sort();
        assert_eq!(removed, vec!["Llama-3.2-3B-Instruct-Q4_K_M.gguf".to_string()]);
        assert!(models.join("Qwen3-4B-Instruct-Q4_K_M.gguf").exists());
        assert!(models.join("behavioral-v1-Qwen3-4B.gguf").exists());
        assert!(models.join("notes.txt").exists());
        assert!(!models.join("Llama-3.2-3B-Instruct-Q4_K_M.gguf").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_missing_dir_is_noop() {
        let dir = std::env::temp_dir().join("cleophis-sweep-nope-xyz/models");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(sweep_models(&dir, &[]).is_empty());
    }
}
