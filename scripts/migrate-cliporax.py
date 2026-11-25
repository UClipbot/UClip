#!/usr/bin/env python3
"""Migrate a Cliporax SQLite library to an initialized UClip library.

Use a current UClip CLI on PATH or pass --uclip. Preview is the default.
The source is opened read-only, including committed contents of its SQLite WAL.
"""

import argparse
from collections import Counter
from contextlib import closing
from datetime import datetime, timezone
from itertools import chain
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import sqlite3
import subprocess
import sys
from urllib.parse import unquote, urlsplit
from uuid import NAMESPACE_URL, uuid5


MIB = 1024 * 1024
CONTENT_LIMITS = {"text": 4 * MIB, "image": 54 * MIB, "file": 8 * MIB}
EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)


class MigrationError(Exception):
    pass


class SkipRecord(Exception):
    pass


def host_platform():
    if sys.platform == "win32":
        return "windows"
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("linux"):
        return "linux"
    raise MigrationError("Supported platforms are Windows, macOS, and Linux.")


def default_source(platform):
    if platform == "windows":
        root = Path(os.environ.get("APPDATA") or Path.home() / "AppData" / "Roaming")
    elif platform == "macos":
        root = Path.home() / "Library" / "Application Support"
    else:
        xdg = Path(os.environ.get("XDG_DATA_HOME", ""))
        root = xdg if xdg.is_absolute() else Path.home() / ".local" / "share"
    return root / "com.cliporax.app" / "cliporax.db"


def columns(source, table):
    return {column["name"] for column in source.execute(f"PRAGMA table_info({table})")}


def namespace(source, platform, source_id):
    if source_id is None and "device_id" in columns(source, "sync_device"):
        row = source.execute("SELECT device_id FROM sync_device WHERE id=1").fetchone()
        source_id = row[0] if row else None
    if not isinstance(source_id, str) or not source_id.strip() or len(source_id) > 256:
        raise MigrationError("Cliporax has no usable device ID. Supply a stable --source-id for this library.")
    # The Windows namespace matches the previous one-time migration.
    return uuid5(NAMESPACE_URL, f"uclip:cliporax-{platform}:{source_id}")


def identity(library, kind, legacy_id):
    return str(uuid5(library, f"{kind}:{legacy_id}"))


def timestamp(value):
    if value is None:
        raise SkipRecord("invalid_timestamp")
    try:
        value = value.decode("utf-8")
        if value.endswith("Z"):
            value = value[:-1] + "+00:00"
        date = datetime.fromisoformat(value)
        if date.tzinfo is None:
            date = date.replace(tzinfo=timezone.utc)
        delta = date - EPOCH
        milliseconds = (delta.days * 86400 + delta.seconds) * 1000 + delta.microseconds // 1000
        if milliseconds >= 0:
            return milliseconds
    except (OverflowError, ValueError):
        pass
    raise SkipRecord("invalid_timestamp")


def file_reference(value, windows):
    if value.startswith("file://"):
        uri = urlsplit(value)
        if uri.query or uri.fragment:
            raise ValueError()
        value = unquote(uri.path, errors="strict")
        if uri.netloc and uri.netloc.lower() != "localhost":
            if not windows:
                raise ValueError()
            value = f"//{uri.netloc}{value}"
        elif windows and len(value) >= 3 and value[0] == "/" and value[2] == ":":
            value = value[1:]
    path_type = PureWindowsPath if windows else PurePosixPath
    if "\0" in value or len(value.encode()) > 32768 or not path_type(value).is_absolute():
        raise ValueError()
    return value


def file_list(content, source_platform, destination_platform):
    windows = source_platform == "windows"
    if windows != (destination_platform == "windows"):
        raise SkipRecord("incompatible_file_platform")
    try:
        if content.lstrip().startswith("["):
            paths = json.loads(content)
        else:
            paths = content.split("\n")
            if windows:
                paths = [path.removesuffix("\r") for path in paths]
        if not isinstance(paths, list) or not 0 < len(paths) <= 256:
            raise ValueError()
        if not all(isinstance(path, str) for path in paths):
            raise ValueError()
        return [file_reference(path, windows) for path in paths]
    except (ValueError, RecursionError):
        raise SkipRecord("invalid_file_references") from None


def payload(row, source_platform, destination_platform):
    kind = row["type"]
    if kind not in CONTENT_LIMITS:
        raise SkipRecord("unsupported_type")
    if row["content"] is None:
        raise SkipRecord("missing_or_oversized_content")
    try:
        content = row["content"].decode("utf-8")
    except UnicodeError:
        raise SkipRecord("invalid_content_encoding") from None
    if kind == "file":
        return "files", file_list(content, source_platform, destination_platform)
    if kind == "image":
        header, _, encoded = content.partition(",")
        if header != "data:image/png;base64" or not encoded:
            raise SkipRecord("unsupported_image_format")
        # The CLI validates the base64, dimensions, and decoded PNG.
        return kind, encoded + "=" * (-len(encoded) % 4)
    if not content:
        raise SkipRecord("empty_text")
    return kind, content


def source_app(metadata):
    if metadata:
        try:
            metadata = json.loads(metadata)
            if isinstance(metadata, dict) and isinstance(metadata.get("source_app"), str):
                name = metadata["source_app"]
                name.encode("utf-8")
                return name
        except (ValueError, RecursionError):
            pass
    return None


def frames(source, source_platform, destination_platform, source_id, skipped):
    if source.execute("PRAGMA application_id").fetchone()[0] != 0:
        raise MigrationError("Expected a Cliporax database.")
    if source.execute("PRAGMA quick_check(1)").fetchone()[0] != "ok":
        raise MigrationError("Cliporax database failed the SQLite integrity check.")
    tab_columns = columns(source, "tabs")
    item_columns = columns(source, "clipboard_items")
    if not ({"id", "name", "is_default"} <= tab_columns and
            {"id", "type", "content", "tab_id", "is_sensitive", "created_at"} <= item_columns):
        raise MigrationError("Unrecognized Cliporax schema or missing sensitivity markers.")

    library = namespace(source, source_platform, source_id)
    tabs = {}
    trash = set()
    tab_order = [name for name in ("display_order", "created_at", "id") if name in tab_columns]
    trash_flag = "is_trash" if "is_trash" in tab_columns else "0"
    yield {"type": "header", "version": 1}
    for tab in source.execute(f"SELECT id,name,is_default,{trash_flag} AS in_trash FROM tabs ORDER BY {','.join(tab_order)}"):
        tab_id = tab["id"]
        if not isinstance(tab_id, int):
            raise MigrationError("Cliporax contains an invalid tab ID.")
        if tab["in_trash"]:
            trash.add(tab_id)
        if tab["is_default"] or tab["in_trash"]:
            tabs[tab_id] = None
        else:
            tabs[tab_id] = identity(library, "tab", tab_id)
            yield {"type": "collection", "id": tabs[tab_id], "name": tab["name"]}

    order = []
    if "display_order" in item_columns:
        order.append("COALESCE(display_order,0) DESC")
    order.extend(("updated_at" if "updated_at" in item_columns else "created_at", "id"))
    optional = ", ".join(name if name in item_columns else f"NULL AS {name}" for name in
                         ("deleted_at", "deleted_from_tab_id", "is_pinned"))
    metadata = ("CASE WHEN length(CAST(metadata AS BLOB)) <= 65536 THEN CAST(metadata AS BLOB) END"
                if "metadata" in item_columns else "NULL")
    limits = "CASE type " + " ".join(f"WHEN '{kind}' THEN {limit}" for kind, limit in CONTENT_LIMITS.items()) + " END"
    item_query = f"""
        SELECT id,type,tab_id,is_sensitive,{optional},
               CASE WHEN typeof(created_at)='text' THEN CAST(created_at AS BLOB) END AS created_at,
               CASE WHEN is_sensitive=0 THEN {metadata} END AS metadata,
               CASE WHEN is_sensitive=0 AND typeof(content)='text'
                         AND length(CAST(content AS BLOB)) <= ({limits})
                    THEN CAST(content AS BLOB) END AS content
        FROM clipboard_items WHERE id=?
    """
    # The sort walks row IDs only, avoiding a temporary plaintext payload store.
    for (legacy_id,) in source.execute(f"SELECT id FROM clipboard_items ORDER BY {','.join(order)}"):
        if not isinstance(legacy_id, int):
            raise MigrationError("Cliporax contains an invalid record ID.")
        row = source.execute(item_query, (legacy_id,)).fetchone()
        if row["is_sensitive"] != 0:
            skipped["marked_sensitive"] += 1
            continue
        deleted = row["tab_id"] in trash or row["deleted_at"] is not None
        old_tab = row["deleted_from_tab_id"] if deleted else row["tab_id"]
        try:
            if not deleted and old_tab is not None and old_tab not in tabs:
                raise SkipRecord("unknown_collection")
            captured_at = timestamp(row["created_at"])
            kind, content = payload(row, source_platform, destination_platform)
        except SkipRecord as error:
            skipped[str(error)] += 1
            print(f"Cliporax: skipped record {legacy_id} ({error})", file=sys.stderr)
            continue
        yield {
            "type": "clip", "id": identity(library, "clip", legacy_id), "kind": kind, "content": content,
            "collection": tabs.get(old_tab), "captured_at": captured_at, "deleted": deleted,
            "pinned": bool(row["is_pinned"]) and not deleted, "source_app": source_app(row["metadata"]),
        }
    yield {"type": "end"}


def run(args):
    destination_platform = host_platform()
    source_platform = args.source_platform or destination_platform
    if args.source is None and source_platform != destination_platform:
        raise MigrationError("Specify --source when migrating a library from another platform.")
    path = (args.source or default_source(destination_platform)).expanduser().resolve()
    if path.is_dir():
        path /= "cliporax.db"
    if not path.is_file():
        raise MigrationError(f"Cliporax database not found at {path}. Use --source to select a library.")

    cmd = [args.uclip]
    if args.target_dir is not None:
        cmd += ["--data-dir", str(args.target_dir.expanduser().resolve())]
    elif args.dev:
        cmd.append("--dev")
    elif args.portable:
        cmd.append("--portable")
    cmd += ["import", "--skip-invalid"]
    if args.apply:
        cmd.append("--apply")
    skipped = Counter()
    with closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)) as source:
        source.row_factory = sqlite3.Row
        source.execute("PRAGMA trusted_schema=OFF")
        source.execute("PRAGMA query_only=ON")
        source.execute("BEGIN")
        records = frames(source, source_platform, destination_platform, args.source_id, skipped)
        header = next(records)
        with subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE) as child:
            try:
                with child.stdin:
                    for frame in chain((header,), records):
                        child.stdin.write(json.dumps(frame, ensure_ascii=False).encode("utf-8") + b"\n")
            except BrokenPipeError:
                pass
            output = child.stdout.read()
            status = child.wait()
        if status:
            return status
    report = json.loads(output)
    report["legacy_skipped"] = dict(skipped)
    print(json.dumps(report, indent=2, ensure_ascii=True))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, help="Cliporax database or data directory")
    parser.add_argument("--source-platform", choices=("windows", "macos", "linux"),
                        help="Platform that created the source (defaults to the current platform)")
    parser.add_argument("--source-id", help="Stable source identity for a database without a Cliporax device ID")
    target = parser.add_mutually_exclusive_group()
    target.add_argument("--target-dir", type=Path, help="Directory containing the UClip foundation.sqlite3")
    target.add_argument("--dev", action="store_true", help="Use the UClip development library")
    target.add_argument("--portable", action="store_true", help="Use the UClip portable CLI library")
    parser.add_argument("--uclip", default="uclip", help="Path to the UClip CLI executable")
    parser.add_argument("--apply", action="store_true", help="Back up the UClip library and commit the import")
    args = parser.parse_args()
    try:
        return run(args)
    except MigrationError as error:
        print(f"Cliporax: {error}", file=sys.stderr)
    except sqlite3.Error:
        print("Cliporax: SQLite read failed. Quit Cliporax or select a complete SQLite backup.", file=sys.stderr)
    except FileNotFoundError:
        print("Cliporax: current UClip CLI not found. Set --uclip to its executable.", file=sys.stderr)
    except (OSError, UnicodeError, json.JSONDecodeError):
        print("Cliporax: failed to read the source or communicate with the UClip CLI.", file=sys.stderr)
    except KeyboardInterrupt:
        return 130
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
