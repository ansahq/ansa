"""Tests for the stdlib-only Cargo dependency inventory tooling.

Run from the repository root:

    python -m unittest discover -s scripts/dependencies/tests -v
"""

from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
TOOL = ROOT / "scripts" / "dependencies" / "inventory.py"


def load_tool_module():
    spec = importlib.util.spec_from_file_location("ansa_inventory", TOOL)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {TOOL}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


inventory = load_tool_module()


def package(
    name: str,
    version: str,
    *,
    source: str | None = "registry+https://github.com/rust-lang/crates.io-index",
    checksum: str | None = "0" * 64,
    dependencies: list[str] | None = None,
) -> dict:
    value: dict = {"name": name, "version": version}
    if source is not None:
        value["source"] = source
    if checksum is not None:
        value["checksum"] = checksum
    if dependencies:
        value["dependencies"] = dependencies
    return value


def meta_package(
    name: str,
    version: str,
    *,
    license: str | None = "MIT OR Apache-2.0",
    license_file: str | None = None,
    package_id: str | None = None,
) -> dict:
    value: dict = {
        "id": package_id or f"registry+https://github.com/rust-lang/crates.io-index#{name}@{version}",
        "name": name,
        "version": version,
        "license": license,
    }
    if license_file:
        value["license_file"] = license_file
    return value


def metadata_doc(packages: list[dict], workspace_members: list[str] | None = None) -> dict:
    return {
        "packages": packages,
        "workspace_members": workspace_members or [],
    }


class InventoryUnitTests(unittest.TestCase):
    def test_review_status_platforms_and_unknown_license(self) -> None:
        workspace_id = "path+file:///repo#ansa-domain@0.1.0"
        lock = {
            "version": 3,
            "package": [
                package("ansa-domain", "0.1.0", source=None, checksum=None),
                package("shared", "1.0.0"),
                package("winonly", "2.0.0"),
                package("unlisted", "3.0.0"),
            ],
        }
        metadata = metadata_doc(
            [
                meta_package(
                    "ansa-domain",
                    "0.1.0",
                    license="Apache-2.0",
                    package_id=workspace_id,
                ),
                meta_package("shared", "1.0.0"),
                meta_package("winonly", "2.0.0"),
                meta_package("unlisted", "3.0.0", license=None),
            ],
            workspace_members=[workspace_id],
        )
        filtered = {
            "windows": metadata_doc(
                [meta_package("shared", "1.0.0"), meta_package("winonly", "2.0.0")]
            ),
            "linux": metadata_doc([meta_package("shared", "1.0.0")]),
        }
        result = inventory.build_inventory(
            lock,
            lock["package"],
            metadata,
            filtered,
            {"shared@1.0.0": "license file reviewed in test fixture"},
        )
        records = {record["name"]: record for record in result["records"]}

        self.assertEqual(records["ansa-domain"]["review_status"], "reviewed")
        self.assertIn("Apache-2.0", records["ansa-domain"]["review_detail"])
        self.assertIn("not a legal review", records["ansa-domain"]["review_detail"])
        self.assertEqual(records["shared"]["review_status"], "reviewed")
        self.assertEqual(
            records["shared"]["review_detail"], "license file reviewed in test fixture"
        )
        self.assertEqual(records["winonly"]["review_status"], "unreviewed")
        self.assertEqual(records["unlisted"]["review_status"], "unreviewed")
        self.assertEqual(records["unlisted"]["license_status"], "unknown")
        self.assertEqual(records["unlisted"]["license"], "NOASSERTION")
        self.assertEqual(records["winonly"]["platforms"], ["windows"])
        self.assertEqual(records["unlisted"]["platforms"], [])
        self.assertEqual(
            records["unlisted"]["platform_scope"],
            "locked-but-not-in-declared-ci-target",
        )
        self.assertEqual(result["not_observed_count"], 2)

    def test_metadata_lock_mismatch_is_rejected(self) -> None:
        lock = {"version": 3, "package": [package("present", "1.0.0")]}
        metadata = metadata_doc([meta_package("different", "1.0.0")])
        with self.assertRaises(inventory.InventoryError):
            inventory.build_inventory(lock, lock["package"], metadata, {}, {})

    def test_legacy_slash_license_is_not_rewritten(self) -> None:
        self.assertFalse(inventory.is_spdx_expression("MIT/Apache-2.0"))
        self.assertTrue(inventory.is_spdx_expression("MIT OR Apache-2.0"))
        self.assertTrue(inventory.is_spdx_expression("Apache-2.0 WITH LLVM-exception"))
        self.assertFalse(inventory.is_spdx_expression("Unknown-License-1.0"))

    def test_spdx_parser_rejects_parenthesis_operator_and_with_misuse(self) -> None:
        for value in (
            "MIT)",
            "(MIT",
            "()",
            "(MIT))",
            "MIT AND",
            "MIT OR",
            "OR MIT",
            "MIT OR OR Apache-2.0",
            "MIT AND ()",
            "MIT WITH MIT",
            "MIT WITH Unknown-exception",
            "MIT WITH",
            "LLVM-exception",
            "MIT WITH (LLVM-exception)",
            "(MIT OR Apache-2.0) WITH LLVM-exception",
            "MIT WITH LLVM-exception WITH Apache-2.0",
        ):
            with self.subTest(value=value):
                self.assertFalse(inventory.is_spdx_expression(value))

    def test_spdx_parser_accepts_valid_nested_expressions(self) -> None:
        for value in (
            "(MIT)",
            "((MIT OR Apache-2.0) AND BSD-3-Clause)",
            "MIT AND (Apache-2.0 WITH LLVM-exception)",
            "MIT WITH LLVM-exception OR Apache-2.0",
            "(MIT OR (Apache-2.0 AND BSD-3-Clause))",
        ):
            with self.subTest(value=value):
                self.assertTrue(inventory.is_spdx_expression(value))

    def test_spdx_parser_accepts_current_allowlist_ids(self) -> None:
        for value in (
            "(MIT OR Apache-2.0) AND NCSA",
            "CC0-1.0 OR MIT-0 OR Apache-2.0",
            "Apache-2.0 OR GPL-2.0-only",
        ):
            with self.subTest(value=value):
                self.assertTrue(inventory.is_spdx_expression(value))

    def test_first_party_review_detail_derives_current_license(self) -> None:
        workspace_id = "path+file:///repo#future-license@0.1.0"
        lock = {
            "version": 3,
            "package": [package("future-license", "0.1.0", source=None, checksum=None)],
        }
        metadata = metadata_doc(
            [
                meta_package(
                    "future-license",
                    "0.1.0",
                    license="MIT",
                    package_id=workspace_id,
                )
            ],
            workspace_members=[workspace_id],
        )
        result = inventory.build_inventory(lock, lock["package"], metadata, {}, {})
        record = result["records"][0]
        self.assertEqual(record["review_status"], "reviewed")
        self.assertIn("MIT", record["review_detail"])
        self.assertNotIn("Apache-2.0", record["review_detail"])
        self.assertIn("not a legal review", record["review_detail"])

        metadata["packages"][0]["license"] = None
        result = inventory.build_inventory(lock, lock["package"], metadata, {}, {})
        record = result["records"][0]
        self.assertEqual(record["review_status"], "unreviewed")
        self.assertIn("no declared license", record["review_detail"])

    def test_lock_identity_normalizes_only_crlf_and_tracks_version(self) -> None:
        lf = b'version = 3\n\n[[package]]\nname = "demo"\nversion = "1.2.3"\n'
        crlf = lf.replace(b"\n", b"\r\n")
        self.assertEqual(
            inventory.canonical_lock_bytes(b"a\r\nb\rc\n"),
            b"a\nb\rc\n",
        )
        self.assertEqual(
            inventory.lock_identity_sha256(lf),
            inventory.lock_identity_sha256(crlf),
        )
        changed = lf.replace(b"1.2.3", b"1.2.4")
        self.assertNotEqual(
            inventory.lock_identity_sha256(lf),
            inventory.lock_identity_sha256(changed),
        )

    def test_equivalent_lf_crlf_locks_render_identical_outputs(self) -> None:
        lock_text = (
            'version = 3\n\n[[package]]\nname = "demo"\nversion = "1.2.3"\n'
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            lf_path = root / "Cargo.lock.lf"
            crlf_path = root / "Cargo.lock.crlf"
            lf_path.write_bytes(lock_text.encode("utf-8"))
            crlf_path.write_bytes(lock_text.replace("\n", "\r\n").encode("utf-8"))

            raw_lf, packages_lf = inventory.load_lockfile(lf_path)
            raw_crlf, packages_crlf = inventory.load_lockfile(crlf_path)
            self.assertEqual(raw_lf, raw_crlf)
            self.assertEqual(packages_lf, packages_crlf)

            metadata = metadata_doc([meta_package("demo", "1.2.3", license="MIT")])
            result_lf = inventory.build_inventory(
                raw_lf, packages_lf, metadata, {}, {}
            )
            result_crlf = inventory.build_inventory(
                raw_crlf, packages_crlf, metadata, {}, {}
            )
            hash_lf = inventory.lock_identity_sha256(lf_path.read_bytes())
            hash_crlf = inventory.lock_identity_sha256(crlf_path.read_bytes())
            self.assertEqual(hash_lf, hash_crlf)

            outputs_lf = {
                inventory.INVENTORY_MARKDOWN: inventory.render_markdown(
                    result_lf, hash_lf
                ),
                inventory.INVENTORY_SPDX_JSON: inventory.render_spdx(
                    result_lf, hash_lf
                ),
            }
            outputs_crlf = {
                inventory.INVENTORY_MARKDOWN: inventory.render_markdown(
                    result_crlf, hash_crlf
                ),
                inventory.INVENTORY_SPDX_JSON: inventory.render_spdx(
                    result_crlf, hash_crlf
                ),
            }
            self.assertEqual(outputs_lf, outputs_crlf)
            self.assertIn(
                "Cargo.lock SHA-256 (LF-normalized)",
                outputs_lf[inventory.INVENTORY_MARKDOWN],
            )
            spdx = json.loads(outputs_lf[inventory.INVENTORY_SPDX_JSON])
            self.assertIn("cargo-lock-lf-", spdx["documentNamespace"])
            self.assertIn("LF-normalized", spdx["comment"])

            changed_hash = inventory.lock_identity_sha256(
                lock_text.replace("1.2.3", "1.2.4").encode("utf-8")
            )
            self.assertNotEqual(hash_lf, changed_hash)

    def test_spdx_namespace_changes_with_content_not_only_lockfile(self) -> None:
        lock = {
            "version": 3,
            "package": [package("demo", "1.2.3", checksum="a" * 64)],
        }
        metadata = metadata_doc([meta_package("demo", "1.2.3", license="MIT")])
        unreviewed = inventory.build_inventory(lock, lock["package"], metadata, {}, {})
        reviewed = inventory.build_inventory(
            lock,
            lock["package"],
            metadata,
            {},
            {"demo@1.2.3": "reviewed in test fixture"},
        )
        lock_hash = "b" * 64
        first = json.loads(inventory.render_spdx(unreviewed, lock_hash))
        second = json.loads(inventory.render_spdx(reviewed, lock_hash))
        repeated = json.loads(inventory.render_spdx(unreviewed, lock_hash))
        self.assertEqual(first["documentNamespace"], repeated["documentNamespace"])
        self.assertNotEqual(first["documentNamespace"], second["documentNamespace"])
        self.assertIn(lock_hash, first["documentNamespace"])
        self.assertIn(lock_hash, first["comment"])
        self.assertIn("SPDX content digest", first["comment"])

    def test_spdx_relationships_and_unique_ids(self) -> None:
        lock = {
            "version": 3,
            "package": [
                package("parent", "1.0.0", dependencies=["child 2.0.0"]),
                package("child", "2.0.0"),
            ],
        }
        metadata = metadata_doc(
            [meta_package("parent", "1.0.0"), meta_package("child", "2.0.0")]
        )
        result = inventory.build_inventory(lock, lock["package"], metadata, {}, {})
        ids = [record["spdx_id"] for record in result["records"]]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertTrue(all(record["checksum"] for record in result["records"]))
        self.assertEqual(len(result["relationships"]), 1)
        self.assertEqual(result["relationships"][0]["relationshipType"], "DEPENDS_ON")

    def test_duplicate_name_version_is_rejected_as_unsupported_multisource_identity(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            lock_path = Path(temporary) / "Cargo.lock"
            lock_path.write_text(
                """
version = 3

[[package]]
name = "same"
version = "1.0.0"
source = "registry+https://example.invalid/index-a"
checksum = "1111111111111111111111111111111111111111111111111111111111111111"

[[package]]
name = "same"
version = "1.0.0"
source = "registry+https://example.invalid/index-b"
checksum = "2222222222222222222222222222222222222222222222222222222222222222"
""",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                inventory.InventoryError,
                "source-disambiguated.*failing closed",
            ):
                inventory.load_lockfile(lock_path)

    def test_check_reports_drift_and_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output_dir = Path(temporary)
            problems = inventory.check_outputs(
                output_dir,
                {
                    inventory.INVENTORY_MARKDOWN: "expected\n",
                    inventory.INVENTORY_SPDX_JSON: "{}\n",
                },
            )
            self.assertEqual(len(problems), 2)
            (output_dir / inventory.INVENTORY_MARKDOWN).write_text(
                "different\n", encoding="utf-8"
            )
            problems = inventory.check_outputs(
                output_dir,
                {
                    inventory.INVENTORY_MARKDOWN: "expected\n",
                    inventory.INVENTORY_SPDX_JSON: "{}\n",
                },
            )
            self.assertTrue(any("out of date" in problem for problem in problems))

    def test_render_outputs_contain_review_and_checksum(self) -> None:
        lock = {
            "version": 3,
            "package": [package("demo", "1.2.3", checksum="a" * 64)],
        }
        metadata = metadata_doc([meta_package("demo", "1.2.3", license="MIT")])
        result = inventory.build_inventory(lock, lock["package"], metadata, {}, {})
        markdown = inventory.render_markdown(result, "b" * 64)
        spdx = json.loads(inventory.render_spdx(result, "b" * 64))
        self.assertIn("a" * 64, markdown)
        self.assertIn("unreviewed", markdown)
        self.assertIn("b" * 64, spdx["documentNamespace"])
        self.assertEqual(len(spdx["packages"]), 1)
        self.assertEqual(spdx["packages"][0]["licenseDeclared"], "MIT")
        self.assertEqual(spdx["packages"][0]["licenseConcluded"], "NOASSERTION")


@unittest.skipUnless(shutil.which("cargo"), "cargo is required for the locked-resolution test")
class LockedResolutionTests(unittest.TestCase):
    def test_deliberately_unlocked_dependency_fails(self) -> None:
        """A changed manifest without a matching lockfile must fail loudly."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "leaf" / "src").mkdir(parents=True)
            (root / "leaf" / "Cargo.toml").write_text(
                '[package]\nname = "leaf"\nversion = "0.1.0"\nedition = "2021"\n',
                encoding="utf-8",
            )
            (root / "leaf" / "src" / "lib.rs").write_text("pub fn leaf() {}\n", encoding="utf-8")
            (root / "src").mkdir()
            (root / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
            (root / "Cargo.toml").write_text(
                '[package]\nname = "stale-fixture"\nversion = "0.1.0"\nedition = "2021"\n'
                '[dependencies]\nleaf = { path = "leaf" }\n',
                encoding="utf-8",
            )
            generated = subprocess.run(
                ["cargo", "generate-lockfile", "--offline"],
                cwd=root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                check=False,
            )
            self.assertEqual(
                generated.returncode,
                0,
                msg=f"generate-lockfile failed:\n{generated.stderr}\n{generated.stdout}",
            )

            (root / "extra" / "src").mkdir(parents=True)
            (root / "extra" / "Cargo.toml").write_text(
                '[package]\nname = "extra"\nversion = "0.1.0"\nedition = "2021"\n',
                encoding="utf-8",
            )
            (root / "extra" / "src" / "lib.rs").write_text("pub fn extra() {}\n", encoding="utf-8")
            (root / "Cargo.toml").write_text(
                '[package]\nname = "stale-fixture"\nversion = "0.1.0"\nedition = "2021"\n'
                '[dependencies]\nleaf = { path = "leaf" }\nextra = { path = "extra" }\n',
                encoding="utf-8",
            )

            completed = subprocess.run(
                [
                    sys.executable,
                    str(TOOL),
                    "--root",
                    str(root),
                    "--output-dir",
                    str(root / "generated"),
                    "--check",
                ],
                cwd=root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                check=False,
            )
            self.assertNotEqual(completed.returncode, 0, msg=completed.stdout)
            combined = (completed.stderr + completed.stdout).lower()
            self.assertIn("cargo metadata failed", combined)
            self.assertTrue("lock" in combined or "stale" in combined)
            self.assertFalse((root / "generated").exists())


if __name__ == "__main__":
    unittest.main()
