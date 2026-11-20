#!/usr/bin/env python3
"""
Generates 36x12 and 48x16 PNG thumbnails for all wallpapers in uis/sway/files/home/Pictures/Wallpapers/
and copies them to ~/.cache/hayato/wallpapers/thumbnails/ and ~/Pictures/Wallpapers/thumbnails/.
"""

import os
import subprocess
import glob

WP_DIRS = [
    os.path.abspath("uis/sway/files/home/Pictures/Wallpapers"),
    os.path.expanduser("~/Pictures/Wallpapers"),
]


def thumbnail_filename(filename):
    """The file name ffmpeg writes, keeping the source extension in the name."""
    return f"{filename}.png"


def clean_thumbnail_name(filename):
    """The same thumbnail without the duplicated extension (`a.jpg` -> `a.png`)."""
    return os.path.splitext(filename)[0] + ".png"


def main():
    for wp_dir in WP_DIRS:
        if not os.path.exists(wp_dir):
            continue
        thumb_dir = os.path.join(wp_dir, "thumbnails")
        os.makedirs(thumb_dir, exist_ok=True)

        # Process all jpg and png files (except inside thumbnails)
        for ext in ("*.jpg", "*.png"):
            for path in glob.glob(os.path.join(wp_dir, ext)):
                filename = os.path.basename(path)
                thumb_path = os.path.join(thumb_dir, thumbnail_filename(filename))

                # Generate 40x16 thumbnail with center crop
                cmd = [
                    "ffmpeg", "-y", "-i", path,
                    "-vf", "scale=40:16:force_original_aspect_ratio=increase,crop=40:16",
                    thumb_path
                ]
                try:
                    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    print(f"Generated {thumb_path}")
                    # Also generate clean name without duplicate ext
                    clean_name = clean_thumbnail_name(filename)
                    clean_path = os.path.join(thumb_dir, clean_name)
                    if clean_path != thumb_path:
                        subprocess.run(["cp", "-f", thumb_path, clean_path])
                except Exception as e:
                    print(f"Error generating thumbnail for {path}: {e}")

    # Also copy to cache directory
    cache_thumb_dir = os.path.expanduser("~/.cache/hayato/wallpapers/thumbnails")
    os.makedirs(cache_thumb_dir, exist_ok=True)
    for path in glob.glob(os.path.join(WP_DIRS[0], "thumbnails", "*.png")):
        filename = os.path.basename(path)
        subprocess.run(["cp", "-f", path, os.path.join(cache_thumb_dir, filename)])

    print("Thumbnails generated successfully!")


if __name__ == "__main__":
    main()
