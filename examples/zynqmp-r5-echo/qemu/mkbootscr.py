#!/usr/bin/env python3
# SPDX-License-Identifier: BSD-3-Clause
"""Writes a U-Boot legacy script image (what `mkimage -T script` makes).

usage: mkbootscr.py SCRIPT_TEXT_FILE OUT
"""
import struct
import sys
import zlib

IH_MAGIC = 0x27051956
IH_OS_LINUX, IH_ARCH_ARM64, IH_TYPE_SCRIPT, IH_COMP_NONE = 5, 22, 6, 0


def main():
    text = open(sys.argv[1], "rb").read()
    # Script images are multi-file: a zero-terminated size list, then data.
    data = struct.pack(">II", len(text), 0) + text
    hdr = struct.pack(
        ">IIIIIIIBBBB32s",
        IH_MAGIC, 0, 0, len(data), 0, 0, zlib.crc32(data),
        IH_OS_LINUX, IH_ARCH_ARM64, IH_TYPE_SCRIPT, IH_COMP_NONE,
        b"rustamp boot script",
    )
    hdr = hdr[:4] + struct.pack(">I", zlib.crc32(hdr)) + hdr[8:]
    open(sys.argv[2], "wb").write(hdr + data)


if __name__ == "__main__":
    main()
