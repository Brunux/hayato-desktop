#!/usr/bin/env python3
"""The full-home export must skip rootless container storage.

Rootless podman/buildah layers are owned by the user's subuids, so the user
cannot read them and tar aborts the whole archive with "Cannot open: Permission
denied" after copying everything else. The test makes a layer unreadable and
asserts both that the export succeeds and that the tree is absent from the
archive, which is the failure the Developer menu hit.
"""

import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = (
    pathlib.Path(__file__).resolve().parents[2]
    / "uis/sway/files/home/.config/bin/hayato-profile"
)


@unittest.skipUnless(
    all(shutil.which(cmd) for cmd in ("sh", "tar", "zstd", "du", "df")),
    "sh, tar, zstd, du and df are needed for the profile export test",
)
class HomeExportTests(unittest.TestCase):
    def test_container_storage_is_skipped_and_documents_travel(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = pathlib.Path(tmp) / "home"
            target = pathlib.Path(tmp) / "target"
            layer = home / ".local/share/containers/storage/overlay/layer"
            (home / "Documents").mkdir(parents=True)
            layer.mkdir(parents=True)
            target.mkdir()
            (home / "Documents/user.txt").write_text("keep\n")
            (layer / "secret.txt").write_text("layer\n")
            os.chmod(layer, 0)

            env = os.environ.copy()
            env["HOME"] = str(home)
            env["XDG_CACHE_HOME"] = str(home / ".cache")
            try:
                result = subprocess.run(
                    [
                        "sh",
                        str(SCRIPT),
                        "export",
                        str(target),
                        "--include-home",
                        "--no-encrypt",
                    ],
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=120,
                )
                self.assertEqual(result.returncode, 0, result.stderr)

                archives = list(target.glob("hayato-profile-*.tar.zst"))
                self.assertEqual(1, len(archives), "one archive is written")
                listing = subprocess.run(
                    ["tar", "--zstd", "-tf", str(archives[0])],
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout.splitlines()

                self.assertIn("Documents/user.txt", listing)
                self.assertFalse(
                    any("containers" in name for name in listing),
                    "rootless container storage must not be in the archive",
                )
            finally:
                os.chmod(layer, 0o700)


if __name__ == "__main__":
    unittest.main()
