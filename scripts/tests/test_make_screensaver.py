"""Unit tests for the Hayato pixel-hacker Golden Eagle screensaver artworks."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
MODULE_PATH = SCRIPTS_DIR / "make-screensaver.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("make_screensaver", MODULE_PATH)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class ScreensaverArtTests(unittest.TestCase):
    def test_all_three_golden_eagle_variants_exist_and_have_uniform_width(self) -> None:
        mod = _load_module()
        out_dir: Path = mod.OUT_DIR
        self.assertEqual(len(mod.VARIANTS), 3)
        for filename, bitmap in mod.VARIANTS:
            p = out_dir / filename
            self.assertTrue(p.is_file(), f"missing {p}")
            text = p.read_text(encoding="utf-8")
            self.assertEqual(text, mod.compose_screensaver(bitmap))
            lines = text.splitlines()
            self.assertGreaterEqual(len(lines), 16)
            widths = {len(line) for line in lines}
            self.assertEqual(len(widths), 1, f"{filename} lines are not uniform width")
            # Ultra-minimal: no arcade/caption prose
            self.assertNotIn("press any key", text.lower())
            self.assertNotIn("light. fast. fun.", text.lower())
            # Contains half-block pixel art
            self.assertIn("\u2588", text)


if __name__ == "__main__":
    unittest.main()
