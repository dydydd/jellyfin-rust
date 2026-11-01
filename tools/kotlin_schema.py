"""Extract the jellyfin-sdk-kotlin generated model schema.

kotlinx.serialization (used by the Android SDK) is strict:
  * a non-nullable property without a default value MUST be present and non-null
  * enum values must match a @SerialName exactly (case sensitive)
  * isLenient = false, so JSON types must match exactly (no "12" for an Int)
The SDK sets coerceInputValues = true, which rescues *nullable* enum properties
(unknown -> null) and enum properties that declare a default, but nothing else.
"""
import os
import re
import json
from pathlib import Path

SDK = os.path.join(os.path.dirname(__file__), os.pardir,
                   "jellyfin-sdk-kotlin", "jellyfin-model", "src",
                   "commonMain", "kotlin-generated", "org", "jellyfin", "sdk", "model")
API = os.path.join(SDK, "api")
CUSTOM = os.path.join(SDK, "custom")

serial_re = re.compile(r'@SerialName\("((?:[^"\\]|\\.)*)"\)')
prop_re = re.compile(
    r'^\s*public val (?P<name>\w+)\s*:\s*(?P<type>.+?)(?:\s*=\s*(?P<default>.+?))?\s*,?\s*$',
    re.M)
enum_entry_re = re.compile(r'^\t@SerialName\("((?:[^"\\]|\\.)*)"\)\s*\n\s*(\w+)\(', re.M)


def strip_comments(text):
    text = re.sub(r'/\*.*?\*/', '', text, flags=re.S)
    text = re.sub(r'(?m)^\s*//.*$', '', text)
    return text


def parse_class(name, body):
    """Return list of {serial, kotlinType, nullable, required}."""
    fields = []
    # Only look at the primary constructor parameter list.
    m = re.search(r'\((.*?)\n\)', body, re.S)
    if not m:
        return fields
    params = m.group(1)
    # Split top-level commas (ignore nested <> and ())
    parts, depth, cur = [], 0, ''
    for ch in params:
        if ch in '(<[':
            depth += 1
        elif ch in ')>]':
            depth -= 1
        if ch == ',' and depth == 0:
            parts.append(cur)
            cur = ''
        else:
            cur += ch
    parts.append(cur)
    for part in parts:
        part = part.strip()
        if not part:
            continue
        sm = serial_re.search(part)
        pm = re.search(r'public val (\w+)\s*:\s*(.+?)(?:\s*=\s*(.+))?$', part, re.S)
        if not pm:
            continue
        kotlin_type = pm.group(2).strip()
        default = pm.group(3)
        # A trailing "= ..." can be swallowed into the type by the lazy match.
        if '=' in kotlin_type:
            head, _, tail = kotlin_type.partition('=')
            if default is None:
                kotlin_type, default = head.strip(), tail.strip()
        nullable = kotlin_type.endswith('?')
        fields.append({
            'name': pm.group(1),
            'serial': sm.group(1) if sm else pm.group(1),
            'kotlinType': kotlin_type,
            'nullable': nullable,
            'hasDefault': default is not None,
            'default': default,
        })
    return fields


def load():
    models, enums, aliases = {}, {}, {}
    for root, _dirs, files in os.walk(API):
        for fn in files:
            if not fn.endswith('.kt'):
                continue
            path = os.path.join(root, fn)
            text = strip_comments(Path(path).read_text(encoding='utf-8'))
            mname = fn[:-3]
            if re.search(r'\benum class\b', text):
                vals = [a for a, _b in enum_entry_re.findall(text)]
                if not vals:
                    vals = re.findall(r'@SerialName\("((?:[^"\\]|\\.)*)"\)', text)
                enums[mname] = vals
            elif re.search(r'\bdata class\b', text):
                models[mname] = parse_class(mname, text)
            else:
                am = re.search(r'^public (?:sealed )?class|^public (?:sealed )?interface', text, re.M)
                if re.search(r'^public typealias|^@Serializable\s*\npublic (?:sealed )?(?:class|interface)', text, re.M):
                    aliases[mname] = text
    return models, enums, aliases


if __name__ == '__main__':
    models, enums, aliases = load()
    out = {'models': models, 'enums': enums,
           'aliases': sorted(aliases), 'custom': sorted(
               f[:-3] for f in os.listdir(CUSTOM) if f.endswith('.kt')) if os.path.isdir(CUSTOM) else []}
    print(json.dumps(out, indent=1, sort_keys=True))
