#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: tools/validate_mobile_dump.sh <dump-directory>" >&2
    exit 2
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
dump_dir=$1
manifest="$dump_dir/manifest.json"
validated=0

if [[ ! -f "$manifest" ]]; then
    echo "mobile dump manifest is missing: $manifest" >&2
    exit 1
fi

# Materialize jq's output before entering the loop. A process substitution here
# would run jq in a separate process whose failure is invisible to `set -e`.
entries=$(jq -er '
    def manifest_string:
        type == "string"
        and length > 0
        and (test("[\\t\\r\\n]") | not);
    if type != "array" or length == 0 then
        error("manifest must be a nonempty array")
    elif (all(.[];
        type == "object"
        and (.route | manifest_string)
        and (.model | manifest_string)
        and (.file | manifest_string)
        and ((.swiftModel == null) or (.swiftModel | manifest_string))) | not)
    then
        error("manifest entries require nonempty route, model, and file strings")
    else
        .[] | [.model, (.swiftModel // .model), .file] | @tsv
    end
' "$manifest")

while IFS=$'\t' read -r kotlin_model swift_model file; do
    python3 "$repo_root/tools/kotlin_validate.py" "$kotlin_model" "$dump_dir/$file"
    python3 "$repo_root/tools/swift_validate.py" "$swift_model" "$dump_dir/$file"
    validated=$((validated + 1))
done <<< "$entries"

if (( validated == 0 )); then
    echo "mobile dump manifest contained no responses" >&2
    exit 1
fi

echo "validated_responses=$validated validators=Kotlin,Swift"
