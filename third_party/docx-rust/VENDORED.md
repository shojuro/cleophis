# Vendored: docx-rust 0.1.11 on zip 9

- **Upstream crate:** `docx-rust` 0.1.11 (crates.io, published 2026-01-22)
- **Upstream repo:** https://github.com/cstkingkey/docx-rs, head b3852d2 (2026-08-27) still declares `zip = "4"`
- **Licence:** MIT (`LICENSE` kept unchanged)
- **Wired in by:** `[patch.crates-io] docx-rust = { path = "third_party/docx-rust" }` in the root `Cargo.toml`. The version stays `0.1.11` so the `=0.1.11` pin in `crates/kpack-core/Cargo.toml` still matches.

## Why

docx-rust 0.1.11 requires `zip = "4"`. The zip 4.6.1 chain carried RUSTSEC-2026-0367/0368/0369/0429 (cargo-audit) and CVE-2026-115905/-115909/-115906 (Dependabot). Upstream has no release on zip 9. Vendoring with the requirement raised lets the single remaining zip in the lock be 9.x (shared with `epub`).

## Every difference from the registry crate

The tree is the crates.io 0.1.11 tarball (as unpacked in `~/.cargo/registry/src`) minus the omissions below, plus the edits and added files below. Nothing else differs.

### Omitted (not built by this repository)

- Tarball metadata: `.cargo_vcs_info.json`, `Cargo.toml.orig`, `.cargo-ok`.
- `tests/`: `integration_test.rs`, `tests/aaa/` (`AA1/`, `aa_list/`, the `19323...` extracted-docx directory, `aa.xlsx`) and `tests/pandoc/`. Upstream's own `.gitignore` had already excluded `*.docx` from git, so a vendored copy would have held only a half set. Those ignored files are also omitted: `tests/aaa/aa.docx`, `tests/aaa/aa_list.docx` and the 39 files `tests/pandoc/*.docx`.
- `examples/` (`hello_world.rs`, `table.rs`), `.github/` (`workflows/test.yml`), `.editorconfig`, `validator/` (a C# docx validator).
- Upstream `Cargo.lock` (the crate is a library; our own lock governs).
- Upstream `.gitignore`. It silently dropped the files named above, and the repository's root ignore rules suffice.
- Consequence: the `[[example]]` and `[[test]]` sections of the vendored `Cargo.toml` still name the missing paths. Cargo ignores them for a path dependency (`cargo metadata --locked` succeeds). Standalone `cargo test` inside this directory would fail; the crate is tested through `kpack-core`.

### Attributes

`third_party/.gitattributes` already applies `* -text -diff` to the whole tree. The nested `.gitattributes` here (`VENDORED.md diff`, `LICENSE diff`, `README.md diff`) only makes the human-readable files diffable again. Source files under `src/` stay `-diff`, so this record is their readable diff.

### Edited lines (before -> after)
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

Added files: `VENDORED.md` and the nested `.gitattributes` described above.

## Re-vendor

1. `cp -a ~/.cargo/registry/src/index.crates.io-*/docx-rust-<ver>/. third_party/docx-rust/` then delete everything listed under Omitted.
2. Re-apply the `zip` version line, the two `file_names()` lines and the `numbering.rs` lifetime above, and keep the nested `.gitattributes` and this file.
3. `cargo update -p docx-rust`, then confirm `grep -A1 '^name = "zip"$' Cargo.lock` shows one zip >= 9.0.5.
4. Retire this vendoring once upstream releases on zip 9 or later.
