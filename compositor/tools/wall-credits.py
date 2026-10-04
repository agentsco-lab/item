#!/usr/bin/env python3
"""The credits beside a wallpaper (NAME.txt for NAME.jpg), from the text of
its Unsplash page as saved next to it (wall-credits.py PAGE.txt > NAME.txt).

The picker shows them top left on the left panel: who took it, where, with
what, from where."""
import re, sys

CAMERAS = ("FUJIFILM", "Canon", "Apple", "SONY", "Sony", "NIKON", "Nikon", "Panasonic", "OLYMPUS", "OM Digital", "Leica", "LEICA", "RICOH", "Google", "samsung", "Samsung", "Hasselblad", "DJI", "PENTAX", "Xiaomi", "NORITSU", "OnePlus", "HUAWEI", "Fujifilm")

path = sys.argv[1]
lines = [l.strip() for l in open(path, encoding="utf-8") if l.strip()]
slug = lines[0] if lines else ""
# Who took it: the page's line after the slug, else the slug's name part
# (the page file is named by the photo's id).
photo_id = path.rsplit("/", 1)[-1].rsplit(".", 1)[0]
author = lines[1] if len(lines) > 1 and not lines[1].startswith("Original Size") else ""
if not author and slug.endswith(f"-{photo_id}-unsplash"):
    author = slug[: -len(f"-{photo_id}-unsplash")].replace("-", " ").title()
place, camera, published = "", "", False
for l in lines[2:]:
    if l.startswith("Published"):
        published = True
        continue
    if l.startswith(("Original Size", "Free to use", "Unsplash+")):
        continue
    # After the date, or a known maker: the camera; before it, the place.
    if published or l.startswith(CAMERAS):
        camera = camera or re.sub(r",\s*", " ", l)
    elif not place:
        place = l
source = "Unsplash" if "Unsplash" in " ".join(lines) else "Pexels" if "Pexels" in " ".join(lines) else ""
for out in (f"Photo by {author}" if author else "", place, camera, source):
    if out:
        print(out)
