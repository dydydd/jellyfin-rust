#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
validator="$repo_root/tools/validate_emby_swift_dump.sh"
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

mkdir -p "$scratch/bin"
printf '#!/usr/bin/env bash\nexit 0\n' > "$scratch/bin/python3"
chmod +x "$scratch/bin/python3"

expect_failure() {
    local case_dir=$1
    if PATH="$scratch/bin:$PATH" "$validator" "$case_dir" >/dev/null 2>&1; then
        echo "expected validation failure for $case_dir" >&2
        exit 1
    fi
}

mkdir -p "$scratch/missing" "$scratch/malformed" "$scratch/empty" "$scratch/invalid" "$scratch/valid"
printf '{' > "$scratch/malformed/manifest.json"
printf '{"cases":[]}\n' > "$scratch/empty/manifest.json"
printf '{"cases":[{"route":"","model":"SystemInfo","body":{}}]}\n' \
    > "$scratch/invalid/manifest.json"
printf '{"cases":[{"route":"/emby/System/Info","model":"SystemInfo","body":{}}]}\n' \
    > "$scratch/valid/manifest.json"

expect_failure "$scratch/missing"
expect_failure "$scratch/malformed"
expect_failure "$scratch/empty"
expect_failure "$scratch/invalid"
PATH="$scratch/bin:$PATH" "$validator" "$scratch/valid" >/dev/null

echo "validate_emby_swift_dump failure-path checks passed"
