#!/usr/bin/env python3
"""Validate the privacy and consistency contract of an acceptance record."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import pathlib
import re
import sys

ROOT_KEYS = {
    "schema_version", "recorded_at", "observer_role", "environment", "window",
    "reading", "ground_truth", "scenarios", "warnings_emitted", "usability",
}
IDENTITY_KEYS = {"plan_type", "model_slug", "service_tier", "schema_fingerprint"}
SCENARIOS = {"normal", "low_activity", "stale", "missing_data", "threshold_crossing", "concurrent_sessions", "weekly_reset"}
UNDERSTANDING = {"clear", "unclear", "not_observed"}
NOTE_CODES = {"no_help_needed", "needed_public_docs", "needed_unpublished_help", "wording_unclear", "threshold_too_early", "threshold_too_late", "duplicate_notice", "privacy_wording_clear", "hook_trust_unclear"}


def fail(message: str) -> None:
    raise ValueError(message)


def exact_keys(value: object, keys: set[str], where: str) -> dict:
    if not isinstance(value, dict):
        fail(f"{where} must be an object")
    missing = keys - value.keys()
    extra = value.keys() - keys
    if missing or extra:
        fail(f"{where} fields mismatch: missing={sorted(missing)}, extra={sorted(extra)}")
    return value


def timestamp(value: object, where: str, nullable: bool = False) -> None:
    if value is None and nullable:
        return
    if not isinstance(value, str):
        fail(f"{where} must be an RFC 3339 timestamp")
    try:
        dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        fail(f"{where} is not an RFC 3339 timestamp: {error}")


def number(value: object, where: str, nullable: bool = False) -> None:
    if value is None and nullable:
        return
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
        fail(f"{where} must be a finite non-negative number")


def validate(record: object) -> None:
    root = exact_keys(record, ROOT_KEYS, "record")
    if root["schema_version"] != 1:
        fail("schema_version must be 1")
    timestamp(root["recorded_at"], "recorded_at")
    if root["observer_role"] not in {"maintainer", "independent_tester"}:
        fail("observer_role is invalid")

    environment = exact_keys(root["environment"], {"os", "architecture", "codex_version", "artifact_version", "artifact_sha256", "compatibility_result", "compatibility_identity"}, "environment")
    for key in ("os", "architecture", "codex_version", "artifact_version"):
        if not isinstance(environment[key], str) or not environment[key]:
            fail(f"environment.{key} must be a non-empty string")
    if not isinstance(environment["artifact_sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", environment["artifact_sha256"]):
        fail("environment.artifact_sha256 must be 64 lowercase hexadecimal characters")
    if environment["compatibility_result"] not in {"compatible", "review", "degraded"}:
        fail("environment.compatibility_result is invalid")
    identity = exact_keys(environment["compatibility_identity"], IDENTITY_KEYS, "environment.compatibility_identity")
    if any(not isinstance(value, str) for value in identity.values()):
        fail("compatibility identity values must be strings")

    window = exact_keys(root["window"], {"started_at", "ends_at", "observed_at"}, "window")
    for key, value in window.items():
        timestamp(value, f"window.{key}", nullable=True)

    reading = exact_keys(root["reading"], {"five_hour_percent", "value_source", "weekly_movement_points", "freshness"}, "reading")
    number(reading["five_hour_percent"], "reading.five_hour_percent", nullable=True)
    number(reading["weekly_movement_points"], "reading.weekly_movement_points", nullable=True)
    if reading["value_source"] not in {"local_calibrated_estimate", "real_server_five_hour", "unknown"}:
        fail("reading.value_source is invalid")
    if reading["freshness"] not in {"fresh", "stale", "unknown"}:
        fail("reading.freshness is invalid")
    if reading["freshness"] == "unknown" and reading["five_hour_percent"] is not None:
        fail("an unknown reading cannot contain a five-hour value")

    truth = exact_keys(root["ground_truth"], {"available", "identity_safe", "five_hour_percent", "absolute_error_points"}, "ground_truth")
    if not isinstance(truth["available"], bool) or not isinstance(truth["identity_safe"], bool):
        fail("ground_truth availability and identity safety must be booleans")
    number(truth["five_hour_percent"], "ground_truth.five_hour_percent", nullable=True)
    number(truth["absolute_error_points"], "ground_truth.absolute_error_points", nullable=True)
    if truth["available"]:
        if not truth["identity_safe"] or truth["five_hour_percent"] is None or reading["five_hour_percent"] is None:
            fail("available ground truth requires identity-safe truth and both numeric values")
        expected = abs(reading["five_hour_percent"] - truth["five_hour_percent"])
        if truth["absolute_error_points"] is None or not math.isclose(truth["absolute_error_points"], expected, abs_tol=0.05):
            fail(f"absolute_error_points must equal {expected:.2f}")
    elif any((truth["identity_safe"], truth["five_hour_percent"] is not None, truth["absolute_error_points"] is not None)):
        fail("unavailable ground truth must use false/null/null")

    if not isinstance(root["scenarios"], list) or len(root["scenarios"]) != len(set(root["scenarios"])) or not set(root["scenarios"]).issubset(SCENARIOS):
        fail("scenarios must be a unique list of allowed values")
    warnings = root["warnings_emitted"]
    if not isinstance(warnings, list) or len(warnings) != len(set(warnings)) or any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in warnings):
        fail("warnings_emitted must be unique non-negative integers")

    usability = exact_keys(root["usability"], {"estimated", "weekly_cost", "stale", "unknown", "over_100", "notice_usefulness", "note_codes"}, "usability")
    for key in ("estimated", "weekly_cost", "stale", "unknown", "over_100"):
        if usability[key] not in UNDERSTANDING:
            fail(f"usability.{key} is invalid")
    if usability["notice_usefulness"] not in {"useful", "annoying", "neutral", "not_observed"}:
        fail("usability.notice_usefulness is invalid")
    notes = usability["note_codes"]
    if not isinstance(notes, list) or len(notes) != len(set(notes)) or not set(notes).issubset(NOTE_CODES):
        fail("usability.note_codes must be a unique list of allowed values")

    # The fixed field allowlist is intentional: it prevents transcript paths,
    # account IDs, prompt text, and arbitrary free-form notes from entering evidence.
    encoded_keys = {key.lower() for key in walk_keys(root)}
    forbidden = {"prompt", "response", "reasoning", "tool_arguments", "command_output", "source_code", "transcript_path", "account_id", "email"}
    if encoded_keys & forbidden:
        fail(f"record contains forbidden private fields: {sorted(encoded_keys & forbidden)}")


def walk_keys(value: object):
    if isinstance(value, dict):
        for key, nested in value.items():
            yield key
            yield from walk_keys(nested)
    elif isinstance(value, list):
        for nested in value:
            yield from walk_keys(nested)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=pathlib.Path)
    arguments = parser.parse_args()
    try:
        record = json.loads(arguments.record.read_text(encoding="utf-8"))
        validate(record)
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as error:
        print(f"acceptance record: FAIL: {error}", file=sys.stderr)
        return 2
    print(f"acceptance record: PASS: {arguments.record}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
