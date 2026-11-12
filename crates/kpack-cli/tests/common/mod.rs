//! Helpers shared by the kpack-cli integration tests.
#![allow(dead_code)]

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn repo() -> PathBuf {
    manifest_dir().join("..").join("..")
}

pub fn unique_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kpack-cli-test-{}-{nanos}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run the built `kpack-cli` binary. `epoch` sets (or, when `None`,
/// removes) `SOURCE_DATE_EPOCH`.
pub fn run_cli(args: &[&str], epoch: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kpack-cli"));
    cmd.args(args);
    match epoch {
        Some(e) => cmd.env("SOURCE_DATE_EPOCH", e),
        None => cmd.env_remove("SOURCE_DATE_EPOCH"),
    };
    cmd.output().expect("run kpack-cli")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A throwaway curator key (fixed seed) — never the production key.
pub fn test_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

/// Write `<pack>.sig` (the detached signature `Pack::mount` reads) and
/// return the verifying key to put in the `LoadContext`.
pub fn sign(pack: &Path) -> VerifyingKey {
    let key = test_key();
    let sig = key.sign(&std::fs::read(pack).unwrap());
    let mut p = pack.as_os_str().to_os_string();
    p.push(".sig");
    std::fs::write(PathBuf::from(p), sig.to_bytes()).unwrap();
    key.verifying_key()
}
