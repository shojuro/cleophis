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
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::cloud::session::Cloud;
use crate::cloud::store::Entitlement;
use crate::inference::Engine;

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
    /// The one change for this billing period is already spent; unlocks at
    /// renewal (the current period's `expires_at`).
    AlreadySwitchedThisPeriod { available_at: Option<String> },
    /// Perpetual-grant cooldown not yet elapsed; `available_at` is epoch secs.
    CooldownActive { available_at: i64 },
}

/// The switch-limit state machine (pure). Free until the user has `committed`
/// (chatted) on the active model; after that, one change per billing period
/// (period key = the entitlement `expires_at`), or one per 30 days for a
/// perpetual grant.
///
/// It does NOT hard-block on a missing entitlement. When no billing PERIOD can
/// be resolved — an offline sync gap, a perpetual/library grant, a transient
/// lookup miss, or a shared-device account that never synced — we allow the
/// change: the limit exists to curb *repetitive flipping*, not to gate access
/// (that's the Get/checkout flow's job), and a wrongly-disabled selector is a
/// worse failure than an occasional extra switch. Only a resolved-and-already-
/// used period, or an in-progress perpetual cooldown, blocks.
pub fn switch_allowed(
    committed: bool,
    ent: &HeroEntitlement,
    recorded_period_key: Option<&str>,
    switched_at: Option<i64>,
    now: i64,
) -> Result<(), SwitchDenied> {
    if !committed {
        // Setup / pre-first-chat: changing the tier is free.
        return Ok(());
    }
    match ent {
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
        // No resolvable period → don't gray out the selector; allow the change.
        HeroEntitlement::None => Ok(()),
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
pub fn selection_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("tier_selection.json"))
}

/// The `models/` directory downloads land in, or `None` if the app-data dir
/// can't be resolved.
fn models_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("models"))
}

/// The persisted selection for this install (default `auto` when absent).
pub fn read_app_selection(app: &AppHandle) -> TierSelection {
    selection_path(app).map(|p| read_selection(&p)).unwrap_or_default()
}

/// The effective device tier for this install: override if set, else
/// `hardware::detect().tier`. This is the single tier the launch, verify, and
/// download paths all key off, so they agree.
pub fn effective_tier(app: &AppHandle) -> String {
    let sel = read_app_selection(app);
    effective_tier_from(&sel, &crate::hardware::detect().tier)
}

/// The hero's [`crate::catalog::ResolvedHero`] for `tier`, read from the
/// bundled catalog. Shared by the switch commands to learn the target tier's
/// filenames + `base_model`.
fn hero_variant_for(app: &AppHandle, tier: &str) -> Result<crate::catalog::ResolvedHero, String> {
    let root = crate::inference::resources_root(app);
    let raw = std::fs::read_to_string(root.join("catalog.json")).map_err(|e| e.to_string())?;
    let entries = crate::catalog::parse_catalog(&raw)?;
    let hero = crate::catalog::hero(&entries).ok_or_else(|| "catalog has no hero".to_string())?;
    Ok(crate::catalog::hero_variant(hero, tier))
}

/// The basename of a `models/<file>.gguf` catalog path.
fn basename(path: &str) -> Option<String> {
    Path::new(path).file_name().and_then(|n| n.to_str()).map(str::to_string)
}

/// The base + adapter basenames of a resolved tier variant — the "keep set"
/// for [`sweep_models`].
fn keep_basenames(variant: &crate::catalog::ResolvedHero) -> Vec<String> {
    let mut keep = Vec::new();
    if let Some(m) = variant.model_file.as_deref().and_then(basename) {
        keep.push(m);
    }
    if let Some(a) = variant.adapter_file.as_deref().and_then(basename) {
        keep.push(a);
    }
    keep
}

/// Does the tier variant's base + adapter both already exist under `models/`?
fn variant_on_disk(app: &AppHandle, variant: &crate::catalog::ResolvedHero) -> bool {
    let Some(dir) = models_dir(app) else {
        return false;
    };
    keep_basenames(variant)
        .iter()
        .all(|name| dir.join(name).exists())
}

/// The active hero entitlement, resolved from the LOCALLY-CACHED entitlements
/// (`Cloud::entitlements` returns the cache when offline), for the switch
/// limit. See [`resolve_hero_entitlement`].
fn current_entitlement(cloud: &Cloud) -> HeroEntitlement {
    // A transient online lookup failure must NOT read as "no entitlement" (it
    // would wrongly show the limit) — fall back to the locally-cached list.
    let ents = cloud
        .entitlements()
        .unwrap_or_else(|_| cloud.cached_entitlements());
    resolve_hero_entitlement(&ents, Utc::now())
}

/// Classify the hero product's current access for the switch limit: a live
/// billing period (`expires_at` in the future) wins, else a perpetual grant
/// (no `expires_at`), else none. Only `model_id == HERO_MODEL_ID`
/// entitlements are considered; expired periods are ignored.
pub fn resolve_hero_entitlement(entitlements: &[Entitlement], now: DateTime<Utc>) -> HeroEntitlement {
    let mut best_period: Option<(DateTime<Utc>, String)> = None;
    let mut has_perpetual = false;
    for e in entitlements {
        if e.model_id != HERO_MODEL_ID {
            continue;
        }
        match e.expires_at.as_deref() {
            None => has_perpetual = true,
            Some(s) => {
                if let Ok(parsed) = DateTime::parse_from_rfc3339(s) {
                    let exp = parsed.with_timezone(&Utc);
                    if exp > now && best_period.as_ref().map_or(true, |(b, _)| exp > *b) {
                        best_period = Some((exp, s.to_string()));
                    }
                }
            }
        }
    }
    if let Some((_, key)) = best_period {
        HeroEntitlement::Period(key)
    } else if has_perpetual {
        HeroEntitlement::Perpetual
    } else {
        HeroEntitlement::None
    }
}

fn period_key_of(ent: &HeroEntitlement) -> Option<String> {
    match ent {
        HeroEntitlement::Period(k) => Some(k.clone()),
        _ => None,
    }
}

/// The tier currently on disk / running: the stored `active_tier`, or — before
/// the first install has recorded one — the effective tier (a fresh auto
/// install runs the detected tier). Prevents a "pick the tier I'm already on"
/// click from counting as a real change (which would burn the period's switch
/// and needlessly reload the same model).
fn current_active_tier(sel: &TierSelection, detected: &str) -> String {
    if sel.active_tier.is_empty() {
        effective_tier_from(sel, detected)
    } else {
        sel.active_tier.clone()
    }
}

/// Map a [`SwitchDenied`] to user-facing copy for the FE.
fn denial_message(d: &SwitchDenied) -> String {
    match d {
        SwitchDenied::AlreadySwitchedThisPeriod { available_at } => match available_at {
            Some(when) => format!(
                "You can change your model once per billing period — next change after {when}."
            ),
            None => "You can change your model once per billing period.".to_string(),
        },
        SwitchDenied::CooldownActive { available_at } => {
            let when = DateTime::from_timestamp(*available_at, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default();
            format!("You can change your model again after {when}.")
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri commands.
// ---------------------------------------------------------------------------

/// What the FE renders the tier selector from.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TierSelectionInfo {
    pub mode: String,
    pub active_tier: String,
    pub effective_tier: String,
    pub committed: bool,
    /// Whether a tier change is allowed right now (false → the per-period /
    /// cooldown limit is in effect, or there's no active entitlement).
    pub switch_available: bool,
    /// When `switch_available` is false, when the next change unlocks (the
    /// billing-period renewal, or the cooldown end) — for the FE hint.
    pub next_change_at: Option<String>,
}

/// The plan `begin_tier_switch` hands back once the switch is permitted.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SwitchPlan {
    pub target_tier: String,
    pub base_model: String,
    /// True → the FE must download the target tier's base+adapter before
    /// calling `complete_tier_switch`. False → the pair is already on disk
    /// (or the tier didn't change); go straight to `complete_tier_switch`.
    pub needs_download: bool,
    /// True → the effective model does not actually change (a mode relabel,
    /// e.g. explicit `mid` → `auto` on a mid machine): no download, no
    /// engine restart, no limit consumed.
    pub no_op: bool,
}

fn valid_mode(mode: &str) -> bool {
    matches!(mode, "auto" | "low" | "mid" | "high")
}

/// The tier-selector state for the FE (current mode, effective tier, and
/// whether a change is allowed right now).
#[tauri::command]
pub async fn get_tier_selection(
    app: AppHandle,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<TierSelectionInfo, String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let sel = read_app_selection(&app);
        let detected = crate::hardware::detect().tier;
        let effective = effective_tier_from(&sel, &detected);
        let ent = current_entitlement(&cloud);
        let now = Utc::now().timestamp();
        let (switch_available, next_change_at) = match switch_allowed(
            sel.committed,
            &ent,
            sel.period_key.as_deref(),
            sel.switched_at,
            now,
        ) {
            Ok(()) => (true, None),
            Err(d) => {
                let next = match &d {
                    SwitchDenied::AlreadySwitchedThisPeriod { available_at } => available_at.clone(),
                    SwitchDenied::CooldownActive { available_at } => {
                        DateTime::from_timestamp(*available_at, 0).map(|d| d.to_rfc3339())
                    }
                };
                (false, next)
            }
        };
        let active_tier = if sel.active_tier.is_empty() {
            effective.clone()
        } else {
            sel.active_tier.clone()
        };
        TierSelectionInfo {
            mode: sel.mode,
            active_tier,
            effective_tier: effective,
            committed: sel.committed,
            switch_available,
            next_change_at,
        }
    })
    .await
    .map_err(|_| "Something went wrong on this device. Please try again.".to_string())
}

/// Validate a tier change and return the plan. Enforces the switch limit
/// (offline, against cached entitlements). `Err` carries a user-facing denial
/// message; `Ok` tells the FE whether it must download first.
#[tauri::command]
pub async fn begin_tier_switch(
    app: AppHandle,
    cloud: State<'_, Arc<Cloud>>,
    mode: String,
) -> Result<SwitchPlan, String> {
    if !valid_mode(&mode) {
        return Err("Unknown model tier.".to_string());
    }
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let sel = read_app_selection(&app);
        let detected = crate::hardware::detect().tier;
        let target = mode_effective_tier(&mode, &sel, &detected);
        let variant = hero_variant_for(&app, &target)?;
        let base_model = variant.base_model.clone().unwrap_or_default();

        // A mode relabel that doesn't change the installed model (e.g. explicit
        // "mid" ↔ "auto" on a mid box): no download, no restart, no limit.
        if target == current_active_tier(&sel, &detected) {
            return Ok(SwitchPlan {
                target_tier: target,
                base_model,
                needs_download: false,
                no_op: true,
            });
        }

        // A real change → enforce the switch limit (a no-op before the first
        // chat; see `switch_allowed`).
        let now = Utc::now().timestamp();
        let ent = current_entitlement(&cloud);
        if let Err(d) = switch_allowed(sel.committed, &ent, sel.period_key.as_deref(), sel.switched_at, now) {
            return Err(denial_message(&d));
        }

        let needs_download = !variant_on_disk(&app, &variant);
        Ok(SwitchPlan {
            target_tier: target,
            base_model,
            needs_download,
            no_op: false,
        })
    })
    .await
    .map_err(|_| "Something went wrong on this device. Please try again.".to_string())?
}

/// Persist the new selection and (when the model actually changes) relaunch
/// the engine on it, then sweep every other model file off disk. Called after
/// `begin_tier_switch` (and any needed downloads). Re-checks the switch limit
/// so it can't be bypassed by calling it directly.
#[tauri::command]
pub async fn complete_tier_switch(
    app: AppHandle,
    cloud: State<'_, Arc<Cloud>>,
    engine: State<'_, Arc<Engine>>,
    mode: String,
) -> Result<(), String> {
    if !valid_mode(&mode) {
        return Err("Unknown model tier.".to_string());
    }
    let cloud = cloud.inner().clone();
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let sel = read_app_selection(&app);
        let detected = crate::hardware::detect().tier;
        let target = mode_effective_tier(&mode, &sel, &detected);
        let changed = target != current_active_tier(&sel, &detected);

        let ent = current_entitlement(&cloud);
        let now = Utc::now().timestamp();

        // Re-enforce the limit for a committed, real change (authoritative —
        // the FE's `begin_tier_switch` check is advisory).
        let consumed = changed && sel.committed;
        if consumed {
            if let Err(d) = switch_allowed(true, &ent, sel.period_key.as_deref(), sel.switched_at, now) {
                return Err(denial_message(&d));
            }
        }

        let new_sel = TierSelection {
            mode: mode.clone(),
            active_tier: target.clone(),
            committed: sel.committed, // one-way latch; only mark_tier_committed sets it
            period_key: if consumed {
                period_key_of(&ent)
            } else {
                sel.period_key.clone()
            },
            switched_at: if consumed { Some(now) } else { sel.switched_at },
        };

        let path = selection_path(&app).ok_or_else(|| "no app-data dir".to_string())?;

        if changed {
            // The TARGET pair (not the currently-resolved one) must be fully on
            // disk before we relaunch onto it — checked directly, since
            // `resolve_launch` would still resolve the OLD tier until the new
            // selection is written.
            let variant = hero_variant_for(&app, &target)?;
            if !variant_on_disk(&app, &variant) {
                // Don't persist / restart / sweep onto a missing model — the FE
                // routes back through the download flow.
                return Err("The selected model isn't fully downloaded yet.".to_string());
            }
            // Persist BEFORE restart so `resolve_launch` inside the engine
            // thread resolves the new tier.
            write_selection(&path, &new_sel)?;
            crate::inference::restart(app.clone(), engine);
            // Sweep only once the new tier actually resolves on disk (defensive:
            // never strip the previous model if the relaunch had nothing to
            // load) — keep just the new active pair.
            if crate::inference::resolve_launch(&app).is_some() {
                if let Some(dir) = models_dir(&app) {
                    let removed = sweep_models(&dir, &keep_basenames(&variant));
                    if !removed.is_empty() {
                        eprintln!("tier switch: swept {} stale model file(s)", removed.len());
                    }
                }
            }
        } else {
            // Mode relabel only — persist, no engine work.
            write_selection(&path, &new_sel)?;
        }
        Ok(())
    })
    .await
    .map_err(|_| "Something went wrong on this device. Please try again.".to_string())?
}

/// Latch `committed` on the user's first chat with the active model — after
/// this, tier changes are limited to once per billing period. Idempotent.
#[tauri::command]
pub async fn mark_tier_committed(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut sel = read_app_selection(&app);
        if sel.committed {
            return Ok(());
        }
        sel.committed = true;
        let path = selection_path(&app).ok_or_else(|| "no app-data dir".to_string())?;
        write_selection(&path, &sel)
    })
    .await
    .map_err(|_| "Something went wrong on this device. Please try again.".to_string())?
}

/// The effective tier under a candidate `mode`, reusing the rest of `sel`
/// (only `mode` changes). Pulled out so the switch commands compute the target
/// the same way [`effective_tier_from`] does for the live selection.
fn mode_effective_tier(mode: &str, sel: &TierSelection, detected: &str) -> String {
    let candidate = TierSelection {
        mode: mode.to_string(),
        ..sel.clone()
    };
    effective_tier_from(&candidate, detected)
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
    fn no_resolvable_period_is_lenient_not_blocked() {
        // A missing/unresolvable entitlement must NOT gray out the selector —
        // the limit curbs repetitive flipping, it is not access control.
        assert_eq!(switch_allowed(false, &HeroEntitlement::None, None, None, 0), Ok(()));
        assert_eq!(switch_allowed(true, &HeroEntitlement::None, None, None, 0), Ok(()));
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

    // --- resolve_hero_entitlement -------------------------------------------

    fn ent(model: &str, expires: Option<&str>) -> Entitlement {
        Entitlement {
            model_id: model.to_string(),
            source: "purchase".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            expires_at: expires.map(str::to_string),
        }
    }

    fn now_at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn entitlement_period_when_active_purchase() {
        let ents = vec![ent(HERO_MODEL_ID, Some("2026-08-21T00:00:00Z"))];
        assert_eq!(
            resolve_hero_entitlement(&ents, now_at("2026-07-21T00:00:00Z")),
            HeroEntitlement::Period("2026-08-21T00:00:00Z".to_string())
        );
    }

    #[test]
    fn entitlement_none_when_expired() {
        let ents = vec![ent(HERO_MODEL_ID, Some("2026-07-01T00:00:00Z"))];
        assert_eq!(
            resolve_hero_entitlement(&ents, now_at("2026-07-21T00:00:00Z")),
            HeroEntitlement::None
        );
    }

    #[test]
    fn entitlement_perpetual_when_no_expiry() {
        let ents = vec![ent(HERO_MODEL_ID, None)];
        assert_eq!(
            resolve_hero_entitlement(&ents, now_at("2026-07-21T00:00:00Z")),
            HeroEntitlement::Perpetual
        );
    }

    #[test]
    fn entitlement_prefers_latest_live_period_over_perpetual_and_other_models() {
        let ents = vec![
            ent("other-model", Some("2027-01-01T00:00:00Z")), // not hero → ignored
            ent(HERO_MODEL_ID, None),                         // perpetual grant
            ent(HERO_MODEL_ID, Some("2026-08-21T00:00:00Z")), // live period
            ent(HERO_MODEL_ID, Some("2026-09-21T00:00:00Z")), // later live period wins
            ent(HERO_MODEL_ID, Some("2026-06-01T00:00:00Z")), // expired → ignored
        ];
        assert_eq!(
            resolve_hero_entitlement(&ents, now_at("2026-07-21T00:00:00Z")),
            HeroEntitlement::Period("2026-09-21T00:00:00Z".to_string())
        );
    }

    #[test]
    fn entitlement_none_for_other_model_only() {
        let ents = vec![ent("other", Some("2027-01-01T00:00:00Z"))];
        assert_eq!(
            resolve_hero_entitlement(&ents, now_at("2026-07-21T00:00:00Z")),
            HeroEntitlement::None
        );
    }
}
