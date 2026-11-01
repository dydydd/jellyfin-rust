#!/usr/bin/env python3
"""Focused regressions for the static Kotlin serialization validator."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from kotlin_validate import validate


class KotlinValidatorTests(unittest.TestCase):
    def test_accepts_a_primitive_root_response(self):
        self.assertEqual(validate("Boolean", True), [])


if __name__ == "__main__":
    unittest.main()
