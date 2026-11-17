#!/usr/bin/env python3
"""Tests for the F01 reference validator and its fail-closed schema subset.

Run directly:
    python scripts/reference/test_validate_reference.py

ReferenceValidatorTests copies docs/reference into a temporary root, applies one
mutation and asserts that validation fails with the intended error code.
JsonSchemaLiteTests directly exercises unsupported-keyword, format, malformed
schema, $ref-sibling and JSON numeric-equality behavior. Standard library only;
the working tree is never modified.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
FIXTURE_ROOT = REPO_ROOT
sys.path.insert(0, str(SCRIPT_DIR))

from validate_reference import validate_root  # noqa: E402
from json_schema_lite import validate as validate_schema_lite  # noqa: E402


class ReferenceValidatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        shutil.copytree(FIXTURE_ROOT / "docs" / "reference", self.root / "docs" / "reference")

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def read(self, relative: str):
        return json.loads((self.root / "docs" / "reference" / relative).read_text(encoding="utf-8"))

    def write(self, relative: str, data) -> None:
        (self.root / "docs" / "reference" / relative).write_text(
            json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

    def codes(self):
        report = validate_root(self.root)
        self.assertFalse(report.ok, "mutation should have failed validation")
        return {issue.code for issue in report.errors}

    def row(self, ledger, scenario_id):
        return next(row for row in ledger["rows"] if row["scenario_id"] == scenario_id)

    def test_baseline_passes(self) -> None:
        report = validate_root(self.root)
        self.assertTrue(report.ok, [issue.__dict__ for issue in report.errors])
        self.assertEqual(report.stats["rows"], 100)
        self.assertEqual(report.stats["behavior_coverage"], {"windows": 48, "macos": 48})
        self.assertEqual(report.stats["observed_rows"], 0)
        self.assertEqual(report.stats["test_modes"], {"manual": 100})
        self.assertEqual(
            report.stats["buckets"],
            {"deferred": 26, "specified": 72, "unsupported": 2},
        )

    def test_duplicate_scenario_id(self) -> None:
        ledger = self.read("scenario-ledger.json")
        ledger["rows"][1]["scenario_id"] = ledger["rows"][0]["scenario_id"]
        self.write("scenario-ledger.json", ledger)
        self.assertIn("LEDGER_DUPLICATE_ID", self.codes())

    def test_missing_row_breaks_count(self) -> None:
        ledger = self.read("scenario-ledger.json")
        ledger["rows"].pop()
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"SCHEMA_LEDGER", "LEDGER_MISSING_ID"} & codes)

    def test_platform_imbalance(self) -> None:
        ledger = self.read("scenario-ledger.json")
        ledger["rows"] = [row for row in ledger["rows"] if row["scenario_id"] != "S50-MAC"]
        self.write("scenario-ledger.json", ledger)
        self.assertIn("PLATFORM_COUNT", self.codes())

    def test_removing_deferred_rows_fails_denominator(self) -> None:
        ledger = self.read("scenario-ledger.json")
        ledger["rows"] = [
            row
            for row in ledger["rows"]
            if row["denominator"]["bucket"] != "deferred"
        ]
        self.write("scenario-ledger.json", ledger)
        self.assertIn("DENOMINATOR_DEFERRED", self.codes())

    def test_excluding_a_deferred_row_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S06-WIN")["denominator"]["included"] = False
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"ROW_EXCLUDED", "DENOMINATOR_INCLUDED"} & codes)

    def test_missing_a_behavior_on_one_os(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S47-WIN")["behaviors"].remove("emoji.graphemes")
        self.write("scenario-ledger.json", ledger)
        report = validate_root(self.root)
        self.assertFalse(report.ok)
        matching = [
            issue
            for issue in report.errors
            if issue.code == "A_BEHAVIOR_UNCOVERED" and "emoji.graphemes:windows" in issue.path
        ]
        self.assertTrue(matching, [issue.__dict__ for issue in report.errors])
        self.assertFalse(
            [
                issue
                for issue in report.errors
                if issue.code == "A_BEHAVIOR_UNCOVERED"
                and "emoji.graphemes:macos" in issue.path
            ]
        )

    def test_runtime_evidence_without_identity_fails(self) -> None:
        catalog = self.read("evidence-catalog.json")
        catalog["entries"].append(
            {
                "evidence_id": "EV-RUNTIME-TEST",
                "kind": "runtime_observation",
                "title": "Synthetic runtime evidence",
                "source": "test-only",
                "retrieved": "2026-11-17",
                "confidence": "high",
                "limitations": ["Synthetic negative-test fixture."],
                "unknowns": ["Not a real observation."],
            }
        )
        self.write("evidence-catalog.json", catalog)
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-WIN")["evidence_refs"].append("EV-RUNTIME-TEST")
        self.write("scenario-ledger.json", ledger)
        self.assertIn("EVIDENCE_RUNTIME_UNBOUND", self.codes())

    def test_observed_row_with_unknown_identity_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        row = self.row(ledger, "S01-WIN")
        row["observation"].update(
            {
                "state": "observed",
                "actual": "Synthetic text claiming an impossible observation.",
                "evidence_id": "LOCAL-01",
                "recording": "recording.mp4",
                "observed_at": "2026-11-17",
            }
        )
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue(
            {
                "OBSERVATION_UNBOUND",
                "OBSERVATION_COUNT_MISMATCH",
                "OBSERVATION_RUNTIME_EVIDENCE",
            }
            <= codes
        )

    def test_not_observed_row_cannot_carry_actual_text(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-MAC")["observation"]["actual"] = "Invented text"
        self.write("scenario-ledger.json", ledger)
        self.assertIn("OBSERVATION_INVENTED", self.codes())

    def test_invented_hash_in_manifest_text_fails(self) -> None:
        manifest = self.read("identity/windows-reference.json")
        manifest["unknown_reason"] += " " + "a" * 64
        self.write("identity/windows-reference.json", manifest)
        self.assertIn("INVENTED_HASH", self.codes())

    def test_unknown_identity_value_fails(self) -> None:
        manifest = self.read("identity/windows-reference.json")
        manifest["identity_fields"]["application"]["version"]["value"] = "1.2.3"
        self.write("identity/windows-reference.json", manifest)
        self.assertIn("IDENTITY_INVENTED_VALUE", self.codes())

    def test_missing_permission_fails(self) -> None:
        manifest = self.read("identity/macos-reference.json")
        manifest["identity_fields"]["permissions"].pop()
        self.write("identity/macos-reference.json", manifest)
        codes = self.codes()
        self.assertTrue({"SCHEMA_IDENTITY", "IDENTITY_PERMISSIONS"} & codes)

    def test_invented_rea_id_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-WIN")["status_reason"] += " See REA-123456 for details."
        self.write("scenario-ledger.json", ledger)
        self.assertIn("INVENTED_REA_ID", self.codes())

    def test_unknown_evidence_reference_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S02-WIN")["evidence_refs"].append("EV-NOT-REAL")
        self.write("scenario-ledger.json", ledger)
        self.assertIn("ROW_EVIDENCE_MISSING", self.codes())

    def test_inflated_confidence_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-WIN")["confidence"] = "high"
        self.write("scenario-ledger.json", ledger)
        self.assertIn("ROW_CONFIDENCE", self.codes())

    def test_executable_test_without_command_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        test = self.row(ledger, "S02-WIN")["implementation_test"]
        test["mode"] = "executable"
        test["state"] = "planned"
        test["command"] = None
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"SCHEMA_LEDGER", "TEST_COMMAND", "TEST_EXECUTABLE_PLANNED"} & codes)

    def test_supported_bucket_is_rejected(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-WIN")["denominator"]["bucket"] = "supported"
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"SCHEMA_LEDGER", "ROW_BUCKET"} & codes)

    def test_proposed_citation_must_stay_low(self) -> None:
        catalog = self.read("evidence-catalog.json")
        entry = next(
            item
            for item in catalog["entries"]
            if item["evidence_id"] == "EV-DOC-OS-PERMISSIONS"
        )
        entry["confidence"] = "medium"
        self.write("evidence-catalog.json", catalog)
        self.assertIn("CATALOG_PROPOSED_CONFIDENCE", self.codes())

    def test_fetched_source_requires_declaration(self) -> None:
        catalog = self.read("evidence-catalog.json")
        entry = next(
            item for item in catalog["entries"] if item["evidence_id"] == "EV-DOC-MANUAL"
        )
        entry["fetch_state"] = "fetched_this_session"
        self.write("evidence-catalog.json", catalog)
        self.assertIn("CATALOG_FETCH_UNDECLARED", self.codes())

    def test_passed_test_without_observation_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S02-WIN")["implementation_test"]["state"] = "passed"
        self.write("scenario-ledger.json", ledger)
        self.assertIn("TEST_STATE_UNBOUND", self.codes())

    def test_missing_seed_row_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        ledger["rows"] = [
            row for row in ledger["rows"] if row["scenario_id"] != "S20-MAC"
        ]
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"SEED_ROW_MISSING", "LEDGER_MISSING_ID"} & codes)

    def test_seed_backlink_mismatch_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-WIN")["seed_id"] = None
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"SEED_BACKLINK", "SEED_ROW_COUNT"} & codes)

    def test_audit_empty_object_fails(self) -> None:
        self.write("audits/design-source-baseline-consistency-2026-11-17.json", {})
        self.assertIn("AUDIT_SHAPE", self.codes())

    def test_audit_empty_array_fails(self) -> None:
        self.write("audits/design-source-baseline-consistency-2026-11-17.json", [])
        self.assertIn("AUDIT_SHAPE", self.codes())

    def test_audit_null_fails(self) -> None:
        self.write("audits/design-source-baseline-consistency-2026-11-17.json", None)
        self.assertIn("AUDIT_SHAPE", self.codes())

    def test_audit_missing_file_fails(self) -> None:
        (
            self.root
            / "docs"
            / "reference"
            / "audits"
            / "design-source-baseline-consistency-2026-11-17.json"
        ).unlink()
        codes = self.codes()
        self.assertTrue({"AUDIT_FILE", "AUDIT_SHAPE"} & codes)

    def test_duplicate_evidence_id_fails(self) -> None:
        catalog = self.read("evidence-catalog.json")
        catalog["entries"].append(dict(catalog["entries"][0]))
        self.write("evidence-catalog.json", catalog)
        self.assertIn("CATALOG_DUPLICATE_ID", self.codes())

    def test_invalid_calendar_dates_fail(self) -> None:
        ledger = self.read("scenario-ledger.json")
        ledger["created"] = "2026-02-30"
        self.write("scenario-ledger.json", ledger)

        manifest = self.read("identity/windows-reference.json")
        manifest["last_checked"] = "2026-13-01"
        self.write("identity/windows-reference.json", manifest)

        catalog = self.read("evidence-catalog.json")
        catalog["created"] = "2026-99-99"
        catalog["entries"][0]["source_date"] = "2026-04-31"
        self.write("evidence-catalog.json", catalog)

        audit = self.read("audits/design-source-baseline-consistency-2026-11-17.json")
        audit["created"] = "2026-02-30"
        self.write("audits/design-source-baseline-consistency-2026-11-17.json", audit)

        codes = self.codes()
        self.assertTrue(
            {
                "LEDGER_DATE",
                "IDENTITY_DATE",
                "CATALOG_CREATED_DATE",
                "CATALOG_SOURCE_DATE",
                "AUDIT_DATE",
            }
            <= codes
        )

    def test_tested_status_is_unreachable(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-WIN")["parity_status"] = "tested"
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"SCHEMA_LEDGER", "ROW_STATUS_FROZEN"} & codes)

    def test_audit_runtime_claim_fails(self) -> None:
        audit = self.read("audits/design-source-baseline-consistency-2026-11-17.json")
        audit["runtime_claim"] = True
        self.write("audits/design-source-baseline-consistency-2026-11-17.json", audit)
        self.assertIn("AUDIT_RUNTIME_CLAIM", self.codes())

    def test_audit_requires_ten_rows(self) -> None:
        audit = self.read("audits/design-source-baseline-consistency-2026-11-17.json")
        audit["rows"].pop()
        audit["row_count"] = 9
        self.write("audits/design-source-baseline-consistency-2026-11-17.json", audit)
        self.assertIn("AUDIT_ROW_COUNT", self.codes())

    def test_audit_evidence_must_match_row(self) -> None:
        audit = self.read("audits/design-source-baseline-consistency-2026-11-17.json")
        audit["rows"][0]["evidence_replayed"] = ["EV-DOC-FILE-SEARCH"]
        self.write("audits/design-source-baseline-consistency-2026-11-17.json", audit)
        self.assertIn("AUDIT_EVIDENCE_MISMATCH", self.codes())

    def test_invalid_enum_fails_schema(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S01-WIN")["priority"] = "Z"
        self.write("scenario-ledger.json", ledger)
        self.assertIn("SCHEMA_LEDGER", self.codes())

    def test_a_row_without_behavior_fails(self) -> None:
        ledger = self.read("scenario-ledger.json")
        self.row(ledger, "S21-WIN")["behaviors"] = []
        self.write("scenario-ledger.json", ledger)
        self.assertIn("A_ROW_NO_BEHAVIOR", self.codes())

    def test_unsupported_rows_must_remain(self) -> None:
        ledger = self.read("scenario-ledger.json")
        ledger["rows"] = [
            row
            for row in ledger["rows"]
            if row["denominator"]["bucket"] != "unsupported"
        ]
        self.write("scenario-ledger.json", ledger)
        codes = self.codes()
        self.assertTrue({"DENOMINATOR_UNSUPPORTED", "SEED_S20_DENOMINATOR"} & codes)


class JsonSchemaLiteTests(unittest.TestCase):
    def test_unknown_keyword_fails(self) -> None:
        for schema in (
            {"type": "string", "minLenght": 3},
            {"type": "object", "properties": {"a": {"minLenght": 1}}},
        ):
            errors = validate_schema_lite("abc", schema)
            self.assertTrue(errors)
            self.assertIn("unsupported schema keyword", " ".join(errors))

    def test_unsupported_format_fails(self) -> None:
        errors = validate_schema_lite("a@b.c", {"type": "string", "format": "email"})
        self.assertTrue(errors)
        self.assertIn("format assertions are not implemented", " ".join(errors))

    def test_invalid_or_non_object_schema_fails(self) -> None:
        self.assertIn(
            "boolean schemas are unsupported",
            " ".join(validate_schema_lite("x", True)),
        )
        self.assertIn(
            "unsupported type",
            " ".join(validate_schema_lite("x", {"type": "mystery"})),
        )
        self.assertIn(
            "schema must be an object",
            " ".join(
                validate_schema_lite(
                    [], {"type": "array", "items": [{"type": "string"}]}
                )
            ),
        )

    def test_ref_siblings_are_rejected(self) -> None:
        schema = {
            "$defs": {"s": {"type": "string"}},
            "$ref": "#/$defs/s",
            "minLength": 2,
        }
        errors = validate_schema_lite("abc", schema)
        self.assertTrue(errors)
        self.assertIn("$ref with siblings", " ".join(errors))

    def test_unique_items_uses_json_numeric_equality(self) -> None:
        schema = {"type": "array", "uniqueItems": True}
        self.assertIn(
            "duplicate array item",
            " ".join(validate_schema_lite([1, 1.0], schema)),
        )
        self.assertEqual(validate_schema_lite([1, True], schema), [])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=REPO_ROOT,
        help="reference repository root copied into each temporary fixture tree",
    )
    args, remaining = parser.parse_known_args()
    FIXTURE_ROOT = args.root.resolve()
    unittest.main(argv=[sys.argv[0], *remaining], verbosity=2)
