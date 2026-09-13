#!/usr/bin/env python3
"""Extract a deterministic Emby operation inventory from the generated SDK spec.

The full ``Emby.ApiClients`` checkout is intentionally not committed.  This
tool reduces its OpenAPI document to the small, reviewable contract consumed by
the Rust route-coverage tests.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import yaml


HTTP_METHODS = {"delete", "get", "head", "patch", "post", "put"}


def extract(spec_path: Path) -> dict[str, object]:
    document = yaml.safe_load(spec_path.read_text(encoding="utf-8"))
    operations: list[dict[str, str]] = []
    for path, path_item in document["paths"].items():
        for method, operation in path_item.items():
            method = method.lower()
            if method not in HTTP_METHODS:
                continue
            tags = operation.get("tags") or []
            if len(tags) != 1:
                raise ValueError(
                    f"{method.upper()} {path} must have exactly one service tag"
                )
            operation_id = operation.get("operationId")
            if not isinstance(operation_id, str) or not operation_id:
                raise ValueError(f"{method.upper()} {path} has no operationId")
            operations.append(
                {
                    "method": method.upper(),
                    "path": path,
                    "operationId": operation_id,
                    "tag": tags[0],
                }
            )
    return {
        "version": str(document["info"]["version"]),
        "operations": operations,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("spec", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    inventory = extract(args.spec)
    args.output.write_text(
        json.dumps(inventory, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
