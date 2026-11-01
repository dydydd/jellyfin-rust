#!/usr/bin/env python3
import unittest
from unittest.mock import patch

from runtime_sdk_validate import item_id, item_ids, named_item, recursive_item_pages, response_json


class RuntimeSdkValidateTests(unittest.TestCase):
    def test_item_selection_requires_matching_typed_values(self):
        result = {"Items": [{"Type": "Movie", "Id": "movie"}, {"Type": "Series", "Id": 1}]}
        self.assertEqual(item_id(result, "Movie"), "movie")
        self.assertIsNone(item_id(result, "Series"))
        self.assertEqual(item_ids(result), {"Movie": "movie"})
        self.assertEqual(named_item({"Items": [{"Name": 1}, {"Name": "Genre"}]}), "Genre")

    @patch("runtime_sdk_validate.get", return_value=(200, b'{"Items": []}'))
    def test_response_json_decodes_successful_json(self, _get):
        self.assertEqual(response_json("http://server", "token", "/Items"), (200, {"Items": []}))

    @patch("runtime_sdk_validate.get", return_value=(200, b'{"Items": [{"Id": "movie"}], "TotalRecordCount": 1}'))
    def test_recursive_item_pages_stops_at_total(self, _get):
        self.assertEqual(len(list(recursive_item_pages("http://server", "token"))), 1)


if __name__ == "__main__":
    unittest.main()
