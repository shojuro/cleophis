use std::path::{Path, PathBuf};

fn main() {
    reference_pack_cfg();
    tauri_build::build()
}

/// The bundled reference pack's file names under `resources/packs/`. The
/// signature is `<pack>.sig`, the name `Pack::mount` reads beside the pack.
const REFERENCE_PACK: &str = "reference-uk-v1.kpack";
const REFERENCE_PACK_SIG: &str = "reference-uk-v1.kpack.sig";

/// Phase 1h M5: emit `cfg(cleophis_reference_pack)` — which makes
/// `resources_embed` `include_bytes!` the pack and its signature into the
/// Android binary — only when BOTH files exist AND this is the triage
/// variant (`CLEOPHIS_VARIANT=triage`, exported by
/// `build-android-apk.sh --variant=triage`). The tutor build, and any tree
/// without the pack, compile exactly as before. Also exports the pack's
/// sha256 as `CLEOPHIS_REFERENCE_PACK_SHA256` for the materializer's stamp.
fn reference_pack_cfg() {
    println!("cargo:rustc-check-cfg=cfg(cleophis_reference_pack)");
    println!("cargo:rerun-if-env-changed=CLEOPHIS_VARIANT");
    let resources = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("resources");
    let dir = resources.join("packs");
    // Watch the packs dir when it exists (a pack added/replaced reruns this),
    // else `resources/` (so creating `packs/` does). Never a missing path:
    // cargo treats one as always-stale and would rerun this every build.
    let watched: &Path = if dir.is_dir() { &dir } else { &resources };
    println!("cargo:rerun-if-changed={}", watched.display());

    let pack = dir.join(REFERENCE_PACK);
    let sig = dir.join(REFERENCE_PACK_SIG);
    let triage = std::env::var("CLEOPHIS_VARIANT").as_deref() == Ok("triage");
    match (triage, pack.is_file(), sig.is_file()) {
        (true, true, true) => {
            use sha2::{Digest, Sha256};
            let bytes = std::fs::read(&pack).expect("read the reference pack");
            let digest = format!("{:x}", Sha256::digest(&bytes));
            println!("cargo:rustc-cfg=cleophis_reference_pack");
            println!("cargo:rustc-env=CLEOPHIS_REFERENCE_PACK_SHA256={digest}");
        }
        (true, _, _) => println!(
            "cargo:warning=triage build without resources/packs/{REFERENCE_PACK} and its .sig: \
             the reference lookup will find no bundled pack"
        ),
        _ => {}
    }
}
