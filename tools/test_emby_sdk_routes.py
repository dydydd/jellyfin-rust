import json
import tempfile
import unittest
from pathlib import Path

from tools.emby_sdk_routes import extract


class EmbySdkRoutesTests(unittest.TestCase):
    def test_extracts_operations_in_source_order(self):
        spec = """
openapi: 3.0.1
info:
  version: 4.10.0.40
paths:
  /System/Ping:
    get:
      operationId: getPingSystem
      tags: [SystemService]
    post:
      operationId: postPingSystem
      tags: [SystemService]
"""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "swagger.yaml"
            path.write_text(spec, encoding="utf-8")
            inventory = extract(path)
        self.assertEqual(inventory["version"], "4.10.0.40")
        self.assertEqual(
            inventory["operations"],
            [
                {
                    "method": "GET",
                    "path": "/System/Ping",
                    "operationId": "getPingSystem",
                    "tag": "SystemService",
                },
                {
                    "method": "POST",
                    "path": "/System/Ping",
                    "operationId": "postPingSystem",
                    "tag": "SystemService",
                },
            ],
        )

    def test_rejects_ambiguous_or_missing_metadata(self):
        for operation in [
            "tags: [SystemService, OtherService]\n      operationId: ping",
            "tags: [SystemService]",
        ]:
            spec = f"""
openapi: 3.0.1
info:
  version: test
paths:
  /System/Ping:
    get:
      {operation}
"""
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "swagger.yaml"
                path.write_text(spec, encoding="utf-8")
                with self.assertRaises(ValueError):
                    extract(path)

    def test_checked_in_inventory_is_valid_json_when_present(self):
        inventory = Path("src/jellyfin-emby-api/tests/fixtures/emby_operations.json")
        if inventory.exists():
            document = json.loads(inventory.read_text(encoding="utf-8"))
            self.assertIsInstance(document["version"], str)
            self.assertTrue(document["operations"])

    def test_checked_in_inventory_matches_local_generated_client_when_present(self):
        spec = Path("Emby.ApiClients/Clients/Go/api/swagger.yaml")
        inventory = Path("src/jellyfin-emby-api/tests/fixtures/emby_operations.json")
        if not spec.exists() or not inventory.exists():
            self.skipTest("the generated Emby client checkout is optional")
        self.assertEqual(
            json.loads(inventory.read_text(encoding="utf-8")),
            extract(spec),
            "refresh emby_operations.json from the generated client contract",
        )


if __name__ == "__main__":
    unittest.main()
