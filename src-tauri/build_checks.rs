//! Build-time checks `build.rs` runs, kept in their own file so the crate's
//! own test suite can compile them too (`lib.rs` includes this file under
//! `#[cfg(test)]`). Pure: no I/O, no environment.
//!
//! Phase 1h whole-branch review I1/I2: a triage build must embed every
//! reference pack its catalog names, and the embedded pack's sha256 must be
//! the catalog's pin — otherwise the lookup would run on no pack (a visible
//! button over nothing) or record provenance for bytes it did not serve.

/// Check the triage catalog (`resources/catalog.triage.json`, as JSON text)
/// against what the build embeds: `embedded_id` is the one pack id the build
/// can embed, and `embedded_sha256` is `Some(lowercase hex sha256 of the
/// .kpack)` when both it and its `.sig` are present, else `None`. Every
/// entry with a non-null `referencePack` must name `embedded_id`, that pack
/// must be embedded, and its `sha256` pin must equal the embedded bytes'.
/// `Err` carries the message the build panics with.
#[allow(dead_code)] // the crate's tests include this file; only build.rs calls it
pub fn check_triage_reference_packs(
    catalog_json: &str,
    embedded_id: &str,
    embedded_sha256: Option<&str>,
) -> Result<(), String> {
    let catalog: serde_json::Value = serde_json::from_str(catalog_json)
        .map_err(|e| format!("catalog.triage.json is not valid JSON: {e}"))?;
    let entries = catalog
        .as_array()
        .ok_or_else(|| "catalog.triage.json must be a JSON array of entries".to_string())?;
    for entry in entries {
        let Some(pack) = entry.get("referencePack").filter(|p| !p.is_null()) else {
            continue;
        };
        let entry_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("<no id>");
        let pack_id = pack
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("catalog entry {entry_id}: referencePack has no string id"))?;
        if pack_id != embedded_id {
            return Err(format!(
                "catalog entry {entry_id} names referencePack {pack_id}, but this build can embed only \
                 {embedded_id}: the lookup would find no bundled pack"
            ));
        }
        let Some(actual) = embedded_sha256 else {
            return Err(format!(
                "catalog entry {entry_id} names referencePack {pack_id}, but \
                 src-tauri/resources/packs/{pack_id}.kpack and its .sig are not both present: \
                 copy the signed pair there before a triage build \
                 (docs/superpowers/mobile-tools/build-reference-pack.md)"
            ));
        };
        let pinned = pack
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("catalog entry {entry_id}: referencePack has no string sha256"))?;
        if !pinned.eq_ignore_ascii_case(actual) {
            return Err(format!(
                "catalog entry {entry_id}: the embedded {pack_id}.kpack has sha256 {actual}, but the \
                 catalog pins {pinned}: embed the pinned pack or update the pin"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_triage_reference_packs as check;

    const SHA: &str = "5c7b2c98337118ecd8a6fbd07887a639be81371b4e325504997768b41cff1853";

    fn catalog(pack: &str) -> String {
        format!(r#"[{{"id":"med-triage","referencePack":{pack}}},{{"id":"socratic-tutor"}}]"#)
    }

    #[test]
    fn a_catalog_whose_pack_is_embedded_with_the_pinned_sha_passes() {
        let c = catalog(&format!(r#"{{"id":"reference-uk-v1","sha256":"{SHA}"}}"#));
        assert_eq!(check(&c, "reference-uk-v1", Some(SHA)), Ok(()));
    }

    #[test]
    fn a_catalog_naming_no_pack_passes_with_or_without_one_embedded() {
        let c = r#"[{"id":"med-triage","referencePack":null},{"id":"socratic-tutor"}]"#;
        assert_eq!(check(c, "reference-uk-v1", None), Ok(()));
        assert_eq!(check(c, "reference-uk-v1", Some(SHA)), Ok(()));
    }

    #[test]
    fn a_named_pack_that_is_not_embedded_fails() {
        let c = catalog(&format!(r#"{{"id":"reference-uk-v1","sha256":"{SHA}"}}"#));
        let err = check(&c, "reference-uk-v1", None).unwrap_err();
        assert!(err.contains("not both present"), "{err}");
    }

    #[test]
    fn a_named_pack_the_build_cannot_embed_fails() {
        let c = catalog(&format!(r#"{{"id":"reference-uk-v2","sha256":"{SHA}"}}"#));
        let err = check(&c, "reference-uk-v1", Some(SHA)).unwrap_err();
        assert!(err.contains("can embed only reference-uk-v1"), "{err}");
    }

    #[test]
    fn an_embedded_pack_whose_sha_differs_from_the_pin_fails() {
        let c = catalog(&format!(r#"{{"id":"reference-uk-v1","sha256":"{SHA}"}}"#));
        let err = check(&c, "reference-uk-v1", Some(&"0".repeat(64))).unwrap_err();
        assert!(err.contains("catalog pins"), "{err}");
    }

    #[test]
    fn a_malformed_catalog_or_pack_record_fails() {
        assert!(check("{}", "reference-uk-v1", Some(SHA)).is_err());
        assert!(check("not json", "reference-uk-v1", Some(SHA)).is_err());
        assert!(check(&catalog(r#"{"sha256":"x"}"#), "reference-uk-v1", Some(SHA)).is_err());
        assert!(check(&catalog(r#"{"id":"reference-uk-v1"}"#), "reference-uk-v1", Some(SHA)).is_err());
    }

    /// The committed triage catalog names the one pack this build embeds.
    #[test]
    fn the_committed_triage_catalog_names_the_embeddable_pack() {
        let c = include_str!("resources/catalog.triage.json");
        let err = check(c, "reference-uk-v1", None).unwrap_err();
        assert!(err.contains("reference-uk-v1"), "{err}");
        assert_eq!(check(c, "reference-uk-v1", Some(SHA)), Ok(()));
    }
}
