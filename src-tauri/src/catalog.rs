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
        assert_eq!(h.version, Some(1));
    }
}
