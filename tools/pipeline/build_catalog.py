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

path derivation, per manifest "kind" (Phase 1g M3 — derived from the
manifest's identity fields, refusing any manifest "name" that is not the
exact derived basename):
    kind == "base":
        name = f"{base_model}-Instruct-{quant}.gguf"   (quant from the manifest)
        path = f"models/{base_model}/v1/{name}"
    kind == "adapter" | "contract-adapter":
        name = f"{adapter_name}-{version}-{base_model}.gguf"
        path = f"adapters/{adapter_name}/{version}/{base_model}/{name}"
      adapter_name/version come from the manifest's "adapter_name"/"version"
      fields. Defaults (back-compat): kind "adapter" -> behavioral + v1
      (today's behavioral-v1-<base> paths, byte for byte); kind
      "contract-adapter" -> contract + the legacy "dist_version" (or v1).
      adapter_name must match [a-z][a-z0-9]*(-[a-z0-9]+)*, version
      v<N>(.<N>)*, quant [A-Za-z0-9_]+ — each becomes a bucket path segment.
The catalog entry "version" wire field is "v1" for base and kind "adapter"
entries (as every live adapter entry carries, tutor-v1.3 included) and the
path version for a contract-adapter (live: "v3.1").

--carry-forward <catalog.json> --carry-forward-pubkey <hex> (M3): carry a
previously published catalog's artifacts into this one, verbatim and first.
The file's detached `<file>.sig` must verify under the given curator PUBLIC
key, every carried entry must re-derive unchanged through the rules above,
no bucket path may repeat, the new catalog_version must exceed the carried
one, and with --base-url the carried catalog_version must equal the live one
(a stale copy would silently drop later artifacts).

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
          "kind": "base" | "adapter" | "contract-adapter",
          "base_model": "<one of KNOWN_BASE_MODELS>",
          "version": "v1" (contract-adapter: its path version),
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
import re
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import NamedTuple

PIPELINE_ROOT = Path(__file__).resolve().parent
DEFAULT_OUT = PIPELINE_ROOT / "work" / "catalog" / "catalog.json"

# The two artifact kinds/basenames/base_model this v0 catalog format knows
# about — cross-track binding values, verbatim from README.md's "Binding
# cross-track values" section. Duplicated here (not imported from
# build_base.py/build_adapter.py) so this script stays a standalone,
# env-dumb unit, matching the house pattern build_adapter.py already set
# for its own duplicated DEFAULT_BASE_REVISION constant.
# The tier base_models this catalog format knows — the three trained tiers.
# base_model is the CLEAN tier name (never the unsloth-bnb-4bit training-repo
# name); the app's per-tier resolution keys off exactly these strings.
KNOWN_BASE_MODELS = frozenset({"Qwen3-4B", "Llama-3.2-1B", "Qwen3-8B", "Qwen3-1.7B"})
# "contract-adapter" (adapter v2) is a distinct kind so the app can pick the
# contract-grounding adapter apart from the always-on behavioral "adapter"
# (both share a base_model).
VALID_KINDS = ("base", "adapter", "contract-adapter")
CATALOG_ARTIFACT_VERSION = "v1"


# Adapter identity (Phase 1g M3): an adapter manifest names itself with
# `adapter_name` (e.g. "behavioral", "triage", "tutor", "voice-adult") and
# `version` (e.g. "v1", "v3", "v1.3"); both default for back-compat (a kind
# "adapter" manifest without them is behavioral + v1, today's only shape; a
# "contract-adapter" is always "contract" and may still carry the legacy
# `dist_version` field build_adapter.py writes). Both segments land verbatim
# in a bucket path, so both are allowlisted by regex — no "/", no "..".
ADAPTER_NAME_RE = re.compile(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$")
ADAPTER_VERSION_RE = re.compile(r"^v[0-9]+(?:\.[0-9]+)*$")
# A base's quant ("Q4_K_M", "Q6_K", "Q5_K_M", "F16", ...) lands in its
# basename and in publish.py's local subdir name.
BASE_QUANT_RE = re.compile(r"^[A-Za-z0-9_]+$")
DEFAULT_ADAPTER_NAME = "behavioral"
CONTRACT_ADAPTER_NAME = "contract"


def adapter_identity(kind: str, manifest: dict, manifest_path: Path) -> tuple[str, str]:
    """(adapter_name, version) for an adapter-kind manifest — see the
    ADAPTER_NAME_RE comment above for the defaults. Refuses a name/version
    that fails the allowlist, a kind "adapter" claiming the reserved
    "contract" name, a "contract-adapter" claiming any other name, and a
    contract manifest whose `version` and legacy `dist_version` disagree."""
    if kind == "adapter":
        name = manifest.get("adapter_name", DEFAULT_ADAPTER_NAME)
        version = manifest.get("version", CATALOG_ARTIFACT_VERSION)
        if name == CONTRACT_ADAPTER_NAME:
            raise CatalogAssemblyError(
                f'{manifest_path}: adapter_name="contract" is reserved for kind "contract-adapter"'
            )
    elif kind == "contract-adapter":
        name = manifest.get("adapter_name", CONTRACT_ADAPTER_NAME)
        if name != CONTRACT_ADAPTER_NAME:
            raise CatalogAssemblyError(
                f'{manifest_path}: kind "contract-adapter" must have adapter_name "contract", got {name!r}'
            )
        version = manifest.get("version")
        legacy = manifest.get("dist_version")
        if version is not None and legacy is not None and version != legacy:
            raise CatalogAssemblyError(
                f"{manifest_path}: version={version!r} and dist_version={legacy!r} disagree"
            )
        version = version if version is not None else (legacy if legacy is not None else CATALOG_ARTIFACT_VERSION)
    else:
        raise AssertionError(f"adapter_identity called for non-adapter kind {kind!r}")
    if not (isinstance(name, str) and ADAPTER_NAME_RE.match(name)):
        raise CatalogAssemblyError(
            f"{manifest_path}: adapter_name={name!r} must match {ADAPTER_NAME_RE.pattern} (it becomes a bucket path segment)"
        )
    if not (isinstance(version, str) and ADAPTER_VERSION_RE.match(version)):
        raise CatalogAssemblyError(
            f"{manifest_path}: version={version!r} must match {ADAPTER_VERSION_RE.pattern} (it becomes a bucket path segment)"
        )
    return name, version


def expected_basename(
    kind: str,
    base_model: str,
    quant: str | None = None,
    adapter_name: str | None = None,
    version: str | None = None,
) -> str | None:
    """Artifact basename DERIVED from the manifest's identity fields — a
    derivation, not a per-tier table, so a new tier/adapter needs no edit
    here (only KNOWN_BASE_MODELS for a new tier):
        base:              <base_model>-Instruct-<quant>.gguf   (build_base.py's shape)
        adapter / contract-adapter:
                           <adapter_name>-<version>-<base_model>.gguf
    behavioral + v1 is exactly build_adapter.py's behavioral-v1-<base>.gguf.
    Returns None for an unknown kind, or a base with no quant to derive from."""
    if kind == "base":
        return f"{base_model}-Instruct-{quant}.gguf" if quant else None
    if kind in ("adapter", "contract-adapter"):
        return f"{adapter_name}-{version}-{base_model}.gguf"
    return None

REQUIRED_MANIFEST_FIELDS = ("kind", "base_model", "sha256", "size", "name")

# Sanity cap on a fetched `--base-url` catalog.json response — mirrors the
# app's own `catalog_dist.rs::MAX_CATALOG_BYTES` defensive cap, so this
# script can't be made to buffer an unbounded response from a
# misbehaving/compromised endpoint.
MAX_FETCH_BYTES = 1024 * 1024  # 1 MiB

# Exclusive sane ceiling on a `catalog_version` this script will ever trust,
# whether read directly off a fetched (UNAUTHENTICATED, attacker-reachable)
# `--base-url` response or computed as that value + 1 — see
# `check_version_in_bounds`'s docstring for why this exists.
MAX_SANE_CATALOG_VERSION = 1_000_000

# The production curator PUBLIC key (crates/kpack-core/src/sign.rs
# CURATOR_PUBLIC_KEY, hex) — used ONLY by --self-test to verify the pinned
# live-catalog fixture. The CLI never defaults --carry-forward-pubkey to it.
PROD_CURATOR_PUBKEY_HEX = "158cb99e9756e2e4d01d88b7ecfeb99a76547821ffe9f9f1985316c5451cd0c0"


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
        default=None,
        type=Path,
        metavar="MANIFEST_JSON",
        help="explicit list of manifest.json paths to include (required unless --self-test; refuses if any is missing)",
    )
    # required=False here — "at least one of these two" is enforced by hand
    # in main(), ONLY when --self-test isn't given (mirrors sign_catalog.py's
    # --catalog: not argparse-required, checked explicitly so --self-test can
    # run standalone with no other flags). Mutual EXCLUSIVITY (refuse if
    # BOTH are given) is still enforced by argparse itself either way.
    version_group = parser.add_mutually_exclusive_group(required=False)
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
    parser.add_argument(
        "--carry-forward",
        type=Path,
        default=None,
        metavar="CATALOG_JSON",
        help="a previously published catalog.json (its .sig beside it) whose artifacts are carried into this "
        "catalog verbatim, first; requires --carry-forward-pubkey",
    )
    parser.add_argument(
        "--carry-forward-pubkey",
        default=None,
        metavar="HEX",
        help="64-hex-char ed25519 curator PUBLIC key the carried catalog's .sig must verify under",
    )
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT, help=f"output path (default: {DEFAULT_OUT})")
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the hostile-catalog_version bound-check self-test and exit (no --manifests/--catalog-version/--base-url needed)",
    )
    return parser.parse_args(argv)


def parse_license_overrides(raw_list: list[str]) -> dict[str, str]:
    overrides: dict[str, str] = {}
    for item in raw_list:
        if "=" not in item:
            raise CatalogAssemblyError(f'--license-override {item!r} is not in KIND=VALUE form')
        kind, _, value = item.partition("=")
        kind = kind.strip()
        value = value.strip()
        if kind not in VALID_KINDS:
            raise CatalogAssemblyError(
                f'--license-override kind {kind!r} must be one of {sorted(VALID_KINDS)}'
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


def artifact_path_for(
    kind: str,
    base_model: str,
    name: str,
    quant: str | None,
    manifest_path: Path,
    adapter_name: str | None = None,
    version: str | None = None,
) -> str:
    """Bucket path for one artifact; refuses unless `name` is exactly the
    derived basename (see expected_basename).
        base:     models/<base_model>/v1/<name>
        adapters: adapters/<adapter_name>/<version>/<base_model>/<name>
    behavioral + v1 and contract + <v> reproduce the pre-M3 paths byte-for-byte."""
    if kind == "base" and quant is not None and not (isinstance(quant, str) and BASE_QUANT_RE.match(quant)):
        raise CatalogAssemblyError(f"{manifest_path}: quant={quant!r} must match {BASE_QUANT_RE.pattern}")
    expected_name = expected_basename(kind, base_model, quant, adapter_name, version)
    if expected_name is None:
        detail = (
            'a base manifest needs a "quant" field to derive its basename'
            if kind == "base"
            else f"this script only knows the path layout for {sorted(VALID_KINDS)}"
        )
        raise CatalogAssemblyError(f"{manifest_path}: kind={kind!r} — {detail}")
    if name != expected_name:
        raise CatalogAssemblyError(
            f'{manifest_path}: name={name!r}, expected exactly {expected_name!r} for kind={kind!r} '
            f"base_model={base_model!r} (binding cross-track basename — see tools/pipeline/README.md)"
        )
    if kind == "base":
        return f"models/{base_model}/v1/{name}"
    if kind in ("adapter", "contract-adapter"):
        return f"adapters/{adapter_name}/{version}/{base_model}/{name}"
    raise AssertionError(f"unreachable: kind {kind!r} passed the expected_basename check above")


def manifest_to_artifact(manifest: dict, manifest_path: Path, overrides: dict[str, str]) -> dict:
    missing = [f for f in REQUIRED_MANIFEST_FIELDS if f not in manifest]
    if missing:
        raise CatalogAssemblyError(
            f"{manifest_path}: missing required field(s) {missing} — refusing to assemble a partial catalog entry"
        )

    kind = manifest["kind"]
    if kind not in VALID_KINDS:
        raise CatalogAssemblyError(
            f'{manifest_path}: kind={kind!r} is not one of {list(VALID_KINDS)} — the only kinds the catalog wire format allows'
        )

    base_model = manifest["base_model"]
    if base_model not in KNOWN_BASE_MODELS:
        raise CatalogAssemblyError(
            f"{manifest_path}: base_model={base_model!r} is not one of the known tiers "
            f"{sorted(KNOWN_BASE_MODELS)} (binding cross-track value — see tools/pipeline/README.md)"
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

    # Catalog entry "version" (wire field; the app reads nothing from it):
    # base and kind "adapter" entries carry the fixed "v1" — as every live
    # kind "adapter" entry does, including tutor-v1.3 — while a
    # contract-adapter's entry carries its path version (live: "v3.1").
    license_id = resolve_license(manifest, overrides, manifest_path)
    if kind == "base":
        path = artifact_path_for(kind, base_model, name, manifest.get("quant"), manifest_path)
        entry_version = CATALOG_ARTIFACT_VERSION
    else:
        adapter_name, adapter_version = adapter_identity(kind, manifest, manifest_path)
        path = artifact_path_for(kind, base_model, name, None, manifest_path, adapter_name, adapter_version)
        entry_version = adapter_version if kind == "contract-adapter" else CATALOG_ARTIFACT_VERSION

    return {
        "path": path,
        "sha256": sha256.lower(),
        "size": size,
        "kind": kind,
        "base_model": base_model,
        "version": entry_version,
        "license": license_id,
    }


def check_version_in_bounds(version: int, ceiling: int = MAX_SANE_CATALOG_VERSION) -> None:
    """Refuses (raises CatalogAssemblyError) any `version` outside
    `[0, ceiling)`. A pure function of one value — no network, no argparse
    — specifically so the hostile-input cases can be unit-tested directly
    on a value, not just through a live/mocked HTTP fetch.

    Why this exists: `--base-url` reads `catalog_version` off a public,
    UNAUTHENTICATED HTTP response — the one place in this whole pipeline
    an attacker-influenceable value flows in without ever touching the
    curator signing key. Before this check existed, a poisoned published
    catalog claiming e.g. `catalog_version: 18446744073709551615` (2**64-1)
    would make `fetch_published_version` return it verbatim; the operator
    would then sign `published + 1` as a perfectly normal-looking catalog;
    every client that verified it would persist that value as its
    highest-ever-verified version (`catalog_dist.rs::check_not_downgrade`'s
    persisted state) — a PERMANENT downgrade-guard lockout (no legitimate
    future catalog_version could ever be "newer"), achieved without the
    attacker ever forging a signature or touching the private key. Called
    both on the raw fetched value AND on `fetched + 1` (see `main()`) —
    checking only the raw value would still let a value one below the
    ceiling silently overflow into an out-of-bounds "next" version.
    """
    if not (0 <= version < ceiling):
        raise CatalogAssemblyError(
            f"fetched catalog_version={version} is outside the sane bound [0, {ceiling}) — refusing to trust it. "
            "This could mean the published catalog is corrupted or poisoned. Investigate it, and/or pass "
            "--catalog-version explicitly instead of --base-url."
        )


def fetch_published_version(base_url: str, timeout: float = 15.0) -> int:
    """GET `<base_url>/catalog.json` (public, unauthenticated) and return
    its `catalog_version` field. Any failure — network error, non-2xx,
    oversized response, malformed JSON, missing/non-integer field, or a
    value `check_version_in_bounds` rejects — raises CatalogAssemblyError;
    there is deliberately no fallback to 0/1 (see the module docstring's
    "catalog_version precedence" section for why)."""
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
    check_version_in_bounds(version)
    return version


WIRE_FIELDS = ("path", "sha256", "size", "kind", "base_model", "version", "license")
BASE_PATH_RE = re.compile(r"^models/(?P<base>[^/]+)/v1/(?P=base)-Instruct-(?P<quant>[^/]+)\.gguf$")
ADAPTER_PATH_RE = re.compile(r"^adapters/(?P<name>[^/]+)/(?P<version>[^/]+)/(?P<base>[^/]+)/(?P<file>[^/]+)$")
SIG_LEN = 64


class CarriedCatalog(NamedTuple):
    catalog_version: int
    artifacts: list[dict]


def entry_to_manifest(entry: dict) -> dict:
    """Invert a catalog entry's path into the manifest that would produce
    it — used only to prove a carried-forward entry is one this builder
    could itself have derived (see check_carried_entries)."""
    m = {k: entry[k] for k in ("kind", "base_model", "sha256", "size", "license")}
    path = entry["path"]
    m["name"] = path.rsplit("/", 1)[-1]
    if entry["kind"] == "base":
        match = BASE_PATH_RE.match(path)
        if match:
            m["quant"] = match["quant"]
    else:
        match = ADAPTER_PATH_RE.match(path)
        if match:
            m["adapter_name"] = match["name"]
            m["version"] = match["version"]
    return m


def check_carried_entries(entries: object, source: Path) -> list[dict]:
    """Every carried entry must have exactly the seven wire fields and must
    re-derive, field for field, through this builder's own manifest path
    (the same refusals a fresh manifest gets: known kind/tier, hex sha,
    positive size, allowlisted segments, derived basename). An entry that
    doesn't round-trip is refused rather than passed through blind."""
    if not isinstance(entries, list) or not entries:
        raise CatalogAssemblyError(f"{source}: carried catalog has no non-empty artifacts list")
    out = []
    for i, entry in enumerate(entries):
        where = Path(f"{source}#artifacts[{i}]")
        if not isinstance(entry, dict) or set(entry) != set(WIRE_FIELDS):
            raise CatalogAssemblyError(f"{where}: carried entry must have exactly the fields {list(WIRE_FIELDS)}")
        rederived = manifest_to_artifact(entry_to_manifest(entry), where, {})
        if rederived != entry:
            raise CatalogAssemblyError(
                f"{where}: carried entry does not re-derive unchanged (got {rederived}) — refusing to carry it forward"
            )
        out.append(dict(entry))
    return out


def load_carry_forward(path: Path, pubkey_hex: str) -> CarriedCatalog:
    """Read a previously published catalog.json and its detached
    `<path>.sig`, verify the signature over the exact bytes under
    `pubkey_hex` (the production curator PUBLIC key), then return its
    catalog_version and its artifacts (checked by check_carried_entries).
    The file normally comes from an unauthenticated public GET, and its
    entries end up inside a catalog the founder signs — so an unverified
    carry-forward is refused outright."""
    from sign_catalog import verify_bytes  # lazy: keeps the no-carry path stdlib-only

    sig_path = path.with_name(path.name + ".sig")
    if not path.is_file() or not sig_path.is_file():
        raise CatalogAssemblyError(f"--carry-forward needs both {path} and {sig_path}")
    if path.stat().st_size > MAX_FETCH_BYTES:
        raise CatalogAssemblyError(f"{path} exceeds the {MAX_FETCH_BYTES}-byte sanity cap")
    data = path.read_bytes()
    sig = sig_path.read_bytes()
    if len(sig) != SIG_LEN:
        raise CatalogAssemblyError(f"{sig_path} is {len(sig)} bytes, expected exactly {SIG_LEN}")
    try:
        verified = verify_bytes(pubkey_hex, data, sig)
    except Exception as exc:  # malformed pubkey hex etc.
        raise CatalogAssemblyError(f"could not verify {sig_path}: {exc}") from exc
    if not verified:
        raise CatalogAssemblyError(f"{sig_path} does NOT verify against {path} under the given public key — refusing to carry it forward")
    try:
        catalog = json.loads(data)
    except json.JSONDecodeError as exc:
        raise CatalogAssemblyError(f"{path}: not valid JSON: {exc}") from exc
    version = catalog.get("catalog_version") if isinstance(catalog, dict) else None
    if not isinstance(version, int) or isinstance(version, bool):
        raise CatalogAssemblyError(f"{path}: no integer catalog_version")
    check_version_in_bounds(version)
    return CarriedCatalog(version, check_carried_entries(catalog.get("artifacts"), path))


def check_version_after_carried(new_version: int, carried_version: int) -> None:
    if new_version <= carried_version:
        raise CatalogAssemblyError(
            f"catalog_version {new_version} is not newer than the carried-forward catalog's {carried_version}"
        )


def check_carried_is_published(*, carried_version: int, published_version: int) -> None:
    """With --base-url, the carried file must BE the live catalog: a stale
    copy would silently drop whatever was published after it."""
    if carried_version != published_version:
        raise CatalogAssemblyError(
            f"--carry-forward file is catalog_version {carried_version} but {published_version} is live — "
            "re-fetch the live catalog.json + .sig and carry that forward"
        )


def build_catalog(
    manifest_paths: list[Path],
    catalog_version: int,
    overrides: dict[str, str],
    carried: list[dict] | None = None,
) -> dict:
    """Carried entries (verbatim, in their published order) first, then one
    entry per manifest. Any repeated bucket path is refused — paths are
    immutable, so a duplicate is either a no-op or a conflict, never intended."""
    artifacts = [dict(a) for a in (carried or [])]
    artifacts += [manifest_to_artifact(load_manifest(p), p, overrides) for p in manifest_paths]
    seen: set[str] = set()
    for a in artifacts:
        if a["path"] in seen:
            raise CatalogAssemblyError(f"duplicate artifact path {a['path']!r} — refusing to assemble the catalog")
        seen.add(a["path"])
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


def self_test() -> bool:
    """Local, network-free checks for `check_version_in_bounds` — the
    hostile-`catalog_version` guard (see its own docstring for the attack
    it closes). Exercises the reviewed hostile-input cases directly on
    values, with no HTTP fetch/mock server needed. Returns True iff every
    check passes."""
    ok = True

    def check(name: str, cond: bool) -> None:
        nonlocal ok
        print(f"[self-test] {'PASS' if cond else 'FAIL'}: {name}", flush=True)
        if not cond:
            ok = False

    def refuses(version: int) -> bool:
        try:
            check_version_in_bounds(version)
            return False
        except CatalogAssemblyError:
            return True

    check("a poisoned huge version (2**64 - 1) is refused", refuses(2**64 - 1))
    check("a negative version (-1) is refused", refuses(-1))
    check("999_999 (one below the ceiling) is in-bounds on its own", not refuses(999_999))
    check(
        "999_999 + 1 (== the 1_000_000 ceiling) is refused — the +1-overflow case",
        refuses(999_999 + 1),
    )
    check("a normal small version (5) is in-bounds", not refuses(5))
    check("that normal version + 1 (6) is still in-bounds", not refuses(6))
    check("0 (the very first catalog_version) is in-bounds", not refuses(0))

    # `main()`'s manual `--catalog-version` branch runs the fetched value
    # through this same `check_version_in_bounds` — a fat-fingered huge
    # manual value (e.g. today's date typed where a small integer was
    # meant) must be refused just as hard as a poisoned fetched one, or
    # it becomes a permanent fleet-wide downgrade-guard lockout the
    # instant it's signed and published.
    check(
        "manual --catalog-version 20260721 (a fat-fingered date-shaped value) is refused",
        refuses(20260721),
    )
    check("manual --catalog-version 999_999 (one below the ceiling) is allowed", not refuses(999_999))
    check("manual --catalog-version 1 (a normal first-publish value) is allowed", not refuses(1))

    # Phase 1g M3 — the live signed v10 catalog (fixtures/, fetched verbatim)
    # verifies, and every one of its entries re-derives unchanged through
    # this builder's own manifest path; the served-form pair derives as pinned.
    fixture = PIPELINE_ROOT / "fixtures" / "catalog-v10.json"
    try:
        carried_v10 = load_carry_forward(fixture, PROD_CURATOR_PUBKEY_HEX)
        check(f"live v10 fixture verifies and all {len(carried_v10.artifacts)} entries re-derive unchanged",
              carried_v10.catalog_version == 10 and len(carried_v10.artifacts) == 11)
        for a in carried_v10.artifacts:
            check(f"re-derives: {a['path']}", manifest_to_artifact(entry_to_manifest(a), fixture, {}) == a)
    except CatalogAssemblyError as exc:
        check(f"live v10 fixture carry-forward ({exc})", False)
    except ImportError as exc:
        check(f"live v10 fixture carry-forward needs the venv's cryptography ({exc})", False)

    def derived_path(manifest: dict) -> str | None:
        try:
            return manifest_to_artifact(manifest, Path("self-test"), {})["path"]
        except CatalogAssemblyError:
            return None

    sha = "0" * 64
    check("Q6_K served base -> models/Qwen3-1.7B/v1/Qwen3-1.7B-Instruct-Q6_K.gguf",
          derived_path({"kind": "base", "base_model": "Qwen3-1.7B", "name": "Qwen3-1.7B-Instruct-Q6_K.gguf",
                        "quant": "Q6_K", "sha256": sha, "size": 1, "license": "Apache-2.0"})
          == "models/Qwen3-1.7B/v1/Qwen3-1.7B-Instruct-Q6_K.gguf")
    check("triage v3 adapter -> adapters/triage/v3/Qwen3-1.7B/triage-v3-Qwen3-1.7B.gguf",
          derived_path({"kind": "adapter", "base_model": "Qwen3-1.7B", "name": "triage-v3-Qwen3-1.7B.gguf",
                        "adapter_name": "triage", "version": "v3", "sha256": sha, "size": 1, "license": "Apache-2.0"})
          == "adapters/triage/v3/Qwen3-1.7B/triage-v3-Qwen3-1.7B.gguf")
    check("a manifest with no adapter_name/version is behavioral + v1",
          derived_path({"kind": "adapter", "base_model": "Qwen3-1.7B", "name": "behavioral-v1-Qwen3-1.7B.gguf",
                        "sha256": sha, "size": 1, "license": "Apache-2.0"})
          == "adapters/behavioral/v1/Qwen3-1.7B/behavioral-v1-Qwen3-1.7B.gguf")
    check("a path-traversal adapter_name is refused",
          derived_path({"kind": "adapter", "base_model": "Qwen3-1.7B", "name": "..-v3-Qwen3-1.7B.gguf",
                        "adapter_name": "..", "version": "v3", "sha256": sha, "size": 1, "license": "Apache-2.0"}) is None)
    check("the old triage basename (triage-armb-v3-...) is refused against adapter_name triage",
          derived_path({"kind": "adapter", "base_model": "Qwen3-1.7B", "name": "triage-armb-v3-Qwen3-1.7B.gguf",
                        "adapter_name": "triage", "version": "v3", "sha256": sha, "size": 1, "license": "Apache-2.0"}) is None)

    return ok


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if args.self_test:
        return 0 if self_test() else 1

    if not args.manifests:
        print("error: --manifests is required (unless --self-test)", file=sys.stderr)
        return 1

    if args.catalog_version is None and args.base_url is None:
        print("error: one of --catalog-version or --base-url is required (unless --self-test)", file=sys.stderr)
        return 1

    try:
        overrides = parse_license_overrides(args.license_override)
    except CatalogAssemblyError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    carried: CarriedCatalog | None = None
    if args.carry_forward is not None or args.carry_forward_pubkey is not None:
        if args.carry_forward is None or not args.carry_forward_pubkey:
            print("error: --carry-forward and --carry-forward-pubkey must be given together", file=sys.stderr)
            return 1
        try:
            carried = load_carry_forward(args.carry_forward, args.carry_forward_pubkey)
        except CatalogAssemblyError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        print(
            f"[carry-forward] {args.carry_forward} verifies; carrying {len(carried.artifacts)} artifact(s) "
            f"from catalog_version={carried.catalog_version}",
            flush=True,
        )

    if args.catalog_version is not None:
        try:
            check_version_in_bounds(args.catalog_version)
        except CatalogAssemblyError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        catalog_version = args.catalog_version
    else:
        try:
            published = fetch_published_version(args.base_url)
        except CatalogAssemblyError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        catalog_version = published + 1
        try:
            # fetch_published_version already bound-checked `published`
            # itself; this second check catches the value ONE BELOW the
            # ceiling (in-bounds on its own) overflowing into an
            # out-of-bounds "next" version once +1 is applied.
            check_version_in_bounds(catalog_version)
        except CatalogAssemblyError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        print(
            f"[version] currently published catalog_version={published} at {args.base_url} -> using {catalog_version}",
            flush=True,
        )
        if carried is not None:
            try:
                check_carried_is_published(carried_version=carried.catalog_version, published_version=published)
            except CatalogAssemblyError as exc:
                print(f"error: {exc}", file=sys.stderr)
                return 1

    if carried is not None:
        try:
            check_version_after_carried(catalog_version, carried.catalog_version)
        except CatalogAssemblyError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1

    try:
        catalog = build_catalog(args.manifests, catalog_version, overrides, carried=carried.artifacts if carried else None)
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
