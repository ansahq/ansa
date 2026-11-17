#!/usr/bin/env python3
"""Small, fail-closed JSON Schema subset validator.

Supported validation keywords: type, enum, const, pattern, minLength, maxLength,
minimum, maximum, minItems, maxItems, uniqueItems, items, required, properties,
additionalProperties (boolean), allOf, anyOf, oneOf, not, if/then/else and local
$ref ("#/$defs/...").

Annotations are allowed and ignored: $schema, $id, title, description, default,
examples, deprecated, readOnly, writeOnly and $comment.

Everything else fails closed. In particular this subset rejects unknown
keywords, `format`, boolean/tuple schemas, schema-valued additionalProperties,
remote refs and a `$ref` with siblings. The preflight runs before instance
validation, so a silently ignored keyword is not possible.
"""

from __future__ import annotations

import json
import re
from typing import Any

ANNOTATION_KEYWORDS = {
    "$schema",
    "$id",
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$comment",
}
VALIDATION_KEYWORDS = {
    "$ref",
    "type",
    "enum",
    "const",
    "pattern",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "minItems",
    "maxItems",
    "uniqueItems",
    "items",
    "required",
    "properties",
    "additionalProperties",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
}
CONTAINER_KEYWORDS = {"$defs", "properties"}
SUPPORTED_TYPES = {
    "object",
    "array",
    "string",
    "number",
    "integer",
    "boolean",
    "null",
}
_TYPES = {
    "object": dict,
    "array": list,
    "string": str,
    "number": (int, float),
    "integer": int,
    "boolean": bool,
    "null": type(None),
}


def _is_type(value: Any, name: str) -> bool:
    if name == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if name == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    expected = _TYPES.get(name)
    return expected is not None and isinstance(value, expected)


def _json_equal(left: Any, right: Any) -> bool:
    if isinstance(left, bool) or isinstance(right, bool):
        return isinstance(left, bool) and isinstance(right, bool) and left == right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return left == right
    if type(left) is not type(right):
        return False
    if isinstance(left, list):
        return len(left) == len(right) and all(
            _json_equal(a, b) for a, b in zip(left, right)
        )
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(
            _json_equal(left[k], right[k]) for k in left
        )
    return left == right


def _resolve_ref(ref: str, root: dict[str, Any]) -> dict[str, Any]:
    if not ref.startswith("#/"):
        raise ValueError(f"only local $ref values are supported: {ref}")
    node: Any = root
    for part in ref[2:].split("/"):
        part = part.replace("~1", "/").replace("~0", "~")
        if not isinstance(node, dict) or part not in node:
            raise ValueError(f"unresolvable $ref: {ref}")
        node = node[part]
    if not isinstance(node, dict):
        raise ValueError(f"$ref does not resolve to an object schema: {ref}")
    return node


def preflight(schema: Any) -> list[str]:
    """Reject unsupported or malformed schema constructs before validation."""
    errors: list[str] = []
    _preflight(schema, schema, "$schema", errors)
    return errors


def _preflight(
    schema: Any,
    root: dict[str, Any],
    path: str,
    errors: list[str],
) -> None:
    if isinstance(schema, bool):
        errors.append(f"{path}: boolean schemas are unsupported; schema must be an object")
        return
    if not isinstance(schema, dict):
        errors.append(f"{path}: schema must be an object, found {type(schema).__name__}")
        return

    keys = set(schema)
    if "$ref" in keys and len(keys) > 1:
        siblings = sorted(keys - {"$ref"})
        errors.append(
            f"{path}: $ref with siblings is unsupported in this subset: {siblings}"
        )

    for key, value in schema.items():
        if key in ANNOTATION_KEYWORDS:
            continue
        if key == "$defs":
            if not isinstance(value, dict):
                errors.append(f"{path}.$defs: must be an object of schemas")
                continue
            for name, subschema in value.items():
                _preflight(subschema, root, f"{path}.$defs.{name}", errors)
            continue
        if key == "properties":
            if not isinstance(value, dict):
                errors.append(f"{path}.properties: must be an object of schemas")
                continue
            for name, subschema in value.items():
                _preflight(subschema, root, f"{path}.properties.{name}", errors)
            continue
        if key == "$ref":
            if not isinstance(value, str):
                errors.append(f"{path}.$ref: must be a string")
                continue
            try:
                _resolve_ref(value, root)
            except ValueError as exc:
                errors.append(f"{path}.$ref: {exc}")
            continue
        if key == "type":
            names = value if isinstance(value, list) else [value]
            if not isinstance(value, (str, list)) or not value:
                errors.append(f"{path}.type: must be a string or non-empty list")
                continue
            if not all(isinstance(name, str) for name in names):
                errors.append(f"{path}.type: entries must be strings")
                continue
            if len(names) != len(set(names)):
                errors.append(f"{path}.type: duplicate type names")
            for name in names:
                if name not in SUPPORTED_TYPES:
                    errors.append(f"{path}.type: unsupported type {name!r}")
            continue
        if key == "enum":
            if not isinstance(value, list) or not value:
                errors.append(f"{path}.enum: must be a non-empty array")
            continue
        if key == "const":
            continue
        if key == "pattern":
            if not isinstance(value, str):
                errors.append(f"{path}.pattern: must be a string")
                continue
            try:
                re.compile(value)
            except re.error as exc:
                errors.append(f"{path}.pattern: invalid regular expression: {exc}")
            continue
        if key in {"minLength", "maxLength", "minItems", "maxItems"}:
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                errors.append(f"{path}.{key}: must be a non-negative integer")
            continue
        if key in {"minimum", "maximum"}:
            if not isinstance(value, (int, float)) or isinstance(value, bool):
                errors.append(f"{path}.{key}: must be a number")
            continue
        if key == "uniqueItems":
            if not isinstance(value, bool):
                errors.append(f"{path}.uniqueItems: must be a boolean")
            continue
        if key == "items":
            _preflight(value, root, f"{path}.items", errors)
            continue
        if key == "required":
            if not isinstance(value, list) or not all(
                isinstance(item, str) for item in value
            ):
                errors.append(f"{path}.required: must be an array of strings")
                continue
            if len(value) != len(set(value)):
                errors.append(f"{path}.required: duplicate property names")
            continue
        if key == "additionalProperties":
            if not isinstance(value, bool):
                errors.append(
                    f"{path}.additionalProperties: only boolean values are supported"
                )
            continue
        if key in {"allOf", "anyOf", "oneOf"}:
            if not isinstance(value, list) or not value:
                errors.append(f"{path}.{key}: must be a non-empty array of schemas")
                continue
            for index, subschema in enumerate(value):
                _preflight(subschema, root, f"{path}.{key}[{index}]", errors)
            continue
        if key in {"if", "then", "else", "not"}:
            _preflight(value, root, f"{path}.{key}", errors)
            continue
        if key == "format":
            errors.append(
                f"{path}.format: unsupported schema keyword; format assertions are not implemented"
            )
            continue
        errors.append(f"{path}: unsupported schema keyword {key!r}")


def _valid(instance: Any, schema: dict[str, Any], root: dict[str, Any]) -> bool:
    scratch: list[str] = []
    _validate(instance, schema, root, "$", scratch)
    return not scratch


def _validate(
    instance: Any,
    schema: dict[str, Any],
    root: dict[str, Any],
    path: str,
    errors: list[str],
) -> None:
    if not isinstance(schema, dict):
        errors.append(f"{path}: schema is not an object")
        return

    if "$ref" in schema:
        try:
            target = _resolve_ref(schema["$ref"], root)
        except ValueError as exc:
            errors.append(f"{path}: {exc}")
            return
        if len(schema) > 1:
            errors.append(f"{path}: $ref with siblings is unsupported")
            return
        _validate(instance, target, root, path, errors)
        return

    if "type" in schema:
        names = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if not any(_is_type(instance, name) for name in names):
            errors.append(f"{path}: expected type {'/'.join(names)}")
            return

    if "const" in schema and not _json_equal(instance, schema["const"]):
        errors.append(f"{path}: expected const {schema['const']!r}")

    if "enum" in schema and not any(
        _json_equal(instance, allowed) for allowed in schema["enum"]
    ):
        errors.append(f"{path}: value {instance!r} is not in the enum")

    if isinstance(instance, str):
        if "minLength" in schema and len(instance) < schema["minLength"]:
            errors.append(
                f"{path}: string shorter than {schema['minLength']} characters"
            )
        if "maxLength" in schema and len(instance) > schema["maxLength"]:
            errors.append(
                f"{path}: string longer than {schema['maxLength']} characters"
            )
        if "pattern" in schema and re.search(schema["pattern"], instance) is None:
            errors.append(f"{path}: value does not match pattern {schema['pattern']}")

    if isinstance(instance, (int, float)) and not isinstance(instance, bool):
        if "minimum" in schema and instance < schema["minimum"]:
            errors.append(f"{path}: value below minimum {schema['minimum']}")
        if "maximum" in schema and instance > schema["maximum"]:
            errors.append(f"{path}: value above maximum {schema['maximum']}")

    if isinstance(instance, list):
        if "minItems" in schema and len(instance) < schema["minItems"]:
            errors.append(f"{path}: fewer than {schema['minItems']} items")
        if "maxItems" in schema and len(instance) > schema["maxItems"]:
            errors.append(f"{path}: more than {schema['maxItems']} items")
        if schema.get("uniqueItems") is True:
            for index, item in enumerate(instance):
                if any(_json_equal(item, earlier) for earlier in instance[:index]):
                    errors.append(f"{path}: duplicate array item")
                    break
        if "items" in schema:
            for index, item in enumerate(instance):
                _validate(item, schema["items"], root, f"{path}[{index}]", errors)

    if isinstance(instance, dict):
        properties = schema.get("properties", {})
        for name in schema.get("required", []):
            if name not in instance:
                errors.append(f"{path}: missing required property {name!r}")
        if schema.get("additionalProperties") is False:
            for name in instance:
                if name not in properties:
                    errors.append(f"{path}: additional property {name!r} is not allowed")
        for name, subschema in properties.items():
            if name in instance:
                _validate(instance[name], subschema, root, f"{path}.{name}", errors)

    for subschema in schema.get("allOf", []):
        _validate(instance, subschema, root, path, errors)

    if "anyOf" in schema:
        if not any(_valid(instance, sub, root) for sub in schema["anyOf"]):
            errors.append(f"{path}: value does not match any anyOf branch")

    if "oneOf" in schema:
        matches = sum(1 for sub in schema["oneOf"] if _valid(instance, sub, root))
        if matches != 1:
            errors.append(f"{path}: value matches {matches} oneOf branches, expected 1")

    if "not" in schema and _valid(instance, schema["not"], root):
        errors.append(f"{path}: value matches a forbidden 'not' schema")

    if "if" in schema:
        branch = schema.get("then") if _valid(instance, schema["if"], root) else schema.get("else")
        if branch is not None:
            _validate(instance, branch, root, path, errors)


def validate(instance: Any, schema: dict[str, Any]) -> list[str]:
    """Return schema or instance violations; an unsupported schema fails closed."""
    schema_errors = preflight(schema)
    if schema_errors:
        return schema_errors
    errors: list[str] = []
    _validate(instance, schema, schema, "$", errors)
    return errors
