#!/usr/bin/env python3
"""Extract the Codable wire schema from jellyfin-sdk-swift entities.

The generated Swift SDK decodes its models explicitly with StringCodingKey.
Reading those calls instead of inferring names from Swift properties preserves
wire spellings such as `PlaySessionId` and `ProviderIds`.
"""
import os
import re
from pathlib import Path

ENTITIES = os.path.join(os.path.dirname(os.path.abspath(__file__)), os.pardir,
                        "jellyfin-sdk-swift", "Sources", "Entities")

DECODE = re.compile(
    r"self\.\w+\s*=\s*try values\.decodeIfPresent\((.+?)\.self,\s*forKey:\s*\"([^\"]+)\"\)")
ENUM = re.compile(r"public enum\s+(\w+)\s*:\s*String\b")
CASE = re.compile(r"^\s*case\s+`?([A-Za-z_][A-Za-z0-9_]*)`?(?:\s*=\s*\"([^\"]+)\")?", re.M)
TYPEALIAS = re.compile(r"public typealias\s+(\w+)\s*=\s*(.+)$", re.M)


def load():
    """Return structs, String enums, and aliases keyed by public Swift type."""
    structs, enums, aliases = {}, {}, {}
    for name in os.listdir(ENTITIES):
        if not name.endswith(".swift"):
            continue
        path = os.path.join(ENTITIES, name)
        text = Path(path).read_text(encoding="utf-8")
        model = name[:-6]
        fields = [(key, typ.strip()) for typ, key in DECODE.findall(text)]
        if fields:
            structs[model] = fields
        enum_match = ENUM.search(text)
        if enum_match:
            values = []
            for case, raw in CASE.findall(text):
                values.append(raw or case)
            enums[enum_match.group(1)] = values
        alias_match = TYPEALIAS.search(text)
        if alias_match:
            aliases[alias_match.group(1)] = alias_match.group(2).strip()
    return structs, enums, aliases
