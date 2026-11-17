"""Regression guard for the Linux libc/xattr compatibility pin.

``libc`` 0.2.190 removed the deprecated-on-Linux ``ENOATTR`` alias
(rust-lang/libc#5401). ``xattr`` 0.2.3, which reaches the graph through
``zed-async-tar``, references ``::libc::ENOATTR`` unconditionally on unix
targets, so Linux Clippy failed with ``error[E0425]``. The bounded fix is a
lockfile-only downgrade to the newest libc that still defines Linux ``ENOATTR``.

These tests use only ``Cargo.lock``, ``Cargo.toml`` text and the checked-in
compatibility record; they never run Cargo and add no dependencies. The
compatibility record is ``docs/build/linux-compatibility.md``.
"""

from __future__ import annotations

import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
LOCK = ROOT / "Cargo.lock"
CARGO_TOML = ROOT / "Cargo.toml"
DOC = ROOT / "docs" / "build" / "linux-compatibility.md"

# Newest libc release verified to still define ENOATTR for Linux targets, and
# the first release that removed it. Keep these in sync with the record.
PINNED_LIBC = "0.2.189"
FIRST_LIBC_WITHOUT_LINUX_ENOATTR = "0.2.190"
PINNED_LIBC_SHA256 = "3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2"
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"

XATTR_VERSION = "0.2.3"
ZED_ASYNC_TAR_VERSION = "0.5.0-zed"

REMEDIATION = (
    "xattr {xattr} references ::libc::ENOATTR, which libc {first_bad} removed for "
    "Linux; keep libc at {pinned} until xattr/zed-async-tar are upgraded. "
    "Restore with: cargo +stable update -p libc --precise {pinned}"
).format(xattr=XATTR_VERSION, first_bad=FIRST_LIBC_WITHOUT_LINUX_ENOATTR, pinned=PINNED_LIBC)

REQUIRED_DOC_FRAGMENTS = (
    "0.2.189",
    "0.2.190",
    "ENOATTR",
    "44072419286",
    "https://github.com/rust-lang/libc/pull/5401",
    "https://github.com/rust-lang/libc/releases/tag/0.2.190",
    "cargo +stable update -p libc --precise 0.2.189",
    "python scripts/dependencies/inventory.py --toolchain stable",
    PINNED_LIBC_SHA256,
    "not executed locally",
)


def version_tuple(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split("."))


def parse_lockfile() -> list[dict[str, object]]:
    """Parse the subset of Cargo.lock v3 this guard needs."""

    data = tomllib.loads(LOCK.read_text(encoding="utf-8"))
    packages = data.get("package")
    if not isinstance(packages, list):
        raise AssertionError("Cargo.lock has no [[package]] array")
    return packages


def find_package(packages: list[dict[str, object]], name: str) -> list[dict[str, object]]:
    return [package for package in packages if package.get("name") == name]


def require_edge(
    packages: list[dict[str, object]],
    parent: dict[str, object],
    child_name: str,
    child_version: str,
) -> None:
    """Assert ``parent`` has a Cargo.lock dependency edge to child_name@child_version."""

    edges = parent.get("dependencies", [])
    assert isinstance(edges, list), f"{parent['name']} has malformed dependencies"
    for edge in edges:
        assert isinstance(edge, str), f"{parent['name']} has a non-string dependency edge"
        parts = edge.split(" ", 1)
        if parts[0] != child_name:
            continue
        if len(parts) == 2 and parts[1] != child_version:
            continue
        return
    raise AssertionError(
        f"Cargo.lock no longer has {parent['name']} -> {child_name} {child_version}; "
        "the Linux failure chain changed, so revisit the libc pin and "
        "docs/build/linux-compatibility.md"
    )


class LinuxCompatibilityLockTests(unittest.TestCase):
    def setUp(self) -> None:
        self.packages = parse_lockfile()

    def test_libc_is_pinned_to_newest_enoattr_release(self) -> None:
        libc = find_package(self.packages, "libc")
        self.assertEqual(
            len(libc),
            1,
            "Cargo.lock must contain exactly one libc package; multiple libc "
            "versions would mean the pin no longer bounds every target. " + REMEDIATION,
        )
        entry = libc[0]
        version = str(entry.get("version"))
        self.assertEqual(version, PINNED_LIBC, "libc lock version drifted. " + REMEDIATION)
        self.assertLess(
            version_tuple(PINNED_LIBC),
            version_tuple(FIRST_LIBC_WITHOUT_LINUX_ENOATTR),
            "the pin must precede the release that removed Linux ENOATTR",
        )
        self.assertEqual(
            entry.get("source"),
            CRATES_IO_SOURCE,
            "libc must come from crates.io, not a git/path/vendored replacement",
        )
        self.assertRegex(str(entry.get("checksum")), r"^[0-9a-f]{64}$")
        self.assertEqual(
            entry.get("checksum"),
            PINNED_LIBC_SHA256,
            "libc 0.2.189 checksum does not match the reviewed crates.io artifact",
        )

    def test_enoattr_chain_is_still_locked(self) -> None:
        gpui = find_package(self.packages, "gpui")
        http_client = find_package(self.packages, "gpui_http_client")
        async_tar = find_package(self.packages, "zed-async-tar")
        xattr = find_package(self.packages, "xattr")

        self.assertEqual(len(gpui), 1, "expected one locked gpui package")
        self.assertEqual(len(http_client), 1, "expected one locked gpui_http_client package")
        self.assertEqual(len(async_tar), 1, "expected one locked zed-async-tar package")
        self.assertEqual(len(xattr), 1, "expected one locked xattr package")

        self.assertEqual(gpui[0].get("version"), "0.2.2")
        self.assertEqual(http_client[0].get("version"), "0.2.2")
        self.assertEqual(async_tar[0].get("version"), ZED_ASYNC_TAR_VERSION)
        self.assertEqual(
            xattr[0].get("version"),
            XATTR_VERSION,
            "xattr changed; if it now uses ENODATA the libc pin can be reconsidered. "
            + REMEDIATION,
        )

        require_edge(self.packages, gpui[0], "gpui_http_client", "0.2.2")
        require_edge(self.packages, http_client[0], "zed-async-tar", ZED_ASYNC_TAR_VERSION)
        require_edge(self.packages, async_tar[0], "xattr", XATTR_VERSION)
        require_edge(self.packages, xattr[0], "libc", PINNED_LIBC)

    def test_no_patch_or_vendoring_override_masks_the_pin(self) -> None:
        manifest = CARGO_TOML.read_text(encoding="utf-8")
        self.assertNotIn(
            "[patch",
            manifest,
            "a [patch] section would bypass the reviewed crates.io resolution",
        )
        self.assertNotIn(
            "paths =",
            manifest,
            "path/vendored dependency replacement is not part of this bounded fix",
        )
        for config in (ROOT / ".cargo" / "config.toml", ROOT / ".cargo" / "config"):
            if config.is_file():
                text = config.read_text(encoding="utf-8")
                self.assertNotIn("replace-with", text, f"{config} overrides a Cargo source")
                self.assertNotIn("[source.", text, f"{config} defines a Cargo source override")

    def test_compatibility_record_matches_the_lock(self) -> None:
        self.assertTrue(DOC.is_file(), "missing docs/build/linux-compatibility.md")
        text = DOC.read_text(encoding="utf-8")
        for fragment in REQUIRED_DOC_FRAGMENTS:
            self.assertIn(
                fragment,
                text,
                f"docs/build/linux-compatibility.md no longer records {fragment!r}; "
                "keep the pin, the evidence and the upgrade procedure together",
            )


if __name__ == "__main__":
    unittest.main()
