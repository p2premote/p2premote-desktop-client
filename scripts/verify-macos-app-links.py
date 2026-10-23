"""Reject nested macOS apps whose framework symlinks have been expanded."""
import argparse
import os
from pathlib import Path


def verify_links(source: Path, destination: Path) -> int:
    source = source.resolve(strict=True)
    destination = destination.resolve(strict=True)
    # These links must exist even when a caller supplies an already merged app.
    for name in ("App", "FlutterMacOS"):
        framework = source / "Contents" / "Frameworks" / f"{name}.framework"
        for link in (framework / name, framework / "Versions" / "Current"):
            if not link.is_symlink():
                raise ValueError(f"Expected framework symlink: {link}")

    count = 0
    for directory, folders, files in os.walk(source, followlinks=False):
        for name in folders + files:
            original = Path(directory) / name
            if not original.is_symlink():
                continue
            relative = original.relative_to(source)
            copied = destination / relative
            target = os.readlink(original)
            if not copied.is_symlink() or os.readlink(copied) != target:
                raise ValueError(f"Symlink changed or expanded into a copy: {copied}")
            for link, root in ((original, source), (copied, destination)):
                resolved = link.resolve(strict=True)
                if not resolved.is_relative_to(root):
                    raise ValueError(f"Symlink escapes application bundle: {link}")
            count += 1
    return count


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    print(f"Verified {verify_links(args.source, args.destination)} preserved application symlinks")
