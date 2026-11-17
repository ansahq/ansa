#!/usr/bin/env python3
"""Validate the F01 reference ledger, identity manifests, evidence catalog,
A-behavior coverage, seed intents and design/source-baseline consistency audit.

Standard library only. The checked JSON Schema subset lives in
json_schema_lite.py next to this file.

Exit status: 0 = all checks pass, 1 = validation errors, 2 = invocation error.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Any, Iterable

sys.path.insert(0, str(Path(__file__).resolve().parent))
from json_schema_lite import validate as validate_schema  # noqa: E402

A_WORKFLOWS = [
    "launcher",
    "action_navigation",
    "app_catalog",
    "file_search",
    "calculator",
    "quicklinks",
    "emoji_symbols",
]
REQUIRED_PERMISSIONS = [
    "accessibility",
    "screen_recording",
    "automation",
    "full_disk_access",
    "input_monitoring",
]
EVIDENCE_KINDS = [
    "vendor_documentation",
    "source_inspection",
    "static_artifact_analysis",
    "runtime_observation",
    "design_proposal",
]
CONFIDENCE_RANK = {"low": 0, "medium": 1, "high": 2}
STATUS_BUCKET = {
    "documented": "specified",
    "specified": "specified",
    "unsupported": "unsupported",
    "deferred": "deferred",
}
AUDIT_RESULTS = {"design-consistent", "gap-recorded"}
FETCH_STATES = {
    "inherited_not_refetched",
    "proposed_unverified",
    "local_repository_source",
    "fetched_this_session",
}
REA_ID = re.compile(r"\bREA-[A-Za-z0-9][A-Za-z0-9-]{2,}\b")
HEX64 = re.compile(r"\b[0-9a-fA-F]{64}\b")
DATE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")


def valid_calendar_date(value: Any) -> bool:
    if not isinstance(value, str) or DATE.match(value) is None:
        return False
    try:
        date.fromisoformat(value)
    except ValueError:
        return False
    return True


@dataclass
class Issue:
    code: str
    path: str
    message: str


@dataclass
class Report:
    errors: list[Issue] = field(default_factory=list)
    warnings: list[Issue] = field(default_factory=list)
    stats: dict[str, Any] = field(default_factory=dict)

    @property
    def ok(self) -> bool:
        return not self.errors

    def error(self, code: str, path: str, message: str) -> None:
        self.errors.append(Issue(code, path, message))

    def warn(self, code: str, path: str, message: str) -> None:
        self.warnings.append(Issue(code, path, message))


def load_json(path: Path, report: Report, code: str) -> Any | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        report.error(code, str(path), "file is missing")
    except json.JSONDecodeError as exc:
        report.error(code, str(path), f"invalid JSON: {exc}")
    return None


def strings_in(value: Any) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from strings_in(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings_in(item)


def identity_fields(manifest: dict[str, Any]) -> Iterable[tuple[str, dict[str, Any]]]:
    fields = manifest.get("identity_fields", {})
    for section in ("application", "account", "extensions"):
        for name, item in fields.get(section, {}).items():
            if isinstance(item, dict) and "state" in item:
                yield f"identity_fields.{section}.{name}", item
    for name in ("version", "build", "architecture"):
        item = fields.get("os", {}).get(name)
        if isinstance(item, dict):
            yield f"identity_fields.os.{name}", item
    for index, permission in enumerate(fields.get("permissions", [])):
        if isinstance(permission, dict):
            yield f"identity_fields.permissions[{index}]({permission.get('name')})", permission


def field_known(item: dict[str, Any]) -> bool:
    return item.get("state") == "known" and item.get("value") is not None


def identity_complete(manifest: dict[str, Any]) -> bool:
    return all(field_known(item) for _, item in identity_fields(manifest))


class ReferenceValidation:
    def __init__(self, root: Path):
        self.root = root.resolve()
        self.ref = self.root / "docs" / "reference"
        self.report = Report()
        self.ledger: dict[str, Any] = {}
        self.ledger_rows: list[dict[str, Any]] = []
        self.ledger_schema: dict[str, Any] = {}
        self.identity_schema: dict[str, Any] = {}
        self.manifests: list[dict[str, Any]] = []
        self.manifests_by_id: dict[str, dict[str, Any]] = {}
        self.catalog: dict[str, Any] = {}
        self.evidence_by_id: dict[str, dict[str, Any]] = {}
        self.kind_policy: dict[str, Any] = {}
        self.matrix: dict[str, Any] = {}
        self.behaviors_by_id: dict[str, dict[str, Any]] = {}
        self.seeds: list[dict[str, Any]] = []
        self.seed_by_id: dict[str, dict[str, Any]] = {}
        self.audit: dict[str, Any] = {}
        self.rows_by_id: dict[str, dict[str, Any]] = {}
        self.rows_by_platform: dict[str, list[dict[str, Any]]] = {
            "windows": [],
            "macos": [],
        }
        self._known_hashes: set[str] = set()

    # ------------------------------------------------------------------ load
    def load(self) -> None:
        self.ledger_schema = load_json(
            self.ref / "schema" / "scenario-ledger.schema.json",
            self.report,
            "SCHEMA_FILE_LEDGER",
        ) or {}
        self.identity_schema = load_json(
            self.ref / "schema" / "reference-identity.schema.json",
            self.report,
            "SCHEMA_FILE_IDENTITY",
        ) or {}
        self.ledger = load_json(
            self.ref / "scenario-ledger.json", self.report, "LEDGER_FILE"
        ) or {}
        self.catalog = load_json(
            self.ref / "evidence-catalog.json", self.report, "CATALOG_FILE"
        ) or {}
        self.matrix = load_json(
            self.ref / "coverage-matrix.json", self.report, "MATRIX_FILE"
        ) or {}
        seeds_doc = load_json(
            self.ref / "seeds" / "seed-intents.json", self.report, "SEEDS_FILE"
        ) or {}
        self.seeds = seeds_doc.get("seeds", []) if isinstance(seeds_doc, dict) else []
        self.audit = load_json(
            self.ref / "audits" / "design-source-baseline-consistency-2026-11-17.json",
            self.report,
            "AUDIT_FILE",
        ) or {}

        identity_dir = self.ref / "identity"
        manifest_files = sorted(identity_dir.glob("*.json"))
        if len(manifest_files) != 2:
            self.report.error(
                "IDENTITY_COUNT",
                str(identity_dir),
                f"expected exactly two platform identity manifests, found {len(manifest_files)}",
            )
        for path in manifest_files:
            manifest = load_json(path, self.report, "IDENTITY_FILE")
            if isinstance(manifest, dict):
                self.manifests.append(manifest)

        rows = self.ledger.get("rows", []) if isinstance(self.ledger, dict) else []
        self.ledger_rows = [row for row in rows if isinstance(row, dict)]
        for row in self.ledger_rows:
            scenario_id = row.get("scenario_id")
            if isinstance(scenario_id, str):
                self.rows_by_id[scenario_id] = row
                platform = row.get("target", {}).get("platform")
                if platform in self.rows_by_platform:
                    self.rows_by_platform[platform].append(row)
        for manifest in self.manifests:
            identity_id = manifest.get("reference_identity_id")
            if isinstance(identity_id, str):
                self.manifests_by_id[identity_id] = manifest
        entries = (
            self.catalog.get("entries", []) if isinstance(self.catalog, dict) else []
        )
        if not isinstance(entries, list):
            self.report.error(
                "CATALOG_ENTRIES_SHAPE",
                "evidence-catalog.json",
                "entries must be an array",
            )
            entries = []
        for entry in entries:
            if not isinstance(entry, dict):
                self.report.error(
                    "CATALOG_ENTRY_SHAPE",
                    "evidence-catalog.json",
                    "every entries item must be an object",
                )
                continue
            evidence_id = entry.get("evidence_id")
            if not isinstance(evidence_id, str):
                self.report.error(
                    "CATALOG_ENTRY_ID",
                    "evidence-catalog.json",
                    "every entry needs a string evidence_id",
                )
                continue
            if evidence_id in self.evidence_by_id:
                self.report.error(
                    "CATALOG_DUPLICATE_ID",
                    evidence_id,
                    "duplicate evidence IDs are not allowed and are not silently overwritten",
                )
                continue
            self.evidence_by_id[evidence_id] = entry
        self.kind_policy = (
            self.catalog.get("kind_policy", {}) if isinstance(self.catalog, dict) else {}
        )
        for behavior in self.matrix.get("behaviors", []) if isinstance(self.matrix, dict) else []:
            if isinstance(behavior, dict) and isinstance(behavior.get("behavior_id"), str):
                self.behaviors_by_id[behavior["behavior_id"]] = behavior
        for seed in self.seeds:
            if isinstance(seed, dict) and isinstance(seed.get("seed_id"), str):
                self.seed_by_id[seed["seed_id"]] = seed

    # ---------------------------------------------------------------- schema
    def check_schemas(self) -> None:
        if self.ledger and self.ledger_schema:
            for error in validate_schema(self.ledger, self.ledger_schema):
                self.report.error("SCHEMA_LEDGER", error.split(":", 1)[0], error)
        if self.identity_schema:
            for manifest in self.manifests:
                for error in validate_schema(manifest, self.identity_schema):
                    self.report.error(
                        "SCHEMA_IDENTITY",
                        f"{manifest.get('manifest_id', '?')}:{error.split(':', 1)[0]}",
                        error,
                    )

    # -------------------------------------------------------------- identity
    def check_identities(self) -> None:
        platforms = Counter()
        manifest_ids = set()
        for manifest in self.manifests:
            path = str(manifest.get("manifest_id", "?"))
            platform = manifest.get("platform")
            platforms[platform] += 1
            if manifest.get("manifest_id") in manifest_ids:
                self.report.error(
                    "IDENTITY_MANIFEST_ID", path, "manifest IDs must be unique"
                )
            manifest_ids.add(manifest.get("manifest_id"))
            identity_id = manifest.get("reference_identity_id", "")
            expected_prefix = {"windows": "ref-win-", "macos": "ref-mac-"}.get(platform)
            if expected_prefix and not identity_id.startswith(expected_prefix):
                self.report.error(
                    "IDENTITY_ID_PLATFORM",
                    path,
                    f"identity ID {identity_id!r} does not match platform {platform!r}",
                )
            expected_os = {"windows": "Windows", "macos": "macOS"}.get(platform)
            actual_os = manifest.get("identity_fields", {}).get("os", {}).get("name")
            if expected_os and actual_os != expected_os:
                self.report.error(
                    "IDENTITY_OS_NAME",
                    path,
                    f"OS name {actual_os!r} does not match platform {platform!r}",
                )
            if manifest.get("state") != "unknown":
                self.report.error(
                    "IDENTITY_STATE", path, "F01 manifests must remain explicitly unknown until capture"
                )
            if not valid_calendar_date(manifest.get("last_checked")):
                self.report.error(
                    "IDENTITY_DATE",
                    path,
                    f"last_checked {manifest.get('last_checked')!r} is not a valid calendar date",
                )
            fields = manifest.get("identity_fields", {})
            app = fields.get("application", {})
            os_fields = fields.get("os", {})
            account = fields.get("account", {})
            extensions = fields.get("extensions", {})
            required_paths = {
                "application.path": app.get("path"),
                "application.version": app.get("version"),
                "application.build": app.get("build"),
                "application.architecture": app.get("architecture"),
                "application.artifact_sha256": app.get("artifact_sha256"),
                "application.signature_identity": app.get("signature_identity"),
                "application.signature_status": app.get("signature_status"),
                "os.version": os_fields.get("version"),
                "os.build": os_fields.get("build"),
                "os.architecture": os_fields.get("architecture"),
                "account.tier": account.get("tier"),
                "account.signed_in": account.get("signed_in"),
                "extensions.enabled": extensions.get("enabled"),
            }
            for name, item in required_paths.items():
                if not isinstance(item, dict):
                    self.report.error(
                        "IDENTITY_FIELD_MISSING", f"{path}:{name}", "field is missing"
                    )
                    continue
                if item.get("state") != "unknown":
                    self.report.error(
                        "IDENTITY_NOT_EXPLICIT",
                        f"{path}:{name}",
                        "F01 identity is unavailable, so the field must be state=unknown",
                    )
                if item.get("value") is not None:
                    self.report.error(
                        "IDENTITY_INVENTED_VALUE",
                        f"{path}:{name}",
                        "an unknown identity field must not carry a value",
                    )
                if not isinstance(item.get("reason"), str) or len(item["reason"]) < 20:
                    self.report.error(
                        "IDENTITY_REASON",
                        f"{path}:{name}",
                        "an unknown field needs an explicit non-trivial reason",
                    )
                if item.get("evidence_id") is not None:
                    self.report.error(
                        "IDENTITY_EVIDENCE_ID",
                        f"{path}:{name}",
                        "an unknown field must not invent an evidence ID",
                    )

            permissions = fields.get("permissions", [])
            names = [
                permission.get("name")
                for permission in permissions
                if isinstance(permission, dict)
            ]
            if sorted(names) != sorted(REQUIRED_PERMISSIONS):
                self.report.error(
                    "IDENTITY_PERMISSIONS",
                    path,
                    f"permissions must be exactly {REQUIRED_PERMISSIONS}, found {names}",
                )
            if len(names) != len(set(names)):
                self.report.error(
                    "IDENTITY_PERMISSION_DUPLICATE", path, "permission names must be unique"
                )
            for index, permission in enumerate(permissions):
                if not isinstance(permission, dict):
                    continue
                if permission.get("state") != "unknown" or permission.get("value") is not None:
                    self.report.error(
                        "IDENTITY_PERMISSION_STATE",
                        f"{path}:permissions[{index}]",
                        "permission state must be explicitly unknown with a null value",
                    )
                if permission.get("evidence_id") is not None:
                    self.report.error(
                        "IDENTITY_PERMISSION_EVIDENCE",
                        f"{path}:permissions[{index}]",
                        "an unknown permission must not invent an evidence ID",
                    )

            boundary = manifest.get("evidence_boundary", {})
            if boundary.get("reference_app_available") is not False:
                self.report.error(
                    "IDENTITY_APP_AVAILABLE", path, "reference_app_available must be false"
                )
            if boundary.get("runtime_observations") != 0:
                self.report.error(
                    "IDENTITY_RUNTIME_COUNT",
                    path,
                    "runtime_observations must be 0 while no reference app is available",
                )
            if boundary.get("rea_evidence_ids"):
                self.report.error(
                    "IDENTITY_REA_IDS",
                    path,
                    "no REA evidence IDs may be recorded when none were returned",
                )
            capture = manifest.get("capture_environment", {})
            if capture.get("profile_kind") != "dedicated_test_profile":
                self.report.error(
                    "IDENTITY_PROFILE",
                    path,
                    "capture must use a dedicated test profile",
                )
            if capture.get("user_data") != "synthetic_only":
                self.report.error(
                    "IDENTITY_USER_DATA", path, "capture must be synthetic-data only"
                )

        for platform in ("windows", "macos"):
            if platforms[platform] != 1:
                self.report.error(
                    "IDENTITY_PLATFORM_COUNT",
                    "docs/reference/identity",
                    f"expected one {platform} manifest, found {platforms[platform]}",
                )

    # --------------------------------------------------------------- catalog
    def check_catalog(self) -> None:
        if not isinstance(self.catalog, dict):
            self.report.error(
                "CATALOG_SHAPE", "evidence-catalog.json", "catalog must be an object"
            )
            return
        kind_policy = self.kind_policy
        if not valid_calendar_date(self.catalog.get("created")):
            self.report.error(
                "CATALOG_CREATED_DATE",
                "created",
                f"catalog created date {self.catalog.get('created')!r} is not a valid calendar date",
            )
        provenance = self.catalog.get("provenance", {}) if isinstance(self.catalog, dict) else {}
        external_fetches = provenance.get("external_fetches_this_session")
        if not isinstance(external_fetches, list):
            self.report.error(
                "CATALOG_PROVENANCE_FETCHES",
                "provenance",
                "external_fetches_this_session must be an explicit array",
            )
            external_fetches = []
        if external_fetches:
            self.report.error(
                "CATALOG_PROVENANCE_FETCH_CLAIM",
                "provenance",
                "this frozen baseline performed no external fetches; a fetch claim requires a versioned migration",
            )
        for key in (
            "baseline",
            "baseline_date",
            "baseline_date_meaning",
            "local_sources_read_this_session",
            "unverified_supporting_citations",
        ):
            if key not in provenance:
                self.report.error(
                    "CATALOG_PROVENANCE_FIELD",
                    f"provenance.{key}",
                    "explicit provenance is required",
                )
        baseline_meaning = provenance.get("baseline_date_meaning", "")
        if not isinstance(baseline_meaning, str) or "not" not in baseline_meaning.lower():
            self.report.error(
                "CATALOG_PROVENANCE_MEANING",
                "provenance.baseline_date_meaning",
                "the baseline date must explicitly say it is not a fetch by this session",
            )
        if not valid_calendar_date(provenance.get("baseline_date")):
            self.report.error(
                "CATALOG_BASELINE_DATE",
                "provenance.baseline_date",
                f"baseline_date {provenance.get('baseline_date')!r} is not a valid calendar date",
            )
        proposed_ids = {
            evidence_id
            for evidence_id, entry in self.evidence_by_id.items()
            if entry.get("fetch_state") == "proposed_unverified"
        }
        declared_proposed = set(provenance.get("unverified_supporting_citations", []))
        if proposed_ids != declared_proposed:
            self.report.error(
                "CATALOG_PROVENANCE_PROPOSED",
                "provenance.unverified_supporting_citations",
                f"proposed entries {sorted(proposed_ids)} do not match the declared list {sorted(declared_proposed)}",
            )

        for kind in EVIDENCE_KINDS:
            if kind not in kind_policy:
                self.report.error(
                    "CATALOG_KIND_POLICY", kind, "every evidence kind needs an explicit policy"
                )
        used_kinds = Counter()
        for evidence_id, entry in self.evidence_by_id.items():
            kind = entry.get("kind")
            used_kinds[kind] += 1
            if "retrieved" in entry:
                self.report.error(
                    "CATALOG_RETRIEVED_AMBIGUOUS",
                    evidence_id,
                    "replace 'retrieved' with source_date, date_meaning and fetch_state",
                )
            fetch_state = entry.get("fetch_state")
            if fetch_state not in FETCH_STATES:
                self.report.error(
                    "CATALOG_FETCH_STATE",
                    evidence_id,
                    f"fetch_state must be one of {sorted(FETCH_STATES)}",
                )
            if not isinstance(entry.get("date_meaning"), str) or len(
                entry.get("date_meaning", "")
            ) < 20:
                self.report.error(
                    "CATALOG_DATE_MEANING",
                    evidence_id,
                    "date_meaning must explain what the source date means",
                )
            source_date = entry.get("source_date")
            if source_date is not None and not valid_calendar_date(source_date):
                self.report.error(
                    "CATALOG_SOURCE_DATE",
                    evidence_id,
                    "source_date must be a real calendar date or null",
                )
            if fetch_state == "proposed_unverified":
                if source_date is not None:
                    self.report.error(
                        "CATALOG_PROPOSED_DATE",
                        evidence_id,
                        "an unfetched proposed citation must not carry a retrieval date",
                    )
                if entry.get("confidence") != "low":
                    self.report.error(
                        "CATALOG_PROPOSED_CONFIDENCE",
                        evidence_id,
                        "an unfetched proposed citation must not raise confidence above low",
                    )
            if fetch_state in {"inherited_not_refetched", "local_repository_source"}:
                locator = entry.get("source_locator")
                if not isinstance(locator, str) or len(locator) < 10:
                    self.report.error(
                        "CATALOG_SOURCE_LOCATOR",
                        evidence_id,
                        "inherited/local evidence needs a source locator",
                    )
            if fetch_state == "fetched_this_session":
                if entry.get("source") not in external_fetches:
                    self.report.error(
                        "CATALOG_FETCH_UNDECLARED",
                        evidence_id,
                        "fetched_this_session requires the source in provenance.external_fetches_this_session",
                    )
            if kind not in EVIDENCE_KINDS:
                self.report.error(
                    "CATALOG_KIND", evidence_id, f"unknown evidence kind {kind!r}"
                )
                continue
            if not kind_policy.get(kind, {}).get("used"):
                self.report.error(
                    "CATALOG_UNUSED_KIND",
                    evidence_id,
                    f"kind {kind!r} is declared unavailable but has an entry",
                )
            if entry.get("confidence") not in CONFIDENCE_RANK:
                self.report.error(
                    "CATALOG_CONFIDENCE", evidence_id, "invalid confidence value"
                )
            if not isinstance(entry.get("source"), str) or not entry["source"].strip():
                self.report.error("CATALOG_SOURCE", evidence_id, "source is required")
            for key in ("limitations", "unknowns"):
                items = entry.get(key)
                if not isinstance(items, list) or not items or not all(
                    isinstance(item, str) and item.strip() for item in items
                ):
                    self.report.error(
                        "CATALOG_BOUNDARY",
                        f"{evidence_id}:{key}",
                        f"{key} must be a non-empty list of strings",
                    )
        for kind, policy in kind_policy.items():
            if kind not in EVIDENCE_KINDS:
                self.report.error(
                    "CATALOG_POLICY_KIND", kind, "kind policy contains an unknown kind"
                )
                continue
            if policy.get("used") and used_kinds[kind] == 0:
                self.report.error(
                    "CATALOG_KIND_EMPTY",
                    kind,
                    "kind is declared used but has no catalog entries",
                )
            if not policy.get("used") and used_kinds[kind] > 0:
                self.report.error(
                    "CATALOG_KIND_CONFLICT",
                    kind,
                    "kind is declared unused but has catalog entries",
                )
            if not isinstance(policy.get("reason"), str) or len(policy["reason"]) < 20:
                self.report.error(
                    "CATALOG_POLICY_REASON", kind, "kind policy needs a reason"
                )

    # ---------------------------------------------------------------- matrix
    def check_matrix(self) -> None:
        if not isinstance(self.matrix, dict):
            self.report.error(
                "MATRIX_SHAPE", "coverage-matrix.json", "matrix must be an object"
            )
            return
        behaviors = self.matrix.get("behaviors", []) if isinstance(self.matrix, dict) else []
        ids = [
            behavior.get("behavior_id")
            for behavior in behaviors
            if isinstance(behavior, dict)
        ]
        if len(ids) != len(set(ids)):
            self.report.error("MATRIX_DUPLICATE", "coverage-matrix.json", "behavior IDs must be unique")
        if len(behaviors) != 49:
            self.report.error(
                "MATRIX_COUNT",
                "coverage-matrix.json",
                f"expected 49 behavior entries, found {len(behaviors)}",
            )
        for behavior in behaviors:
            if not isinstance(behavior, dict):
                continue
            behavior_id = behavior.get("behavior_id", "?")
            if behavior.get("workflow") not in A_WORKFLOWS:
                self.report.error(
                    "MATRIX_WORKFLOW",
                    behavior_id,
                    "behavior must belong to an A workflow area",
                )
            if behavior.get("priority") not in {"A", "B"}:
                self.report.error(
                    "MATRIX_PRIORITY", behavior_id, "behavior priority must be A or B"
                )
            required = behavior.get("required_per_os")
            if behavior.get("priority") == "A" and required is not True:
                self.report.error(
                    "MATRIX_REQUIRED", behavior_id, "an A behavior must be required per OS"
                )
            for key in ("description", "blueprint_ref"):
                if not isinstance(behavior.get(key), str) or len(behavior[key]) < 10:
                    self.report.error(
                        "MATRIX_TEXT", f"{behavior_id}:{key}", f"{key} is required"
                    )

    # ------------------------------------------------------------ ledger core
    def check_ledger_core(self) -> None:
        if not isinstance(self.ledger, dict):
            self.report.error(
                "LEDGER_SHAPE", "scenario-ledger.json", "ledger must be an object"
            )
            return
        rows = self.ledger_rows
        if not valid_calendar_date(self.ledger.get("created")):
            self.report.error(
                "LEDGER_DATE",
                "created",
                f"ledger created date {self.ledger.get('created')!r} is not a valid calendar date",
            )
        ids = [row.get("scenario_id") for row in rows]
        duplicates = [scenario_id for scenario_id, count in Counter(ids).items() if count > 1]
        for scenario_id in duplicates:
            self.report.error(
                "LEDGER_DUPLICATE_ID", str(scenario_id), "scenario IDs must be unique"
            )
        expected_ids = {
            f"S{index:02d}-{suffix}" for index in range(1, 51) for suffix in ("WIN", "MAC")
        }
        actual_ids = set(ids)
        for missing in sorted(expected_ids - actual_ids):
            self.report.error("LEDGER_MISSING_ID", missing, "required scenario row is missing")
        for extra in sorted(actual_ids - expected_ids):
            self.report.error("LEDGER_EXTRA_ID", str(extra), "unexpected scenario ID")

        for platform, expected_count in (("windows", 50), ("macos", 50)):
            actual = len(self.rows_by_platform[platform])
            if actual != expected_count:
                self.report.error(
                    "PLATFORM_COUNT",
                    platform,
                    f"expected {expected_count} rows, found {actual}",
                )

        declared_workflows = set(self.ledger.get("workflows", []))
        used_workflows = {row.get("workflow") for row in rows}
        if declared_workflows != used_workflows:
            self.report.error(
                "WORKFLOW_INDEX",
                "workflows",
                f"top-level workflows differ from row workflows: declared {sorted(declared_workflows)}, used {sorted(used_workflows)}",
            )

        manifest_by_id = self.manifests_by_id
        known_hashes = set()
        for manifest in self.manifests:
            app_hash = (
                manifest.get("identity_fields", {})
                .get("application", {})
                .get("artifact_sha256", {})
            )
            if field_known(app_hash):
                known_hashes.add(str(app_hash["value"]).lower())
        self._known_hashes = known_hashes

        status_counts = Counter()
        observed_rows = 0
        for row in rows:
            scenario_id = row.get("scenario_id", "?")
            platform = row.get("target", {}).get("platform")
            expected_suffix = {"windows": "WIN", "macos": "MAC"}.get(platform)
            if expected_suffix and not str(scenario_id).endswith(f"-{expected_suffix}"):
                self.report.error(
                    "ROW_SUFFIX",
                    scenario_id,
                    f"scenario suffix does not match target platform {platform}",
                )
            identity_id = row.get("target", {}).get("reference_identity_id")
            manifest = manifest_by_id.get(identity_id)
            if manifest is None:
                self.report.error(
                    "ROW_IDENTITY_MISSING",
                    scenario_id,
                    f"target identity {identity_id!r} has no manifest",
                )
            elif manifest.get("platform") != platform:
                self.report.error(
                    "ROW_IDENTITY_PLATFORM",
                    scenario_id,
                    "target identity manifest belongs to another platform",
                )

            self._check_row_evidence(row, scenario_id)
            self._check_row_test(row, scenario_id)
            self._check_row_observation(row, scenario_id, manifest)

            status = row.get("parity_status")
            status_counts[status] += 1
            bucket = row.get("denominator", {}).get("bucket")
            expected_bucket = STATUS_BUCKET.get(status)
            if expected_bucket is None:
                self.report.error(
                    "ROW_STATUS_FROZEN",
                    scenario_id,
                    f"parity status {status!r} is not allowed in the frozen unknown-only planning baseline",
                )
            elif bucket != expected_bucket:
                self.report.error(
                    "ROW_BUCKET",
                    scenario_id,
                    f"status {status!r} requires denominator bucket {expected_bucket!r}, found {bucket!r}",
                )
            if row.get("denominator", {}).get("included") is not True:
                self.report.error(
                    "ROW_EXCLUDED",
                    scenario_id,
                    "every scenario must stay in the parity denominator",
                )
            if row.get("observation", {}).get("state") == "observed":
                observed_rows += 1

        denominator = self.ledger.get("denominator", {})
        if denominator.get("total") != len(rows):
            self.report.error(
                "DENOMINATOR_TOTAL",
                "denominator",
                f"declared total {denominator.get('total')!r} does not equal {len(rows)} rows",
            )
        included = sum(
            1 for row in rows if row.get("denominator", {}).get("included") is True
        )
        if included != len(rows):
            self.report.error(
                "DENOMINATOR_INCLUDED",
                "denominator",
                f"only {included}/{len(rows)} rows are included",
            )
        buckets = Counter(row.get("denominator", {}).get("bucket") for row in rows)
        if buckets.get("deferred", 0) == 0:
            self.report.error(
                "DENOMINATOR_DEFERRED", "denominator", "deferred rows were dropped"
            )
        if buckets.get("unsupported", 0) == 0:
            self.report.error(
                "DENOMINATOR_UNSUPPORTED", "denominator", "unsupported rows were dropped"
            )
        if sum(buckets.values()) != len(rows):
            self.report.error(
                "DENOMINATOR_BUCKET_SUM",
                "denominator",
                "bucket counts do not sum to the retained denominator",
            )

        if observed_rows:
            self.report.error(
                "OBSERVATION_WITHOUT_MANIFEST_COUNT",
                "ledger",
                "rows claim observations but the identity manifests record zero runtime observations",
            )
        self._status_counts = status_counts
        self._observed_rows = observed_rows

    def _check_row_evidence(self, row: dict[str, Any], scenario_id: str) -> None:
        refs = row.get("evidence_refs", [])
        if not isinstance(refs, list):
            return
        kinds: list[str] = []
        min_rank = 2
        for evidence_id in refs:
            entry = self.evidence_by_id.get(evidence_id)
            if entry is None:
                self.report.error(
                    "ROW_EVIDENCE_MISSING",
                    scenario_id,
                    f"evidence reference {evidence_id!r} is not in the catalog",
                )
                continue
            kind = entry.get("kind")
            kinds.append(kind)
            min_rank = min(min_rank, CONFIDENCE_RANK.get(entry.get("confidence"), 0))
        expected_confidence = (
            "low" if min_rank == 0 else "medium" if min_rank == 1 else "high"
        )
        if row.get("confidence") != expected_confidence:
            self.report.error(
                "ROW_CONFIDENCE",
                scenario_id,
                f"row confidence {row.get('confidence')!r} is stronger than its weakest evidence ({expected_confidence!r})",
            )
        observation = row.get("observation", {})
        manifest = self.manifests_by_id.get(
            row.get("target", {}).get("reference_identity_id"), {}
        )
        if "runtime_observation" in kinds:
            if observation.get("state") != "observed" or not identity_complete(manifest):
                self.report.error(
                    "EVIDENCE_RUNTIME_UNBOUND",
                    scenario_id,
                    "runtime evidence requires an observed row and a complete identity manifest",
                )
        if "static_artifact_analysis" in kinds:
            app_hash = (
                manifest.get("identity_fields", {})
                .get("application", {})
                .get("artifact_sha256", {})
            )
            if not field_known(app_hash):
                self.report.error(
                    "EVIDENCE_STATIC_UNBOUND",
                    scenario_id,
                    "static artifact evidence requires a known artifact hash",
                )
        if "source_inspection" in kinds:
            version = (
                manifest.get("identity_fields", {})
                .get("application", {})
                .get("version", {})
            )
            build = (
                manifest.get("identity_fields", {})
                .get("application", {})
                .get("build", {})
            )
            if not (field_known(version) and field_known(build)):
                self.report.error(
                    "EVIDENCE_SOURCE_UNBOUND",
                    scenario_id,
                    "source-inspection evidence requires a frozen version and build",
                )
        if observation.get("state") != "observed" and row.get("parity_status") in {
            "captured",
            "tested",
        }:
            self.report.error(
                "STATUS_WITHOUT_OBSERVATION",
                scenario_id,
                f"status {row.get('parity_status')!r} requires an actual observation",
            )

    def _check_row_test(self, row: dict[str, Any], scenario_id: str) -> None:
        test = row.get("implementation_test", {})
        mode = test.get("mode")
        state = test.get("state")
        if mode == "executable":
            if not isinstance(test.get("command"), str) or len(test["command"]) < 20:
                self.report.error(
                    "TEST_COMMAND", scenario_id, "executable tests need a concrete command"
                )
            if state == "planned":
                self.report.error(
                    "TEST_EXECUTABLE_PLANNED",
                    scenario_id,
                    "a planned executable test has no asserted runner; use a manual procedure or an existing test",
                )
        elif mode == "manual":
            if test.get("command") is not None:
                self.report.error(
                    "TEST_MANUAL_COMMAND", scenario_id, "manual tests must use a null command"
                )
        else:
            self.report.error("TEST_MODE", scenario_id, f"unknown test mode {mode!r}")
        for key in ("setup", "action", "pass_criteria", "fail_criteria"):
            if not isinstance(test.get(key), str) or len(test[key]) < 20:
                self.report.error(
                    "TEST_FIELD", f"{scenario_id}:{key}", f"{key} is required"
                )
        if state in {"passed", "failed"}:
            if row.get("observation", {}).get("state") != "observed":
                self.report.error(
                    "TEST_STATE_UNBOUND",
                    scenario_id,
                    "a passed/failed test requires an actual observation",
                )
        if state == "passed" and row.get("parity_status") not in {"tested", "implemented"}:
            self.report.error(
                "TEST_PASSED_STATUS",
                scenario_id,
                "a passed test must be reflected in parity_status",
            )

    def _check_row_observation(
        self,
        row: dict[str, Any],
        scenario_id: str,
        manifest: dict[str, Any] | None,
    ) -> None:
        observation = row.get("observation", {})
        state = observation.get("state")
        null_fields = ("actual", "evidence_id", "recording", "observed_at")
        if state == "not_observed":
            for key in null_fields:
                if observation.get(key) is not None:
                    self.report.error(
                        "OBSERVATION_INVENTED",
                        f"{scenario_id}:{key}",
                        "a not_observed row must not carry an observation value",
                    )
        elif state == "observed":
            for key in null_fields:
                if not observation.get(key):
                    self.report.error(
                        "OBSERVATION_INCOMPLETE",
                        f"{scenario_id}:{key}",
                        "an observed row needs actual text, evidence ID, recording and date",
                    )
            if not valid_calendar_date(observation.get("observed_at")):
                self.report.error(
                    "OBSERVATION_DATE",
                    f"{scenario_id}:observed_at",
                    "observed_at must be a real calendar date",
                )
            runtime_refs = [
                evidence_id
                for evidence_id in row.get("evidence_refs", [])
                if self.evidence_by_id.get(evidence_id, {}).get("kind")
                == "runtime_observation"
            ]
            if not runtime_refs:
                self.report.error(
                    "OBSERVATION_RUNTIME_EVIDENCE",
                    scenario_id,
                    "an observed row must cite at least one catalog runtime_observation entry",
                )
            if manifest is None or not identity_complete(manifest):
                self.report.error(
                    "OBSERVATION_UNBOUND",
                    scenario_id,
                    "an observation requires every manifest identity field to be known",
                )
            if manifest and manifest.get("evidence_boundary", {}).get("runtime_observations") == 0:
                self.report.error(
                    "OBSERVATION_COUNT_MISMATCH",
                    scenario_id,
                    "manifest says zero runtime observations while this row is observed",
                )
            evidence_id = observation.get("evidence_id")
            if isinstance(evidence_id, str) and evidence_id.startswith("REA-"):
                allowed = set(
                    (manifest or {}).get("evidence_boundary", {}).get(
                        "rea_evidence_ids", []
                    )
                )
                if evidence_id not in allowed:
                    self.report.error(
                        "REA_ID_INVENTED",
                        scenario_id,
                        f"REA evidence ID {evidence_id!r} was not returned by a tool",
                    )

    # --------------------------------------------------------------- behavior
    def check_behaviors(self) -> None:
        matrix = self.behaviors_by_id
        covered: dict[str, dict[str, list[str]]] = {
            behavior_id: {"windows": [], "macos": []} for behavior_id in matrix
        }
        for row in self.ledger_rows:
            scenario_id = row.get("scenario_id", "?")
            platform = row.get("target", {}).get("platform")
            behavior_ids = row.get("behaviors", [])
            if row.get("priority") == "A" and not behavior_ids:
                self.report.error(
                    "A_ROW_NO_BEHAVIOR",
                    scenario_id,
                    "a priority-A row must name at least one behavior",
                )
            if not isinstance(behavior_ids, list):
                continue
            for behavior_id in behavior_ids:
                behavior = matrix.get(behavior_id)
                if behavior is None:
                    self.report.error(
                        "BEHAVIOR_UNKNOWN",
                        scenario_id,
                        f"behavior {behavior_id!r} is not in coverage-matrix.json",
                    )
                    continue
                if behavior.get("workflow") != row.get("workflow"):
                    self.report.error(
                        "BEHAVIOR_WORKFLOW",
                        scenario_id,
                        f"behavior {behavior_id!r} belongs to {behavior.get('workflow')!r}",
                    )
                if row.get("priority") != behavior.get("priority"):
                    self.report.error(
                        "BEHAVIOR_PRIORITY",
                        scenario_id,
                        f"behavior {behavior_id!r} is priority {behavior.get('priority')!r}, row is {row.get('priority')!r}",
                    )
                if platform in covered[behavior_id]:
                    covered[behavior_id][platform].append(scenario_id)

        for behavior_id, behavior in matrix.items():
            for platform in ("windows", "macos"):
                if behavior.get("required_per_os") and behavior.get("priority") == "A":
                    if not covered[behavior_id][platform]:
                        self.report.error(
                            "A_BEHAVIOR_UNCOVERED",
                            f"{behavior_id}:{platform}",
                            "no priority-A scenario row covers this behavior",
                        )
            if behavior.get("priority") == "B":
                for platform in ("windows", "macos"):
                    if not covered[behavior_id][platform]:
                        self.report.error(
                            "B_BEHAVIOR_UNCOVERED",
                            f"{behavior_id}:{platform}",
                            "planned B behavior must still have a retained row per target OS",
                        )

        for platform in ("windows", "macos"):
            for workflow in A_WORKFLOWS:
                count = sum(
                    1
                    for row in self.rows_by_platform[platform]
                    if row.get("workflow") == workflow and row.get("priority") == "A"
                )
                if count == 0:
                    self.report.error(
                        "A_WORKFLOW_UNCOVERED",
                        f"{workflow}:{platform}",
                        "no priority-A row covers this workflow",
                    )

        required = [
            behavior_id
            for behavior_id, behavior in matrix.items()
            if behavior.get("required_per_os") and behavior.get("priority") == "A"
        ]
        self._behavior_covered = covered
        self._required_behaviors = required

    # ----------------------------------------------------------------- seeds
    def check_seeds(self) -> None:
        expected_seed_ids = {f"S{index:02d}" for index in range(1, 21)}
        actual_seed_ids = set(self.seed_by_id)
        for missing in sorted(expected_seed_ids - actual_seed_ids):
            self.report.error("SEED_MISSING", missing, "seed intent is missing")
        for extra in sorted(actual_seed_ids - expected_seed_ids):
            self.report.error("SEED_EXTRA", str(extra), "unexpected seed intent")

        seed_rows = [
            row for row in self.ledger_rows if isinstance(row.get("seed_id"), str)
        ]
        if len(seed_rows) != 40:
            self.report.error(
                "SEED_ROW_COUNT",
                "ledger",
                f"expected 40 seed-derived rows (20 seeds x 2 OSes), found {len(seed_rows)}",
            )

        for seed_id, seed in self.seed_by_id.items():
            for platform, key in (("windows", "windows_scenario_id"), ("macos", "macos_scenario_id")):
                scenario_id = seed.get(key)
                row = self.rows_by_id.get(scenario_id)
                if row is None:
                    self.report.error(
                        "SEED_ROW_MISSING",
                        f"{seed_id}:{platform}",
                        f"{key} {scenario_id!r} does not exist",
                    )
                    continue
                if row.get("seed_id") != seed_id:
                    self.report.error(
                        "SEED_BACKLINK",
                        scenario_id,
                        f"row seed_id {row.get('seed_id')!r} does not match {seed_id}",
                    )
                if row.get("target", {}).get("platform") != platform:
                    self.report.error(
                        "SEED_PLATFORM", scenario_id, "seed row targets the wrong platform"
                    )
                for key_name in ("workflow", "priority"):
                    if row.get(key_name) != seed.get(key_name):
                        self.report.error(
                            "SEED_MAPPING",
                            scenario_id,
                            f"row {key_name} does not match seed intent",
                        )
                if set(row.get("behaviors", [])) != set(seed.get("behavior_ids", [])):
                    self.report.error(
                        "SEED_BEHAVIORS",
                        scenario_id,
                        "row behaviors do not match seed intent",
                    )

        for suffix in ("WIN", "MAC"):
            s20 = self.rows_by_id.get(f"S20-{suffix}")
            s24 = self.rows_by_id.get(f"S24-{suffix}")
            if not s20 or s20.get("parity_status") != "unsupported":
                self.report.error(
                    "SEED_S20_DENOMINATOR",
                    f"S20-{suffix}",
                    "S20 must remain in the denominator as unsupported",
                )
            if not s24 or "launcher.degraded_capability" not in s24.get("behaviors", []):
                self.report.error(
                    "SEED_S20_INTENT",
                    f"S24-{suffix}",
                    "degraded-capability intent needs a concrete A row",
                )

    # ----------------------------------------------------------------- audit
    def check_audit(self) -> None:
        audit = self.audit
        if not isinstance(audit, dict) or not audit:
            self.report.error(
                "AUDIT_SHAPE",
                "audit",
                "audit must be a non-empty object with ten audited rows",
            )
            return
        required_top = {
            "audit_id",
            "audit_kind",
            "created",
            "runtime_claim",
            "fresh_web_fetch",
            "sources_fetched_this_session",
            "method",
            "row_count",
            "rows",
        }
        for key in sorted(required_top - set(audit)):
            self.report.error(
                "AUDIT_MISSING_KEY", f"audit.{key}", "required audit key is missing"
            )
        if not valid_calendar_date(audit.get("created")):
            self.report.error(
                "AUDIT_DATE",
                "audit.created",
                f"created {audit.get('created')!r} is not a valid calendar date",
            )
        if not isinstance(audit.get("audit_id"), str) or len(audit.get("audit_id", "")) < 8:
            self.report.error("AUDIT_ID", "audit.audit_id", "audit_id is required")
        if not isinstance(audit.get("method"), str) or len(audit.get("method", "")) < 20:
            self.report.error("AUDIT_METHOD", "audit.method", "method is required")
        if audit.get("audit_kind") != "design_source_baseline_consistency":
            self.report.error(
                "AUDIT_KIND",
                "audit",
                "audit must be labelled design/source-baseline consistency, not a fresh runtime or web replay",
            )
        if audit.get("fresh_web_fetch") is not False:
            self.report.error(
                "AUDIT_FRESH_FETCH",
                "audit",
                "the audit must explicitly state that no fresh web fetch was performed",
            )
        if audit.get("sources_fetched_this_session") != []:
            self.report.error(
                "AUDIT_FETCHED_SOURCES",
                "audit",
                "this baseline fetched no sources in-session",
            )
        if audit.get("runtime_claim") is not False:
            self.report.error(
                "AUDIT_RUNTIME_CLAIM", "audit", "the audit must explicitly disclaim any runtime claim"
            )
        rows = audit.get("rows", [])
        if not isinstance(rows, list):
            self.report.error(
                "AUDIT_ROW_COUNT",
                "audit",
                f"expected exactly 10 audited rows, found {type(rows).__name__}",
            )
            rows = []
        elif audit.get("row_count") != 10 or len(rows) != 10:
            self.report.error(
                "AUDIT_ROW_COUNT",
                "audit",
                f"expected exactly 10 audited rows, found {len(rows)}",
            )
        ids = [row.get("scenario_id") for row in rows if isinstance(row, dict)]
        if len(ids) != len(set(ids)):
            self.report.error("AUDIT_DUPLICATE", "audit", "audited scenario IDs must be unique")
        platforms = set()
        workflows = set()
        for item in rows:
            if not isinstance(item, dict):
                self.report.error("AUDIT_ROW", "audit", "audit rows must be objects")
                continue
            required_row = {
                "scenario_id",
                "workflow",
                "claim",
                "evidence_replayed",
                "identity_manifest",
                "identity_state",
                "observation_state",
                "runtime_claim",
                "replay_result",
                "baseline_consistency_notes",
                "gap",
                "next_capture_step",
            }
            for key in sorted(required_row - set(item)):
                self.report.error(
                    "AUDIT_ROW_KEY",
                    f"{item.get('scenario_id', '?')}:{key}",
                    "required audit row key is missing",
                )
            scenario_id = item.get("scenario_id")
            row = self.rows_by_id.get(scenario_id)
            if row is None:
                self.report.error(
                    "AUDIT_UNKNOWN_ROW", str(scenario_id), "audited scenario is not in the ledger"
                )
                continue
            platforms.add(row.get("target", {}).get("platform"))
            workflows.add(row.get("workflow"))
            if item.get("workflow") != row.get("workflow"):
                self.report.error(
                    "AUDIT_WORKFLOW_MISMATCH",
                    scenario_id,
                    "audit workflow does not match the ledger row",
                )
            if item.get("identity_manifest") != row.get("target", {}).get(
                "reference_identity_id"
            ):
                self.report.error(
                    "AUDIT_IDENTITY_MISMATCH",
                    scenario_id,
                    "audit manifest does not match the ledger target identity",
                )
            if not isinstance(item.get("claim"), str) or len(item["claim"]) < 20:
                self.report.error(
                    "AUDIT_CLAIM", scenario_id, "audit must state the claim under review"
                )
            if item.get("runtime_claim") is not False:
                self.report.error(
                    "AUDIT_ROW_RUNTIME",
                    scenario_id,
                    "audit row must disclaim a runtime claim",
                )
            if item.get("observation_state") != "not_observed":
                self.report.error(
                    "AUDIT_OBSERVATION",
                    scenario_id,
                    "an audit cannot claim an observation",
                )
            if item.get("identity_state") != "unknown":
                self.report.error(
                    "AUDIT_IDENTITY",
                    scenario_id,
                    "audit must use the explicitly unknown identity",
                )
            evidence = item.get("evidence_replayed", [])
            if not isinstance(evidence, list) or not evidence:
                self.report.error(
                    "AUDIT_EVIDENCE", scenario_id, "audit must replay evidence references"
                )
                continue
            missing = set(evidence) - set(row.get("evidence_refs", []))
            if missing:
                self.report.error(
                    "AUDIT_EVIDENCE_MISMATCH",
                    scenario_id,
                    f"audit cites evidence not used by the row: {sorted(missing)}",
                )
            for evidence_id in evidence:
                if evidence_id not in self.evidence_by_id:
                    self.report.error(
                        "AUDIT_EVIDENCE_MISSING",
                        scenario_id,
                        f"audit evidence {evidence_id!r} is not in the catalog",
                    )
            if item.get("replay_result") not in AUDIT_RESULTS:
                self.report.error(
                    "AUDIT_RESULT", scenario_id, "invalid replay result"
                )
            for key in ("gap", "next_capture_step"):
                if not isinstance(item.get(key), str) or len(item[key]) < 20:
                    self.report.error(
                        "AUDIT_FIELD", f"{scenario_id}:{key}", f"{key} is required"
                    )
        if len(platforms - {None}) < 2:
            self.report.error(
                "AUDIT_PLATFORMS", "audit", "audit must cover Windows and macOS rows"
            )
        if len(workflows - {None}) < 5:
            self.report.error(
                "AUDIT_WORKFLOWS", "audit", "audit must span at least five workflows"
            )

    # ------------------------------------------------------- invented values
    def check_invented_values(self) -> None:
        allowed_rea = set()
        for manifest in self.manifests:
            allowed_rea.update(
                manifest.get("evidence_boundary", {}).get("rea_evidence_ids", [])
            )
        sources = [("ledger", self.ledger), ("audit", self.audit)]
        sources.extend((f"identity:{manifest.get('manifest_id', '?')}", manifest) for manifest in self.manifests)
        for source_name, value in sources:
            for text in strings_in(value):
                for match in REA_ID.findall(text):
                    if match not in allowed_rea:
                        self.report.error(
                            "INVENTED_REA_ID",
                            source_name,
                            f"REA evidence ID {match!r} is not in any manifest",
                        )
                for match in HEX64.findall(text):
                    if match.lower() not in self._known_hashes:
                        self.report.error(
                            "INVENTED_HASH",
                            source_name,
                            f"64-hex value {match!r} is not a verified artifact hash",
                        )

    # ------------------------------------------------------------------ stats
    def compute_stats(self) -> None:
        statuses = Counter(row.get("parity_status") for row in self.ledger_rows)
        buckets = Counter(
            row.get("denominator", {}).get("bucket") for row in self.ledger_rows
        )
        workflows = Counter(row.get("workflow") for row in self.ledger_rows)
        tests = Counter(row.get("implementation_test", {}).get("mode") for row in self.ledger_rows)
        test_states = Counter(
            row.get("implementation_test", {}).get("state") for row in self.ledger_rows
        )
        priority_a = {
            platform: sum(
                1 for row in self.rows_by_platform[platform] if row.get("priority") == "A"
            )
            for platform in ("windows", "macos")
        }
        behavior_coverage = {
            platform: sum(
                1
                for behavior_id in self._required_behaviors
                if self._behavior_covered.get(behavior_id, {}).get(platform)
            )
            for platform in ("windows", "macos")
        }
        evidence_kinds = Counter(
            entry.get("kind") for entry in self.evidence_by_id.values()
        )
        self.report.stats = {
            "rows": len(self.ledger_rows),
            "platforms": dict(
                Counter(row.get("target", {}).get("platform") for row in self.ledger_rows)
            ),
            "statuses": dict(sorted(statuses.items())),
            "buckets": dict(sorted(buckets.items())),
            "workflows": dict(sorted(workflows.items())),
            "priority_a_rows": priority_a,
            "required_a_behaviors": len(self._required_behaviors),
            "behavior_coverage": behavior_coverage,
            "behaviors": len(self.behaviors_by_id),
            "seeds": len(self.seed_by_id),
            "seed_rows": sum(1 for row in self.ledger_rows if row.get("seed_id")),
            "evidence_kinds": dict(sorted(evidence_kinds.items())),
            "observed_rows": self._observed_rows,
            "test_modes": dict(sorted(tests.items())),
            "test_states": dict(sorted(test_states.items())),
            "audit_rows": len(self.audit.get("rows", [])) if isinstance(self.audit, dict) else 0,
            "denominator_included": sum(
                1
                for row in self.ledger_rows
                if row.get("denominator", {}).get("included") is True
            ),
        }

    # ------------------------------------------------------------------- run
    def run(self) -> Report:
        self.load()
        self.check_schemas()
        self.check_identities()
        self.check_catalog()
        self.check_matrix()
        self.check_ledger_core()
        self.check_behaviors()
        self.check_seeds()
        self.check_audit()
        self.check_invented_values()
        self.compute_stats()
        return self.report


def validate_root(root: Path) -> Report:
    return ReferenceValidation(root).run()


def print_text_report(report: Report) -> None:
    stats = report.stats
    print("Ansa F01 reference validation")
    if stats:
        print(
            f"  ledger: {stats['rows']} rows "
            f"(windows {stats['platforms'].get('windows', 0)}, "
            f"macos {stats['platforms'].get('macos', 0)})"
        )
        print(
            f"  priority A rows: windows {stats['priority_a_rows'].get('windows', 0)}, "
            f"macos {stats['priority_a_rows'].get('macos', 0)}"
        )
        print(
            f"  required A behaviors: {stats['required_a_behaviors']} "
            f"(covered windows {stats['behavior_coverage'].get('windows', 0)}, "
            f"macos {stats['behavior_coverage'].get('macos', 0)})"
        )
        print(
            f"  seeds: {stats['seeds']}/20, seed rows: {stats['seed_rows']}/40, "
            f"audit: {stats['audit_rows']}/10 design/source-baseline rows"
        )
        print(
            f"  denominator: included {stats['denominator_included']}, "
            f"buckets {stats['buckets']}"
        )
        print(f"  statuses: {stats['statuses']}")
        print(
            f"  observations: {stats['observed_rows']} observed, "
            f"{stats['rows'] - stats['observed_rows']} not observed"
        )
        print(
            f"  implementation tests: {stats['test_modes']} / {stats['test_states']}"
        )
        print(f"  evidence kinds: {stats['evidence_kinds']}")
    for warning in report.warnings:
        print(f"WARN {warning.code} [{warning.path}] {warning.message}")
    for issue in report.errors:
        print(f"ERROR {issue.code} [{issue.path}] {issue.message}")
    if report.ok:
        print("PASS: schema, identity boundaries, evidence, A-coverage, seeds and audit are consistent")
    else:
        print(f"FAIL: {len(report.errors)} validation error(s)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path("."),
        help="repository root containing docs/reference (default: current directory)",
    )
    parser.add_argument(
        "--format",
        choices=["text", "json"],
        default="text",
        help="output format for CI (default: text)",
    )
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="suppress the text summary and print only failures",
    )
    args = parser.parse_args(argv)

    report = validate_root(args.root)
    if args.format == "json":
        payload = {
            "ok": report.ok,
            "stats": report.stats,
            "errors": [issue.__dict__ for issue in report.errors],
            "warnings": [issue.__dict__ for issue in report.warnings],
        }
        print(json.dumps(payload, indent=2, ensure_ascii=False))
    elif not args.quiet or not report.ok:
        print_text_report(report)
    return 0 if report.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
