#!/usr/bin/env python3
"""DEBUG: compares an ATCM dump with what the ELF loads there.
usage: tcmcmp.py ATCM.bin ELF"""
import struct, sys

tcm = open(sys.argv[1], "rb").read()
elf = open(sys.argv[2], "rb").read()
phoff, = struct.unpack_from("<I", elf, 28)
phentsize, phnum = struct.unpack_from("<HH", elf, 42)
diffs = []
for i in range(phnum):
    t, off, va, pa, fsz, msz, fl, al = struct.unpack_from("<8I", elf, phoff + i * phentsize)
    if t != 1 or pa >= len(tcm) or fsz == 0:
        continue
    want = elf[off:off + fsz]
    got = tcm[pa:pa + fsz]
    print(f"segment pa={pa:#07x} filesz={fsz:#x}: {'same' if want == got else 'DIFFERS'}")
    run = None
    for j in range(min(len(want), len(got))):
        if want[j] != got[j]:
            if run and j == run[1]:
                run[1] = j + 1
            else:
                run = [j, j + 1]
                diffs.append((pa, run, want, got))
for pa, (a, b), want, got in diffs[:25]:
    print(f"  {pa + a:#07x}..{pa + b:#07x} ({b - a} bytes) "
          f"want {want[a:min(b, a + 16)].hex()} got {got[a:min(b, a + 16)].hex()}")
print(f"{len(diffs)} differing ranges")
