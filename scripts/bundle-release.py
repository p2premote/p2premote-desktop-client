"""Bundle compiled release assets for download and admin ZIP import."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile


def bundle(source, output, version):
    if not re.fullmatch(r"v?\d+\.\d+\.\d+", version):
        raise ValueError("version must be X.Y.Z or vX.Y.Z")
    files = sorted(p for p in source.rglob("*") if p.is_file() and p.resolve() != output.resolve())
    if not files:
        raise ValueError("no compiled assets found")
    names = [p.name.lower() for p in files]
    if len(names) != len(set(names)):
        raise ValueError("duplicate asset filenames")
    manifest = {"schema_version": 1, "version": version.removeprefix("v"), "files": []}
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for path in files:
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            manifest["files"].append({"name": path.name, "size": path.stat().st_size, "sha256": digest.hexdigest()})
            archive.write(path, path.name)
        archive.writestr("manifest.json", json.dumps(manifest, indent=2) + "\n")
    print(f"Created {output}: {len(files)} assets")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    bundle(args.source, args.output, args.version)
