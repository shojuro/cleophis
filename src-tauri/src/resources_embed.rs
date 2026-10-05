//! Android resource materializer (spec §4 / brief architecture item 4).
//!
//! Desktop ships `resources/` next to the installed binary and reads it in
//! place. Android has no such directory: the APK deliberately bundles **no**
//! resources at all (`bundle.resources = null` in `tauri.android.conf.json` —
//! it must be `null`, not `{}`, see that file's history), because the desktop
//! resource map is full of Windows `.dll`/`.exe` that have no business in an
//! ARM APK.
//!
//! So the two things mobile genuinely needs — the catalog and its cover art,
//! ~250 KB total — are compiled into the binary with `include_bytes!` and
//! written out to `<app_data>/resources` on first launch. From that point on
//! `inference::resources_root` returns that directory and **every consumer
//! (`catalog.json` readers, `get_catalog`'s `coverAbs`, `tier_select`) works
//! unchanged.**
//!
//! Only the models themselves are downloaded; they already land in
//! `<app_data>/models` on both platforms and are not touched here.
//!
//! On desktop this module compiles down to just `COVER_FILES` and the tests —
//! no cover bytes are embedded, so the desktop binary is unaffected.
//!
//! Phase 1h M5: the triage variant also embeds the signed reference pack
//! (`resources/packs/reference-uk-v1.kpack` + `.kpack.sig`) when `build.rs`
//! found both at build time (`cfg(cleophis_reference_pack)`), and writes
//! them to `<app_data>/resources/packs/` — the read-only bundled pack root
//! `kpack::rag_lookup` reads. A build without the pack removes any copy a
//! previous (triage) install left there. Integrity is not this module's job:
//! `Pack::mount_lexical` verifies the curator signature on every lookup.

#[cfg(mobile)]
use std::path::PathBuf;
#[cfg(mobile)]
use tauri::{AppHandle, Manager};

/// The single source of truth for which covers ship with the app.
///
/// Declaring them once and deriving both the name list and the byte array from
/// it is what keeps the two in the same order — a hand-maintained second array
/// could silently pair `abx.webp`'s name with `ecg.webp`'s pixels, which no
/// test would notice and every user would.
macro_rules! embedded_covers {
    ($($file:literal),+ $(,)?) => {
        /// Cover filenames, relative to `resources/covers/`. Compiled on every
        /// platform so the desktop test suite can police the list — on desktop
        /// that is its only job, since desktop reads `resources/covers/`
        /// directly off disk.
        #[cfg_attr(not(mobile), allow(dead_code))]
        pub(crate) const COVER_FILES: &[&str] = &[$($file),+];

        #[cfg(mobile)]
        const COVER_BYTES: &[&[u8]] = &[
            $(include_bytes!(concat!("../resources/covers/", $file))),+
        ];
    };
}

embedded_covers!(
    "abx.webp",
    "anticoag.webp",
    "ap-bio.webp",
    "civil-war.webp",
    "drug-interactions.webp",
    "ecg.webp",
    "intro-python.webp",
    "med-triage.webp",
    "neurosurg.webp",
    "sat-prep.webp",
    "socratic-tutor.webp",
    "spanish.webp",
);

#[cfg(mobile)]
const CATALOG_JSON: &[u8] = include_bytes!("../resources/catalog.json");

/// The bundled reference pack's file names, relative to `resources/packs/`
/// (mirrors `build.rs`; the signature name is what `Pack::mount` reads).
#[cfg_attr(not(mobile), allow(dead_code))]
pub(crate) const REFERENCE_PACK_FILES: [&str; 2] =
    ["reference-uk-v1.kpack", "reference-uk-v1.kpack.sig"];

/// `(pack bytes, signature bytes, sha256 of the pack)` — `Some` only in the
/// triage build that had the pack at build time (see `build.rs`).
#[cfg(all(mobile, cleophis_reference_pack))]
const REFERENCE_PACK: Option<(&[u8], &[u8], &str)> = Some((
    include_bytes!("../resources/packs/reference-uk-v1.kpack") as &[u8],
    include_bytes!("../resources/packs/reference-uk-v1.kpack.sig") as &[u8],
    env!("CLEOPHIS_REFERENCE_PACK_SHA256"),
));
#[cfg(all(mobile, not(cleophis_reference_pack)))]
const REFERENCE_PACK: Option<(&[u8], &[u8], &str)> = None;

/// Records which payload the materialized tree came from. Written last, so an
/// interrupted materialization leaves no stamp and simply redoes itself.
#[cfg(mobile)]
const STAMP_FILE: &str = ".embedded-version";

/// Where the embedded resources are materialized to. This is what
/// `inference::resources_root` returns on Android.
#[cfg(mobile)]
pub(crate) fn materialized_root(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("no app data dir")
        .join("resources")
}

/// sha256 over the whole embedded payload — catalog plus every cover, names
/// included. Any edit to any of them changes this, which is what makes an app
/// update rewrite the materialized tree instead of leaving last version's
/// catalog in place.
#[cfg(mobile)]
fn payload_stamp() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(CATALOG_JSON);
    for (name, bytes) in COVER_FILES.iter().zip(COVER_BYTES) {
        h.update(name.as_bytes());
        h.update(bytes);
    }
    // The pack by its build-time hash (not re-hashed here: it can be tens of
    // MB), plus its signature. Absent vs present changes the stamp too, so a
    // tutor build over a triage install rewrites the tree (and removes it).
    match REFERENCE_PACK {
        Some((_, sig, sha)) => {
            h.update(b"reference-pack");
            h.update(sha.as_bytes());
            h.update(sig);
        }
        None => h.update(b"no-reference-pack"),
    }
    format!("{:x}", h.finalize())
}

/// Write the embedded catalog + covers to `<app_data>/resources`, unless a
/// previous run already wrote this exact payload.
///
/// Must run in `setup` **before** anything calls `inference::resources_root` —
/// `model_path`, `resolve_launch` and `get_catalog` all read through it.
#[cfg(mobile)]
pub(crate) fn materialize(app: &AppHandle) -> std::io::Result<()> {
    let root = materialized_root(app);
    let stamp = payload_stamp();
    let stamp_path = root.join(STAMP_FILE);

    if std::fs::read_to_string(&stamp_path)
        .map(|s| s.trim() == stamp)
        .unwrap_or(false)
    {
        return Ok(());
    }

    let covers = root.join("covers");
    std::fs::create_dir_all(&covers)?;
    std::fs::write(root.join("catalog.json"), CATALOG_JSON)?;
    for (name, bytes) in COVER_FILES.iter().zip(COVER_BYTES) {
        std::fs::write(covers.join(name), bytes)?;
    }
    let packs = root.join("packs");
    match REFERENCE_PACK {
        Some((pack, sig, _)) => {
            std::fs::create_dir_all(&packs)?;
            for (name, bytes) in REFERENCE_PACK_FILES.iter().zip([pack, sig]) {
                // Via a `.part` + rename, so a killed write never leaves a
                // truncated pack under the final name.
                let part = packs.join(format!("{name}.part"));
                std::fs::write(&part, bytes)?;
                std::fs::rename(&part, packs.join(name))?;
            }
        }
        None => {
            for name in REFERENCE_PACK_FILES {
                match std::fs::remove_file(packs.join(name)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
        }
    }
    // Last, and only once every file above landed: a stamp present means the
    // tree beside it is complete.
    std::fs::write(&stamp_path, stamp)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::COVER_FILES;
    use std::path::PathBuf;

    fn resources_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")
    }

    /// Every cover on disk is embedded, and every embedded cover exists. Without
    /// this, adding a cover to `resources/covers/` and to the catalog but not to
    /// `embedded_covers!` ships an Android build with a broken image — and the
    /// desktop build, which reads the directory directly, would look fine.
    #[test]
    fn embedded_cover_list_matches_the_resources_directory() {
        let dir = resources_dir().join("covers");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("resources/covers must exist")
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();

        let mut embedded: Vec<String> = COVER_FILES.iter().map(|s| s.to_string()).collect();
        embedded.sort();

        assert_eq!(
            embedded, on_disk,
            "embedded_covers! is out of sync with resources/covers/"
        );
    }

    /// The catalog is the thing that actually names covers at runtime, so it —
    /// not the directory — is the binding contract: a `cover` the mobile build
    /// cannot produce is a broken card in the library.
    ///
    /// BOTH catalogs, since P2.9. `--variant=triage` swaps
    /// `catalog.triage.json` in as `catalog.json` at build time, so a cover
    /// named only there is exactly as capable of shipping broken — and only
    /// on the build nobody has a desktop version of to notice it on.
    #[test]
    fn every_catalog_cover_is_embedded() {
        for name in ["catalog.json", "catalog.triage.json"] {
            let raw = std::fs::read_to_string(resources_dir().join(name))
                .unwrap_or_else(|_| panic!("resources/{name} must exist"));
            let entries = crate::catalog::parse_catalog(&raw).expect("catalog must parse");
            assert_covers_embedded(name, &entries);
        }
    }

    /// `build.rs` decides whether to embed by these same two names; if they
    /// drift, the cfg would name files `include_bytes!` never reads.
    #[test]
    fn reference_pack_names_match_build_rs() {
        let build_rs = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("build.rs"),
        )
        .expect("build.rs");
        for name in super::REFERENCE_PACK_FILES {
            assert!(build_rs.contains(&format!("\"{name}\"")), "build.rs must name {name}");
        }
        assert_eq!(
            super::REFERENCE_PACK_FILES[1],
            format!("{}.sig", super::REFERENCE_PACK_FILES[0]),
            "the signature is <pack>.sig, the name Pack::mount reads"
        );
    }

    fn assert_covers_embedded(catalog: &str, entries: &[crate::catalog::CatalogEntry]) {
        for e in entries {
            let file = e
                .cover
                .rsplit('/')
                .next()
                .expect("cover path must have a filename");
            assert!(
                COVER_FILES.contains(&file),
                "{} entry {:?} references cover {:?}, which is not embedded \
                 — add it to embedded_covers! or the Android build ships a broken image",
                catalog,
                e.id,
                e.cover
            );
            assert!(
                e.cover.starts_with("covers/"),
                "{} entry {:?} has cover {:?} outside covers/ — the mobile \
                 materializer only writes covers/",
                catalog,
                e.id,
                e.cover
            );
        }
    }
}
