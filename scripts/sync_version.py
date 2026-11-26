#!/usr/bin/env python3
"""Propagate the single source of truth for the version to every derived file.

Source: [workspace.package].version in the root Cargo.toml.

Usage:
    python scripts/sync_version.py            # rewrite derived files
    python scripts/sync_version.py --check    # exit 1 on drift (CI)
    python scripts/sync_version.py --print    # print the workspace version

Derived files:
  package.json, apps/desktop/package.json   "version"
  custom_components/ctrlmate/manifest.json  "version"
  packaging/linux/PKGBUILD                  pkgver=
Not derived (read the workspace version directly, nothing to sync):
  tauri.conf.json (no "version": Tauri falls back to the Cargo package version),
  flake.nix (builtins.fromTOML), pack-velopack.ps1 / pack-linux.sh (cargo metadata).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r"\[workspace\.package\](.*?)(?:\n\[|\Z)", text, re.S)
    if not m:
        sys.exit("error: [workspace.package] not found in Cargo.toml")
    v = re.search(r'^\s*version\s*=\s*"([^"]+)"', m.group(1), re.M)
    if not v or not SEMVER.match(v.group(1)):
        sys.exit("error: workspace version missing or not semver")
    return v.group(1)


def json_target(rel: str):
    def read(text: str) -> str | None:
        m = re.search(r'^(\s*"version"\s*:\s*")([^"]*)(")', text, re.M)
        return m.group(2) if m else None

    def write(text: str, version: str) -> str:
        return re.sub(
            r'^(\s*"version"\s*:\s*")([^"]*)(")',
            lambda m: m.group(1) + version + m.group(3),
            text,
            count=1,
            flags=re.M,
        )

    return rel, read, write


def pkgbuild_target():
    def read(text: str) -> str | None:
        m = re.search(r"^pkgver=(\S+)", text, re.M)
        return m.group(1) if m else None

    def write(text: str, version: str) -> str:
        return re.sub(r"^pkgver=\S+", "pkgver=" + version, text, count=1, flags=re.M)

    return "packaging/linux/PKGBUILD", read, write


TARGETS = [
    json_target("package.json"),
    json_target("apps/desktop/package.json"),
    json_target("custom_components/ctrlmate/manifest.json"),
    pkgbuild_target(),
]


def main() -> int:
    args = set(sys.argv[1:])
    version = workspace_version()
    if "--print" in args:
        print(version)
        return 0
    check = "--check" in args
    drift = []
    for rel, read, write in TARGETS:
        path = ROOT / rel
        if not path.exists():
            continue
        raw = path.read_bytes().decode("utf-8")
        current = read(raw)
        if current == version:
            continue
        drift.append(f"{rel}: {current!r} != {version!r}")
        if not check:
            path.write_bytes(write(raw, version).encode("utf-8"))
    # tauri.conf.json must NOT carry its own version (would drift).
    tauri = ROOT / "apps/desktop/src-tauri/tauri.conf.json"
    if tauri.exists() and '"version"' in tauri.read_text(encoding="utf-8"):
        drift.append('apps/desktop/src-tauri/tauri.conf.json: remove the duplicate "version" field')
    if check:
        if drift:
            print("Version drift detected (run: python scripts/sync_version.py):")
            print("\n".join("  " + d for d in drift))
            return 1
        print(f"OK: all files at {version}")
        return 0
    print(f"Workspace version {version}; updated {len(drift)} file(s)")
    for d in drift:
        print("  " + d)
    return 0


if __name__ == "__main__":
    sys.exit(main())
