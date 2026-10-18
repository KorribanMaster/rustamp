# Running the example on QEMU (xilinx-zynq-a9)

Linux runs on CPU0 of QEMU's two-core `xilinx-zynq-a9` machine and the RTU
firmware on CPU1, started by the `a9-rproc` driver. QEMU is an unmodified
upstream release.

```sh
./reproduce.sh            # builds QEMU, Linux, firmware and init, then runs
```

## What the test does

1. Linux 7.2 (`multi_v7` with `a9_rproc.c` added in-tree) boots on CPU0 with
   `maxcpus=1` and `zynq-zc702-rpmsg-qemu.dts`.
2. CPU1 waits in `boot-cpu1.S`, the same loop the Zynq boot ROM/FSBL leaves
   it in on hardware: `wfe`, then poll `0xFFFFFFF0` for an entry address.
3. `/init` (`../../imx8mn-m7-echo/qemu/init.c`, built for 32-bit ARM) starts
   remoteproc0. The driver loads the ELF, writes its entry to `0xFFFFFFF0`
   and signals `sev` and SGI 15.
4. The firmware waits for `DRIVER_OK` and announces `rpmsg-tty`; Linux
   creates `/dev/ttyRPMSG0`.
5. `/init` sends 2,000 messages of 1–256 bytes and checks each echo. It then
   stops the remote (the firmware parks), starts it again (the driver
   releases the park stub) and repeats, twice (`RELOAD_CYCLES`).

## Why `-device loader` instead of `-kernel`

With `-kernel` and `-smp 2`, QEMU places its Linux boot code and its
secondary-CPU stub both at address 0 on this machine and refuses to start
("ROM regions are overlapping"). `run.sh` therefore loads the zImage, DTB
and initramfs with `-device loader` and starts CPU0 in `boot-cpu0.S`, which
does what QEMU's board setup would (ARM PLL) and enters the kernel. This also
puts CPU1 in a boot-ROM-like loop, so the driver's start path is the one used
on hardware.

## Result (18 Oct 2026)

```text
[    3.628547] a9-rproc remoteproc-cpu1: CPU1 as remote processor, SGI 15 to it, SGI 14 from it
[    4.260509] remoteproc remoteproc0: Booting fw image rpmsg-echo.elf, size 464288
[    4.303329] virtio_rpmsg_bus virtio0: rpmsg host is online
[    4.307961] remoteproc remoteproc0: remote processor remoteproc-cpu1 is now up
[    4.316901] virtio_rpmsg_bus virtio0: creating channel rpmsg-tty addr 0x400
[init] 2000 round trips, 256712 bytes echoed
[    7.623349] remoteproc remoteproc0: stopped remote processor remoteproc-cpu1
[init] reload 1: 2000 round trips, 256712 bytes echoed
[init] reload 2: 2000 round trips, 256712 bytes echoed
[init] PASS: 6000 round trips, 770136 bytes echoed by the RustAmp firmware across 2 stop/start reloads
```

RTU (UART0):

```text
rustamp a9: up on zynq-7000 (PERIPHBASE 0xf8f00000), waiting for the host
rustamp a9: DRIVER_OK, ns=true
rustamp a9: announced rpmsg-tty at 0x400
rustamp a9: first message from 0x400
rustamp a9: echoed 2000 messages, 256712 bytes
rustamp a9: stop requested after 2000 messages, parking
rustamp a9: up on zynq-7000 (PERIPHBASE 0xf8f00000), waiting for the host
...
```

## QEMU findings

* `xilinx-zynq-a9` cannot boot SMP Linux through `-kernel` (overlapping boot
  code and secondary stub at 0, see above).
* Its SLCR stores `A9_CPU_RST_CTRL` but does not reset or release CPUs, so
  Linux's `zynq_cpun_start()` cannot start CPU1 there. The driver's
  `release-addr` method and the cooperative park do not depend on it.
