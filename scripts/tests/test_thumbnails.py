#!/usr/bin/env python3
"""Tests for the wallpaper thumbnail naming helpers.

The script is imported by path; its generation lives behind `main()`, so
importing it runs no ffmpeg and touches no directory.
"""

import importlib.util
import pathlib
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]


def load(name):
    path = SCRIPTS / name
    module_name = name.replace("-", "_").replace(".py", "")
    spec = importlib.util.spec_from_file_location(module_name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


thumbnails = load("generate-wallpaper-thumbnails.py")


class NameTests(unittest.TestCase):
    def test_thumbnail_keeps_the_source_extension(self):
        self.assertEqual("one-dark.jpg.png",
                         thumbnails.thumbnail_filename("one-dark.jpg"))

    def test_clean_name_has_one_extension(self):
        self.assertEqual("one-dark.png",
                         thumbnails.clean_thumbnail_name("one-dark.jpg"))

    def test_clean_name_keeps_dots_before_the_extension(self):
        self.assertEqual("dracula-night.v2.png",
                         thumbnails.clean_thumbnail_name("dracula-night.v2.jpg"))

    def test_a_png_source_keeps_its_name(self):
        self.assertEqual("paper.png", thumbnails.clean_thumbnail_name("paper.png"))

    def test_main_is_callable(self):
        self.assertTrue(callable(thumbnails.main))


if __name__ == "__main__":
    unittest.main()
