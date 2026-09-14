#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: tools/validate_emby_swift_dump.sh <dump-directory>" >&2
    exit 2
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
dump_dir=$1
manifest="$dump_dir/manifest.json"

if [[ ! -f "$manifest" ]]; then
    echo "Emby Swift dump manifest is missing: $manifest" >&2
    exit 1
fi

jq -e '
    def nonempty_string:
        type == "string"
        and length > 0
        and (test("[\\t\\r\\n]") | not);
    type == "object"
    and (.cases | type == "array" and length > 0)
    and all(.cases[];
        type == "object"
        and (.route | nonempty_string)
        and (.model | nonempty_string)
        and has("body"))
' "$manifest" >/dev/null

python3 "$repo_root/tools/emby_swift_validate.py" --manifest "$manifest"
echo "validated Emby Swift response dump: $manifest"
