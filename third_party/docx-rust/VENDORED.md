# Vendored: docx-rust 0.1.11 on zip 9

- **Upstream crate:** `docx-rust` 0.1.11 (crates.io, published 2026-01-22)
- **Upstream repo:** https://github.com/cstkingkey/docx-rs, head b3852d2 (2026-08-27) still declares `zip = "4"`
- **Licence:** MIT (`LICENSE` kept unchanged)
- **Wired in by:** `[patch.crates-io] docx-rust = { path = "third_party/docx-rust" }` in the root `Cargo.toml`. The version stays `0.1.11` so the `=0.1.11` pin in `crates/kpack-core/Cargo.toml` still matches.

## Why

docx-rust 0.1.11 requires `zip = "4"`. The zip 4.6.1 chain carried RUSTSEC-2026-0367/0368/0369/0429 (cargo-audit) and CVE-2026-115905/-115909/-115906 (Dependabot). Upstream has no release on zip 9. Vendoring with the requirement raised lets the single remaining zip in the lock be 9.x (shared with `epub`).

## Diff policy

`.gitattributes` in this directory marks everything `-diff`, because the root `.gitattributes` has no `third_party` rule. This record is therefore the only readable diff. Files removed from the crates.io tarball: `.cargo_vcs_info.json`, `Cargo.toml.orig`, `.cargo-ok`.

## Every changed line (before -> after)

`Cargo.toml` line 80, `[dependencies.zip]`:

```
- version = "4"
+ version = "9"
```
(`features = ["deflate"]` and `default-features = false` unchanged. The optional `async` feature stays off; we never enable it.)

`src/docx.rs` line 471 (macro `option_read_multiple`) and line 489 (macro `option_read_multiple_files`). In zip 9, `file_names()` yields `ZipResult<Cow<str>>`, so names that fail to decode are now skipped instead of listed:

```
-                let names: Vec<_> = zip.file_names().map(|x| x.to_string()).collect();
+                let names: Vec<_> = zip.file_names().filter_map(|x| x.ok()).map(|x| x.to_string()).collect();
```

`src/document/numbering.rs` line 163. Needed because path dependencies are not lint-capped, so `RUSTFLAGS='-D warnings'` would fail on `mismatched_lifetime_syntaxes`:

```
-    pub fn numbering_details(&self, id: isize) -> Option<AbstractNum> {
+    pub fn numbering_details(&self, id: isize) -> Option<AbstractNum<'_>> {
```

New file: `.gitattributes` (`* -diff`).

## Re-vendor

1. `cp -a ~/.cargo/registry/src/index.crates.io-*/docx-rust-<ver>/. third_party/docx-rust/` and delete `.cargo_vcs_info.json`, `Cargo.toml.orig`, `.cargo-ok`.
2. Re-apply the `zip` version line, the two `file_names()` lines and the `numbering.rs` lifetime above, and restore `.gitattributes` and this file.
3. `cargo update -p docx-rust`, then confirm `grep -A1 '^name = "zip"$' Cargo.lock` shows one zip >= 9.0.5.
4. Retire this vendoring once upstream releases on zip 9 or later.
