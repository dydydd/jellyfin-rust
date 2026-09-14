#!/usr/bin/env python3
"""Validate `/emby` JSON payloads against Emby's generated Swift Codable models."""

import argparse
import datetime
import json
import re
import sys

from emby_swift_schema import load
from swift_validate import validate as validate_swift


EMBY_DATE_PATTERNS = (
    r"\d{4}-\d{2}-\d{2}",
    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})",
    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{1,9}(?:Z|[+-]\d{2}:\d{2})",
    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{1,9}",
    r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}",
)


def check_emby_date(value):
    """Mirror the formats in generated Swift5 CodableHelper.swift."""
    if not isinstance(value, str):
        return False
    if not any(re.fullmatch(pattern, value) for pattern in EMBY_DATE_PATTERNS):
        return False
    # Foundation's DateFormatter accepts the .NET round-trip timestamps Emby
    # emits with seven fractional digits even though its configured pattern
    # spells `SSS`. Python datetime supports at most microseconds, so trim only
    # for calendar validation after the wire-shape check above.
    normalized = re.sub(r"(\.\d{6})\d+", r"\1", value).replace("Z", "+00:00")
    try:
        datetime.datetime.fromisoformat(normalized)
        return True
    except ValueError:
        pass
    for date_format in ("%Y-%m-%d", "%Y-%m-%d %H:%M:%S"):
        try:
            datetime.datetime.strptime(value, date_format)
            return True
        except ValueError:
            continue
    return False


def validate(root, document):
    return validate_swift(
        root,
        document,
        schema_loader=load,
        date_validator=check_emby_date,
    )


def validate_manifest(document):
    errors = []
    cases = document.get("cases") if isinstance(document, dict) else None
    if not isinstance(cases, list):
        return ["manifest: expected an object containing a cases array"]
    if not cases:
        return ["manifest: cases must be a nonempty array"]
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            errors.append(f"manifest.cases[{index}]: expected an object")
            continue
        name = case.get("name", f"case-{index}")
        model = case.get("model")
        if not isinstance(model, str) or "body" not in case:
            errors.append(f"manifest.cases[{index}] ({name}): model and body are required")
            continue
        errors.extend(f"{name}: {error}" for error in validate(model, case["body"]))
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", nargs="?", help="generated Swift model or list root")
    parser.add_argument("source", nargs="?", help="JSON response file, or - for stdin")
    parser.add_argument("--manifest", help="JSON file containing named model/body cases")
    args = parser.parse_args()

    if args.manifest:
        if args.model or args.source:
            parser.error("model/source cannot be combined with --manifest")
        document = json.load(open(args.manifest, encoding="utf-8"))
        errors = validate_manifest(document)
    else:
        if not args.model or not args.source:
            parser.error("model and source are required without --manifest")
        source = sys.stdin if args.source == "-" else open(args.source, encoding="utf-8")
        document = json.load(source)
        errors = validate(args.model, document)

    for error in errors:
        print(error)
    raise SystemExit(bool(errors))


if __name__ == "__main__":
    main()
