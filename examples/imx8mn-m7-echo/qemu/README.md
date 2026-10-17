# Running the example on QEMU (imx8mp-evk + Cortex-M7)

The firmware runs on the emulated Cortex-M7 of QEMU's `imx8mp-evk` machine,
driven by mainline Linux `imx_rproc` on the emulated Cortex-A53s, with the
[Cortex-M7 AMP boot series](https://patchew.org/QEMU/20260706162859.3361063-1-gaurav.sharma._5F7@nxp.com/)
(v2) applied.

```sh
./reproduce.sh            # builds QEMU, Linux, firmware and init, then runs
```

## What the test does

1. Linux 7.2 boots with `imx8mp-evk-rpmsg-qemu.dts`.
2. `/init` (`init.c`) writes `rpmsg-echo.elf` to remoteproc0's `firmware`,
   then `start` to its `state`.
3. `imx_rproc` loads the ELF and parses its `.resource_table`. It then
   releases the M7 through GPR `CPUWAIT`, and the M7 boots from `0x80000000`.
4. The firmware waits for `DRIVER_OK` and announces `rpmsg-tty`. Linux's
   `rpmsg_tty` then creates `/dev/ttyRPMSG0`.
5. `/init` writes 2,000 messages of 1–256 bytes and checks each echo byte for
   byte, then powers off.

## Result (17 Oct 2026)

```text
[    1.712985] remoteproc remoteproc0: Booting fw image rpmsg-echo.elf, size 368628
[    1.762183] virtio_rpmsg_bus virtio0: creating channel rpmsg-tty addr 0x400
[    1.762445] virtio_rpmsg_bus virtio0: rpmsg host is online
[    1.762767] remoteproc remoteproc0: remote processor imx8mp-cm7 is now up
[init] /dev/ttyRPMSG0 is up after 100 ms
[init] PASS: 2000 round trips, 256712 bytes echoed by the openamp-rs firmware
```

M7 (UART4):

```text
openamp-rs m7: up, waiting for the host (DRIVER_OK)
openamp-rs m7: DRIVER_OK, ns=true
openamp-rs m7: announced rpmsg-tty at 0x400
openamp-rs m7: first message from 0x400
openamp-rs m7: echoed 2000 messages, 256712 bytes
```

## How the QEMU variant differs from the hardware build

`--features qemu-imx8mp` changes three things:

| | Hardware (`tcm.x`) | QEMU (`qemu-ddr.x`) |
|---|---|---|
| Code, data | ITCM `0x0`, DTCM `0x20000000` | DDR `0x80000000` (v2 has no TCM model, and the M7 boots from `0x80000000`) |
| Resource table | `0x2001F000` | `0x80F00000` |
| Host kicks | MU interrupt, `wfi` | Polls MU1-B (v2 does not wire MU1-B to the M7) |
| Debug output | none | UART4 (`-serial file:m7-uart4.log`) |

The rpmsg, virtio and remoteproc code is the same in both builds.

## Findings about the QEMU series

* **The MU model in v2 breaks Linux's mailbox.** `imx8mp_mu_read`/`_write`
  compare the byte offset against register *indices* (`MU_RR0 = 4`,
  `MU_SR = 8`, `MU_CR = 9`), so only TR0 works. A write to TR1 is taken
  as a write to RR0 and dropped (`imx8mp_mu: write to RR0 ignored`), and
  every SR/CR access is rejected. Symptoms:
  `imx_rproc_kick: failed (0, err:-62)` on the Linux side, and a remote
  that blocks forever waiting for TE1. `patches/0001-*.patch` fixes it; v4
  of the series does the equivalent (`reg = offset >> 2`).
* **v2 does not route MU1-B to the M7**, hence the polling. v4 adds an IRQ
  splitter and MU1-B IRQ 97, plus ITCM/DTCM regions, so the hardware build
  (`tcm.x`, interrupt-driven) should run unchanged on v4.
* **Restarting the remote** (`stop` then `start`) is not modelled: v2 resumes
  the M7 without resetting it on the second `start`.
* **NXP's `imx8mp-evk-rpmsg.dtb` from `qemu_test_bins` has a `rsc-table`
  region.** With it, `imx_rproc` copies the resource table to `0x550FF000`
  instead of the firmware's own section, so the firmware never sees
  `DRIVER_OK`. `imx8mp-evk-rpmsg-qemu.dts` leaves the region out. The NXP
  kernel `Image` from that repo also builds `rpmsg_tty`/`rpmsg_char` as
  modules and ships none, so it cannot complete a userspace round trip.

## Files

| File | Purpose |
|---|---|
| `reproduce.sh` | Fetches and builds everything, runs the test, fails unless `PASS` |
| `run.sh` | QEMU command line (initramfs, UART4 capture) |
| `patches/0001-*.patch` | MU offset fix for the v2 series |
| `imx8mp-evk-rpmsg-qemu.dts` | Mainline EVK + M7 node, vrings, buffer pool, M7 carve-out |
| `init.c` | Freestanding aarch64 `/init`: start the remote, echo test, power off |
| `mkinitramfs.py` | Packs `/init` and the firmware into a newc initramfs |
