#!/usr/bin/env python3
"""Extract Codable wire schemas from Emby's generated Swift 5 client models."""

import os
import re
from functools import lru_cache
from pathlib import Path

from swift_schema import CASE, ENUM, TYPEALIAS


MODELS = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    os.pardir,
    "Emby.ApiClients",
    "Clients",
    "Swift5",
    "EmbyClient",
    "Classes",
    "Swaggers",
    "Models",
)

STRUCT = re.compile(r"public struct\s+(\w+)\s*:\s*[^\n{]*\bCodable\b")
PROPERTY = re.compile(
    r"^\s*public var\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^=\n]+?)(?:\s*=\s*.*)?$",
    re.M,
)
CODING_KEYS = re.compile(
    r"public enum CodingKeys\s*:\s*String\s*,\s*CodingKey\s*\{(.*?)\n\s*\}",
    re.S,
)


@lru_cache(maxsize=4)
def load(models_directory=None):
    """Return structs, String enums, and aliases keyed by generated Swift type."""
    directory = Path(models_directory or MODELS)
    if not directory.is_dir():
        raise FileNotFoundError(
            f"Emby generated Swift models not found at {directory}; "
            "checkout Emby.ApiClients before running this validator"
        )

    structs, enums, aliases = {}, {}, {}
    for path in sorted(directory.glob("*.swift")):
        text = path.read_text(encoding="utf-8")

        alias_match = TYPEALIAS.search(text)
        if alias_match:
            aliases[alias_match.group(1)] = alias_match.group(2).strip()

        struct_match = STRUCT.search(text)
        if not struct_match:
            enum_match = ENUM.search(text)
            if enum_match:
                enum_body = text[enum_match.end():]
                enums[enum_match.group(1)] = [
                    raw or case for case, raw in CASE.findall(enum_body)
                ]
            continue
        model = struct_match.group(1)
        properties = [(name, typ.strip()) for name, typ in PROPERTY.findall(text)]

        # Swagger-codegen represents free-form JSON dictionaries with a
        # custom `additionalProperties` Codable implementation instead of a
        # normal keyed struct. Model it as its actual wire type.
        if "decodeMap(" in text:
            additional = next(
                (typ for name, typ in properties if name == "additionalProperties"),
                None,
            )
            if additional is not None:
                aliases[model] = additional
                continue

        key_map = {}
        coding_keys = CODING_KEYS.search(text)
        if coding_keys:
            for property_name, wire_name in CASE.findall(coding_keys.group(1)):
                key_map[property_name] = wire_name or property_name

        fields = []
        for property_name, typ in properties:
            wire_name = key_map.get(property_name, property_name)
            fields.append((wire_name, typ, not typ.endswith("?")))
        if fields:
            structs[model] = fields
    return structs, enums, aliases


if __name__ == "__main__":
    loaded = load()
    print(
        f"Emby Swift schema: {len(loaded[0])} structs, "
        f"{len(loaded[1])} enums, {len(loaded[2])} aliases"
    )
