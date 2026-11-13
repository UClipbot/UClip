#!/usr/bin/python3
"""Controlled external GTK paste target. Synthetic data stays in the temporary test directory."""
import json
import sys
from pathlib import Path
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk, GLib
result = Path(sys.argv[1])
window = Gtk.Window(title='UClip native fixture receiver')
window.set_default_size(480, 300)
view = Gtk.TextView()
window.add(view)
clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)

def paste(_view, event):
    if event.keyval not in (Gdk.KEY_v, Gdk.KEY_V) or not event.state & Gdk.ModifierType.CONTROL_MASK:
        return False
    ok, targets = clipboard.wait_for_targets()
    names = [target.name() for target in targets] if ok else []
    if 'text/uri-list' in names:
        selection = clipboard.wait_for_contents(Gdk.Atom.intern('text/uri-list', False))
        result.write_text(json.dumps({'kind': 'files', 'uris': selection.get_uris()}, ensure_ascii=False))
        return True
    if clipboard.wait_is_image_available():
        image = clipboard.wait_for_image()
        pixels = image.get_pixels()
        width, height, channels, stride = image.get_width(), image.get_height(), image.get_n_channels(), image.get_rowstride()
        rgba = []
        for y in range(height):
            for x in range(width):
                value = list(pixels[y * stride + x * channels:y * stride + x * channels + channels])
                rgba.extend(value if channels == 4 else value + [255])
        result.write_text(json.dumps({'kind': 'image', 'width': width, 'height': height, 'rgba': rgba}))
        return True
    def record_text():
        buffer = view.get_buffer()
        result.write_text(json.dumps({'kind': 'text', 'text': buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True)}, ensure_ascii=False))
        return False
    GLib.timeout_add(80, record_text)
    return False

view.connect('key-press-event', paste)
window.connect('destroy', Gtk.main_quit)
window.show_all()
view.grab_focus()
Gtk.main()
