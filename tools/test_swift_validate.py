#!/usr/bin/env python3
"""Focused regressions for the static Swift Codable validator."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from swift_validate import validate


class SwiftValidatorTests(unittest.TestCase):
    def test_accepts_real_swift_wire_values(self):
        document = {
            "Configuration": {"SubtitleMode": "Default"},
            "ProviderIds": {"Tmdb": "123"},
            "DateCreated": "2026-09-07T01:02:03.0000000Z",
        }
        self.assertEqual(validate("BaseItemDto", document), [])

    def test_rejects_present_invalid_enum(self):
        document = {"Configuration": {"SubtitleMode": "DEFAULT"}}
        errors = validate("UserDto", document)
        self.assertTrue(any("SubtitlePlaybackMode" in error for error in errors), errors)

    def test_rejects_date_without_time_zone(self):
        errors = validate("BaseItemDto", {"DateCreated": "2026-09-08"})
        self.assertTrue(any("DateCreated" in error for error in errors), errors)

    def test_rejects_present_dictionary_value_of_wrong_shape(self):
        document = {"ProviderIds": {"Tmdb": 123}}
        errors = validate("BaseItemDto", document)
        self.assertTrue(any("ProviderIds.Tmdb" in error for error in errors), errors)

    def test_rejects_missing_required_property(self):
        errors = validate("SystemStorageDto", {})
        self.assertTrue(any("CacheFolder" in error for error in errors), errors)


if __name__ == "__main__":
    unittest.main()
