"""Regression tests for evidence intake; all generated files are synthetic."""

import contextlib
import copy
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

import platform_acceptance_gate as gate


class PlatformAcceptanceGateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.root = self.base / "evidence"
        self.root.mkdir()
        self.ledger_path = self.root / "evidence.json"
        self.ledger = json.loads(gate.DEFAULT_LEDGER.read_text(encoding="utf-8"))
        self.row = self.ledger["tickets"]["ANS-88"]["macos-14"]

    def cli(self, mode, *extra, write=True):
        if write:
            self.ledger_path.write_text(json.dumps(self.ledger), encoding="utf-8")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = gate.main([mode, "--ledger", str(self.ledger_path), *extra])
        return code, out.getvalue(), err.getvalue()

    def asset(self, name):
        data = f"TEST-ONLY fixture bytes: {name}\n".encode()
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return {"path": name, "sha256": gate.hashlib.sha256(data).hexdigest()}

    def run_record(self, number, ticket="ANS-88", profile="macos-14", tag=""):
        # This mock deliberately exercises structural completeness. It is not
        # native evidence, and no test supplies a valid release-owner decision
        # except where a test explicitly verifies the full acceptance path.
        self.ledger["candidate_commit"] = "a" * 40
        test_id = f"{tag}mock-{number}"
        config = {
            "os_version": "14.7.1", "os_build": "MOCK-BUILD", "hardware": "TEST",
            "architecture": "aarch64", "display": "TEST", "locale": "TEST",
        }
        if ticket == "ANS-88":
            config["os_version"] = f"{profile.split('-')[1]}.7.1"
            config.update(voiceover_version="TEST", speech_settings="TEST", adapter="TEST")
            kinds = ["tester-notes", "native-speech-log", "native-text-probe"]
        elif ticket == "ANS-87":
            desktop, server = profile.split("-")
            config.update(
                os_version="24.04.1",
                distro="Ubuntu" if desktop == "gnome" else "KDE neon",
                desktop=desktop, desktop_version="46.1" if desktop == "gnome" else "6.2",
                display_server=server,
                compositor="mutter 46.1" if desktop == "gnome" else "KWin 6.2",
                orca_version="46.1", atspi_version="TEST", speech_settings="TEST", adapter="TEST",
            )
            kinds = ["tester-notes", "native-speech-log", "native-text-probe", "native-atspi-trace"]
        else:
            config["os_version"] = f"{profile.split('-')[1]}.7.1"
            config.update(display_profile="TEST")
            kinds = ["native-visual-capture", "measurement-log", "tester-notes"]

        run = {
            "source": "physical-native-manual",
            "run_id": test_id,
            "tester_id": test_id,
            "implemented_change": number == 1,
            "started_at": f"2026-11-21T12:00:0{number}+00:00",
            "configuration": config,
            "build": {
                "commit": self.ledger["candidate_commit"], "working_tree": "clean",
                "binary_path": "/qa/mock/ansa-desktop", "binary": self.asset("shared/binary"),
                "build_log": self.asset("shared/build-log"), "command": "TEST",
                "renderer": "TEST", "fixture": "TEST", "launch_argv": ["/qa/mock/ansa-desktop"],
                "promotion_ref": "TEST", "target": (
                    "aarch64-unknown-linux-gnu" if ticket == "ANS-87" else "aarch64-apple-darwin"
                ),
            },
            "artifacts": {kind: self.asset(f"{test_id}/{kind}") for kind in kinds},
            "results": {
                key: {"outcome": "pass", "observation": "TEST ONLY"}
                for key in (gate.GLASS_CASES if ticket == "ANS-97" else gate.JOURNEYS)
            },
        }
        if ticket == "ANS-97":
            run["measurements"] = {
                "tint_alpha": 0.8,
                "contrast": {
                    key: {
                        "foreground": [0, 0, 0], "background": [255, 255, 255],
                        "capture_locator": "TEST ONLY",
                    }
                    for key in gate.CONTRAST_CASES
                },
                "bounds": {
                    key: [{"state": state, "rect": [0, 0, 760, 500]} for state in transitions]
                    for key, transitions in gate.BOUNDS_TRANSITIONS.items()
                },
            }
        return run

    def pair(self, ticket="ANS-88", profile="macos-14", tag=""):
        row = self.ledger["tickets"][ticket][profile]
        row["runs"] = [self.run_record(number, ticket, profile, tag) for number in (1, 2)]
        row["blockers"] = []
        return row

    def fake_review(self, source="release-owner-native-review"):
        return {
            "source": source, "reviewer_id": "mock-reviewer",
            "reviewed_at": "2026-11-22T12:00:00Z", "decision_ref": "TEST-ONLY",
            "evidence_sha256": "f" * 64,
        }

    def matching_review(self, row, ticket="ANS-88", profile="macos-14"):
        row["review"] = self.fake_review()
        row["review"]["evidence_sha256"] = gate.fingerprint([ticket, profile, row["runs"]])
        return row["review"]

    def independent_record(self, name, value, *, inside=False):
        path = (self.root if inside else self.base) / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return str(path)

    def approval_record(self, row, *, digest=None, inside=False):
        return self.independent_record("review.json", {
            "candidate_commit": "a" * 40,
            "approvals": {"ANS-88/macos-14": digest or gate.fingerprint(row["review"])},
        }, inside=inside)

    def binary_manifest(self, target, digest, *, commit="a" * 40, inside=False):
        return self.independent_record("release.json", {
            "candidate_commit": commit,
            "binaries": {target: digest},
        }, inside=inside)

    def assert_invalid(self, message):
        code, out, err = self.cli("--check")
        self.assertEqual(code, 2)
        self.assertNotIn("ACCEPTED", out)
        self.assertIn(message, err)

    def test_checked_in_empty_matrix_is_valid_and_strict_is_blocked(self):
        code, out, _ = self.cli("--check")
        self.assertEqual(code, 0)
        self.assertEqual(out.count(": BLOCKED"), 8)
        self.assertIn("0/8 profiles", out)
        self.assertEqual(self.cli("--require-accepted")[0], 1)

    def test_ci_and_tree_context_cannot_satisfy_any_profile(self):
        self.assertTrue(any(item["kind"] == "ci-build-startup" for item in self.ledger["non_native_context"]))
        self.assertTrue(any(item["kind"] == "structural-tree-probe" for item in self.ledger["non_native_context"]))
        code, out, _ = self.cli("--require-accepted")
        self.assertEqual(code, 1)
        self.assertNotIn("ACCEPTED", out)

    def test_cannot_remove_required_linux_compositor_row(self):
        del self.ledger["tickets"]["ANS-87"]["kde-wayland"]
        self.assert_invalid("missing ['kde-wayland']")

    def test_claim_field_is_rejected(self):
        self.row["accepted"] = True
        self.assert_invalid("extra ['accepted']")

    def test_explicit_synthetic_ci_tree_and_startup_origins_are_rejected(self):
        run = self.run_record(1)
        self.row["runs"] = [run]
        for source in ("synthetic", "ci-build-startup", "structural-tree-probe", "keyboard-smoke"):
            with self.subTest(source=source):
                run["source"] = source
                self.assert_invalid("only physical-native-manual")

    def test_complete_structural_mock_still_requires_review(self):
        self.pair()
        code, out, err = self.cli("--require-accepted")
        self.assertEqual((code, err), (1, ""))
        self.assertIn("ANS-88/macos-14: REVIEW REQUIRED", out)
        self.assertNotIn("ACCEPTED", out)

    def test_machine_generated_decision_is_not_accepted(self):
        row = self.pair()
        row["review"] = self.fake_review("ci")
        self.assert_invalid("release-owner-native-review is required")

    def test_stale_review_fingerprint_is_rejected(self):
        row = self.pair()
        row["review"] = self.fake_review()
        self.assert_invalid("stale or mismatched evidence fingerprint")

    def test_self_asserted_matching_review_cannot_accept_mock_runs(self):
        row = self.pair()
        self.matching_review(row)
        code, out, err = self.cli("--require-accepted")
        self.assertEqual((code, err), (1, ""))
        self.assertIn("ANS-88/macos-14: REVIEW UNVERIFIED", out)
        self.assertNotIn("ACCEPTED", out)

    def test_independent_review_record_must_match_declared_decision(self):
        row = self.pair()
        self.matching_review(row)
        record = self.approval_record(row, digest="f" * 64)
        code, out, err = self.cli("--require-accepted", "--review-record", record)
        self.assertEqual(code, 2)
        self.assertIn("independent release-owner record does not match", err)
        self.assertNotIn("ACCEPTED", out)

    def test_independent_review_outside_artifact_root_validates_but_needs_shipped_binary(self):
        row = self.pair()
        self.matching_review(row)
        args = ("--artifact-root", str(self.root), "--review-record", self.approval_record(row))
        code, out, err = self.cli("--check", *args)
        self.assertEqual((code, err), (0, ""))
        self.assertIn("ANS-88/macos-14: BINARY UNVERIFIED", out)
        self.assertEqual(self.cli("--require-accepted", *args)[0], 1)

    def test_review_record_inside_explicit_artifact_root_is_rejected_after_resolution(self):
        row = self.pair()
        self.matching_review(row)
        self.approval_record(row, inside=True)
        sibling = self.base / "release"
        sibling.mkdir()
        disguised = sibling / ".." / "evidence" / "review.json"
        code, _, err = self.cli("--check", "--artifact-root", str(self.root),
                                "--review-record", str(disguised))
        self.assertEqual(code, 2)
        self.assertIn("independent review record: must resolve outside the artifact root", err)

    def test_release_binary_manifest_must_be_outside_evidence_bundle(self):
        row = self.pair()
        build = row["runs"][0]["build"]
        manifest = self.binary_manifest(build["target"], build["binary"]["sha256"], inside=True)
        code, _, err = self.cli("--check", "--binary-manifest", manifest)
        self.assertEqual(code, 2)
        self.assertIn("release binary manifest: must resolve outside the artifact root", err)

    def test_release_manifest_binds_target_and_tested_binary_even_for_same_commit(self):
        self.pair()
        manifest = self.binary_manifest("aarch64-apple-darwin", "f" * 64)
        code, _, err = self.cli("--require-accepted", "--binary-manifest", manifest)
        self.assertEqual(code, 2)
        self.assertIn("tested binary SHA-256 differs for aarch64-apple-darwin", err)
        manifest = self.binary_manifest("x86_64-apple-darwin", "f" * 64)
        code, _, err = self.cli("--require-accepted", "--binary-manifest", manifest)
        self.assertEqual(code, 2)
        self.assertIn("missing target aarch64-apple-darwin", err)

    def test_matching_binary_manifest_does_not_replace_native_review(self):
        row = self.pair()
        build = row["runs"][0]["build"]
        manifest = self.binary_manifest(build["target"], build["binary"]["sha256"])
        code, out, err = self.cli("--require-accepted", "--binary-manifest", manifest)
        self.assertEqual((code, err), (1, ""))
        self.assertIn("ANS-88/macos-14: REVIEW REQUIRED", out)

    def test_release_manifest_commit_must_match(self):
        self.pair()
        manifest = self.binary_manifest("aarch64-apple-darwin", "f" * 64, commit="b" * 40)
        code, _, err = self.cli("--check", "--binary-manifest", manifest)
        self.assertEqual(code, 2)
        self.assertIn("release binary manifest: wrong candidate commit", err)

    def test_tester_cannot_review_own_run(self):
        row = self.pair()
        row["review"] = {**self.fake_review(), "reviewer_id": "MOCK-2"}
        self.assert_invalid("reviewer must be distinct")

    def test_review_before_both_runs_is_rejected(self):
        row = self.pair()
        row["review"] = {**self.fake_review(), "reviewed_at": "2026-11-21T12:00:01Z"}
        self.assert_invalid("review must follow both runs")

    def test_open_risk_skipped_journey_or_disagreement_prevents_review(self):
        row = self.pair()
        row["review"] = self.fake_review()
        row["blockers"] = ["Open native semantic gap"]
        self.assert_invalid("cannot accept missing, failed, or blocked runs")
        row["blockers"] = []
        for outcome in ("fail", "not-run"):
            with self.subTest(outcome=outcome):
                row["runs"][1]["results"]["J9"]["outcome"] = outcome
                self.assert_invalid("cannot accept missing, failed, or blocked runs")

    def test_missing_journey_is_not_counted(self):
        row = self.pair()
        del row["runs"][0]["results"]["J10"]
        self.assert_invalid("missing ['J10']")

    def test_single_run_is_valid_but_not_reviewable(self):
        self.row.update(blockers=[], runs=[self.run_record(1)])
        code, out, _ = self.cli("--require-accepted")
        self.assertEqual(code, 1)
        self.assertIn("ANS-88/macos-14: BLOCKED", out)

    def test_testers_must_be_independent(self):
        row = self.pair()
        row["runs"][1]["tester_id"] = row["runs"][0]["tester_id"].upper()
        self.assert_invalid("two different tester IDs")

    def test_at_least_one_tester_cannot_be_an_implementer(self):
        row = self.pair()
        row["runs"][1]["implemented_change"] = True
        self.assert_invalid("at least one tester must not have implemented")

    def test_reused_recording_is_rejected(self):
        row = self.pair()
        row["runs"][1]["artifacts"]["native-speech-log"] = copy.deepcopy(
            row["runs"][0]["artifacts"]["native-speech-log"]
        )
        self.assert_invalid("captured bytes cannot be reused")

    def test_native_capture_cannot_reuse_build_log_or_binary_bytes(self):
        row = self.pair()
        run = row["runs"][0]
        for source in ("binary", "build_log"):
            with self.subTest(source=source):
                build_artifact = run["build"][source]
                self.asset("copy")
                (self.root / "copy").write_bytes((self.root / build_artifact["path"]).read_bytes())
                run["artifacts"]["native-speech-log"] = {**build_artifact, "path": "copy"}
                self.assert_invalid("native capture reuses build binary or log bytes")

    def test_later_build_cannot_reuse_earlier_capture_bytes(self):
        row = self.pair()
        first, second = row["runs"]
        second["build"]["build_log"] = first["artifacts"]["native-text-probe"]
        self.assert_invalid("native capture reuses build binary or log bytes")

    def test_tree_or_screenshot_without_speech_is_rejected(self):
        run = self.run_record(1)
        self.row["runs"] = [run]
        del run["artifacts"]["native-speech-log"]
        run["artifacts"]["tree-snapshot"] = self.asset("tree.txt")
        self.assert_invalid("native screen-reader speech log or audio is required")

    def test_orca_requires_atspi_trace_in_addition_to_speech(self):
        row = self.pair("ANS-87", "gnome-wayland")
        del row["runs"][0]["artifacts"]["native-atspi-trace"]
        self.assert_invalid("missing ['native-atspi-trace']")

    def test_linux_backends_are_not_interchangeable(self):
        row = self.pair("ANS-87", "gnome-wayland")
        row["runs"][0]["configuration"]["display_server"] = "x11"
        self.assert_invalid("expected gnome/wayland")

    def test_macos_major_versions_are_not_interchangeable(self):
        row = self.pair()
        row["runs"][0]["configuration"]["os_version"] = "15.7"
        self.assert_invalid("wrong macOS major version")

    def test_mismatched_binary_or_os_build_blocks_pair(self):
        row = self.pair()
        second = row["runs"][1]
        old_binary = second["build"]["binary"]
        second["build"]["binary"] = self.asset("other/binary")
        self.assert_invalid("must match native configuration and artifact identity")
        second["build"]["binary"] = old_binary
        second["configuration"]["os_build"] = "DIFFERENT"
        self.assert_invalid("must match native configuration and artifact identity")

    def test_dirty_build_or_different_commit_is_rejected(self):
        run = self.run_record(1)
        self.row["runs"] = [run]
        run["build"]["working_tree"] = "dirty"
        self.assert_invalid("must be from a clean checkout")
        run["build"]["working_tree"] = "clean"
        run["build"]["commit"] = "b" * 40
        self.assert_invalid("must match declared candidate commit")

    def test_build_target_must_match_native_os_and_architecture(self):
        row = self.pair()
        for target in ("x86_64-apple-darwin", "aarch64-unknown-linux-gnu"):
            with self.subTest(target=target):
                row["runs"][0]["build"]["target"] = target
                self.assert_invalid("build.target: must match native OS and hardware architecture")

    def test_structured_launch_argv_must_launch_the_hashed_binary(self):
        row = self.pair()
        build = row["runs"][0]["build"]
        build["launch_argv"] = build["binary_path"]
        self.assert_invalid("build.launch_argv: expected nonempty argument array")
        build["launch_argv"] = ["/bin/sh", build["binary_path"]]
        self.assert_invalid("executable must match build.binary_path")

    def test_smoke_launch_arguments_are_rejected(self):
        row = self.pair()
        build = row["runs"][0]["build"]
        for flag in ("--smoke-test", "--interaction-smoke-test", "--smoke-test=1"):
            with self.subTest(flag=flag):
                build["launch_argv"] = [build["binary_path"], flag]
                self.assert_invalid("smoke modes are not native acceptance")

    def test_external_commit_binding_is_checked(self):
        self.pair()
        self.assertEqual(self.cli("--check", "--expected-commit", "a" * 40)[0], 0)
        code, _, err = self.cli("--check", "--expected-commit", "b" * 40)
        self.assertEqual(code, 2)
        self.assertIn("does not target --expected-commit", err)

    def test_missing_tampered_and_escaped_artifact_paths_are_rejected(self):
        run = self.run_record(1)
        self.row["runs"] = [run]
        speech = run["artifacts"]["native-speech-log"]
        path = self.root / speech["path"]
        path.unlink()
        self.assert_invalid("missing, empty, or outside artifact root")
        path.write_bytes(b"altered")
        self.assert_invalid("SHA-256 mismatch")
        speech["path"] = "../outside.txt"
        self.assert_invalid("normalized relative POSIX path")

    def test_glass_requires_physical_visual_capture(self):
        row = self.pair("ANS-97")
        del row["runs"][0]["artifacts"]["native-visual-capture"]
        self.assert_invalid("missing ['native-visual-capture']")

    def test_glass_requires_all_rendered_contrast_cases(self):
        row = self.pair("ANS-97")
        del row["runs"][0]["measurements"]["contrast"]["light-white-selected-text"]
        self.assert_invalid("missing ['light-white-selected-text']")

    def test_glass_recalculates_ratio_and_does_not_round_up(self):
        row = self.pair("ANS-97")
        pair = row["runs"][0]["measurements"]["contrast"]["dark-black-caret"]
        pair.update(foreground=[119] * 3, background=[255] * 3)
        self.assertAlmostEqual(gate.contrast(pair["foreground"], pair["background"]), 4.4781, places=4)
        self.assert_invalid("below 4.5:1")

    def test_glass_tint_other_than_eighty_percent_cannot_pass(self):
        row = self.pair("ANS-97")
        row["runs"][0]["measurements"]["tint_alpha"] = 0.7
        self.assert_invalid("themed tint is not 80%")

    def test_glass_toggle_or_fallback_cannot_pass_when_bounds_move(self):
        row = self.pair("ANS-97")
        row["runs"][0]["measurements"]["bounds"]["fallback"][1]["rect"][0] += 1
        self.assert_invalid("G7: window moved or resized")

    def test_glass_and_theme_toggles_require_three_labeled_round_trips(self):
        row = self.pair("ANS-97")
        bounds = row["runs"][0]["measurements"]["bounds"]
        for case, journey in (("glass-dark", "G3"), ("theme-opaque", "G5")):
            with self.subTest(case=case):
                full = bounds[case]
                bounds[case] = full[:2]
                self.assert_invalid(f"{journey} {case}: expected 7 labeled bounds samples")
                bounds[case] = full
                full[2]["state"], full[3]["state"] = full[3]["state"], full[2]["state"]
                self.assert_invalid(f"{case}: expected transition cycle-1-")
                full[2]["state"], full[3]["state"] = full[3]["state"], full[2]["state"]

    def test_blocker_and_reference_controls_are_escaped_in_output(self):
        injection = "test\nfake\rrow\x1b[2J\u2028end\u202egap"
        self.row["blockers"] = [injection]
        code, out, _ = self.cli("--check")
        self.assertEqual(code, 0)
        escaped = r"test\nfake\rrow\x1b[2J\u2028end\u202egap"
        self.assertIn(escaped, out)
        self.assertNotIn(injection, out)
        self.assertEqual(gate.safe_output(injection), escaped)  # also used for accepted decision_ref

    def test_duplicate_json_keys_cannot_override_state(self):
        self.ledger_path.write_text('{"schema_version": 1, "schema_version": 2}', encoding="utf-8")
        code, _, err = self.cli("--check", write=False)
        self.assertEqual(code, 2)
        self.assertIn("duplicate JSON key: schema_version", err)

    def test_json_parser_rejects_nested_duplicate_keys_and_nonfinite_numbers(self):
        path = self.base / "strict.json"
        path.write_text('{"outer": {"dup": 1, "dup": 2}}', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate JSON key: dup"):
            gate.read_json(path)
        for literal in ("NaN", "Infinity", "-Infinity"):
            path.write_text(f'{{"value": {literal}}}', encoding="utf-8")
            with self.subTest(literal=literal):
                with self.assertRaisesRegex(ValueError, "invalid JSON number"):
                    gate.read_json(path)

    def test_untrusted_non_ascii_cannot_crash_or_inject_diagnostics(self):
        injection = "smile \U0001F600 accent \u00E9 bidi \u202E zero \u200B"
        escaped = r"smile \U0001f600 accent \xe9 bidi \u202e zero \u200b"
        self.assertEqual(gate.safe_output(injection), escaped)
        self.row["blockers"] = [injection]
        code, out, _ = self.cli("--check")
        self.assertEqual(code, 0)
        self.assertTrue(out.isascii())
        self.assertIn(escaped, out)
        run = self.run_record(1)
        self.row["runs"] = [run]
        run["artifacts"]["native-speech-log"]["path"] = "missing-\U0001F600"
        code, _, err = self.cli("--check")
        self.assertEqual(code, 2)
        self.assertTrue(err.isascii())
        self.assertIn(r"\U0001f600", err)

    def test_artifact_paths_reject_absolute_traversal_and_directory_or_empty_files(self):
        run = self.run_record(1)
        self.row["runs"] = [run]
        reference = run["artifacts"]["native-speech-log"]
        for name in ("/etc/passwd", "//server/share", "C:/outside", r"..\outside", "../outside"):
            with self.subTest(path=name):
                reference["path"] = name
                self.assert_invalid("normalized relative POSIX path")
        (self.root / "directory").mkdir()
        reference["path"] = "directory"
        self.assert_invalid("missing, empty, or outside artifact root")
        (self.root / "empty").write_bytes(b"")
        reference["path"] = "empty"
        self.assert_invalid("missing, empty, or outside artifact root")

    def test_symlink_escape_from_artifact_root_is_rejected(self):
        run = self.run_record(1)
        self.row["runs"] = [run]
        outside = self.base / "outside.bin"
        outside.write_bytes(b"outside the evidence root")
        link = self.root / "link"
        try:
            os.symlink(outside, link)
        except (OSError, NotImplementedError):
            self.skipTest("symlink creation is unavailable on this host")
        run["artifacts"]["native-speech-log"]["path"] = "link"
        self.assert_invalid("missing, empty, or outside artifact root")

    def test_external_approval_record_binds_commit_and_known_profile(self):
        row = self.pair()
        self.matching_review(row)
        record = self.independent_record("review.json", {
            "candidate_commit": "b" * 40,
            "approvals": {"ANS-88/macos-14": gate.fingerprint(row["review"])},
        })
        code, _, err = self.cli("--check", "--review-record", record)
        self.assertEqual(code, 2)
        self.assertIn("wrong candidate commit", err)
        record = self.independent_record("review.json", {
            "candidate_commit": "a" * 40,
            "approvals": {"ANS-88/macos-16": "f" * 64},
        })
        code, _, err = self.cli("--check", "--review-record", record)
        self.assertEqual(code, 2)
        self.assertIn("invalid approval profile", err)

    def test_release_manifest_requires_known_targets_and_lowercase_digests(self):
        self.pair()
        cases = (
            ({}, "expected a nonempty map"),
            ({"aarch64-apple-darwin": "F" * 64}, "expected 64 lowercase hex characters"),
            ({"riscv64-unknown-linux-gnu": "f" * 64}, "expected a nonempty map"),
        )
        for binaries, message in cases:
            with self.subTest(binaries=binaries):
                manifest = self.independent_record("release.json", {
                    "candidate_commit": "a" * 40,
                    "binaries": binaries,
                })
                code, _, err = self.cli("--check", "--binary-manifest", manifest)
                self.assertEqual(code, 2)
                self.assertIn(message, err)

    def test_independent_runs_must_record_the_same_launch_arguments(self):
        row = self.pair()
        build = row["runs"][1]["build"]
        build["launch_argv"] = [build["binary_path"], "--opaque"]
        self.assert_invalid("must match native configuration and artifact identity")

    def test_independent_runs_may_install_the_same_binary_at_different_paths(self):
        row = self.pair()
        build = row["runs"][1]["build"]
        build["binary_path"] = "/qa/other/ansa-desktop"
        build["launch_argv"] = [build["binary_path"]]
        code, out, _ = self.cli("--check")
        self.assertEqual(code, 0)
        self.assertIn("ANS-88/macos-14: REVIEW REQUIRED", out)

    def test_external_approval_cannot_replace_the_native_review(self):
        self.pair()
        record = self.independent_record("review.json", {
            "candidate_commit": "a" * 40,
            "approvals": {"ANS-88/macos-14": "f" * 64},
        })
        code, _, err = self.cli("--check", "--review-record", record)
        self.assertEqual(code, 2)
        self.assertIn("independent approval has no matching review", err)

    def test_external_record_hardlink_is_rejected(self):
        row = self.pair()
        self.matching_review(row)
        record = self.approval_record(row)
        link = self.root / "linked-review.json"
        try:
            os.link(record, link)
        except (OSError, NotImplementedError):
            self.skipTest("hardlink creation is unavailable on this host")
        code, _, err = self.cli("--check", "--review-record", record)
        self.assertEqual(code, 2)
        self.assertIn("must not be a hardlink", err)

    def test_complete_externally_bound_structural_evidence_can_accept_all_profiles(self):
        # Test-only synthetic fixtures in a temporary directory. This verifies
        # the positive path is reachable; it never writes acceptance into the
        # checked-in ledger or claims native evidence.
        rows = {}
        for ticket, profiles in gate.PROFILES.items():
            for profile in profiles:
                label = f"{ticket}/{profile}"
                row = self.pair(ticket, profile, tag=label.replace("/", "-") + "-")
                self.matching_review(row, ticket, profile)
                rows[label] = row
        approvals = self.independent_record("review.json", {
            "candidate_commit": "a" * 40,
            "approvals": {label: gate.fingerprint(row["review"]) for label, row in rows.items()},
        })
        macos = rows["ANS-88/macos-14"]["runs"][0]["build"]["binary"]["sha256"]
        linux = rows["ANS-87/gnome-x11"]["runs"][0]["build"]["binary"]["sha256"]
        manifest = self.independent_record("release.json", {
            "candidate_commit": "a" * 40,
            "binaries": {
                "aarch64-apple-darwin": macos,
                "aarch64-unknown-linux-gnu": linux,
            },
        })
        code, out, err = self.cli("--require-accepted", "--review-record", approvals,
                                  "--binary-manifest", manifest)
        self.assertEqual(code, 2)
        self.assertIn("--expected-commit is required", err)
        self.assertNotIn("ACCEPTED", out)
        code, out, err = self.cli("--require-accepted", "--review-record", approvals,
                                  "--binary-manifest", manifest,
                                  "--expected-commit", "a" * 40)
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(out.count(": ACCEPTED"), 8)
        self.assertIn("8/8 profiles", out)


if __name__ == "__main__":
    unittest.main()
