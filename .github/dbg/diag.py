#!/usr/bin/env python3
"""DEBUG: watches the console; on a failure line or a stall, dumps guest
state through the QEMU monitor. usage: diag.py CONSOLE MONITOR_SOCKET OUT"""
import os, re, socket, sys, time

console, sockpath, outpath = sys.argv[1:4]
out = open(outpath, "w", buffering=1)


def mon(cmds, timeout=5):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(sockpath)
    buf = b""

    def until_prompt():
        nonlocal buf
        while not buf.endswith(b"(qemu) "):
            d = s.recv(65536)
            if not d:
                break
            buf += d
        r, buf = buf, b""
        return r.decode(errors="replace")

    until_prompt()
    res = []
    for c in cmds:
        s.sendall(c.encode() + b"\n")
        res.append((c, until_prompt()))
    s.close()
    return res


KEEP = re.compile(r"CPU#|PC=|R\d\d=|PSR=|^[0-9a-f]{16}:")


def dump(why):
    out.write(f"=== diag: {why}\n")
    try:
        # R5_0's ATCM and the table + claim word, as the A53s see them.
        atcm = os.path.abspath("atcm.bin")
        for c, r in mon(["stop", f"pmemsave 0xffe00000 0x10000 {atcm}",
                         "xp /8wx 0x3ed00000", "xp /24wx 0x3ed00020",
                         "cont"]):
            if c.startswith("xp"):
                out.write(c + " -> " + " | ".join(l.strip() for l in r.splitlines()[1:] if KEEP.search(l)) + "\n")
        for rnd in range(3):
            res = mon(["stop", "info registers -a",
                       "xp /8wx 0xff310000", "xp /8wx 0xff300000",
                       "xp /2hx 0x3ed41000", "xp /2hx 0x3ed42000",
                       "xp /2hx 0x3ed45000", "xp /2hx 0x3ed46000", "cont"])
            out.write(f"--- sample {rnd}\n")
            cpu = -1
            for c, r in res:
                if c.startswith("xp"):
                    out.write(c + " -> " + " | ".join(l.strip() for l in r.splitlines()[1:] if KEEP.search(l)) + "\n")
                    continue
                for l in r.replace("\r", "").splitlines():
                    m = re.match(r"\s*CPU#(\d+)", l)
                    if m:
                        cpu = int(m.group(1))
                    if cpu >= 4 and KEEP.search(l) or re.search(r"CPU#|PC=", l):
                        out.write(l.rstrip() + "\n")
            time.sleep(0.3)
    except Exception as e:  # monitor dead: QEMU's main loop is stuck
        out.write(f"monitor: {e!r}\n")


last_size, last_change = 0, time.time()
dumped = set()
while True:
    try:
        data = open(console, "rb").read()
    except FileNotFoundError:
        data = b""
    if len(data) != last_size:
        last_size, last_change = len(data), time.time()
    if b"[init] FAIL" in data and "fail" not in dumped:
        dumped.add("fail")
        dump(data[data.index(b"[init] FAIL"):].split(b"\n")[0].decode(errors="replace"))
    if b"[init] RustAmp" in data and b"reboot: Power down" not in data \
            and time.time() - last_change > 12 and "stall" not in dumped \
            and b"did not appear" not in data:
        dumped.add("stall")
        dump("console silent for 12 s")
    if b"reboot: Power down" in data or b"run.sh: no power-off" in data:
        break
    time.sleep(0.05)
