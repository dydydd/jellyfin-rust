#!/usr/bin/env python3
"""Statically validate Jellyfin JSON against jellyfin-sdk-swift Codable models.

This mirrors the generated decoder calls.  It catches fields that Swift's
`decodeIfPresent` still rejects when present: scalar/container mismatches,
unknown String-backed enums, nested DTO shape errors, and non-ISO dates.
It is deliberately useful without a local Swift compiler; run the generated
SDK's real Codable decoding as an additional check whenever Swift is present.
"""
import datetime
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from swift_schema import load

PRIMITIVES = {
    "Bool": "bool", "String": "str", "Int": "int", "Int8": "int",
    "Int16": "int", "Int32": "int", "Int64": "int", "UInt": "int",
    "UInt8": "int", "UInt16": "int", "UInt32": "int", "UInt64": "int",
    "Float": "num", "Double": "num", "Decimal": "num", "Date": "date",
    "Data": "str", "URL": "str", "Any": None, "JSONValue": None,
}


class Report:
    def __init__(self):
        self.errors = []

    def add(self, path, message):
        self.errors.append(f"{path}: {message}")


def strip_optional(typ):
    return typ.strip().replace("@Indirect ", "").rstrip("?").strip()


def split_generic(content):
    parts, depth, current = [], 0, ""
    for char in content:
        if char in "<[":
            depth += 1
        elif char in ">]":
            depth -= 1
        if char == "," and depth == 0:
            parts.append(current.strip())
            current = ""
        else:
            current += char
    parts.append(current.strip())
    return parts


def json_type(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    return "array" if isinstance(value, list) else "object"


def check_date(value):
    if not isinstance(value, str):
        return False
    # Jellyfin's DateTime converter accepts RFC 3339/ISO 8601 instants.
    try:
        datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return False
    return True


def check(structs, enums, aliases, typ, value, path, report, depth=0):
    typ = strip_optional(typ)
    if value is None:
        return
    if depth > 16:
        return
    if typ.startswith("[") and typ.endswith("]"):
        parts = split_generic(typ[1:-1])
        if len(parts) == 1:
            dictionary = re.match(r"^String\s*:\s*(.+)$", parts[0])
            if dictionary:
                if not isinstance(value, dict):
                    report.add(path, f"expected object for {typ}, got {json_type(value)}")
                    return
                for key, item in value.items():
                    check(structs, enums, aliases, dictionary.group(1), item,
                          f"{path}.{key}", report, depth + 1)
                return
            if not isinstance(value, list):
                report.add(path, f"expected array for {typ}, got {json_type(value)}")
                return
            for index, item in enumerate(value):
                check(structs, enums, aliases, parts[0], item, f"{path}[{index}]", report, depth + 1)
            return
        if len(parts) == 2 and parts[0] == "String":
            if not isinstance(value, dict):
                report.add(path, f"expected object for {typ}, got {json_type(value)}")
                return
            for key, item in value.items():
                check(structs, enums, aliases, parts[1], item, f"{path}.{key}", report, depth + 1)
            return
    if typ in PRIMITIVES:
        expected = PRIMITIVES[typ]
        if expected is None:
            return
        valid = {
            "bool": isinstance(value, bool),
            "int": isinstance(value, int) and not isinstance(value, bool),
            "num": isinstance(value, (int, float)) and not isinstance(value, bool),
            "str": isinstance(value, str),
            "date": check_date(value),
        }[expected]
        if not valid:
            report.add(path, f"{typ} expects {expected}, got {json_type(value)}")
        return
    if typ in enums:
        if not isinstance(value, str):
            report.add(path, f"enum {typ} needs a JSON string, got {json_type(value)}")
        elif value not in enums[typ]:
            report.add(path, f"{typ} value {value!r} is not supported by the Swift SDK")
        return
    if typ in aliases:
        check(structs, enums, aliases, aliases[typ], value, path, report, depth + 1)
        return
    if typ in structs:
        if not isinstance(value, dict):
            report.add(path, f"expected object for {typ}, got {json_type(value)}")
            return
        for key, field_type in structs[typ]:
            if key in value:
                check(structs, enums, aliases, field_type, value[key], f"{path}.{key}", report, depth + 1)
        return
    # Hand-written discriminated unions are intentionally not guessed here.


def validate(root, document):
    structs, enums, aliases = load()
    report = Report()
    check(structs, enums, aliases, root, document, root, report)
    return report.errors


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: swift_validate.py <ModelName> <response.json|->")
    root, source = sys.argv[1:]
    document = json.load(sys.stdin if source == "-" else open(source, encoding="utf-8"))
    errors = validate(root, document)
    for error in errors:
        print(error)
    print(f"{root}: {len(errors)} error(s)", file=sys.stderr)
    raise SystemExit(bool(errors))
