#!/usr/bin/env python3
# SPDX-License-Identifier: BSD-3-Clause
"""Packs /init and the M7 firmware into a gzipped newc initramfs.

usage: mkinitramfs.py INIT FIRMWARE.elf OUT.cpio.gz
"""
import gzip
import os
import sys


def entry(name, mode, data=b"", rdev=(0, 0)):
    name_b = name.encode() + b"\0"
    hdr = "070701" + "".join(
        f"{v:08x}"
        for v in (
            entry.ino, mode, 0, 0, 1, 0, len(data), 0, 0, rdev[0], rdev[1],
            len(name_b), 0,
        )
    )
    entry.ino += 1
    out = hdr.encode() + name_b
    out += b"\0" * (-len(out) % 4)
    out += data + b"\0" * (-len(data) % 4)
    return out


entry.ino = 1


def main():
    init, fw, dst = sys.argv[1:4]
    blob = b""
    for d in ("dev", "sys", "proc", "lib", "lib/firmware"):
        blob += entry(d, 0o040755)
    blob += entry("dev/console", 0o020600, rdev=(5, 1))
    blob += entry("init", 0o100755, open(init, "rb").read())
    blob += entry("lib/firmware/" + os.path.basename(fw), 0o100644, open(fw, "rb").read())
    blob += entry("TRAILER!!!", 0)
    with gzip.open(dst, "wb") as f:
        f.write(blob)


if __name__ == "__main__":
    main()
