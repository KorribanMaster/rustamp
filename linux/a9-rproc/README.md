# a9-rproc: a spare Cortex-A9 core as a Linux remote processor

A small remoteproc driver (GPL-2.0) for running firmware on a Cortex-A9 core
that Linux does not use: CPU1 of a Zynq-7000 or Arria 10 booted with
`maxcpus=1`, or any A9 MPCore with a way to release a secondary core. It pairs
with [`examples/cortex-a9-rtu-echo`](../../examples/cortex-a9-rtu-echo).

* **Notifications:** SGIs both ways, with no kernel changes. The driver raises
  `openamp-rs,tx-sgi` (15) on the remote by writing `GICD_SGIR`. It receives
  `openamp-rs,rx-sgi` (14) by mapping it in the GIC's IRQ domain with a
  one-cell fwspec (`irq_create_fwspec_mapping()`, the same convention the GIC
  driver uses for its own IPIs) and `request_percpu_irq()`. Linux allocates
  only SGIs 0–7.
* **First start:** `release-addr` writes the entry to a mailbox word the
  parked core polls (Zynq boot ROM after the FSBL: `0xfffffff0`) and signals
  `sev` and the SGI. `socfpga-a10` releases CPU1 through the Arria 10 reset
  manager, as `socfpga_a10_boot_secondary()` does.
* **Stop and restart:** cooperative, through a park page in the firmware's
  carve-out (see `examples/cortex-a9-rtu-echo/src/park.rs`). The firmware
  parks itself; the next start releases it at the new entry.
* **Memory:** `memory-region` entries become carveouts with da == pa; the
  first, `vdev0buffer`, is the rpmsg buffer pool.

Binding: [`openamp-rs,cortex-a9-rproc.yaml`](openamp-rs,cortex-a9-rproc.yaml).

## Build

Out of tree:

```sh
make KDIR=/path/to/linux-build ARCH=arm LLVM=1     # -> a9_rproc.ko
```

In tree: copy `a9_rproc.c` to `drivers/remoteproc/`, append `Kconfig` to
`drivers/remoteproc/Kconfig` and `obj-$(CONFIG_A9_REMOTEPROC) += a9_rproc.o`
to its `Makefile` (what `examples/cortex-a9-rtu-echo/qemu/reproduce.sh`
does), then enable `CONFIG_A9_REMOTEPROC`.

## Status

Runs on QEMU `xilinx-zynq-a9` with Linux 7.2: start, 2,000 checked rpmsg-tty
echoes, two stop/start reloads. The `socfpga-a10` path compiles but has not run
anywhere: QEMU has no Arria 10 machine, and no hardware has been tried yet.
