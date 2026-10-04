#!/usr/bin/env python3
"""The credits beside a wallpaper (NAME.txt for NAME.jpg), from the text of
its Unsplash page as saved next to it (wall-credits.py PAGE.txt > NAME.txt).

The picker shows them top left on the left panel: who took it, where, with
what, from where."""
import re, sys

CAMERAS = ("FUJIFILM", "Canon", "Apple", "SONY", "Sony", "NIKON", "Nikon", "Panasonic", "OLYMPUS", "OM Digital", "Leica", "LEICA", "RICOH", "Google", "samsung", "Samsung", "Hasselblad", "DJI", "PENTAX", "Xiaomi")

lines = [l.strip() for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
author, place, camera = lines[1] if len(lines) > 1 else "", "", ""
for l in lines[2:]:
    if l.startswith(("Original Size", "Published", "Free to use", "Unsplash+")):
        continue
    if l.startswith(CAMERAS):
        camera = re.sub(r",\s*", " ", l)
    elif not place:
        place = l
source = "Unsplash" if "Unsplash" in " ".join(lines) else "Pexels" if "Pexels" in " ".join(lines) else ""
for out in (f"Photo by {author}" if author else "", place, camera, source):
    if out:
        print(out)
