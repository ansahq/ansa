#!/usr/bin/env python3
"""Generate and check the Ansa Cargo.lock dependency/license inventory.

The inventory is intentionally a build/tooling artifact, not a legal review.
It reads package identity, source and checksum from Cargo.lock, obtains declared
license metadata from `cargo metadata --locked --offline`, and records explicit
review status for every package.

Run from the repository root:

    python scripts/dependencies/inventory.py
    python scripts/dependencies/inventory.py --check

`--check` is deterministic and suitable for CI. It does not modify files and
exits non-zero when the committed inventory does not match Cargo.lock/metadata.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from collections import Counter
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import quote

SPDX_VERSION = "SPDX-2.3"
GENERATOR = "scripts/dependencies/inventory.py"
INVENTORY_MARKDOWN = "dependency-inventory.md"
INVENTORY_SPDX_JSON = "dependency-inventory.spdx.json"
REVIEW_FILE = "scripts/dependencies/review-status.toml"

# The filters describe the host triples used by .github/workflows/ci.yml. They
# are deliberately not a claim that every Cargo.lock target is represented.
PLATFORM_TARGETS: dict[str, tuple[str, ...]] = {
    "windows": ("x86_64-pc-windows-msvc",),
    "macos": ("x86_64-apple-darwin", "aarch64-apple-darwin"),
    "linux": ("x86_64-unknown-linux-gnu",),
}

# Conservative lists used only to decide whether a Cargo `license` string can
# be emitted as an SPDX expression without silently rewriting its meaning.
# Payload/exception IDs are deliberately separate: an exception is never a
# standalone license, and WITH accepts only a known license plus a known
# exception. Unknown or legacy slash-separated expressions are emitted as
# NOASSERTION and retained verbatim in licenseComments and the Markdown table.
KNOWN_LICENSE_IDS = {
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "ISC",
    "GPL-2.0-only",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "MIT",
    "MIT-0",
    "MPL-2.0",
    "NCSA",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Unlicense",
    "Zlib",
}
# Extend only with reviewed SPDX exception identifiers. A future exception not
# listed here makes the expression fall back to NOASSERTION rather than being
# accepted by guesswork.
KNOWN_EXCEPTIONS = {
    "LLVM-exception",
}

SPDX_ID_RE = re.compile(r"[^A-Za-z0-9.-]+")
SPDX_TOKEN_RE = re.compile(r"\s*(\(|\)|[A-Za-z0-9][A-Za-z0-9.+-]*)")


class InventoryError(RuntimeError):
    """A deterministic, user-facing inventory failure."""


def default_root() -> Path:
    return Path(__file__).resolve().parents[2]


def canonical_lock_bytes(raw: bytes) -> bytes:
    """Normalize only CRLF pairs to LF for lock identity hashing.

    Lone carriage returns, UTF-8 bytes and every other byte are preserved. The
    generator never rewrites Cargo.lock; this is an identity normalization so a
    Windows CRLF checkout and a Linux LF checkout describe the same lock.
    """
    return raw.replace(b"\r\n", b"\n")


def lock_identity_sha256(raw: bytes) -> str:
    return hashlib.sha256(canonical_lock_bytes(raw)).hexdigest()


def load_lockfile(path: Path) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise InventoryError(f"cannot read {path}: {error}") from error
    except tomllib.TOMLDecodeError as error:
        raise InventoryError(f"invalid TOML in {path}: {error}") from error

    lock_version = raw.get("version")
    if not isinstance(lock_version, int) or lock_version < 3:
        raise InventoryError(
            f"{path}: unsupported lockfile version {lock_version!r}; expected integer >= 3"
        )
    packages = raw.get("package")
    if not isinstance(packages, list):
        raise InventoryError(f"{path}: missing [[package]] entries")

    seen: set[tuple[str, str]] = set()
    for package in packages:
        if not isinstance(package, dict):
            raise InventoryError(f"{path}: package entry is not a table")
        name = package.get("name")
        version = package.get("version")
        if not isinstance(name, str) or not isinstance(version, str):
            raise InventoryError(f"{path}: package without string name/version: {package!r}")
        key = (name, version)
        if key in seen:
            raise InventoryError(
                f"{path}: duplicate name/version {name} {version}; "
                "source-disambiguated Cargo.lock identities are not supported by "
                "this inventory slice (failing closed)"
            )
        seen.add(key)
    return raw, packages


def run_cargo_metadata(
    root: Path,
    cargo: str,
    toolchain: str | None,
    target: str | None = None,
) -> dict[str, Any]:
    command = [cargo]
    if toolchain:
        command.append(f"+{toolchain}")
    command.extend(["metadata", "--locked", "--offline", "--format-version", "1"])
    if target:
        command.extend(["--filter-platform", target])

    try:
        completed = subprocess.run(
            command,
            cwd=root,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            check=False,
        )
    except OSError as error:
        raise InventoryError(f"cannot execute {' '.join(command)}: {error}") from error

    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        raise InventoryError(
            "cargo metadata failed; Cargo.lock is stale or the locked/offline "
            f"resolution is unavailable.\n  command: {' '.join(command)}\n  {detail}"
        )
    try:
        metadata = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise InventoryError(f"cargo metadata returned invalid JSON: {error}") from error
    if not isinstance(metadata, dict):
        raise InventoryError("cargo metadata did not return a JSON object")
    return metadata


def metadata_packages(metadata: dict[str, Any], context: str) -> dict[tuple[str, str], dict[str, Any]]:
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise InventoryError(f"{context}: cargo metadata package list missing")
    indexed: dict[tuple[str, str], dict[str, Any]] = {}
    for package in packages:
        name = package.get("name")
        version = package.get("version")
        if not isinstance(name, str) or not isinstance(version, str):
            raise InventoryError(f"{context}: metadata package without name/version")
        key = (name, version)
        if key in indexed:
            raise InventoryError(f"{context}: duplicate metadata package {name} {version}")
        indexed[key] = package
    return indexed


def load_review_status(path: Path) -> dict[str, str]:
    if not path.exists():
        raise InventoryError(f"review status file is missing: {path}")
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise InventoryError(f"cannot read review status {path}: {error}") from error

    reviewed = data.get("reviewed", {})
    if not isinstance(reviewed, dict):
        raise InventoryError(f"{path}: [reviewed] must be a table")
    result: dict[str, str] = {}
    for key, reason in reviewed.items():
        if not isinstance(key, str) or not re.fullmatch(r"[^@\s]+@[^@\s]+", key):
            raise InventoryError(f"{path}: reviewed key must be 'name@version', got {key!r}")
        if not isinstance(reason, str) or not reason.strip():
            raise InventoryError(f"{path}: reviewed entry {key!r} needs a non-empty reason")
        result[key] = reason.strip()
    return result


class _SpdxParseError(ValueError):
    pass


class _SpdxExpressionParser:
    """Small fail-closed parser for the SPDX expression subset we emit.

    Grammar::

        or_expr  := and_expr (OR and_expr)*
        and_expr := with_expr (AND with_expr)*
        with_expr := LICENSE_ID (WITH EXCEPTION_ID)? | '(' or_expr ')'

    WITH is deliberately restricted to a simple known license ID followed by a
    known exception ID. Parenthesised compounds and standalone exceptions are
    rejected, as are unbalanced or trailing tokens.
    """

    def __init__(self, tokens: list[str]) -> None:
        self.tokens = tokens
        self.position = 0

    def parse(self) -> bool:
        if not self.tokens:
            return False
        try:
            self._parse_or()
            return self.position == len(self.tokens)
        except _SpdxParseError:
            return False

    def _peek(self) -> str | None:
        if self.position >= len(self.tokens):
            return None
        return self.tokens[self.position]

    def _consume(self, expected: str | None = None) -> str:
        token = self._peek()
        if token is None:
            raise _SpdxParseError("unexpected end of expression")
        if expected is not None and token != expected:
            raise _SpdxParseError(f"expected {expected!r}, got {token!r}")
        self.position += 1
        return token

    def _consume_identifier(self) -> str:
        token = self._peek()
        if token is None or token in {"(", ")", "AND", "OR", "WITH"}:
            raise _SpdxParseError("expected an identifier")
        self.position += 1
        return token

    def _parse_or(self) -> None:
        self._parse_and()
        while self._peek() == "OR":
            self._consume("OR")
            self._parse_and()

    def _parse_and(self) -> None:
        self._parse_with()
        while self._peek() == "AND":
            self._consume("AND")
            self._parse_with()

    def _parse_with(self) -> None:
        if self._peek() == "(":
            self._consume("(")
            self._parse_or()
            self._consume(")")
            if self._peek() == "WITH":
                raise _SpdxParseError("WITH may not follow a parenthesised expression")
            return

        license_id = self._consume_identifier()
        if license_id not in KNOWN_LICENSE_IDS:
            raise _SpdxParseError(f"unknown license identifier {license_id!r}")
        if self._peek() == "WITH":
            self._consume("WITH")
            exception_id = self._consume_identifier()
            if exception_id not in KNOWN_EXCEPTIONS:
                raise _SpdxParseError(
                    f"unknown license exception identifier {exception_id!r}"
                )


def is_spdx_expression(value: str) -> bool:
    """Accept only balanced, fully recognised SPDX expressions.

    This is intentionally conservative: refusing to normalise legacy forms such
    as `MIT/Apache-2.0` is safer than inventing a legal interpretation. ``MIT)``,
    ``MIT WITH MIT``, ``LLVM-exception`` and trailing operators all fail closed.
    """
    text = value.strip()
    if not text or "/" in text:
        return False

    tokens: list[str] = []
    position = 0
    while position < len(text):
        match = SPDX_TOKEN_RE.match(text, position)
        if match is None:
            return False
        tokens.append(match.group(1))
        position = match.end()

    return _SpdxExpressionParser(tokens).parse()


def source_origin(source: str | None, is_workspace: bool) -> str:
    if is_workspace:
        return "workspace"
    if source and source.startswith("registry+"):
        return "registry"
    if source and source.startswith("git+"):
        return "git"
    return "path"


def download_location(record: dict[str, Any]) -> str:
    if record["origin"] == "registry":
        name = record["name"]
        version = record["version"]
        return f"https://crates.io/api/v1/crates/{name}/{version}/download"
    if record["origin"] == "git":
        source = record["source"] or ""
        url = source[4:] if source.startswith("git+") else source
        return url.split("#", 1)[0] or "NOASSERTION"
    return "NOASSERTION"


def spdx_package_id(record: dict[str, Any]) -> str:
    base = SPDX_ID_RE.sub("-", f"{record['name']}-{record['version']}").strip("-.")
    digest = hashlib.sha256(
        f"{record['name']}\0{record['version']}".encode("utf-8")
    ).hexdigest()[:10]
    return f"SPDXRef-Package-{base}-{digest}"


def resolve_lock_dependency(
    dependency: str,
    packages_by_name: dict[str, list[tuple[str, str]]],
) -> tuple[str, str]:
    name, _, version = dependency.partition(" ")
    candidates = packages_by_name.get(name)
    if not candidates:
        raise InventoryError(f"Cargo.lock edge names unknown package {name!r}")
    if version:
        matches = [key for key in candidates if key[1] == version]
        if len(matches) != 1:
            raise InventoryError(
                f"Cargo.lock edge {dependency!r} does not identify one package"
            )
        return matches[0]
    if len(candidates) != 1:
        raise InventoryError(
            f"Cargo.lock edge {dependency!r} is ambiguous; version is required"
        )
    return candidates[0]


def build_inventory(
    lock_raw: dict[str, Any],
    lock_packages: list[dict[str, Any]],
    metadata: dict[str, Any],
    platform_metadata: dict[str, dict[str, Any]],
    reviewed: dict[str, str],
) -> dict[str, Any]:
    all_metadata = metadata_packages(metadata, "default metadata")
    lock_by_key = {(package["name"], package["version"]): package for package in lock_packages}
    lock_keys = set(lock_by_key)
    metadata_keys = set(all_metadata)

    if lock_keys != metadata_keys:
        missing = sorted(lock_keys - metadata_keys)
        extra = sorted(metadata_keys - lock_keys)
        raise InventoryError(
            "cargo metadata and Cargo.lock package sets differ; refusing to claim "
            f"a complete inventory (missing metadata={missing[:5]}, "
            f"extra metadata={extra[:5]})"
        )

    # cargo metadata always reports workspace_members in real invocations. The
    # empty case is retained for small unit fixtures and simply means no
    # first-party package is present.
    workspace_ids = set(metadata.get("workspace_members") or [])
    workspace_keys = {
        key for key, package in all_metadata.items() if package.get("id") in workspace_ids
    }

    platform_keys: dict[str, set[tuple[str, str]]] = {}
    for platform, filtered in platform_metadata.items():
        keys = set(metadata_packages(filtered, f"{platform} metadata"))
        unexpected = keys - lock_keys
        if unexpected:
            raise InventoryError(
                f"{platform} metadata contains packages absent from Cargo.lock: "
                f"{sorted(unexpected)[:5]}"
            )
        platform_keys[platform] = keys

    packages_by_name: dict[str, list[tuple[str, str]]] = {}
    for name, version in sorted(lock_keys):
        packages_by_name.setdefault(name, []).append((name, version))

    records: list[dict[str, Any]] = []
    for key in sorted(lock_keys):
        name, version = key
        lock_package = lock_by_key[key]
        metadata_package = all_metadata[key]
        is_workspace = key in workspace_keys
        source = lock_package.get("source")
        if source is not None and not isinstance(source, str):
            raise InventoryError(f"Cargo.lock source for {name} {version} is not a string")
        checksum = lock_package.get("checksum")
        if checksum is not None and not isinstance(checksum, str):
            raise InventoryError(f"Cargo.lock checksum for {name} {version} is not a string")

        declared = (metadata_package.get("license") or "").strip()
        license_file = metadata_package.get("license_file")
        if declared:
            license_status = "declared"
            license_value = declared
        else:
            license_status = "unknown"
            license_value = "NOASSERTION"

        key_text = f"{name}@{version}"
        if is_workspace:
            if declared:
                review_status = "reviewed"
                review_detail = (
                    "first-party workspace member; declared license: "
                    f"{license_value}; manifest declaration, not a legal review"
                )
            else:
                review_status = "unreviewed"
                review_detail = (
                    "first-party workspace member has no declared license; "
                    "not reviewed"
                )
        elif key_text in reviewed:
            review_status = "reviewed"
            review_detail = reviewed[key_text]
        else:
            review_status = "unreviewed"
            review_detail = "no entry in scripts/dependencies/review-status.toml"

        record: dict[str, Any] = {
            "name": name,
            "version": version,
            "source": source,
            "checksum": checksum,
            "license": license_value,
            "license_status": license_status,
            "license_file": license_file if isinstance(license_file, str) else None,
            "review_status": review_status,
            "review_detail": review_detail,
            "is_workspace": is_workspace,
            "platforms": [
                platform
                for platform, keys in platform_keys.items()
                if key in keys
            ],
        }
        record["origin"] = source_origin(source, is_workspace)
        record["platform_scope"] = (
            "declared-ci-target"
            if record["platforms"]
            else "locked-but-not-in-declared-ci-target"
        )
        record["download_location"] = download_location(record)
        record["spdx_declared"] = (
            record["license"] if is_spdx_expression(record["license"]) else "NOASSERTION"
        )
        record["spdx_id"] = spdx_package_id(record)
        records.append(record)

    relationships: list[dict[str, str]] = []
    by_key = {record["name"] + "\0" + record["version"]: record for record in records}
    for record in records:
        parent = lock_by_key[(record["name"], record["version"])]
        dependencies = parent.get("dependencies", [])
        if not isinstance(dependencies, list):
            raise InventoryError(
                f"Cargo.lock dependencies for {record['name']} {record['version']} "
                "is not a list"
            )
        for dependency in dependencies:
            if not isinstance(dependency, str):
                raise InventoryError("Cargo.lock dependency edge is not a string")
            dep_key = resolve_lock_dependency(dependency, packages_by_name)
            child = by_key[dep_key[0] + "\0" + dep_key[1]]
            relationships.append(
                {
                    "spdxElementId": record["spdx_id"],
                    "relationshipType": "DEPENDS_ON",
                    "relatedSpdxElement": child["spdx_id"],
                }
            )

    return {
        "lock_version": lock_raw.get("version"),
        "records": records,
        "relationships": relationships,
        "platform_diagnostics": {
            platform: len(keys) for platform, keys in sorted(platform_keys.items())
        },
        "workspace_count": len(workspace_keys),
        "not_observed_count": sum(
            1 for record in records if not record["platforms"]
        ),
    }


def markdown_escape(value: str) -> str:
    return value.replace("|", "\\|")


def render_markdown(inventory: dict[str, Any], lock_sha256: str) -> str:
    records: list[dict[str, Any]] = inventory["records"]
    total = len(records)
    workspace = inventory["workspace_count"]
    third_party = total - workspace
    unknown_license = sum(1 for r in records if r["license_status"] == "unknown")
    noassertion_license = sum(
        1 for r in records if r["spdx_declared"] == "NOASSERTION"
    )
    unreviewed = sum(1 for r in records if r["review_status"] == "unreviewed")
    reviewed = total - unreviewed
    licenses = Counter(r["license"] for r in records)
    not_observed = inventory["not_observed_count"]

    lines = [
        "# Cargo dependency and license inventory",
        "",
        f"<!-- Generated by {GENERATOR}; do not edit by hand. -->",
        "",
        "This file is generated from `Cargo.lock`, `cargo metadata --locked --offline`",
        "and `scripts/dependencies/review-status.toml`. It is a build/tooling artifact,",
        "not a legal review or a claim that every locked package is used on every OS.",
        "",
        f"- Cargo.lock SHA-256 (LF-normalized): `{lock_sha256}`",
        "- Lock identity normalizes CRLF pairs to LF only; lone CR, UTF-8 and all",
        "  other bytes are preserved. Registry artifact checksums below are raw.",
        f"- Lockfile format: v{inventory['lock_version']}",
        f"- Packages: **{total}** ({workspace} first-party workspace, {third_party} third-party)",
        f"- License declared: **{total - unknown_license}**; license unknown: **{unknown_license}**",
        f"- SPDX expression emitted: **{total - noassertion_license}**; `NOASSERTION`: **{noassertion_license}** (allowlist/legacy limitation, not an invalid-license judgment)",
        f"- Review status: **{reviewed} reviewed, {unreviewed} unreviewed**",
        "- Review policy: first-party workspace members with a declared license are",
        "  marked reviewed for that manifest declaration; this is not a legal review.",
        "  Third-party packages remain unreviewed unless explicitly recorded in",
        "  `scripts/dependencies/review-status.toml`.",
        "",
        "## Platform scope",
        "",
        "Package membership was filtered with `cargo metadata --filter-platform` for",
        "the host triples used by CI: `x86_64-pc-windows-msvc`,",
        "`x86_64-apple-darwin`, `aarch64-apple-darwin` and",
        "`x86_64-unknown-linux-gnu`. A package with no listed platform is still in",
        f"`Cargo.lock` but was not observed for those filters ({not_observed} packages).",
        "This is not a cross-platform build claim, and it excludes system/native",
        "libraries, Rust toolchain components, GitHub Actions, and the excluded",
        "`experiments/` workspace.",
        "",
        "| Filter | Packages observed |",
        "|---|---:|",
    ]
    for platform, count in inventory["platform_diagnostics"].items():
        lines.append(f"| {platform} | {count} |")

    lines.extend(
        [
            "",
            "## License expressions",
            "",
            "| Declared license | Packages |",
            "|---|---:|",
        ]
    )
    for license_value, count in sorted(licenses.items(), key=lambda item: (-item[1], item[0])):
        lines.append(f"| {markdown_escape(license_value)} | {count} |")

    lines.extend(
        [
            "",
            "License strings are copied verbatim from Cargo metadata. Slash-separated",
            "legacy forms are not normalised into a different SPDX expression.",
            "",
            "## Packages",
            "",
            "| Package | Version | Platforms | Source | SHA-256 | License | Review |",
            "|---|---|---|---|---|---|---|",
        ]
    )
    for record in records:
        platforms = ", ".join(record["platforms"]) if record["platforms"] else "none (locked only)"
        source = record["source"] or ("workspace path" if record["is_workspace"] else "path")
        review = record["review_status"]
        lines.append(
            "| {name} | {version} | {platforms} | {source} | {checksum} | {license} | {review} |".format(
                name=markdown_escape(record["name"]),
                version=markdown_escape(record["version"]),
                platforms=markdown_escape(platforms),
                source=markdown_escape(source),
                checksum=record["checksum"] or "none",
                license=markdown_escape(record["license"]),
                review=review,
            )
        )
    lines.append("")
    return "\n".join(lines)


def render_spdx(inventory: dict[str, Any], lock_sha256: str) -> str:
    records: list[dict[str, Any]] = inventory["records"]
    spdx_packages: list[dict[str, Any]] = []
    relationships: list[dict[str, str]] = []
    for record in records:
        comment_parts = [
            f"Review status: {record['review_status']}",
            record["review_detail"],
            f"Platform scope: {record['platform_scope']}",
            f"Platforms: {', '.join(record['platforms']) if record['platforms'] else 'none'}",
            f"Cargo source: {record['source'] or 'path'}",
            f"License verbatim: {record['license']}",
        ]
        if record["license_status"] == "unknown":
            comment_parts.append("Cargo metadata did not declare a license.")
        if record["spdx_declared"] == "NOASSERTION" and record["license"] != "NOASSERTION":
            comment_parts.append(
                "Declared string was not emitted as an SPDX expression; no normalisation performed."
            )
        package = {
            "SPDXID": record["spdx_id"],
            "name": record["name"],
            "versionInfo": record["version"],
            "downloadLocation": record["download_location"],
            "sourceInfo": record["source"] or "path",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": record["spdx_declared"],
            "copyrightText": "NOASSERTION",
            "licenseComments": "; ".join(comment_parts),
            "comment": (
                f"Review status: {record['review_status']}; "
                f"platform scope: {record['platform_scope']}"
            ),
        }
        if record["checksum"]:
            package["checksums"] = [
                {"algorithm": "SHA256", "checksumValue": record["checksum"]}
            ]
        if record["license_file"]:
            package["comment"] += f"; license file: {record['license_file']}"
        package["externalRefs"] = [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": "pkg:cargo/{}@{}".format(
                    quote(record["name"], safe=""),
                    quote(record["version"], safe=""),
                ),
            }
        ]
        spdx_packages.append(package)
        relationships.append(
            {
                "spdxElementId": "SPDXRef-DOCUMENT",
                "relationshipType": "DESCRIBES",
                "relatedSpdxElement": record["spdx_id"],
            }
        )

    for relation in inventory["relationships"]:
        relationships.append(relation)

    document: dict[str, Any] = {
        "spdxVersion": SPDX_VERSION,
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": "Ansa Cargo.lock dependency inventory",
        "creationInfo": {
            "created": "1970-01-01T00:00:00Z",
            "creators": [
                f"Tool: {GENERATOR}",
                "Organization: Ansa contributors",
            ],
            "comment": (
                "Deterministic artifact generated from Cargo.lock, cargo metadata "
                "--locked --offline and an explicit review-status file. The fixed "
                "creation date avoids a timestamp-dependent diff. licenseConcluded is "
                "NOASSERTION and no legal review is claimed."
            ),
        },
        "packages": spdx_packages,
        "relationships": relationships,
        "annotations": [
            {
                "annotationDate": "1970-01-01T00:00:00Z",
                "annotationType": "OTHER",
                "annotator": f"Tool: {GENERATOR}",
                "comment": (
                    "Review statuses are explicit per package. All third-party "
                    "packages are unreviewed unless review-status.toml records evidence."
                ),
            }
        ],
    }

    # The namespace must change when license metadata or review status changes,
    # even if Cargo.lock is unchanged. Hash the canonical document content plus
    # the lock hash, then expose both the lock hash and content digest. The
    # namespace field itself is excluded to avoid a circular definition.
    canonical = json.dumps(
        document,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    )
    content_digest = hashlib.sha256(
        f"{lock_sha256}\n{canonical}".encode("utf-8")
    ).hexdigest()
    document["documentNamespace"] = (
        f"https://ansa.invalid/spdx/cargo-lock-lf-{lock_sha256}-{content_digest}"
    )
    document["comment"] = (
        f"Cargo.lock SHA-256 (LF-normalized, CRLF pairs only): {lock_sha256}. "
        f"SPDX content digest: {content_digest}."
    )
    return json.dumps(document, indent=2, ensure_ascii=False) + "\n"


def render_outputs(
    root: Path,
    lock_path: Path,
    cargo: str,
    toolchain: str | None,
    review_path: Path,
) -> tuple[dict[str, str], dict[str, Any]]:
    lock_raw, lock_packages = load_lockfile(lock_path)
    metadata = run_cargo_metadata(root, cargo, toolchain)
    platform_metadata: dict[str, dict[str, Any]] = {}
    for target in sorted({target for targets in PLATFORM_TARGETS.values() for target in targets}):
        platform_metadata[target] = run_cargo_metadata(root, cargo, toolchain, target)

    reviewed = load_review_status(review_path)
    # Build one merged metadata document per platform label. A package is
    # observed for the platform when any configured target triple reaches it.
    merged_packages: dict[str, dict[tuple[str, str], dict[str, Any]]] = {}
    for platform, targets in PLATFORM_TARGETS.items():
        merged: dict[tuple[str, str], dict[str, Any]] = {}
        for target in targets:
            for package in platform_metadata[target].get("packages", []):
                merged[(package["name"], package["version"])] = package
        merged_packages[platform] = merged

    merged_metadata: dict[str, dict[str, Any]] = {
        platform: {"packages": list(packages.values())}
        for platform, packages in merged_packages.items()
    }
    inventory = build_inventory(
        lock_raw,
        lock_packages,
        metadata,
        merged_metadata,
        reviewed,
    )
    # Hash a canonical LF view, not the raw checkout bytes: Windows CRLF and
    # Linux LF worktrees must produce the same inventory identity. Cargo.lock
    # itself is never rewritten, and Cargo's registry checksums stay raw.
    lock_sha256 = lock_identity_sha256(lock_path.read_bytes())
    outputs = {
        INVENTORY_MARKDOWN: render_markdown(inventory, lock_sha256),
        INVENTORY_SPDX_JSON: render_spdx(inventory, lock_sha256),
    }
    return outputs, inventory


def write_outputs(output_dir: Path, outputs: dict[str, str]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    for name, content in outputs.items():
        (output_dir / name).write_text(content, encoding="utf-8", newline="\n")


def check_outputs(output_dir: Path, outputs: dict[str, str]) -> list[str]:
    problems: list[str] = []
    for name, expected in outputs.items():
        path = output_dir / name
        if not path.exists():
            problems.append(f"missing {path}")
            continue
        actual = path.read_text(encoding="utf-8")
        if actual != expected:
            problems.append(f"out of date {path}")
    return problems


def parse_args(argv: Iterable[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=default_root(),
        help="Repository root (default: parent of scripts/dependencies)",
    )
    parser.add_argument("--lockfile", type=Path, help="Cargo.lock path (default: ROOT/Cargo.lock)")
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Generated artifact directory (default: ROOT/docs/build)",
    )
    parser.add_argument(
        "--review-file",
        type=Path,
        default=None,
        help=f"Review status TOML (default: ROOT/{REVIEW_FILE})",
    )
    parser.add_argument("--cargo", default="cargo", help="Cargo executable")
    parser.add_argument(
        "--toolchain",
        default=None,
        help="Optional rustup toolchain, e.g. stable (passes cargo +TOOLCHAIN)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Verify committed outputs without writing; non-zero on drift",
    )
    return parser.parse_args(argv)


def main(argv: Iterable[str] | None = None) -> int:
    args = parse_args(argv)
    root = args.root.resolve()
    lock_path = (args.lockfile or root / "Cargo.lock").resolve()
    output_dir = (args.output_dir or root / "docs" / "build").resolve()
    review_path = (args.review_file or root / REVIEW_FILE).resolve()

    try:
        outputs, inventory = render_outputs(
            root=root,
            lock_path=lock_path,
            cargo=args.cargo,
            toolchain=args.toolchain,
            review_path=review_path,
        )
        if args.check:
            problems = check_outputs(output_dir, outputs)
            if problems:
                for problem in problems:
                    print(f"inventory: {problem}", file=sys.stderr)
                print(
                    "inventory: regenerate with `python scripts/dependencies/inventory.py` "
                    "and review the diff",
                    file=sys.stderr,
                )
                return 1
            print(
                f"inventory: {len(inventory['records'])} packages checked; "
                f"{inventory['not_observed_count']} not observed for declared CI targets"
            )
            return 0
        write_outputs(output_dir, outputs)
        print(
            f"inventory: wrote {len(inventory['records'])} packages to {output_dir}; "
            f"{inventory['not_observed_count']} not observed for declared CI targets"
        )
        return 0
    except InventoryError as error:
        print(f"inventory: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
