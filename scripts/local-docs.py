#!/usr/bin/env python3
"""Edit selected local documents using the existing SSH backup receiver."""

import argparse
import difflib
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import shlex
import subprocess
import sys
import tarfile
import tempfile

DOC_DIRS = {"docs", "doc", "plans", "research", "roadmap", "roadmaps", "design-system"}
EXCLUDED_DIRS = {".git", "node_modules", "target", "dist"}
FILE_LIMIT = 32 * 1024 * 1024
TOTAL_LIMIT = 500 * 1024 * 1024
FILE_COUNT_LIMIT = 4000

REMOTE_SNAPSHOT = """
import fcntl, json, pathlib, sys, tarfile
root = pathlib.Path(sys.argv[1])
latest = root / 'latest'
with (root / '.sync.lock').open('rb') as lock:
    fcntl.flock(lock, fcntl.LOCK_SH)
    if not latest.is_dir() or latest.resolve() != latest:
        raise RuntimeError('Invalid snapshot directory')
    with tarfile.open(fileobj=sys.stdout.buffer, mode='w|gz') as archive:
        total = 0
        for rel in json.loads(sys.argv[2]):
            path = latest / rel
            if path.resolve() != path or path.exists() and not path.is_file():
                raise RuntimeError('Not a regular document: ' + rel)
            if not path.is_file():
                continue
            size = path.stat().st_size
            total += size
            if size > 32 * 1024 * 1024 or total > 500 * 1024 * 1024:
                raise RuntimeError('Snapshot exceeds document size limit')
            archive.add(path, arcname=rel, recursive=False)
"""

# Run the installed receiver with two checks inside its existing exclusive lock.
# Its source and Windows invocation stay untouched.
REMOTE_SYNC = r"""
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
expected = json.loads(sys.argv[2])

def digest(latest, rel):
    path = latest / rel
    if path.resolve() != path or path.exists() and not path.is_file():
        raise RuntimeError('Not a regular document: ' + rel)
    if not path.is_file():
        return None
    with path.open('rb') as stream:
        data = stream.read(32 * 1024 * 1024 + 1)
    if len(data) > 32 * 1024 * 1024:
        raise RuntimeError('Remote document exceeds size limit: ' + rel)
    return hashlib.sha256(data).hexdigest()

def check_before(latest, manifest):
    if latest != root / 'latest' or manifest['files'].keys() != expected.keys():
        raise RuntimeError('Receiver destination or file list differs')
    for rel, wanted in expected.items():
        if digest(latest, rel) not in (wanted, manifest['files'][rel]):
            raise RuntimeError('Remote changed since snapshot: ' + rel)

def verify_readback(latest, manifest):
    for rel, wanted in manifest['files'].items():
        if digest(latest, rel) != wanted:
            raise RuntimeError('Remote read-back hash differs: ' + rel)

receiver = root / 'receive.py'
source = receiver.read_text()
for anchor, position, hook in [
    ('  latest=ROOT/"latest"\n', 'after', '  check_before(latest, manifest)\n'),
    ('  retained=sorted(', 'before', '  verify_readback(latest, manifest)\n'),
]:
    if source.count(anchor) != 1:
        raise RuntimeError('Unsupported backup receiver version')
    source = source.replace(anchor, anchor + hook if position == 'after' else hook + anchor)
exec(compile(source, str(receiver), 'exec'), {
    '__name__': '__main__',
    'check_before': check_before,
    'verify_readback': verify_readback,
})
"""


def digest(data):
    return hashlib.sha256(data).hexdigest() if data is not None else None


def document_names(root, names):
    if not names or len(names) > FILE_COUNT_LIMIT or len(set(names)) != len(names):
        raise ValueError("Select unique document paths within the file count limit")
    for name in names:
        path = PurePosixPath(name)
        if (not path.parts or name != path.as_posix() or path.is_absolute()
                or ".." in path.parts or set(path.parts) & EXCLUDED_DIRS
                or re.search(r"[\\:\x00-\x1f\x7f]", name)
                or (path.parts[0] not in DOC_DIRS and path.suffix.lower() != ".md")):
            raise ValueError(f"Expected a repository-relative document path: {name}")
    tracked = subprocess.check_output(
        ["git", "--literal-pathspecs", "ls-files", "-z", "--", *names], cwd=root
    )
    if tracked:
        first = tracked.split(b"\0", 1)[0].decode("utf-8", errors="replace")
        raise ValueError(f"Git-tracked document must use Git: {first}")
    return names


def documents(root, names):
    total = 0
    for rel in names:
        path = root / rel
        if path.resolve() != path or path.exists() and not path.is_file():
            raise ValueError(f"Not a regular document: {rel}")
        data = None
        if path.is_file():
            with path.open("rb") as source:
                data = source.read(FILE_LIMIT + 1)
            total += len(data)
            if len(data) > FILE_LIMIT or total > TOTAL_LIMIT:
                raise ValueError(f"Document selection exceeds size limit at {rel}")
        yield rel, data


def ssh_call(remote, code, data, **options):
    host, separator, root = remote.partition(":")
    if (not separator or not re.fullmatch(r"[\w.@-]+", host) or host.startswith("-")
            or not root.startswith("/") or PurePosixPath(root).as_posix() != root
            or ".." in PurePosixPath(root).parts or root == "/"):
        raise ValueError("Use a backup root in the form user@host:/absolute/path")
    command = shlex.join(["python3", "-c", code, root, json.dumps(data)])
    result = subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
         "-o", "ConnectTimeout=5", host, command],
        stderr=subprocess.PIPE, **options,
    )
    if result.returncode:
        raise ValueError(result.stderr.decode("utf-8", errors="replace").strip()
                         or f"SSH exited with status {result.returncode}")
    return result.stdout


def save(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def snapshot(root, remote, names):
    directory = Path(tempfile.mkdtemp(prefix="uclip-local-docs-")).resolve()
    if directory.is_relative_to(root):
        raise ValueError("The snapshot directory must be outside the repository")
    print(f"Snapshot: {directory}", flush=True)
    local = dict(documents(root, names))
    for rel, data in local.items():
        if data is not None:
            save(directory / "local" / rel, data)
    with tempfile.TemporaryFile() as stream:
        ssh_call(remote, REMOTE_SNAPSHOT, names, stdout=stream)
        stream.seek(0)
        with tarfile.open(fileobj=stream, mode="r|gz") as archive:
            seen = set()
            total = 0
            for member in archive:
                rel = member.name
                total += member.size
                if (rel not in local or rel in seen or not member.isfile()
                        or member.size > FILE_LIMIT or total > TOTAL_LIMIT):
                    raise ValueError(f"Unexpected snapshot entry: {rel}")
                seen.add(rel)
                save(directory / "remote" / rel, archive.extractfile(member).read())
    before = {rel: digest(data) for rel, data in documents(directory / "remote", names)}
    # Missing remote files are meaningful and remain in the snapshot manifest.
    for rel, data in local.items():
        local_hash = digest(data)
        remote_hash = before[rel]
        if local_hash == remote_hash:
            state = "same" if local_hash is not None else "missing on both"
        elif remote_hash is None:
            state = "local only"
        elif local_hash is None:
            state = "remote only"
        else:
            state = "different"
            if PurePosixPath(rel).suffix.lower() == ".md":
                previous = (directory / "remote" / rel).read_text(encoding="utf-8", errors="replace")
                current = data.decode("utf-8", errors="replace")
                diff = difflib.unified_diff(previous.splitlines(True), current.splitlines(True),
                                            "remote/" + rel, "local/" + rel)
                save(directory / "diff" / (rel + ".diff"), "".join(diff).encode("utf-8"))
        print(f"{state}: {rel}")
    metadata = {"root": str(root), "remote": remote, "before": before,
                "local": {rel: digest(data) for rel, data in local.items()}}
    save(directory / "snapshot.json", json.dumps(metadata).encode("utf-8"))


def sync(root, directory, resolved):
    directory = directory.resolve()
    if directory.is_relative_to(root):
        raise ValueError("The snapshot directory must be outside the repository")
    metadata = json.loads((directory / "snapshot.json").read_text(encoding="utf-8"))
    if metadata["root"] != str(root):
        raise ValueError("Snapshot belongs to a different checkout")
    before = metadata["before"]
    names = document_names(root, list(before))
    if set(resolved) - set(names):
        raise ValueError("--resolved must name a document in the snapshot")
    data = dict(documents(root, names))
    hashes = {rel: digest(value) for rel, value in data.items()}
    for rel, current in hashes.items():
        if current is None:
            raise ValueError(f"Document is missing locally: {rel}")
        if (before[rel] is not None and current != before[rel]
                and metadata["local"][rel] != before[rel] and rel not in resolved):
            raise ValueError(f"Compare snapshot copies of {rel}, then pass --resolved {rel}")
    manifest = json.dumps({"files": hashes, "source": "UClip selected local documents"}).encode("utf-8")
    with tempfile.TemporaryFile() as stream:
        with tarfile.open(fileobj=stream, mode="w:gz") as archive:
            entries = {"manifest.json": manifest}
            entries.update({"files/" + rel: value for rel, value in data.items()})
            for name, value in entries.items():
                info = tarfile.TarInfo(name)
                info.size = len(value)
                archive.addfile(info, io.BytesIO(value))
        stream.seek(0)
        output = ssh_call(metadata["remote"], REMOTE_SYNC, before, stdin=stream, stdout=subprocess.PIPE)
    receipt = json.loads(output)
    if not receipt.get("ok") or receipt.get("verified_files") != len(names):
        raise ValueError("Backup receiver returned an unexpected receipt")
    save(directory / "receipt.json", output)
    for rel, value in documents(root, names):
        if digest(value) != hashes[rel]:
            raise ValueError(f"Local file changed during upload: {rel}")
    print(f"Updated {receipt['updated_files']} file(s). Read-back SHA-256 verified for {len(names)}.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    capture = commands.add_parser("snapshot", help="Save and compare the selected documents")
    capture.add_argument("--remote", required=True, help="Backup root, without the /latest suffix")
    capture.add_argument("paths", nargs="+", help="Repository-relative document paths")
    publish = commands.add_parser("sync", help="Upload the selected documents and verify the result")
    publish.add_argument("snapshot", type=Path)
    publish.add_argument("--resolved", action="append", default=[], metavar="PATH",
                         help="A pre-existing difference that has been reviewed or merged")
    args = parser.parse_args()
    try:
        root = Path(subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], text=True
        ).strip()).resolve()
        if args.command == "snapshot":
            snapshot(root, args.remote, document_names(root, args.paths))
        else:
            sync(root, args.snapshot, args.resolved)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError, tarfile.TarError) as error:
        print(f"Local document sync: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
