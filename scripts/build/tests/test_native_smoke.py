"""Tests for the bounded native startup smoke helper and its CI wiring.

The helper tests use tiny Python child processes instead of the desktop binary,
so they run on every host without opening a window. They cover the real failure
modes: missing marker, marker on stderr, non-zero exit, and a hung process that
must be killed at the timeout.
"""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path

HELPER_DIRECTORY = Path(__file__).resolve().parents[1]
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(HELPER_DIRECTORY))

import native_smoke  # noqa: E402


def python_command(source: str, *arguments: str) -> list[str]:
    return [sys.executable, "-c", source, *arguments]


class NativeSmokeHelperTests(unittest.TestCase):
    def run_with_source(self, source: str, *arguments: str, timeout: float = 10.0):
        artifact_directory = Path(self.temp_directory.name) / "artifacts"
        marker = native_smoke.STARTUP_MARKER
        arguments = arguments or (marker,)
        return native_smoke.run_smoke(
            Path("fake-ansa-desktop"),
            timeout,
            artifact_directory,
            command_factory=lambda _flag: python_command(source, *arguments),
        )

    def setUp(self) -> None:
        self.temp_directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp_directory.cleanup)

    def test_accepts_exact_marker_and_zero_exit(self) -> None:
        passed, result, errors = self.run_with_source(
            "import sys; print(sys.argv[1])"
        )
        self.assertTrue(passed, errors)
        self.assertEqual(errors, [])
        self.assertEqual(result.exit_code, 0)
        self.assertFalse(result.timed_out)
        summary = (
            Path(self.temp_directory.name)
            / "artifacts"
            / "summary.txt"
        ).read_text(encoding="utf-8")
        self.assertIn("result: PASS", summary)
        self.assertTrue(
            (
                Path(self.temp_directory.name)
                / "artifacts"
                / "startup.stdout.log"
            ).is_file()
        )

    def test_rejects_zero_exit_without_marker(self) -> None:
        passed, _, errors = self.run_with_source("print('different output')")
        self.assertFalse(passed)
        self.assertTrue(
            any("success marker was not emitted" in error for error in errors)
        )

    def test_rejects_nonzero_exit_even_with_marker(self) -> None:
        passed, result, errors = self.run_with_source(
            "import sys; print(sys.argv[1]); raise SystemExit(7)"
        )
        self.assertFalse(passed)
        self.assertEqual(result.exit_code, 7)
        self.assertTrue(any("expected exit code 0" in error for error in errors))

    def test_rejects_marker_emitted_only_on_stderr(self) -> None:
        passed, _, errors = self.run_with_source(
            "import sys; print(sys.argv[1], file=sys.stderr)"
        )
        self.assertFalse(passed)
        self.assertTrue(
            any("success marker was not emitted on stdout" in error for error in errors)
        )

    def test_rejects_modified_marker(self) -> None:
        passed, _, errors = self.run_with_source(
            "import sys; print(sys.argv[1].replace('success', 'done'))"
        )
        self.assertFalse(passed)
        self.assertTrue(errors)

    def test_kills_process_at_bounded_timeout(self) -> None:
        passed, result, errors = self.run_with_source(
            "import time; time.sleep(30)", timeout=0.4
        )
        self.assertFalse(passed)
        self.assertTrue(result.timed_out)
        self.assertIsNone(result.exit_code)
        self.assertLess(result.duration_seconds, 15.0)
        self.assertTrue(any("bounded timeout" in error for error in errors))

    def test_missing_binary_is_a_configuration_error(self) -> None:
        artifact_directory = Path(self.temp_directory.name) / "missing"
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            exit_code = native_smoke.main(
                [
                    "--binary",
                    str(Path(self.temp_directory.name) / "does-not-exist"),
                    "--artifact-dir",
                    str(artifact_directory),
                ]
            )
        self.assertEqual(exit_code, 2)
        self.assertIn("Native smoke binary not found", stderr.getvalue())
        self.assertIn(
            "Native smoke binary not found",
            (artifact_directory / "summary.txt").read_text(encoding="utf-8"),
        )

    def test_rejects_nonfinite_or_nonpositive_timeouts(self) -> None:
        for timeout in ("nan", "inf", "-inf", "0", "-1"):
            with self.subTest(timeout=timeout), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(native_smoke.main([f"--timeout={timeout}"]), 2)

    def test_launch_failure_preserves_diagnostics(self) -> None:
        artifacts = Path(self.temp_directory.name) / "launch-failure"
        passed, result, errors = native_smoke.run_smoke(
            Path(self.temp_directory.name) / "missing-binary", 1, artifacts
        )
        self.assertFalse(passed)
        self.assertTrue(errors)
        self.assertIn("Could not launch", result.stderr)
        self.assertIn("result: FAIL", (artifacts / "summary.txt").read_text())
        self.assertIn("Could not launch", (artifacts / "startup.stderr.log").read_text())


class NativeSmokeWorkflowContractTests(unittest.TestCase):
    APPLICATION_SOURCES = (
        "crates/ui-gpui/src/lib.rs",
        "crates/ui-gpui/src/shell.rs",
    )

    def read(self, relative: str) -> str:
        return (ROOT / relative).read_text(encoding="utf-8")

    def application_source(self) -> str:
        return "\n".join(self.read(path) for path in self.APPLICATION_SOURCES)

    def test_workflow_runs_native_smoke_after_desktop_build(self) -> None:
        workflow = self.read(".github/workflows/ci.yml")
        build = workflow.index("cargo build -p ansa-desktop --locked --offline")
        smoke = workflow.index("scripts/build/native_smoke.py")
        self.assertGreater(smoke, build)
        self.assertIn("python -m unittest discover -s scripts/build/tests -v", workflow)

    def test_linux_prerequisites_are_present(self) -> None:
        workflow = self.read(".github/workflows/ci.yml")
        for package in ("xvfb", "mesa-vulkan-drivers", "libvulkan1"):
            self.assertIn(package, workflow)
        self.assertIn("xvfb-run", workflow)
        self.assertIn("VK_ICD_FILENAMES", workflow)
        self.assertIn("--timeout 120", workflow)

    def test_logs_are_uploaded_even_on_failure(self) -> None:
        workflow = self.read(".github/workflows/ci.yml")
        self.assertIn("actions/upload-artifact@v4", workflow)
        self.assertIn("path: target/native-smoke/", workflow)
        self.assertIn("if: always()", workflow)

    def test_emitted_marker_matches_application_source(self) -> None:
        application = self.application_source()
        self.assertIn(native_smoke.STARTUP_MARKER, application)

    def test_hwnd_lookup_is_windows_only(self) -> None:
        application = self.application_source()
        self.assertIn('#[cfg(target_os = "windows")]\nfn native_handle(window:', application)
        self.assertIn('#[cfg(not(target_os = "windows"))]\nfn native_handle(_window:', application)


if __name__ == "__main__":
    unittest.main()
