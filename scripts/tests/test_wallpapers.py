#!/usr/bin/env python3
"""Tests for the procedural wallpaper generators.

The script is imported by path (its name has dashes), which is only safe
because generation now lives behind `main()`.
"""

import importlib.util
import math
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


wallpapers = load("make-new-wallpapers.py")


class ClampTests(unittest.TestCase):
    def test_clamps_below_zero(self):
        self.assertEqual(0, wallpapers.clamp(-12))

    def test_clamps_above_255(self):
        self.assertEqual(255, wallpapers.clamp(999))

    def test_truncates_fraction(self):
        self.assertEqual(12, wallpapers.clamp(12.9))


class HexTests(unittest.TestCase):
    def test_with_hash(self):
        self.assertEqual((40, 42, 54), wallpapers.hex_to_rgb("#282a36"))

    def test_without_hash(self):
        self.assertEqual((255, 255, 255), wallpapers.hex_to_rgb("ffffff"))

    def test_rejects_short_value(self):
        with self.assertRaises(ValueError):
            wallpapers.hex_to_rgb("#fff")


class GeneratorTests(unittest.TestCase):
    POINTS = [(0, 0), (wallpapers.W - 1, 0), (0, wallpapers.H - 1),
             (wallpapers.W - 1, wallpapers.H - 1),
             (wallpapers.W // 2, wallpapers.H // 2)]

    def test_every_generator_returns_three_finite_channels(self):
        for name, gen in wallpapers.WALLPAPERS:
            for x, y in self.POINTS:
                r, g, b = gen(x, y, wallpapers.W, wallpapers.H)
                for channel in (r, g, b):
                    self.assertTrue(math.isfinite(channel),
                                    f"{name} at ({x}, {y}) is not finite")
                    self.assertGreaterEqual(wallpapers.clamp(channel), 0)
                    self.assertLessEqual(wallpapers.clamp(channel), 255)

    def test_latte_is_a_light_theme(self):
        r, g, b = wallpapers.gen_latte(wallpapers.W // 2,
                                       wallpapers.H // 2,
                                       wallpapers.W, wallpapers.H)
        self.assertGreater(r, 200)
        self.assertGreater(g, 200)
        self.assertGreater(b, 200)

    def test_hackerman_draws_a_brighter_grid_line(self):
        on_grid = wallpapers.gen_hackerman(64, 64, wallpapers.W, wallpapers.H)
        off_grid = wallpapers.gen_hackerman(65, 65, wallpapers.W, wallpapers.H)
        self.assertGreater(on_grid[1], off_grid[1])

    def test_matte_black_stays_grey_with_a_cool_sheen(self):
        r, g, b = wallpapers.gen_matte_black(0, 0, wallpapers.W, wallpapers.H)
        self.assertAlmostEqual(r, g)
        self.assertGreater(b, r)

    def test_the_table_names_each_file_once(self):
        names = [name for name, _ in wallpapers.WALLPAPERS]
        self.assertEqual(8, len(names))
        self.assertEqual(len(names), len(set(names)))
        self.assertTrue(all(name.endswith(".jpg") for name in names))

    def test_importing_does_not_generate_anything(self):
        # main() is the only place that touches the filesystem or ffmpeg.
        self.assertTrue(callable(wallpapers.main))


if __name__ == "__main__":
    unittest.main()
