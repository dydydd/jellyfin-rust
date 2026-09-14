import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from emby_swift_schema import MODELS, load
from emby_swift_validate import check_emby_date, validate, validate_manifest


class EmbySwiftSchemaTests(unittest.TestCase):
    def test_extracts_synthesized_codable_keys_enums_and_dictionary_models(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Example.swift").write_text(
                """
public struct Example: Codable {
    public var _id: String?
    public var count: Int
    public enum CodingKeys: String, CodingKey {
        case _id = "Id"
        case count
    }
}
""",
                encoding="utf-8",
            )
            (root / "Choice.swift").write_text(
                """
public enum Choice: String, Codable {
    case first = "First"
    case second = "Second"
}
""",
                encoding="utf-8",
            )
            (root / "Bag.swift").write_text(
                """
public struct Bag: Codable {
    public var additionalProperties: [String:String] = [:]
    public init(from decoder: Decoder) throws {
        additionalProperties = try decoder.container(keyedBy: String.self)
            .decodeMap(String.self, excludedKeys: Set<String>())
    }
}
""",
                encoding="utf-8",
            )
            structs, enums, aliases = load(directory)
        self.assertEqual(
            structs["Example"],
            [("Id", "String?", False), ("count", "Int", True)],
        )
        self.assertEqual(enums["Choice"], ["First", "Second"])
        self.assertEqual(aliases["Bag"], "[String:String]")

    def test_loads_representative_generated_emby_models(self):
        if not Path(MODELS).is_dir():
            self.skipTest("the generated Emby Swift client checkout is optional")
        structs, enums, aliases = load()
        self.assertGreater(len(structs), 240)
        self.assertGreater(len(enums), 80)
        self.assertIn(("ServerName", "String?", False), structs["SystemInfo"])
        self.assertIn(("Items", "[BaseItemDto]?", False), structs["QueryResultBaseItemDto"])
        self.assertEqual(enums["SortOrder"], ["Ascending", "Descending"])
        self.assertEqual(aliases["ProviderIdDictionary"], "[String:String]")


class EmbySwiftValidatorTests(unittest.TestCase):
    def setUp(self):
        if not Path(MODELS).is_dir():
            self.skipTest("the generated Emby Swift client checkout is optional")

    def test_representative_emby_response_fixtures_decode(self):
        manifest = json.loads(
            Path("tools/fixtures/emby_swift_responses.json").read_text(encoding="utf-8")
        )
        self.assertEqual(validate_manifest(manifest), [])

    def test_rejects_present_wrong_scalar_nested_map_and_enum_shapes(self):
        errors = validate("SystemInfo", {"HttpServerPortNumber": "18096"})
        self.assertTrue(any("HttpServerPortNumber" in error for error in errors), errors)

        errors = validate("BaseItemDto", {"ProviderIds": {"Tmdb": 123}})
        self.assertTrue(any("ProviderIds.Tmdb" in error for error in errors), errors)

        errors = validate("BaseItemDto", {"LocationType": "Remote"})
        self.assertTrue(any("LocationType" in error for error in errors), errors)

        errors = validate("DisplayPreferences", {"CustomPrefs": []})
        self.assertTrue(any("CustomPrefs" in error for error in errors), errors)

    def test_uses_generated_emby_date_decoder_formats(self):
        for value in [
            "2026-09-14",
            "2026-09-14T01:02:03Z",
            "2026-09-14T01:02:03.123+08:00",
            "2026-09-14T01:02:03.123",
            "2026-09-14 01:02:03",
        ]:
            self.assertTrue(check_emby_date(value), value)
            self.assertEqual(validate("UserDto", {"DateCreated": value}), [], value)
        self.assertFalse(check_emby_date("09/14/2026"))
        self.assertFalse(check_emby_date("2026-09-14T01:02:03"))
        self.assertTrue(check_emby_date("2026-09-14T01:02:03.1234567Z"))
        self.assertFalse(check_emby_date("2026-09-14T01:02:03.1234567890Z"))

    def test_manifest_reports_the_named_response(self):
        errors = validate_manifest(
            {
                "cases": [
                    {
                        "name": "bad-system-info",
                        "model": "SystemInfo",
                        "body": {"SupportsHttps": "false"},
                    }
                ]
            }
        )
        self.assertTrue(errors)
        self.assertTrue(errors[0].startswith("bad-system-info:"), errors)

    def test_empty_manifest_is_rejected(self):
        self.assertTrue(validate_manifest({"cases": []}))


if __name__ == "__main__":
    unittest.main()
