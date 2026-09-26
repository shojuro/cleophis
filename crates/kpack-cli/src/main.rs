//! `kpack-cli` — see the library doc (`src/lib.rs`) for the full contract.
//!
//! ```text
//! SOURCE_DATE_EPOCH=<secs> kpack-cli build-reference --corpus DIR --clusters FILE --out DIR
//!                                  [--pack-id ID] [--pack-version V]
//! kpack-cli verify --corpus DIR --clusters FILE --dir DIR
//! ```

use kpack_cli::{build_reference, verify, Options, DEFAULT_PACK_ID, DEFAULT_PACK_VERSION};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  SOURCE_DATE_EPOCH=<secs> kpack-cli build-reference --corpus DIR --clusters FILE --out DIR [--pack-id ID] [--pack-version V]
  kpack-cli verify --corpus DIR --clusters FILE --dir DIR";

/// `--flag value` pairs after the subcommand; unknown or repeated flags and
/// a flag without a value are errors.
fn flags(args: &[String], allowed: &[&str]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let name = flag.strip_prefix("--").filter(|n| allowed.contains(n)).ok_or_else(|| format!("unknown argument {flag:?}"))?;
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if out.insert(name.to_string(), value.clone()).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    Ok(out)
}

fn required(f: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    f.get(name).map(PathBuf::from).ok_or_else(|| format!("--{name} is required"))
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or("no subcommand")?;
    match cmd.as_str() {
        "build-reference" => {
            let f = flags(rest, &["corpus", "clusters", "out", "pack-id", "pack-version"])?;
            let epoch = std::env::var("SOURCE_DATE_EPOCH")
                .map_err(|_| "SOURCE_DATE_EPOCH must be set: every timestamp in the pack derives from it".to_string())?;
            let source_date_epoch: u64 =
                epoch.trim().parse().map_err(|_| format!("SOURCE_DATE_EPOCH {epoch:?} is not a whole number of seconds"))?;
            let opts = Options {
                pack_id: f.get("pack-id").cloned().unwrap_or_else(|| DEFAULT_PACK_ID.to_string()),
                pack_version: f.get("pack-version").cloned().unwrap_or_else(|| DEFAULT_PACK_VERSION.to_string()),
                source_date_epoch,
            };
            let s = build_reference(&required(&f, "corpus")?, &required(&f, "clusters")?, &required(&f, "out")?, &opts)
                .map_err(|e| e.to_string())?;
            println!("pack            {}", s.pack_path.display());
            println!("pack sha256     {}", s.pack_sha256);
            println!("content sha256  {}", s.content_sha256);
            println!("docs            {}", s.docs);
            println!("chunks          {}", s.chunks);
            Ok(())
        }
        "verify" => {
            let f = flags(rest, &["corpus", "clusters", "dir"])?;
            let r = verify(&required(&f, "corpus")?, &required(&f, "clusters")?, &required(&f, "dir")?)
                .map_err(|e| e.to_string())?;
            println!("content sha256  {}", r.content_sha256);
            println!("pack sha256     {}", r.pack_sha256);
            println!("pack compared   {}", if r.pack_compared { "yes" } else { "no (not present)" });
            if r.mismatches.is_empty() {
                println!("OK: a fresh build is byte-identical");
                Ok(())
            } else {
                Err(format!("verify FAILED:\n  {}", r.mismatches.join("\n  ")))
            }
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown subcommand {other:?}")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
