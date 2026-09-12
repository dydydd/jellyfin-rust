#!/usr/bin/env python3
"""Focused regressions for the Kotlin serialization validator."""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import kotlin_schema
from kotlin_validate import validate


class KotlinValidatorTests(unittest.TestCase):
    def test_file_serializer_annotation_is_not_the_primary_constructor(self):
        models, _enums, _aliases = kotlin_schema.load()

        self.assertFalse([name for name, fields in models.items() if not fields])
        self.assertGreater(len(models["BaseItemDto"]), 100)
        self.assertGreater(len(models["SessionInfoDto"]), 20)
        self.assertIn("Id", {field["serial"] for field in models["BaseItemDto"]})
        self.assertIn(
            "PlayableMediaTypes",
            {field["serial"] for field in models["SessionInfoDto"]},
        )
        self.assertEqual(
            {field["serial"] for field in models["ActivityLogEntryMessage"]},
            {"Data", "MessageId"},
        )

    def test_required_fields_from_annotated_models_are_enforced(self):
        base_item_errors = validate("BaseItemDto", {})
        session_errors = validate("SessionInfoDto", {})

        self.assertTrue(any("BaseItemDto.Id" in error for error in base_item_errors))
        self.assertTrue(
            any("SessionInfoDto.PlayableMediaTypes" in error for error in session_errors)
        )
        self.assertTrue(any("SessionInfoDto.UserId" in error for error in session_errors))

    def test_enum_keyed_maps_and_nonnullable_elements_are_enforced(self):
        invalid_key = validate(
            "BaseItemDto",
            {"Id": "00000000-0000-0000-0000-000000000001", "ImageTags": {"Nope": "x"}},
        )
        invalid_value = validate(
            "BaseItemDto",
            {"Id": "00000000-0000-0000-0000-000000000001", "ImageTags": {"Primary": 1}},
        )
        null_element = validate(
            "BaseItemDtoQueryResult",
            {"Items": [None], "TotalRecordCount": 1, "StartIndex": 0},
        )

        self.assertTrue(any("ImageType map key" in error for error in invalid_key))
        self.assertTrue(any("String expects str" in error for error in invalid_value))
        self.assertTrue(any("BaseItemDto does not accept null" in error for error in null_element))

    def test_uuid_serializer_syntax_is_enforced(self):
        invalid = validate("BaseItemDto", {"Id": "not-a-guid", "Type": "Movie"})
        self.assertTrue(any("UUID expects" in error for error in invalid))
        self.assertEqual(
            validate(
                "BaseItemDto",
                {"Id": "0123456789abcdef0123456789abcdef", "Type": "Movie"},
            ),
            [],
        )


if __name__ == "__main__":
    unittest.main()
