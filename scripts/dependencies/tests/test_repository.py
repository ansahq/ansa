"""Repository-level checks for the F03 build/tooling contract.

These tests deliberately use only text/TOML stdlib parsing. They verify that the
toolchain pin, blocking CI matrix, trigger scoping, offline/locked commands and
documentation stay in sync; they do not claim that the remote CI matrix has been
executed.
"""

from __future__ import annotations

import json
import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RUST_VERSION = "1.99.1"

_TRIGGER_KEY = re.compile(r"(?P<name>[A-Za-z_][A-Za-z0-9_-]*):$")
_BRANCHES_FLOW = re.compile(r"branches:\s*(?P<sequence>\[[^\]]*\])$")


def parse_trigger_block(workflow: str) -> dict[str, list[str] | None]:
    """Parse only the top-level ``on:`` block of a workflow, structurally.

    The standard library has no YAML parser, so this supports the narrow shape
    this repository uses instead of guessing at general YAML: a top-level
    ``on:`` line, one two-space-indented trigger per line, and an optional
    four-space-indented ``branches: [ ... ]`` flow sequence. Anything else
    (comments, inline configuration, block sequences) raises ``AssertionError``
    rather than being silently misread.

    Returns the configured triggers mapped to their branch filters, where
    ``None`` means the trigger has no filter (so it fires for every branch).
    """

    lines = workflow.splitlines()
    try:
        start = next(index for index, line in enumerate(lines) if line.rstrip() == "on:")
    except StopIteration:
        raise AssertionError("workflow has no top-level 'on:' trigger block") from None

    triggers: dict[str, list[str] | None] = {}
    current: str | None = None
    for raw in lines[start + 1 :]:
        if not raw.strip():
            continue
        indent = len(raw) - len(raw.lstrip(" "))
        if indent == 0:
            break
        if "#" in raw:
            raise AssertionError(
                f"comments inside the trigger block are unsupported: {raw!r}"
            )
        entry = raw.strip()
        if indent == 2:
            match = _TRIGGER_KEY.fullmatch(entry)
            if match is None:
                raise AssertionError(f"unsupported trigger line: {raw!r}")
            current = match.group("name")
            if current in triggers:
                raise AssertionError(f"duplicate trigger entry: {current!r}")
            triggers[current] = None
        elif indent == 4 and current is not None:
            match = _BRANCHES_FLOW.fullmatch(entry)
            if match is None:
                raise AssertionError(
                    f"unsupported nested trigger configuration: {raw!r}"
                )
            if triggers[current] is not None:
                raise AssertionError(f"duplicate branches filter for {current!r}")
            items = [
                item.strip().strip("'\"")
                for item in match.group("sequence")[1:-1].split(",")
                if item.strip()
            ]
            triggers[current] = items
        else:
            raise AssertionError(f"unsupported structure in trigger block: {raw!r}")

    if not triggers:
        raise AssertionError("trigger block configures no events")
    return triggers


class RepositoryContractTests(unittest.TestCase):
    def read(self, relative: str) -> str:
        return (ROOT / relative).read_text(encoding="utf-8")

    def test_rust_toolchain_is_exact_pin(self) -> None:
        with (ROOT / "rust-toolchain.toml").open("rb") as handle:
            toolchain = tomllib.load(handle)["toolchain"]
        self.assertEqual(toolchain["channel"], RUST_VERSION)
        self.assertEqual(toolchain["profile"], "minimal")
        self.assertIn("rustfmt", toolchain["components"])
        self.assertIn("clippy", toolchain["components"])

    def test_ci_pins_same_toolchain(self) -> None:
        ci = self.read(".github/workflows/ci.yml")
        self.assertIn(f'RUST_VERSION: "{RUST_VERSION}"', ci)
        self.assertIn("toolchain: ${{ env.RUST_VERSION }}", ci)
        self.assertIn("components: rustfmt, clippy", ci)

    def test_ci_push_is_main_only_and_pull_requests_are_retained(self) -> None:
        triggers = parse_trigger_block(self.read(".github/workflows/ci.yml"))
        self.assertIn("pull_request", triggers, "pull_request trigger must be retained")
        self.assertIsNone(
            triggers["pull_request"],
            "pull_request must stay unfiltered so every pull request is validated",
        )
        self.assertIn("push", triggers, "push trigger must be retained for merged main")
        branches = triggers["push"]
        self.assertIsNotNone(
            branches,
            "a bare 'push:' runs CI on every branch and duplicates pull_request runs",
        )
        self.assertEqual(
            branches, ["main"], f"push must be filtered to main, got {branches!r}"
        )

    def test_ci_is_blocking_for_all_hosts(self) -> None:
        ci = self.read(".github/workflows/ci.yml")
        for os_name in ("windows-latest", "macos-latest", "ubuntu-latest"):
            self.assertIn(os_name, ci)
        for command in (
            "cargo fmt --all -- --check",
            "cargo clippy --workspace --all-targets --locked --offline",
            "cargo test --workspace --lib --bins --tests --locked --offline",
            "cargo test --workspace --doc --locked --offline",
            "cargo build -p ansa-desktop --locked --offline",
            "scripts/dependencies/inventory.py --check",
            "python -m unittest discover -s scripts/dependencies/tests -v",
            "cargo fetch --locked",
        ):
            self.assertIn(command, ci)
        self.assertIn("fail-fast: false", ci)

    def test_ci_does_not_request_secrets_or_run_untrusted_pr_code_with_write(self) -> None:
        ci = self.read(".github/workflows/ci.yml")
        self.assertNotIn("secrets.", ci)
        self.assertNotIn("pull_request_target", ci)
        self.assertIn("permissions:\n  contents: read", ci)
        self.assertIn("persist-credentials: false", ci)

    def test_ci_runs_reference_validation(self) -> None:
        ci = self.read(".github/workflows/ci.yml")
        self.assertIn("python scripts/reference/validate_reference.py --root .", ci)
        self.assertIn("python scripts/reference/test_validate_reference.py", ci)
        self.assertIn("Reference ledger (${{ matrix.os }})", ci)

    def test_native_acceptance_evidence_gate_is_in_ci_and_local_validation(self) -> None:
        ci = self.read(".github/workflows/ci.yml")
        self.assertIn("python scripts/platform_acceptance_gate.py --check", ci)
        self.assertIn(
            "python -m unittest discover -s scripts -p test_platform_acceptance_gate.py -v",
            ci,
        )
        script = self.read("scripts/validate.ps1")
        self.assertIn("'scripts/platform_acceptance_gate.py', '--check'", script)
        self.assertIn("'test_platform_acceptance_gate.py'", script)

    def test_ci_runs_isolated_reuse_safety_tests(self) -> None:
        ci = self.read(".github/workflows/ci.yml")
        self.assertIn("pwsh -NoProfile -File scripts/reuse/tests/run-tests.ps1", ci)
        script = self.read("scripts/validate.ps1")
        self.assertIn("SkipReuseToolingTests", script)
        self.assertIn("scripts/reuse/tests/run-tests.ps1", script)
        self.assertIn("$env:OS -eq 'Windows_NT'", script)
        self.assertNotIn("if ($IsWindows)", script)

    def test_validate_ps1_runs_locked_quality_gates(self) -> None:
        script = self.read("scripts/validate.ps1")
        for fragment in (
            "'fmt'",
            "'clippy'",
            "'--workspace'",
            "'--all-targets'",
            "'--doc'",
            "'build'",
            "'ansa-desktop'",
            "'--locked'",
            "inventory.py",
            "--check",
        ):
            self.assertIn(fragment, script)

    def test_build_docs_cover_cache_offline_and_pin_updates(self) -> None:
        readme = self.read("docs/build/README.md")
        toolchain = self.read("docs/build/toolchain.md")
        ci = self.read("docs/build/ci.md")
        self.assertIn("--offline", readme)
        self.assertIn("--check", readme)
        self.assertIn("review-status.toml", readme)
        self.assertIn("update", toolchain.lower())
        self.assertIn(RUST_VERSION, toolchain)
        self.assertIn("cache", ci.lower())
        self.assertIn("offline", ci.lower())
        self.assertIn("not executed", ci.lower())
        self.assertIn("branches: [main]", ci)
        self.assertIn("duplicate", ci.lower())

    def test_generated_inventory_exists_and_is_marked_generated(self) -> None:
        markdown = self.read("docs/build/dependency-inventory.md")
        self.assertIn("Generated by scripts/dependencies/inventory.py", markdown)
        self.assertIn("Cargo.lock SHA-256", markdown)
        self.assertIn("unreviewed", markdown)
        self.assertIn("not a legal review", markdown)

    def test_generated_spdx_is_self_consistent(self) -> None:
        document = json.loads(self.read("docs/build/dependency-inventory.spdx.json"))
        self.assertEqual(document["spdxVersion"], "SPDX-2.3")
        self.assertEqual(document["dataLicense"], "CC0-1.0")
        packages = document["packages"]
        ids = {package["SPDXID"] for package in packages}
        self.assertEqual(len(ids), len(packages))
        self.assertIn("SPDXRef-DOCUMENT", ids | {"SPDXRef-DOCUMENT"})
        for relationship in document["relationships"]:
            self.assertIn(relationship["spdxElementId"], ids | {"SPDXRef-DOCUMENT"})
            self.assertIn(relationship["relatedSpdxElement"], ids | {"SPDXRef-DOCUMENT"})
        for package in packages:
            self.assertEqual(package["licenseConcluded"], "NOASSERTION")
            for checksum in package.get("checksums", []):
                self.assertEqual(checksum["algorithm"], "SHA256")
                self.assertEqual(len(checksum["checksumValue"]), 64)


if __name__ == "__main__":
    unittest.main()
