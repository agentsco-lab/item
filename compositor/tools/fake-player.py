#!/usr/bin/env python3
"""fake-player.py [SECONDS]: a media player on the session bus (MPRIS) that
plays nothing, for trying the shade's player card with no one at the phone.
It says it plays a track, and answers Previous, PlayPause and Next."""
import sys
from gi.repository import Gio, GLib

XML = """<node>
 <interface name="org.mpris.MediaPlayer2"><property name="Identity" type="s" access="read"/></interface>
 <interface name="org.mpris.MediaPlayer2.Player">
  <method name="Previous"/><method name="Next"/><method name="PlayPause"/>
  <property name="PlaybackStatus" type="s" access="read"/>
  <property name="Metadata" type="a{sv}" access="read"/>
 </interface>
</node>"""
TRACKS = [("Weightless", "Marconi Union"), ("Clair de Lune", "Claude Debussy"), ("Gymnopédie No. 1", "Erik Satie")]
state = {"track": 0, "playing": True}


def call(conn, sender, path, iface, method, params, inv):
    if method == "Next":
        state["track"] = (state["track"] + 1) % len(TRACKS)
    elif method == "Previous":
        state["track"] = (state["track"] - 1) % len(TRACKS)
    elif method == "PlayPause":
        state["playing"] = not state["playing"]
    print("fake-player:", method, flush=True)
    inv.return_value(None)


def get(conn, sender, path, iface, prop):
    title, artist = TRACKS[state["track"]]
    if prop == "Identity":
        return GLib.Variant("s", "Fake Player")
    if prop == "PlaybackStatus":
        return GLib.Variant("s", "Playing" if state["playing"] else "Paused")
    if prop == "Metadata":
        return GLib.Variant("a{sv}", {"xesam:title": GLib.Variant("s", title), "xesam:artist": GLib.Variant("as", [artist])})


def acquired(conn, name):
    info = Gio.DBusNodeInfo.new_for_xml(XML)
    for i in info.interfaces:
        conn.register_object("/org/mpris/MediaPlayer2", i, call, get, None)


Gio.bus_own_name(Gio.BusType.SESSION, "org.mpris.MediaPlayer2.fake", Gio.BusNameOwnerFlags.NONE, acquired, None, None)
loop = GLib.MainLoop()
GLib.timeout_add_seconds(int(sys.argv[1]) if len(sys.argv) > 1 else 120, loop.quit)
loop.run()
