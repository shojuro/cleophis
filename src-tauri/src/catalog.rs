use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    pub subject: String,
    pub cover: String,
    pub size_params: String,
    pub quant: String,
    pub file_bytes: u64,
    #[serde(default)]
    pub model_file: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    /// The LoRA adapter loaded alongside the base via llama.cpp `--lora`
    /// (never merged). When set, the hero is treated as not-installed until
    /// this file is present too — see `inference::resolve_launch`.
    #[serde(default)]
    pub adapter_file: Option<String>,
    /// Load-time integrity hash for `adapter_file`, checked exactly like
    /// `sha256` is for the base (see `inference::verify_adapter_once`).
    #[serde(default)]
    pub adapter_sha256: Option<String>,
    /// Stamped onto new chats' `adapter_ids` as provenance when this hero
    /// declares an always-on adapter.
    #[serde(default)]
    pub adapter_id: Option<String>,
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub chat_template: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub greeting: Option<String>,
    pub blurb: String,
    #[serde(default)]
    pub real: bool,
    #[serde(default)]
    pub price: Option<String>,
    #[serde(default)]
    pub pro: bool,
    #[serde(default)]
    pub long: Option<String>,
    #[serde(default)]
    pub inside: Vec<String>,
    #[serde(default)]
    pub tps: Option<String>,
    #[serde(default)]
    pub eval: Option<String>,
    /// Per-device-tier base+adapter variants (wrapper tier-selection). When
    /// present, the launch/verify/download paths resolve the base + adapter
    /// through [`hero_variant`] keyed off the effective device tier, instead
    /// of the flat `model_file`/`sha256`/… fields above. The flat fields stay
    /// mirrored to the `mid` variant for back-compat with any reader that
    /// hasn't moved to `hero_variant` yet. Absent on non-hero entries.
    #[serde(default)]
    pub tiers: Option<Tiers>,
}

/// One device tier's fully-pinned base+adapter identity — everything a launch
/// or an integrity check needs, self-contained so the app knows all three
/// tiers offline without a network fetch. Hashes here are cross-checked
/// against the signed dist catalog at download time (that catalog stays the
/// download + hash authority; this is the launch-time mirror).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TierVariant {
    /// Dist-catalog `base_model` label (e.g. `"Qwen3-4B"`) — the key the FE
    /// picks the `base`/`adapter` artifacts by out of `fetch_dist_catalog`.
    pub base_model: String,
    pub size_params: String,
    pub model_file: String,
    pub sha256: String,
    pub adapter_file: String,
    pub adapter_sha256: String,
    pub adapter_id: String,
    /// The contract-grounding (adapter v2) LoRA, composed onto the base
    /// ALONGSIDE `adapter_file` via a second `--lora` (static composition).
    /// `Option` — absent until adapter v2 ships for this tier; when present,
    /// all three files (base + behavioral + contract) must be on disk to
    /// launch (see `inference::resolve_launch`).
    #[serde(default)]
    pub contract_adapter_file: Option<String>,
    #[serde(default)]
    pub contract_adapter_sha256: Option<String>,
    #[serde(default)]
    pub contract_adapter_id: Option<String>,
    pub file_bytes: u64,
}

/// The three device tiers the wrapper selects between: `low` → 1B, `mid` →
/// 4B, `high` → 8B (matching `hardware::tier_for`'s `"low"/"mid"/"high"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tiers {
    pub low: TierVariant,
    pub mid: TierVariant,
    pub high: TierVariant,
}

impl Tiers {
    /// The variant for a `hardware::detect().tier` string. `mid` is the
    /// default for any unrecognized tier (defensive — `detect()` only ever
    /// returns one of the three).
    pub fn get(&self, tier: &str) -> &TierVariant {
        match tier {
            "low" => &self.low,
            "high" => &self.high,
            _ => &self.mid,
        }
    }
}

/// The hero's resolved base+adapter identity for a given device `tier`,
/// unifying the two sources: when the entry carries a `tiers` block the
/// requested tier's [`TierVariant`] is projected out (all fields `Some`);
/// otherwise it falls back to the entry's flat fields (pre-tiers fixtures /
/// non-hero entries), so callers behave exactly as before tier-selection
/// existed. Every field is `Option` because the flat-field fallback may pin
/// none of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedHero {
    pub base_model: Option<String>,
    pub size_params: Option<String>,
    pub model_file: Option<String>,
    pub sha256: Option<String>,
    pub adapter_file: Option<String>,
    pub adapter_sha256: Option<String>,
    pub adapter_id: Option<String>,
    /// The contract-grounding (adapter v2) LoRA for this tier, composed
    /// alongside `adapter_file`. `None` on the flat-field fallback and until
    /// v2 ships.
    pub contract_adapter_file: Option<String>,
    pub contract_adapter_sha256: Option<String>,
    pub contract_adapter_id: Option<String>,
    pub file_bytes: Option<u64>,
}

/// Resolve `entry`'s base+adapter identity for device `tier`. Uses the
/// `tiers` block when present (keyed by tier, `mid`-default), else the flat
/// fields — see [`ResolvedHero`].
pub fn hero_variant(entry: &CatalogEntry, tier: &str) -> ResolvedHero {
    if let Some(tiers) = &entry.tiers {
        let v = tiers.get(tier);
        ResolvedHero {
            base_model: Some(v.base_model.clone()),
            size_params: Some(v.size_params.clone()),
            model_file: Some(v.model_file.clone()),
            sha256: Some(v.sha256.clone()),
            adapter_file: Some(v.adapter_file.clone()),
            adapter_sha256: Some(v.adapter_sha256.clone()),
            adapter_id: Some(v.adapter_id.clone()),
            contract_adapter_file: v.contract_adapter_file.clone(),
            contract_adapter_sha256: v.contract_adapter_sha256.clone(),
            contract_adapter_id: v.contract_adapter_id.clone(),
            file_bytes: Some(v.file_bytes),
        }
    } else {
        ResolvedHero {
            base_model: None,
            size_params: Some(entry.size_params.clone()),
            model_file: entry.model_file.clone(),
            sha256: entry.sha256.clone(),
            adapter_file: entry.adapter_file.clone(),
            adapter_sha256: entry.adapter_sha256.clone(),
            adapter_id: entry.adapter_id.clone(),
            contract_adapter_file: None,
            contract_adapter_sha256: None,
            contract_adapter_id: None,
            file_bytes: Some(entry.file_bytes),
        }
    }
}

pub fn parse_catalog(json: &str) -> Result<Vec<CatalogEntry>, String> {
    serde_json::from_str(json).map_err(|e| format!("catalog.json invalid: {e}"))
}

pub fn hero(entries: &[CatalogEntry]) -> Option<&CatalogEntry> {
    entries.iter().find(|e| e.real && e.model_file.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"id":"a","name":"A","category":"education","subject":"Math","cover":"covers/a.webp",
       "sizeParams":"3B","quant":"Q4_K_M","fileBytes":100,"modelFile":"models/a.gguf",
       "systemPrompt":"sp","greeting":"hi","blurb":"b","real":true},
      {"id":"b","name":"B","category":"medical","subject":"Reference","cover":"covers/b.webp",
       "sizeParams":"7B","quant":"Q4_K_M","fileBytes":200,"blurb":"b2","real":false}
    ]"#;

    #[test]
    fn parses_sample() {
        let v = parse_catalog(SAMPLE).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "a");
        assert_eq!(v[0].file_bytes, 100);
        assert!(v[0].real);
        assert_eq!(v[1].model_file, None);
    }

    #[test]
    fn hero_is_first_real_with_model() {
        let v = parse_catalog(SAMPLE).unwrap();
        assert_eq!(hero(&v).unwrap().id, "a");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_catalog("not json").is_err());
    }

    #[test]
    fn real_catalog_file_parses_and_has_hero() {
        let raw = include_str!("../resources/catalog.json");
        let v = parse_catalog(raw).unwrap();
        assert_eq!(v.len(), 11, "expected 11 catalog entries");
        let h = hero(&v).expect("catalog must contain the hero model");
        assert_eq!(h.id, "socratic-tutor");
        assert!(h.system_prompt.is_some() && h.greeting.is_some());
        assert!(
            h.sha256.as_deref().map(|s| s.len() == 64).unwrap_or(false),
            "hero must carry a 64-hex sha256"
        );
        // B4: the hero repoints to Qwen3-4B base + its always-on behavioral
        // adapter. The entry now DECLARES an adapter (file + 64-hex hash +
        // id) and bumps to version 2.
        assert_eq!(h.adapter_id.as_deref(), Some("behavioral-v1-qwen3-4b"));
        assert!(h.adapter_file.is_some(), "hero must declare an adapter file");
        assert!(
            h.adapter_sha256.as_deref().map(|s| s.len() == 64).unwrap_or(false),
            "hero must carry a 64-hex adapter sha256"
        );
        assert_eq!(h.version, Some(2));
    }

    #[test]
    fn hero_carries_all_three_tier_variants_with_pinned_hashes() {
        let raw = include_str!("../resources/catalog.json");
        let v = parse_catalog(raw).unwrap();
        let h = hero(&v).expect("catalog must contain the hero model");
        let tiers = h.tiers.as_ref().expect("hero must declare a tiers block");
        for (tier, expect_base) in [
            (&tiers.low, "Llama-3.2-1B"),
            (&tiers.mid, "Qwen3-4B"),
            (&tiers.high, "Qwen3-8B"),
        ] {
            assert_eq!(tier.base_model, expect_base);
            assert_eq!(tier.sha256.len(), 64, "{expect_base} base sha256 must be 64-hex");
            assert_eq!(tier.adapter_sha256.len(), 64, "{expect_base} adapter sha256 must be 64-hex");
            assert!(tier.model_file.ends_with(".gguf"));
            assert!(tier.adapter_file.ends_with(".gguf"));
            assert!(tier.file_bytes > 0);
        }
        // The flat fields stay mirrored to the mid variant for back-compat.
        assert_eq!(h.model_file.as_deref(), Some(tiers.mid.model_file.as_str()));
        assert_eq!(h.sha256.as_deref(), Some(tiers.mid.sha256.as_str()));
        assert_eq!(h.adapter_sha256.as_deref(), Some(tiers.mid.adapter_sha256.as_str()));
    }

    #[test]
    fn hero_variant_selects_per_tier_and_defaults_mid() {
        let raw = include_str!("../resources/catalog.json");
        let v = parse_catalog(raw).unwrap();
        let h = hero(&v).unwrap();
        assert_eq!(hero_variant(h, "low").base_model.as_deref(), Some("Llama-3.2-1B"));
        assert_eq!(hero_variant(h, "mid").base_model.as_deref(), Some("Qwen3-4B"));
        assert_eq!(hero_variant(h, "high").base_model.as_deref(), Some("Qwen3-8B"));
        // Unknown tier defends to mid.
        assert_eq!(hero_variant(h, "banana").base_model.as_deref(), Some("Qwen3-4B"));
    }

    #[test]
    fn hero_variant_falls_back_to_flat_fields_without_tiers() {
        // A pre-tiers entry (no `tiers` block) still resolves via flat fields.
        let v = parse_catalog(SAMPLE).unwrap();
        let entry = &v[0];
        assert!(entry.tiers.is_none());
        let resolved = hero_variant(entry, "mid");
        assert_eq!(resolved.model_file.as_deref(), Some("models/a.gguf"));
        assert_eq!(resolved.base_model, None); // flat entries pin no base_model
    }
}
