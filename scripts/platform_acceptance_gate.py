#!/usr/bin/env python3
"""Validate and report the native platform acceptance ledger (stdlib only)."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime
from pathlib import Path, PurePosixPath, PureWindowsPath

DEFAULT_LEDGER = Path(__file__).resolve().parents[1] / "docs/platform-acceptance-evidence.json"
PROFILES = {
    "ANS-88": ("macos-14", "macos-15"),
    "ANS-87": ("gnome-x11", "gnome-wayland", "kde-x11", "kde-wayland"),
    "ANS-97": ("macos-14", "macos-15"),
}
JOURNEYS = tuple(f"J{i}" for i in range(1, 11))
GLASS_CASES = tuple(f"G{i}" for i in range(1, 8))
CONTRAST_CASES = tuple(
    f"{theme}-{backdrop}-{feature}"
    for theme in ("dark", "light")
    for backdrop in ("black", "white")
    for feature in ("primary", "secondary", "caret", "selected-text")
)


def round_trips(start, alternate):
    return (f"initial-{start}",) + tuple(
        f"cycle-{cycle}-{state}"
        for cycle in range(1, 4)
        for state in (alternate, start)
    )


BOUNDS_TRANSITIONS = {
    "glass-dark": round_trips("on", "off"),
    "glass-light": round_trips("on", "off"),
    "startup-dark": ("glass-default", "opaque-startup", "glass-restored"),
    "startup-light": ("glass-default", "opaque-startup", "glass-restored"),
    "theme-glass": round_trips("dark", "light"),
    "theme-opaque": round_trips("dark", "light"),
    "reduce-transparency": ("initial", "enabled-live", "enabled-restart", "restored"),
    "increase-contrast": ("initial", "enabled-live", "enabled-restart", "restored"),
    "fallback": ("available-baseline", "unavailable-startup", "enable-rejected"),
}
TARGETS = {
    "macos": {f"{architecture}-apple-darwin" for architecture in ("aarch64", "x86_64")},
    "linux": {f"{architecture}-unknown-linux-gnu" for architecture in ("aarch64", "x86_64")},
}
SMOKE_FLAGS = {"--smoke-test", "--interaction-smoke-test"}
CONTEXT_KINDS = {
    "ci-build-startup", "source-inspection", "structural-tree-probe", "host-inventory",
}
RUN_FIELDS = {
    "source", "run_id", "tester_id", "implemented_change", "started_at",
    "configuration", "build", "artifacts", "results",
}


def need(condition, message):
    if not condition:
        raise ValueError(message)


def keys(value, expected, label):
    need(isinstance(value, dict), f"{label}: expected an object")
    missing = set(expected) - value.keys()
    extra = value.keys() - set(expected)
    need(not missing and not extra, f"{label}: missing {sorted(missing)}; extra {sorted(extra)}")


def text(value, label):
    need(isinstance(value, str) and bool(value.strip()), f"{label}: expected nonempty text")
    need(value == value.strip(), f"{label}: remove leading/trailing whitespace")
    return value


def hex_id(value, length, label):
    need(isinstance(value, str) and re.fullmatch(f"[0-9a-f]{{{length}}}", value),
         f"{label}: expected {length} lowercase hex characters")


def timestamp(value, label):
    text(value, label)
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    need("T" in value and parsed.utcoffset() is not None, f"{label}: expected timestamp with offset")
    return parsed


def fingerprint(value):
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("ascii")
    ).hexdigest()


def safe_output(value):
    """Render untrusted diagnostics as printable ASCII.

    Evidence text is not trusted: control characters could spoof terminal
    output, and non-ASCII output can crash a gate running with a legacy
    console encoding. Escape everything outside printable ASCII.
    """
    return "".join(
        char if 0x20 <= ord(char) <= 0x7E else ascii(char)[1:-1]
        for char in value
    )


def artifact(value, root, label):
    keys(value, {"path", "sha256"}, label)
    name = text(value["path"], label)
    relative = PurePosixPath(name)
    need(not relative.is_absolute() and ".." not in relative.parts and "\\" not in name
         and ":" not in name and relative.as_posix() == name,
         f"{label}: use a normalized relative POSIX path")
    path = (root / relative).resolve()
    need(path.is_relative_to(root) and path.is_file() and path.stat().st_size > 0,
         f"{label}: missing, empty, or outside artifact root: {name}")
    hex_id(value["sha256"], 64, label)
    with path.open("rb") as stream:
        actual = hashlib.file_digest(stream, "sha256").hexdigest()
    need(actual == value["sha256"], f"{label}: SHA-256 mismatch: {name}")
    return actual


def configuration(value, ticket, profile):
    common = {"os_version", "os_build", "hardware", "architecture", "display", "locale"}
    if ticket == "ANS-87":
        expected = common | {
            "distro", "desktop", "desktop_version", "display_server", "compositor", "orca_version",
            "atspi_version", "speech_settings", "adapter",
        }
    elif ticket == "ANS-88":
        expected = common | {"voiceover_version", "speech_settings", "adapter"}
    else:
        expected = common | {"display_profile"}
    keys(value, expected, "configuration")
    for field, val in value.items():
        text(val, f"configuration.{field}")
    if ticket == "ANS-87":
        desktop, server = profile.split("-")
        need(value["desktop"] == desktop and value["display_server"] == server,
             f"configuration: expected {desktop}/{server}")
        if desktop == "gnome":
            need(value["distro"] == "Ubuntu" and value["os_version"].split(".")[:2] == ["24", "04"],
                 "configuration: GNOME baseline is Ubuntu 24.04 LTS")
            need(value["desktop_version"].split(".")[0] == "46"
                 and value["orca_version"].split(".")[0] == "46",
                 "configuration: GNOME baseline uses GNOME Shell 46 and Orca 46.x")
    else:
        need(value["os_version"].split(".")[0] == profile.split("-")[1],
             f"configuration: wrong macOS major version for {profile}")


def contrast(foreground, background):
    def luminance(rgb):
        need(isinstance(rgb, list) and len(rgb) == 3
             and all(type(channel) is int and 0 <= channel <= 255 for channel in rgb),
             "contrast measurement: expected three 8-bit sRGB channels")
        values = [channel / 255 for channel in rgb]
        linear = [v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4 for v in values]
        return sum(w * v for w, v in zip((0.2126, 0.7152, 0.0722), linear))

    low, high = sorted((luminance(foreground), luminance(background)))
    return (high + 0.05) / (low + 0.05)


def glass_measurements(value, results):
    keys(value, {"tint_alpha", "contrast", "bounds"}, "measurements")
    alpha = value["tint_alpha"]
    need(type(alpha) in (int, float) and 0 <= alpha <= 1, "measurements.tint_alpha: expected 0–1")
    if results["G2"]["outcome"] == "pass":
        need(alpha == 0.8, "G2: themed tint is not 80%")
    samples = value["contrast"]
    need(isinstance(samples, dict) and set(samples) <= set(CONTRAST_CASES),
         "measurements.contrast: invalid case")
    if results["G2"]["outcome"] == "pass":
        keys(samples, CONTRAST_CASES, "G2 contrast coverage")
    for name, pair in samples.items():
        keys(pair, {"foreground", "background", "capture_locator"}, name)
        text(pair["capture_locator"], f"{name}.capture_locator")
        ratio = contrast(pair["foreground"], pair["background"])
        need(results["G2"]["outcome"] != "pass" or ratio >= 4.5,
             f"G2 {name}: {ratio:.4f}:1 is below 4.5:1")

    bounds = value["bounds"]
    need(isinstance(bounds, dict) and set(bounds) <= set(BOUNDS_TRANSITIONS),
         "measurements.bounds: invalid case")
    for case, expected in BOUNDS_TRANSITIONS.items():
        journey = {
            "glass": "G3", "startup": "G4", "theme": "G5", "reduce": "G6",
            "increase": "G6", "fallback": "G7",
        }[case.split("-")[0]]
        passed = results[journey]["outcome"] == "pass"
        need(not passed or case in bounds, f"{journey}: missing bounds comparison for {case}")
        if case not in bounds:
            continue
        series = bounds[case]
        need(isinstance(series, list) and 0 < len(series) <= len(expected),
             f"{case}: invalid bounds sample count")
        need(not passed or len(series) == len(expected),
             f"{journey} {case}: expected {len(expected)} labeled bounds samples")
        rectangles = []
        for item, state in zip(series, expected):
            keys(item, {"state", "rect"}, f"{case} transition")
            need(item["state"] == state, f"{case}: expected transition {state}")
            rect = item["rect"]
            need(isinstance(rect, list) and len(rect) == 4 and all(type(v) is int for v in rect)
                 and rect[2] > 0 and rect[3] > 0, f"{case}: expected integer [x, y, w, h]")
            rectangles.append(rect)
        need(not passed or all(rect == rectangles[0] for rect in rectangles[1:]),
             f"{journey}: window moved or resized in {case}")


def validate_run(run, ticket, profile, commit, root, run_ids, used_captures, build_hashes, binaries):
    glass = ticket == "ANS-97"
    keys(run, RUN_FIELDS | ({"measurements"} if glass else set()), "run")
    need(run["source"] == "physical-native-manual", "run: only physical-native-manual evidence is eligible")
    run_id = text(run["run_id"], "run_id").casefold()
    need(run_id not in run_ids, f"run_id must be unique: {run_id}")
    run_ids.add(run_id)
    tester = text(run["tester_id"], "tester_id").casefold()
    need(type(run["implemented_change"]) is bool, "implemented_change: expected boolean")
    started = timestamp(run["started_at"], "started_at")
    configuration(run["configuration"], ticket, profile)

    build = run["build"]
    keys(build, {
        "commit", "working_tree", "binary_path", "binary", "build_log", "target",
        "command", "renderer", "fixture", "launch_argv", "promotion_ref",
    }, "build")
    need(commit is not None and build["commit"] == commit, "build: must match declared candidate commit")
    need(build["working_tree"] == "clean", "build: must be from a clean checkout")
    build_path = text(build["binary_path"], "build.binary_path")
    need(PurePosixPath(build_path).is_absolute() and not PureWindowsPath(build_path).drive,
         "build.binary_path: expected a native POSIX absolute path")
    for field in ("command", "renderer", "fixture", "promotion_ref"):
        text(build[field], f"build.{field}")
    argv = build["launch_argv"]
    need(isinstance(argv, list) and bool(argv), "build.launch_argv: expected nonempty argument array")
    for arg in argv:
        text(arg, "build.launch_argv argument")
    need(argv[0] == build_path, "build.launch_argv: executable must match build.binary_path")
    need(not any(arg.split("=", 1)[0] in SMOKE_FLAGS for arg in argv[1:]),
         "build.launch_argv: smoke modes are not native acceptance")
    target = text(build["target"], "build.target")
    platform = "linux" if ticket == "ANS-87" else "macos"
    need(target in TARGETS[platform] and target.split("-", 1)[0] == run["configuration"]["architecture"],
         "build.target: must match native OS and hardware architecture")
    binary_hash = artifact(build["binary"], root, "build.binary")
    log_hash = artifact(build["build_log"], root, "build.build_log")
    need(binary_hash != log_hash, "build binary and log must be different artifacts")
    for digest in (binary_hash, log_hash):
        need(digest not in used_captures, "native capture reuses build binary or log bytes")
        build_hashes.add(digest)
    if binaries is not None:
        need(target in binaries, f"release binary manifest: missing target {target}")
        need(binaries[target] == binary_hash,
             f"release binary manifest: tested binary SHA-256 differs for {target}")

    captures = run["artifacts"]
    if glass:
        keys(captures, {"native-visual-capture", "measurement-log", "tester-notes"}, "artifacts")
    else:
        required = {"tester-notes", "native-text-probe"}
        if ticket == "ANS-87":
            required.add("native-atspi-trace")
        speech = {"native-speech-log", "native-speech-audio"}
        need(isinstance(captures, dict) and bool(captures.keys() & speech),
             "artifacts: actual native screen-reader speech log or audio is required")
        keys(captures, required | (captures.keys() & speech), "artifacts")
    for kind, ref in captures.items():
        digest = artifact(ref, root, kind)
        need(digest not in used_captures, f"captured bytes cannot be reused across records: {kind}")
        need(digest not in build_hashes, f"native capture reuses build binary or log bytes: {kind}")
        used_captures.add(digest)

    results = run["results"]
    keys(results, GLASS_CASES if glass else JOURNEYS, "results")
    for name, result in results.items():
        keys(result, {"outcome", "observation"}, name)
        need(result["outcome"] in ("pass", "fail", "not-run"), f"{name}: invalid outcome")
        text(result["observation"], f"{name}.observation")
    if glass:
        glass_measurements(run["measurements"], results)
    results_pass = all(item["outcome"] == "pass" for item in results.values())
    identity = (
        run["configuration"], target, binary_hash, build["renderer"], build["fixture"],
        tuple(build["launch_argv"][1:]),
    )
    return tester, started, identity, results_pass


def validate(ledger, root, review_record=None, binary_manifest=None):
    """Return (ticket, profile, state, detail) for every required matrix row."""
    keys(ledger, {"schema_version", "candidate_commit", "non_native_context", "tickets"}, "ledger")
    need(type(ledger["schema_version"]) is int and ledger["schema_version"] == 1,
         "unsupported schema_version")
    commit = ledger["candidate_commit"]
    if commit is not None:
        hex_id(commit, 40, "candidate_commit")
    binaries = None
    if binary_manifest is not None:
        keys(binary_manifest, {"candidate_commit", "binaries"}, "release binary manifest")
        need(commit is not None and binary_manifest["candidate_commit"] == commit,
             "release binary manifest: wrong candidate commit")
        binaries = binary_manifest["binaries"]
        allowed_targets = TARGETS["macos"] | TARGETS["linux"]
        need(isinstance(binaries, dict) and bool(binaries) and set(binaries) <= allowed_targets,
             "release binary manifest: expected a nonempty map of native Rust target triples")
        for target, digest in binaries.items():
            hex_id(digest, 64, f"release binary manifest {target}")
    approvals = {}
    if review_record is not None:
        keys(review_record, {"candidate_commit", "approvals"}, "independent review record")
        need(commit is not None and review_record["candidate_commit"] == commit,
             "independent review record: wrong candidate commit")
        approvals = review_record["approvals"]
        allowed = {f"{ticket}/{profile}" for ticket, profiles in PROFILES.items() for profile in profiles}
        need(isinstance(approvals, dict) and set(approvals) <= allowed,
             "independent review record: invalid approval profile")
        for label, digest in approvals.items():
            hex_id(digest, 64, f"independent review record {label}")
    context = ledger["non_native_context"]
    need(isinstance(context, list), "non_native_context: expected array")
    for record in context:
        keys(record, {"kind", "reference", "finding"}, "non_native_context record")
        need(text(record["kind"], "kind") in CONTEXT_KINDS, "non_native_context: unsupported kind")
        text(record["reference"], "reference")
        text(record["finding"], "finding")

    tickets = ledger["tickets"]
    keys(tickets, PROFILES, "tickets")
    output, run_ids, used_captures, build_hashes = [], set(), set(), set()
    for ticket, profiles in PROFILES.items():
        keys(tickets[ticket], profiles, ticket)
        for profile, row in tickets[ticket].items():
            label = f"{ticket}/{profile}"
            try:
                keys(row, {"blockers", "runs", "review"}, label)
                blockers, runs, review = row["blockers"], row["runs"], row["review"]
                need(isinstance(blockers, list), "blockers: expected array")
                for item in blockers:
                    text(item, "blocker")
                need(isinstance(runs, list) and len(runs) <= 2, "runs: expected 0–2 current runs")
                records = [validate_run(run, ticket, profile, commit, root, run_ids, used_captures,
                                        build_hashes, binaries)
                           for run in runs]
                testers = {item[0] for item in records}
                if len(records) == 2:
                    need(len(testers) == 2, "runs: need two different tester IDs")
                    need(not all(run["implemented_change"] for run in runs),
                         "runs: at least one tester must not have implemented the change")
                    need(records[0][2] == records[1][2],
                         "runs: independent runs must match native configuration and artifact identity")
                ready = len(records) == 2 and not blockers and all(item[3] for item in records)
                if review is not None:
                    need(ready, "review: cannot accept missing, failed, or blocked runs")
                    keys(review, {"source", "reviewer_id", "reviewed_at", "decision_ref",
                                  "evidence_sha256"}, "review")
                    need(review["source"] == "release-owner-native-review",
                         "review: release-owner-native-review is required")
                    reviewer = text(review["reviewer_id"], "reviewer_id").casefold()
                    need(reviewer not in testers, "review: reviewer must be distinct from both testers")
                    reviewed = timestamp(review["reviewed_at"], "reviewed_at")
                    need(reviewed > max(item[1] for item in records), "review must follow both runs")
                    text(review["decision_ref"], "decision_ref")
                    need(review["evidence_sha256"] == fingerprint([ticket, profile, runs]),
                         "review: stale or mismatched evidence fingerprint")
                    if label in approvals:
                        need(approvals[label] == fingerprint(review),
                             "review: independent release-owner record does not match")
                        if binaries is not None:
                            target = runs[0]["build"]["target"]
                            output.append((ticket, profile, "ACCEPTED",
                                           f"{target}; {review['decision_ref']}"))
                        else:
                            output.append((ticket, profile, "BINARY UNVERIFIED",
                                           "release-owner binary manifest required"))
                    else:
                        output.append((ticket, profile, "REVIEW UNVERIFIED",
                                       f"independent review record required for {fingerprint(review)}"))
                elif ready:
                    need(label not in approvals, "review: independent approval has no matching review")
                    output.append((ticket, profile, "REVIEW REQUIRED", fingerprint([ticket, profile, runs])))
                else:
                    need(label not in approvals, "review: independent approval has no matching review")
                    detail = "; ".join(blockers) or "needs two complete, passing native runs"
                    output.append((ticket, profile, "BLOCKED", detail))
            except (OSError, ValueError) as exc:
                raise ValueError(f"{label}: {exc}") from exc
    return output


def strict_object(pairs):
    result = {}
    for key, value in pairs:
        need(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def invalid_constant(value):
    raise ValueError(f"invalid JSON number: {value}")


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=strict_object,
                      parse_constant=invalid_constant)


def independent_record(path, root, label):
    resolved = path.resolve()
    need(not resolved.is_relative_to(root), f"{label}: must resolve outside the artifact root")
    need(resolved.is_file(), f"{label}: expected a regular file")
    need(resolved.stat().st_nlink <= 1, f"{label}: must not be a hardlink")
    record = read_json(resolved)
    need(isinstance(record, dict), f"{label}: expected an object")
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="Validate consistency; blocked is valid.")
    mode.add_argument("--require-accepted", action="store_true",
                      help="Require recorded native acceptance on all mandatory profiles.")
    parser.add_argument("--ledger", type=Path, default=DEFAULT_LEDGER)
    parser.add_argument("--artifact-root", type=Path, help="Defaults to the ledger's directory.")
    parser.add_argument("--expected-commit", help="Reject a ledger targeting a different commit.")
    parser.add_argument("--review-record", type=Path,
                        help="Independent approval record retrieved from release-owner-controlled storage.")
    parser.add_argument("--binary-manifest", type=Path,
                        help="Exact shipped binary SHA-256 by target from release-owner-controlled storage.")
    args = parser.parse_args(argv)
    try:
        ledger = read_json(args.ledger)
        root = (args.artifact_root or args.ledger.parent).resolve()
        review_record = (independent_record(args.review_record, root, "independent review record")
                         if args.review_record else None)
        binary_manifest = (independent_record(args.binary_manifest, root, "release binary manifest")
                           if args.binary_manifest else None)
        rows = validate(ledger, root, review_record, binary_manifest)
        accepted = sum(state == "ACCEPTED" for _, _, state, _ in rows)
        if args.require_accepted and accepted == len(rows) and args.expected_commit is None:
            print("Invalid platform acceptance ledger: --expected-commit is required "
                  "before accepting every profile", file=sys.stderr)
            return 2
        if args.expected_commit is not None:
            hex_id(args.expected_commit, 40, "--expected-commit")
            need(ledger["candidate_commit"] == args.expected_commit,
                 "ledger does not target --expected-commit")
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"Invalid platform acceptance ledger: {safe_output(str(exc))}", file=sys.stderr)
        return 2
    for ticket, profile, state, detail in rows:
        print(f"{ticket}/{profile}: {state} - {safe_output(detail)}")
    print(f"Ledger valid; {accepted}/{len(rows)} profiles have recorded native acceptance.")
    if args.require_accepted and accepted != len(rows):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
