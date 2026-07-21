#!/usr/bin/env python3
"""build_catalog.py — Task A4: assemble catalog.json from artifact manifests.

Reads one or more artifact `manifest.json` files (as written by
`build_base.py`/`build_adapter.py`) and assembles the signed distribution
catalog's UNSIGNED wire body: `tools/pipeline/work/catalog/catalog.json`.
Signing (a separate step, over these exact bytes) is `sign_catalog.py`'s
job, not this script's.

Env-dumb (see README.md "The env-dumb contract"): this script's only
inputs are its CLI flags below plus, ONLY when `--base-url` is given, one
unauthenticated public GET of `<base-url>/catalog.json` (no credentials,
no `.env` involvement — `.env` is never read by this script at all; there
is nothing here that needs a secret). It never reads the calling shell's
exported environment and never assumes a working directory other than its
own `tools/pipeline/` root.

Inputs:
    --manifests <path> [<path> ...]
                    Explicit list of manifest.json paths to include in the
                    catalog (REQUIRED — no default, no glob/auto-discovery:
                    an implicit "whatever's on disk" scan risks silently
                    publishing a partial catalog if a manifest hasn't been
                    built yet, or including a stale one nobody meant to
                    ship). Refuses — writes nothing — if ANY listed path is
                    missing or fails to parse as a JSON object.
    --catalog-version <int>  XOR  --base-url <url>
                    Exactly one of these two is required (enforced by an
                    argparse mutually-exclusive required group, so "both
                    given" and "neither given" are both a clean CLI usage
                    error, not ambiguous runtime behavior). See
                    "catalog_version precedence" below.
    --license-override KIND=VALUE
                    Repeatable. An explicit, operator-supplied fallback
                    license string for artifacts of the given kind
                    ("base" or "adapter") whose manifest carries no
                    resolvable license — see "license resolution" below.
                    This is never a value the script invents on its own;
                    it only ever comes from something the operator typed.
    --out <path>    Where to write catalog.json (default:
                    tools/pipeline/work/catalog/catalog.json).

Outputs:
    <out> (default work/catalog/catalog.json) — see "Wire format" below.
    Written atomically (`.tmp` + `os.replace`-equivalent rename), so a
    reader (including `sign_catalog.py` run right after this) never
    observes a partially-written file.

Documented default invocation, once BOTH artifacts exist (post-handoff —
pre-handoff the adapter manifest does not exist yet, so this exact command
refuses with a clear "manifest not found" error until `build_adapter.py`
has actually run against the real bucket; a single-manifest catalog is
fine for pre-handoff testing but is NOT this script's documented default):

    ./.venv/bin/python3 build_catalog.py \\
      --manifests work/out/Qwen3-4B/Q4_K_M/manifest.json \\
                  work/out/Qwen3-4B/adapter/manifest.json \\
      --catalog-version 1

(or `--base-url <cleophis-dist base URL>` in place of `--catalog-version`,
once a prior catalog has actually been published, to auto-increment.)

catalog_version precedence: exactly one of --catalog-version/--base-url.
  --catalog-version N: use N verbatim — the operator has already checked
    what's currently published (e.g. via `verify_published.py`, A6) or
    knows this is the very first catalog ever published.
  --base-url URL: GET `<URL>/catalog.json` (public, unauthenticated), read
    its own `catalog_version` field, and use value + 1. A fetch/parse
    failure (network error, non-2xx, malformed JSON, missing/non-integer
    catalog_version) is a HARD refusal — it does NOT fall back to 1,
    because "nothing has ever been published" and "the fetch is broken"
    are indistinguishable from here, and silently guessing 1 risks
    re-publishing a version number that already exists. For the actual
    first-ever publish, pass --catalog-version 1 explicitly instead.

license resolution, per manifest (first match wins):
  1. the manifest's own top-level "license" field, if a non-empty string
     (this is how build_base.py's manifests carry it — see its own
     `--license` flag, default "Apache-2.0");
  2. its "source_manifest.license" field, if a non-empty string
     (build_adapter.py carries the PEFT source dir's own manifest.json
     forward VERBATIM under "source_manifest" when the source provides
     one — this is where an adapter's license is expected to come from,
     since build_adapter.py's own manifest has no top-level "license"
     key);
  3. a --license-override for that manifest's "kind", if one was given;
  4. otherwise: refuse with a clear per-manifest error naming exactly what
     was checked, rather than emitting a catalog entry with a
     guessed/blank/empty license string.

path derivation, per manifest "kind" (v0 hardcodes the two artifact
layouts the spec defines — see README.md's "Binding cross-track values"
table; any other kind, or a manifest "name" that doesn't match the exact
basename that table pins, is a refusal rather than a guessed path shape):
    kind == "base":    path = f"models/{base_model}/v1/{name}"
                        requires name == "Qwen3-4B-Instruct-Q4_K_M.gguf"
    kind == "adapter": path = f"adapters/behavioral/v1/{base_model}/{name}"
                        requires name == "behavioral-v1-Qwen3-4B.gguf"
The catalog artifact "version" field is a fixed "v1" for every artifact in
this v0 catalog format — it is NOT read from the manifest (neither
manifest shape carries a field with that meaning today).

Wire format (fixed by the Rust consumer,
`src-tauri/src/catalog_dist.rs::DistCatalog`/`Artifact` — plain `derive`,
no `rename`, so field names must match byte-for-byte):

    {
      "catalog_version": <u64>,
      "generated_at": "<ISO-8601 UTC, e.g. 2026-07-21T00:00:00Z>",
      "artifacts": [
        {
          "path": "<relative to the bucket root>",
          "sha256": "<64 lowercase hex chars>",
          "size": <int, bytes>,
          "kind": "base" | "adapter",
          "base_model": "Qwen3-4B",
          "version": "v1",
          "license": "<license identifier>"
        }
      ]
    }

No extra fields are ever added to an artifact entry or to the catalog
object, even though a manifest may (and does, for adapters) carry extra
fields such as A3's `peft_content_sha256` — those inform nothing here and
must not leak into the catalog.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

PIPELINE_ROOT = Path(__file__).resolve().parent
DEFAULT_OUT = PIPELINE_ROOT / "work" / "catalog" / "catalog.json"

# The two artifact kinds/basenames/base_model this v0 catalog format knows
# about — cross-track binding values, verbatim from README.md's "Binding
# cross-track values" section. Duplicated here (not imported from
# build_base.py/build_adapter.py) so this script stays a standalone,
# env-dumb unit, matching the house pattern build_adapter.py already set
# for its own duplicated DEFAULT_BASE_REVISION constant.
EXPECTED_BASE_MODEL = "Qwen3-4B"
EXPECTED_BASENAME_BY_KIND = {
    "base": "Qwen3-4B-Instruct-Q4_K_M.gguf",
    "adapter": "behavioral-v1-Qwen3-4B.gguf",
}
CATALOG_ARTIFACT_VERSION = "v1"

REQUIRED_MANIFEST_FIELDS = ("kind", "base_model", "sha256", "size", "name")

# Sanity cap on a fetched `--base-url` catalog.json response — mirrors the
# app's own `catalog_dist.rs::MAX_CATALOG_BYTES` defensive cap, so this
# script can't be made to buffer an unbounded response from a
# misbehaving/compromised endpoint.
MAX_FETCH_BYTES = 1024 * 1024  # 1 MiB


class CatalogAssemblyError(RuntimeError):
    """Any refusal while assembling the catalog — missing/malformed
    manifest, unresolvable license, unknown kind, mismatched basename,
    or a failed --base-url version fetch. Always caught at main()'s top
    level and reported as a clean one-line error, never a raw traceback."""


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Assemble tools/pipeline/work/catalog/catalog.json from artifact manifest.json files.",
        epilog=(
            "Documented default invocation once both artifacts exist (post-handoff):\n"
            "  ./.venv/bin/python3 build_catalog.py \\\n"
            "    --manifests work/out/Qwen3-4B/Q4_K_M/manifest.json work/out/Qwen3-4B/adapter/manifest.json \\\n"
            "    --catalog-version 1\n"
            "(or --base-url <cleophis-dist base URL> in place of --catalog-version, once a prior\n"
            "catalog has actually been published, to auto-increment.)"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--manifests",
        nargs="+",
        required=True,
        type=Path,
        metavar="MANIFEST_JSON",
        help="explicit list of manifest.json paths to include (required; refuses if any is missing)",
    )
    version_group = parser.add_mutually_exclusive_group(required=True)
    version_group.add_argument(
        "--catalog-version",
        type=int,
        default=None,
        help="use this catalog_version verbatim",
    )
    version_group.add_argument(
        "--base-url",
        default=None,
        help="fetch <base-url>/catalog.json (public, unauthenticated) and use its catalog_version + 1",
    )
    parser.add_argument(
        "--license-override",
        action="append",
        default=[],
        metavar="KIND=VALUE",
        help='fallback license for manifests of the given kind ("base" or "adapter") that carry none; repeatable',
    )
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT, help=f"output path (default: {DEFAULT_OUT})")
    return parser.parse_args(argv)


def parse_license_overrides(raw_list: list[str]) -> dict[str, str]:
    overrides: dict[str, str] = {}
    for item in raw_list:
        if "=" not in item:
            raise CatalogAssemblyError(f'--license-override {item!r} is not in KIND=VALUE form')
        kind, _, value = item.partition("=")
        kind = kind.strip()
        value = value.strip()
        if kind not in EXPECTED_BASENAME_BY_KIND:
            raise CatalogAssemblyError(
                f'--license-override kind {kind!r} must be one of {sorted(EXPECTED_BASENAME_BY_KIND)}'
            )
        if not value:
            raise CatalogAssemblyError(f"--license-override {item!r} has an empty value")
        overrides[kind] = value
    return overrides


def load_manifest(path: Path) -> dict:
    if not path.is_file():
        raise CatalogAssemblyError(
            f"manifest not found: {path} — refusing to assemble a catalog with a missing artifact "
            "(no silent partial catalogs)"
        )
    try:
        data = json.loads(path.read_text())
    except (json.JSONDecodeError, OSError) as exc:
        raise CatalogAssemblyError(f"{path}: could not read/parse as JSON: {exc}") from exc
    if not isinstance(data, dict):
        raise CatalogAssemblyError(f"{path}: manifest JSON is not an object")
    return data


def resolve_license(manifest: dict, overrides: dict[str, str], manifest_path: Path) -> str:
    top = manifest.get("license")
    if isinstance(top, str) and top:
        return top

    source_manifest = manifest.get("source_manifest")
    if isinstance(source_manifest, dict):
        nested = source_manifest.get("license")
        if isinstance(nested, str) and nested:
            return nested

    kind = manifest.get("kind")
    if kind in overrides:
        return overrides[kind]

    raise CatalogAssemblyError(
        f'{manifest_path}: no resolvable "license" — checked the manifest\'s own top-level "license", '
        f'then "source_manifest.license", and no --license-override {kind}=... was given. Refusing to '
        "guess a license string for the signed catalog."
    )


def artifact_path_for(kind: str, base_model: str, name: str, manifest_path: Path) -> str:
    expected_name = EXPECTED_BASENAME_BY_KIND.get(kind)
    if expected_name is None:
        raise CatalogAssemblyError(
            f'{manifest_path}: kind={kind!r} — this v0 script only knows the path layout for '
            f"{sorted(EXPECTED_BASENAME_BY_KIND)}; add a path rule here before cataloging a new kind"
        )
    if name != expected_name:
        raise CatalogAssemblyError(
            f'{manifest_path}: name={name!r}, expected exactly {expected_name!r} for kind={kind!r} '
            "(binding cross-track basename — see tools/pipeline/README.md)"
        )
    if kind == "base":
        return f"models/{base_model}/v1/{name}"
    if kind == "adapter":
        return f"adapters/behavioral/v1/{base_model}/{name}"
    raise AssertionError(f"unreachable: kind {kind!r} passed the EXPECTED_BASENAME_BY_KIND check above")


def manifest_to_artifact(manifest: dict, manifest_path: Path, overrides: dict[str, str]) -> dict:
    missing = [f for f in REQUIRED_MANIFEST_FIELDS if f not in manifest]
    if missing:
        raise CatalogAssemblyError(
            f"{manifest_path}: missing required field(s) {missing} — refusing to assemble a partial catalog entry"
        )

    kind = manifest["kind"]
    if kind not in ("base", "adapter"):
        raise CatalogAssemblyError(
            f'{manifest_path}: kind={kind!r} is not "base" or "adapter" — the only two the catalog wire format allows'
        )

    base_model = manifest["base_model"]
    if base_model != EXPECTED_BASE_MODEL:
        raise CatalogAssemblyError(
            f"{manifest_path}: base_model={base_model!r}, expected exactly {EXPECTED_BASE_MODEL!r} "
            "(binding cross-track value — see tools/pipeline/README.md)"
        )

    name = manifest["name"]
    if not isinstance(name, str) or not name:
        raise CatalogAssemblyError(f"{manifest_path}: name={name!r} is not a non-empty string")

    sha256 = manifest["sha256"]
    if not (isinstance(sha256, str) and len(sha256) == 64 and all(c in "0123456789abcdef" for c in sha256.lower())):
        raise CatalogAssemblyError(f"{manifest_path}: sha256={sha256!r} does not look like a 64-hex-char sha256 digest")

    size = manifest["size"]
    if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
        raise CatalogAssemblyError(f"{manifest_path}: size={size!r} is not a positive integer")

    license_id = resolve_license(manifest, overrides, manifest_path)
    path = artifact_path_for(kind, base_model, name, manifest_path)

    return {
        "path": path,
        "sha256": sha256.lower(),
        "size": size,
        "kind": kind,
        "base_model": base_model,
        "version": CATALOG_ARTIFACT_VERSION,
        "license": license_id,
    }


def fetch_published_version(base_url: str, timeout: float = 15.0) -> int:
    """GET `<base_url>/catalog.json` (public, unauthenticated) and return
    its `catalog_version` field. Any failure — network error, non-2xx,
    oversized response, malformed JSON, missing/non-integer field — raises
    CatalogAssemblyError; there is deliberately no fallback to 0/1 (see the
    module docstring's "catalog_version precedence" section for why)."""
    url = base_url.rstrip("/") + "/catalog.json"
    req = urllib.request.Request(url, headers={"User-Agent": "cleophis-build_catalog/1.0"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 - deliberate plain public HTTP(S) GET
            raw = resp.read(MAX_FETCH_BYTES + 1)
    except (urllib.error.URLError, OSError, ValueError) as exc:
        raise CatalogAssemblyError(f"failed to GET {url}: {exc}") from exc

    if len(raw) > MAX_FETCH_BYTES:
        raise CatalogAssemblyError(f"{url} response exceeded the {MAX_FETCH_BYTES}-byte sanity cap")

    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise CatalogAssemblyError(f"{url} did not return valid JSON: {exc}") from exc

    version = data.get("catalog_version") if isinstance(data, dict) else None
    if not isinstance(version, int) or isinstance(version, bool):
        raise CatalogAssemblyError(f"{url} response has no integer catalog_version field (got {version!r})")
    return version


def build_catalog(manifest_paths: list[Path], catalog_version: int, overrides: dict[str, str]) -> dict:
    artifacts = [manifest_to_artifact(load_manifest(p), p, overrides) for p in manifest_paths]
    return {
        "catalog_version": catalog_version,
        "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "artifacts": artifacts,
    }


def write_catalog_atomic(path: Path, catalog: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(catalog, indent=2) + "\n")
    tmp.replace(path)  # atomic rename on POSIX, same filesystem


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    try:
        overrides = parse_license_overrides(args.license_override)
    except CatalogAssemblyError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    if args.catalog_version is not None:
        if args.catalog_version < 0:
            print("error: --catalog-version must be >= 0", file=sys.stderr)
            return 1
        catalog_version = args.catalog_version
    else:
        try:
            published = fetch_published_version(args.base_url)
        except CatalogAssemblyError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        catalog_version = published + 1
        print(
            f"[version] currently published catalog_version={published} at {args.base_url} -> using {catalog_version}",
            flush=True,
        )

    try:
        catalog = build_catalog(args.manifests, catalog_version, overrides)
    except CatalogAssemblyError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    write_catalog_atomic(args.out, catalog)
    print(
        f"[catalog] wrote {args.out} (catalog_version={catalog_version}, {len(catalog['artifacts'])} artifact(s))",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
