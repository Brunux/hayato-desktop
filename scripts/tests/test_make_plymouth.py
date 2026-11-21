"""Unit tests for the Hayato pixel-hacker Plymouth boot splash assets and generator."""

from __future__ import annotations

import importlib.util
import struct
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
MODULE_PATH = SCRIPTS_DIR / "make-plymouth.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("make_plymouth", MODULE_PATH)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _png_size(data: bytes) -> tuple[int, int, int]:
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "invalid PNG signature"
    length, tag = struct.unpack(">I4s", data[8:16])
    assert tag == b"IHDR" and length == 13
    width, height, bit_depth, color_type = struct.unpack(">IIBB", data[16:26])
    assert bit_depth == 8
    return width, height, color_type


class PlymouthThemeTests(unittest.TestCase):
    def test_pixel_generators_produce_valid_rgba_pngs(self) -> None:
        mod = _load_module()
        lw, lh, ltype = _png_size(
            mod.render_bitmap_png(mod.LETTER_BITMAPS["H"], mod.FG_RGB)
        )
        self.assertEqual((lw, lh, ltype), (mod.LETTER_W, mod.LETTER_H, 6))

        cw, ch, ctype = _png_size(
            mod.render_solid_block_png(mod.CURSOR_W, mod.CURSOR_H, mod.CURSOR_RGB)
        )
        self.assertEqual((cw, ch, ctype), (mod.CURSOR_W, mod.CURSOR_H, 6))

        dw, dh, dtype = _png_size(
            mod.render_solid_block_png(mod.DOT_SIZE, mod.DOT_SIZE, mod.DOT_LIT_RGB)
        )
        self.assertEqual((dw, dh, dtype), (mod.DOT_SIZE, mod.DOT_SIZE, 6))

    def test_committed_theme_files_and_manifest_are_complete(self) -> None:
        mod = _load_module()
        theme_dir: Path = mod.THEME_DIR
        manifest = theme_dir / "hayato.plymouth"
        script = theme_dir / "hayato.script"
        self.assertTrue(manifest.is_file(), f"missing {manifest}")
        self.assertTrue(script.is_file(), f"missing {script}")

        manifest_text = manifest.read_text(encoding="utf-8")
        self.assertIn("ModuleName=script", manifest_text)
        self.assertIn("ImageDir=/usr/share/plymouth/themes/hayato", manifest_text)
        self.assertIn(
            "ScriptFile=/usr/share/plymouth/themes/hayato/hayato.script",
            manifest_text,
        )

        script_text = script.read_text(encoding="utf-8")
        self.assertIn("Plymouth.SetRefreshFunction", script_text)
        self.assertIn("Window.SetBackgroundTopColor(0.102, 0.106, 0.149)", script_text)

        for idx in range(len(mod.WORD)):
            p = theme_dir / f"letter-{idx}.png"
            self.assertTrue(p.is_file(), f"missing {p}")
            w, h, ctype = _png_size(p.read_bytes())
            self.assertEqual((w, h, ctype), (mod.LETTER_W, mod.LETTER_H, 6))

        for idx in range(len(mod.GLITCH_BITMAPS)):
            p = theme_dir / f"glitch-{idx}.png"
            self.assertTrue(p.is_file(), f"missing {p}")
            w, h, ctype = _png_size(p.read_bytes())
            self.assertEqual((w, h, ctype), (mod.LETTER_W, mod.LETTER_H, 6))

        cw, ch, ctype = _png_size((theme_dir / "cursor.png").read_bytes())
        self.assertEqual((cw, ch, ctype), (mod.CURSOR_W, mod.CURSOR_H, 6))

        for dot_name in ("dot-dim.png", "dot-lit.png"):
            dw, dh, dtype = _png_size((theme_dir / dot_name).read_bytes())
            self.assertEqual((dw, dh, dtype), (mod.DOT_SIZE, mod.DOT_SIZE, 6))


if __name__ == "__main__":
    unittest.main()
