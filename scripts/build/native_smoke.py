#!/usr/bin/env python3
"""Bounded native startup smoke for the built Ansa desktop executable.

This helper launches the real native binary rather than a Rust unit-test
harness. The executable starts its actual GPUI window, reaches two frame callbacks, emits
one success marker from inside the frame callback and exits. Both the marker and
a zero exit status are required, so a build that merely starts and then hangs,
crashes or quits early cannot pass.

Captured stdout/stderr and a summary are written to an artifact directory even
when the phase fails or times out.
"""

from __future__ import annotations

import argparse
import math
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BINARY = Path("target") / "debug" / "ansa-desktop"
DEFAULT_ARTIFACT_DIRECTORY = Path("target") / "native-smoke"
DEFAULT_TIMEOUT_SECONDS = 120.0

STARTUP_FLAG = "--smoke-test"
STARTUP_MARKER = (
    "Ansa native startup smoke: window opened and two GPUI frame callbacks reached, "
    "exiting with success."
)

CommandFactory = Callable[[str], Sequence[str]]


@dataclass
class PhaseResult:
    """One bounded child-process run and everything needed to explain it."""

    name: str
    command: Sequence[str]
    exit_code: int | None
    timed_out: bool
    stdout: str
    stderr: str
    duration_seconds: float


def run_bounded(
    name: str,
    command: Sequence[str],
    timeout_seconds: float,
    cwd: Path | None = None,
) -> PhaseResult:
    """Run ``command`` and kill it after ``timeout_seconds``.

    ``subprocess.run(timeout=...)`` is avoided because its timeout exception
    does not carry the already-captured partial streams on every platform.
    Popen plus communicate() lets the helper preserve diagnostics and still
    enforce a hard bound.
    """

    started = time.monotonic()
    process = subprocess.Popen(
        list(command),
        cwd=str(cwd) if cwd is not None else None,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    timed_out = False
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        process.kill()
        stdout, stderr = process.communicate()

    return PhaseResult(
        name=name,
        command=command,
        exit_code=None if timed_out else process.returncode,
        timed_out=timed_out,
        stdout=stdout or "",
        stderr=stderr or "",
        duration_seconds=time.monotonic() - started,
    )


def marker_lines(stdout: str) -> set[str]:
    """Return stripped output lines, preserving exact marker matching."""

    return {line.strip() for line in stdout.splitlines()}


def validate_phase(result: PhaseResult, marker: str) -> list[str]:
    """Return human-readable failures; an empty list means the phase passed."""

    errors: list[str] = []
    if result.timed_out:
        errors.append(f"{result.name}: exceeded the bounded timeout and was killed")
    if result.exit_code != 0:
        errors.append(
            f"{result.name}: expected exit code 0, got {result.exit_code!r}"
        )
    if marker not in marker_lines(result.stdout):
        errors.append(
            f"{result.name}: success marker was not emitted on stdout: {marker!r}"
        )
    return errors


def write_phase_logs(result: PhaseResult, artifact_directory: Path) -> None:
    (artifact_directory / f"{result.name}.stdout.log").write_text(
        result.stdout, encoding="utf-8"
    )
    (artifact_directory / f"{result.name}.stderr.log").write_text(
        result.stderr, encoding="utf-8"
    )


def write_summary(
    artifact_directory: Path,
    binary: Path,
    timeout_seconds: float,
    result: PhaseResult,
    errors: Sequence[str],
) -> None:
    lines = [
        "Ansa native startup smoke",
        f"binary: {binary}",
        f"python: {sys.version.split()[0]} ({sys.executable})",
        f"platform: {sys.platform}",
        f"timeout: {timeout_seconds:g}s",
        "",
        f"phase: {result.name}",
        f"command: {subprocess.list2cmdline(list(result.command))}",
        f"exit code: {result.exit_code!r}",
        f"timed out: {result.timed_out}",
        f"duration: {result.duration_seconds:.3f}s",
        "",
        "result: " + ("PASS" if not errors else "FAIL"),
    ]
    if errors:
        lines.append("errors:")
        lines.extend(f"- {error}" for error in errors)
    (artifact_directory / "summary.txt").write_text(
        "\n".join(lines) + "\n", encoding="utf-8"
    )


def resolve_binary(binary: Path) -> Path:
    """Resolve a relative path and the Windows executable suffix."""

    resolved = binary if binary.is_absolute() else REPOSITORY_ROOT / binary
    if (
        os.name == "nt"
        and not resolved.exists()
        and not resolved.suffix
        and resolved.with_suffix(".exe").exists()
    ):
        resolved = resolved.with_suffix(".exe")
    return resolved


def default_command_factory(binary: Path) -> CommandFactory:
    return lambda flag: [str(binary), flag]


def run_smoke(
    binary: Path,
    timeout_seconds: float,
    artifact_directory: Path,
    command_factory: CommandFactory | None = None,
) -> tuple[bool, PhaseResult, list[str]]:
    """Run and validate the startup phase; always writes logs and summary."""

    artifact_directory.mkdir(parents=True, exist_ok=True)
    factory = command_factory or default_command_factory(binary)
    command = factory(STARTUP_FLAG)
    try:
        result = run_bounded(
            "startup", command, timeout_seconds, cwd=REPOSITORY_ROOT
        )
    except OSError as error:
        result = PhaseResult(
            name="startup", command=command, exit_code=None, timed_out=False,
            stdout="", stderr=f"Could not launch native smoke: {error}\n",
            duration_seconds=0.0,
        )
    write_phase_logs(result, artifact_directory)

    errors = validate_phase(result, marker=STARTUP_MARKER)
    write_summary(artifact_directory, binary, timeout_seconds, result, errors)
    return not errors, result, errors


def print_phase_output(result: PhaseResult) -> None:
    print(f"--- {result.name} stdout ---")
    print(result.stdout, end="" if result.stdout.endswith("\n") else "\n")
    print(f"--- {result.name} stderr ---")
    print(result.stderr, end="" if result.stderr.endswith("\n") else "\n")


def parse_arguments(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run the built native Ansa desktop through a bounded startup smoke "
            "that requires a genuine success marker and a zero exit status."
        )
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=DEFAULT_BINARY,
        help=(
            "built ansa-desktop executable "
            "(default: target/debug/ansa-desktop[.exe])"
        ),
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT_SECONDS,
        help=(
            "hard timeout in seconds "
            f"(default: {DEFAULT_TIMEOUT_SECONDS:g})"
        ),
    )
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        default=DEFAULT_ARTIFACT_DIRECTORY,
        help=(
            "directory for captured stdout/stderr and summary "
            f"(default: {DEFAULT_ARTIFACT_DIRECTORY.as_posix()})"
        ),
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = parse_arguments(argv)
    if not math.isfinite(arguments.timeout) or arguments.timeout <= 0:
        print("--timeout must be finite and greater than zero", file=sys.stderr)
        return 2

    binary = resolve_binary(arguments.binary)
    artifact_directory = (
        arguments.artifact_dir
        if arguments.artifact_dir.is_absolute()
        else REPOSITORY_ROOT / arguments.artifact_dir
    )
    artifact_directory.mkdir(parents=True, exist_ok=True)

    if not binary.is_file():
        message = (
            f"Native smoke binary not found: {binary}. "
            "Build it first with: cargo build -p ansa-desktop --locked"
        )
        (artifact_directory / "summary.txt").write_text(
            message + "\n", encoding="utf-8"
        )
        print(message, file=sys.stderr)
        return 2

    passed, result, errors = run_smoke(
        binary, arguments.timeout, artifact_directory
    )
    print_phase_output(result)

    print(
        "Native smoke logs: "
        f"{artifact_directory} "
        f"(timeout: {arguments.timeout:g}s)"
    )
    if not passed:
        print("Native startup smoke FAILED:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(
        "Native startup smoke PASSED: the expected success marker was emitted "
        "and the process exited with zero."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
