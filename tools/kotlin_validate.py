#!/usr/bin/env python3
"""Validate a Jellyfin JSON response against the jellyfin-sdk-kotlin model graph.

The Android client decodes every API body with kotlinx.serialization using
  isLenient = false, ignoreUnknownKeys = true, explicitNulls = false,
  coerceInputValues = true
so the rules encoded here are:
  * a non-nullable property with no default is REQUIRED and must not be null
  * a non-nullable property with a default may be absent, but null still fails
    unless the property is optional (then coerceInputValues rescues it)
  * enum strings must equal one of the @SerialName values exactly
  * JSON types must match: quoted numbers and unquoted booleans are fatal
Usage: kotlin_validate.py <ModelName> <response.json> [-] reads stdin
"""
import json
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from kotlin_schema import load

PRIMITIVES = {
    'Boolean': ('bool',), 'Int': ('int',), 'Long': ('int',), 'Float': ('num',),
    'Double': ('num',), 'String': ('str',), 'UUID': ('str',), 'DateTime': ('str',),
    'Any': None, 'ByteArray': ('str',),
}


class Report:
    def __init__(self):
        self.errors = []

    def add(self, path, msg):
        self.errors.append(f'{path}: {msg}')


def base_type(t):
    return t.rstrip('?').strip()


def split_generic_args(content):
    parts, depth, current = [], 0, ''
    for char in content:
        if char in '<([':
            depth += 1
        elif char in '>)]':
            depth -= 1
        if char == ',' and depth == 0:
            parts.append(current.strip())
            current = ''
        else:
            current += char
    parts.append(current.strip())
    return parts


def check(models, enums, aliases, typ, value, path, rep, depth=0):
    nullable = typ.strip().endswith('?')
    typ = base_type(typ)

    if value is None:
        if not nullable:
            rep.add(path, f'{typ} does not accept null')
        return

    m = re_match(r'^(?:List|Collection)<(.+)>$', typ)
    if m:
        if not isinstance(value, list):
            rep.add(path, f'expected array for {typ}, got {json_type(value)}')
            return
        for i, item in enumerate(value):
            check(models, enums, aliases, m.group(1), item, f'{path}[{i}]', rep, depth + 1)
        return
    m = re_match(r'^Map<(.+)>$', typ)
    if m:
        if not isinstance(value, dict):
            rep.add(path, f'expected object for {typ}, got {json_type(value)}')
            return
        arguments = split_generic_args(m.group(1))
        if len(arguments) != 2:
            rep.add(path, f'cannot parse map type {typ}')
            return
        key_type, value_type = arguments
        for k, v in value.items():
            if key_type in enums and k not in enums[key_type]:
                rep.add(f'{path}.{k}', f'{key_type} map key is not supported')
            elif key_type != 'String' and key_type not in enums:
                rep.add(path, f'unsupported map key type {key_type}')
            check(models, enums, aliases, value_type, v,
                  f'{path}.{k}', rep, depth + 1)
        return
    m = re_match(r'^Set<(.+)>$', typ)
    if m:
        if not isinstance(value, list):
            rep.add(path, f'expected array for {typ}, got {json_type(value)}')
            return
        for i, item in enumerate(value):
            check(models, enums, aliases, m.group(1), item, f'{path}[{i}]', rep, depth + 1)
        return

    if typ in enums:
        if not isinstance(value, str):
            rep.add(path, f'enum {typ} needs a JSON string, got {json_type(value)}')
        elif value not in enums[typ]:
            rep.add(path, f'{typ} value {value!r} is not one of {enums[typ]}')
        return

    if typ == 'UUID':
        if not isinstance(value, str) or not (
                re_match(r'^[0-9a-f]{32}$', value)
                or re_match(
                    r'^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-'
                    r'[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$', value)):
            rep.add(path, f'UUID expects a simple-lowercase or hyphenated UUID, got {short(value)}')
        return

    if typ in PRIMITIVES:
        want = PRIMITIVES[typ]
        if want is None:
            return
        ok = {'bool': lambda v: isinstance(v, bool),
              'int': lambda v: isinstance(v, int) and not isinstance(v, bool),
              'num': lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
              'str': lambda v: isinstance(v, str)}[want[0]]
        if not ok(value):
            rep.add(path, f'{typ} expects {want[0]}, got {json_type(value)} ({short(value)})')
        return

    if typ in models:
        if not isinstance(value, dict):
            rep.add(path, f'expected object for {typ}, got {json_type(value)}')
            return
        check_model(models, enums, aliases, typ, value, path, rep, depth)
        return

    if typ in aliases or typ.startswith('OneOf'):
        return  # union / hand written type: accept anything
    # Unknown type (hand-written model outside api package) - ignore.


def check_model(models, enums, aliases, name, value, path, rep, depth=0):
    if depth > 12:
        return
    if not models[name]:
        rep.add(path, f'model schema for {name} has zero parsed fields')
        return
    present = {f['serial']: f for f in models[name]}
    for f in models[name]:
        if f['serial'] not in value:
            if not f['nullable'] and not f['hasDefault']:
                rep.add(f"{path}.{f['serial']}",
                        f'MissingFieldException: required {f["kotlinType"]} absent')
            continue
        v = value[f['serial']]
        if v is None:
            if not f['nullable'] and not f['hasDefault']:
                rep.add(f"{path}.{f['serial']}",
                        f'null for required non-nullable {f["kotlinType"]}')
            elif not f['nullable'] and not coerceable(f['kotlinType'], enums):
                rep.add(f"{path}.{f['serial']}",
                        f'null for non-nullable {f["kotlinType"]} (no coercion)')
            continue
        check(models, enums, aliases, f['kotlinType'], v, f"{path}.{f['serial']}", rep, depth + 1)


def coerceable(typ, enums):
    """coerceInputValues only rescues unknown enum members, not other type errors."""
    return base_type(typ) in enums


def re_match(pattern, s):
    import re
    return re.match(pattern, s)


def json_type(v):
    if v is None:
        return 'null'
    if isinstance(v, bool):
        return 'boolean'
    if isinstance(v, (int, float)):
        return 'number'
    if isinstance(v, str):
        return 'string'
    return 'array' if isinstance(v, list) else 'object'


def short(v):
    s = json.dumps(v, ensure_ascii=False)
    return s if len(s) <= 40 else s[:37] + '...'


def validate(root_model, doc):
    models, enums, aliases = load()
    if (root_model not in models and root_model not in enums
            and root_model not in aliases
            and not re_match(r'^(?:List|Set)<.+>$', root_model)):
        raise SystemExit(f'unknown Kotlin type {root_model}')
    rep = Report()
    check(models, enums, aliases, root_model, doc, root_model, rep)
    return rep.errors


if __name__ == '__main__':
    args = [a for a in sys.argv[1:]]
    root = args[0]
    src = sys.stdin if args[1] == '-' else open(args[1])
    doc = json.load(src)
    errs = validate(root, doc)
    for e in errs:
        print(e)
    print(f'{root}: {len(errs)} error(s)', file=sys.stderr)
    sys.exit(1 if errs else 0)
