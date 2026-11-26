#!/usr/bin/env python3
"""Fail if packaging/linux/99-ctrlmate.rules drifts from the VID:PID lists in the Rust sources.

Sources of truth:
  crates/ctrlmate-hal/src/kraken.rs               NZXT_VID, KRAKEN_PIDS
  crates/ctrlmate-rgb/src/backends/nzxt_rgb.rs    NZXT_VID, NZXT_RGB_PIDS
  crates/ctrlmate-rgb/src/backends/gigabyte_ite.rs GIGABYTE_ITE_VID, GIGABYTE_ITE_PIDS
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def src(rel):
    return (ROOT / rel).read_text(encoding="utf-8")


def vid(text, name):
    return int(re.search(rf"const {name}: u16 = 0x([0-9A-Fa-f]{{4}})", text).group(1), 16)


def block(text, name):
    m = re.search(rf"const {name}[^=]*=\s*&\[(.*?)\];", text, re.S)
    return m.group(1)


expected = set()
k = src("crates/ctrlmate-hal/src/kraken.rs")
for p in re.findall(r"\(\s*0x([0-9A-Fa-f]{4})\s*,", block(k, "KRAKEN_PIDS")):
    expected.add((vid(k, "NZXT_VID"), int(p, 16)))
n = src("crates/ctrlmate-rgb/src/backends/nzxt_rgb.rs")
for p in re.findall(r"\(\s*0x([0-9A-Fa-f]{4})\s*,", block(n, "NZXT_RGB_PIDS")):
    expected.add((vid(n, "NZXT_VID"), int(p, 16)))
g = src("crates/ctrlmate-rgb/src/backends/gigabyte_ite.rs")
for p in re.findall(r"0x([0-9A-Fa-f]{4})", block(g, "GIGABYTE_ITE_PIDS")):
    expected.add((vid(g, "GIGABYTE_ITE_VID"), int(p, 16)))

rules = (ROOT / "packaging/linux/99-ctrlmate.rules").read_text(encoding="utf-8")
actual = set()
for line in rules.splitlines():
    if line.lstrip().startswith("#"):
        continue
    m = re.search(r'idVendor\}=="([0-9a-f]{4})".*idProduct\}=="([0-9a-f]{4})"', line)
    if m:
        actual.add((int(m.group(1), 16), int(m.group(2), 16)))
    elif "idVendor" in line:
        print(f"error: rule without exact idProduct: {line}")
        sys.exit(1)

fmt = lambda s: ", ".join(f"{v:04x}:{p:04x}" for v, p in sorted(s))
ok = True
if expected - actual:
    print("missing from udev rules:", fmt(expected - actual))
    ok = False
if actual - expected:
    print("in udev rules but not supported by the code:", fmt(actual - expected))
    ok = False
print(f"{len(expected)} supported VID:PID pairs" + (", udev rules in sync" if ok else ""))
sys.exit(0 if ok else 1)
