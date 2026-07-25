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
    "neurosurg.webp",
    "sat-prep.webp",
    "socratic-tutor.webp",
    "spanish.webp",
);

#[cfg(mobile)]
const CATALOG_JSON: &[u8] = include_bytes!("../resources/catalog.json");

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
    #[test]
    fn every_catalog_cover_is_embedded() {
        let raw = std::fs::read_to_string(resources_dir().join("catalog.json"))
            .expect("resources/catalog.json must exist");
        let entries = crate::catalog::parse_catalog(&raw).expect("catalog must parse");

        for e in &entries {
            let file = e
                .cover
                .rsplit('/')
                .next()
                .expect("cover path must have a filename");
            assert!(
                COVER_FILES.contains(&file),
                "catalog entry {:?} references cover {:?}, which is not embedded \
                 — add it to embedded_covers! or the Android build ships a broken image",
                e.id,
                e.cover
            );
            assert!(
                e.cover.starts_with("covers/"),
                "catalog entry {:?} has cover {:?} outside covers/ — the mobile \
                 materializer only writes covers/",
                e.id,
                e.cover
            );
        }
    }
}
