#!/usr/bin/python3
"""Create public synthetic acceptance data in a new disposable directory; never read a clipboard."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import zlib


def create(directory):
    directory.mkdir(parents=True, exist_ok=False)
    text = '  中文 Clipboard 🦀\n第一行\n\nSECOND line\t尾部空白  '
    values = {'unicode.txt': text, 'large.txt': 'x' * (1024 * 1024), 'oversize.txt': 'x' * (4 * 1024 * 1024 + 1)}
    for name, value in values.items():
        (directory / name).write_text(value, encoding='utf-8')
    folder = directory / '文件夹 空格 # %'
    folder.mkdir()
    (folder / '样本.txt').write_text('synthetic file content\n', encoding='utf-8')
    reference = directory / '文件 中文 # %.txt'
    reference.write_text('synthetic reference\n', encoding='utf-8')
    rgba = bytes([12, 34, 56, 0, 255, 0, 99, 128, 7, 8, 9, 255, 100, 120, 140, 255])
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b'\0' + rgba[:8] + b'\0' + rgba[8:])) + chunk(b'IEND', b'')
    (directory / 'transparent.png').write_bytes(png)
    manifest = {
        'version': 1,
        'text': {'file': 'unicode.txt', 'sha256': hashlib.sha256(text.encode()).hexdigest(), 'expected': 'exact Unicode, newlines and whitespace'},
        'image': {'file': 'transparent.png', 'width': 2, 'height': 2, 'rgba': list(rgba)},
        'files': [str(reference.resolve()), str(folder.resolve())],
        'missingFile': str((directory / 'does-not-exist.txt').resolve()),
        'limits': {'acceptedText': 'large.txt', 'rejectedText': 'oversize.txt'},
        'expected': ['A→B→A reuses A identity only in capture scope', 'sensitive/excluded/paused data never saved', 'one invalid file rejects the entire write', 'deleted identity remains deleted on recapture', 'OS copy and usage persistence report separately'],
    }
    (directory / 'manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path, help='New directory; existing paths are refused.')
    create(parser.parse_args().directory)
