//! `kpack-cli` — see the library doc (`src/lib.rs`) for the full contract.
//!
//! ```text
//! SOURCE_DATE_EPOCH=<secs> kpack-cli build-reference --corpus DIR --clusters FILE --out DIR
//!                                  [--pack-id ID] [--pack-version V]
//! kpack-cli verify --corpus DIR --clusters FILE --dir DIR
//! kpack-cli lookup --pack FILE (--query TEXT | --batch FILE) [--k N] [--json] [--curator-key HEX]
//! ```

use kpack_cli::lookup::{parse_batch_line, parse_public_key_hex, LookupPack, DEFAULT_LOOKUP_K};
use kpack_cli::{build_reference, verify, Options, DEFAULT_PACK_ID, DEFAULT_PACK_VERSION};
use std::io::Write;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  SOURCE_DATE_EPOCH=<secs> kpack-cli build-reference --corpus DIR --clusters FILE --out DIR [--pack-id ID] [--pack-version V]
  kpack-cli verify --corpus DIR --clusters FILE --dir DIR
  kpack-cli lookup --pack FILE (--query TEXT | --batch FILE) [--k N] [--json] [--curator-key HEX]
    prints one JSON record per query (the output is always JSON; --json is accepted for clarity);
    --batch reads one query per line (a JSON string or {\"query\": ..., \"k\": ...}) and writes
    one record per line in input order; --curator-key overrides the pinned production key";

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
        "lookup" => {
            // `--json` is the only valueless flag: the output is always JSON.
            let rest: Vec<String> = rest.iter().filter(|a| a.as_str() != "--json").cloned().collect();
            let f = flags(&rest, &["pack", "query", "batch", "k", "curator-key"])?;
            let k = match f.get("k") {
                Some(k) => k.parse::<usize>().map_err(|_| format!("--k {k:?} is not a whole number"))?,
                None => DEFAULT_LOOKUP_K,
            };
            let key = f.get("curator-key").map(|h| parse_public_key_hex(h)).transpose().map_err(|e| e.to_string())?;
            let queries: Vec<(String, usize)> = match (f.get("query"), f.get("batch")) {
                (Some(q), None) => vec![(q.clone(), k)],
                (None, Some(path)) => {
                    let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
                    let mut qs = Vec::new();
                    for (i, line) in text.lines().enumerate() {
                        if let Some(q) = parse_batch_line(line, i + 1, k).map_err(|e| e.to_string())? {
                            qs.push(q);
                        }
                    }
                    qs
                }
                _ => return Err("lookup needs exactly one of --query and --batch".to_string()),
            };
            let pack = LookupPack::mount(&required(&f, "pack")?, key).map_err(|e| e.to_string())?;
            let stdout = std::io::stdout();
            let mut out = std::io::BufWriter::new(stdout.lock());
            for (q, k) in queries {
                let rec = pack.lookup(&q, k).map_err(|e| e.to_string())?;
                writeln!(out, "{rec}").map_err(|e| format!("writing output: {e}"))?;
            }
            out.flush().map_err(|e| format!("writing output: {e}"))?;
            Ok(())
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
