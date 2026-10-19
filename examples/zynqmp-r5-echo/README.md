# Zynq UltraScale+ R5 rpmsg echo

Bare-metal firmware for the Cortex-R5F (R5_0, split mode) of a Zynq
UltraScale+ MPSoC, started by Linux on the A53s through mainline
`xlnx_r5_remoteproc`. It announces `rpmsg-tty`, so Linux creates
`/dev/ttyRPMSG0`, and everything written there is echoed back. This is the
standard OpenAMP setup on ZynqMP, after upstream OpenAMP's `zynqmp_r5`
platform in [openamp-system-reference] (`examples/legacy_apps/machine/xlnx/zynqmp_r5`
and `examples/linux/dts/xilinx/zynqmp-openamp.dtso`).

It runs on [`aarch32-rt`](https://docs.rs/aarch32-rt) (vectors, startup,
mode stacks, VFP) and [`aarch32-cpu`](https://docs.rs/aarch32-cpu) (MPU,
interrupt masking, the critical section behind `CsLock`).

| | |
|---|---|
| Build | `cargo build --release` (ZCU102 preset) |
| Build for QEMU | `cargo build --release --features qemu` |
| Tested | AMD/Xilinx QEMU's ZCU102 with Linux 7.2 ([`qemu/`](qemu/README.md)) |

It has not been run on a board yet.

## Memory map (R5_0 view)

| Region | Address | Size | |
|---|---|---|---|
| ATCM | `0x00000000` | 64 KiB | vectors, code, read-only data |
| BTCM | `0x00020000` | 64 KiB | data, bss, mode stacks |
| Resource table | `0x3ED00000` | (section) | in `rproc_0_fw_image` |
| Trace buffer | `0x3ED01000` | 4 KiB | `trace0` in debugfs |
| vring 0 / vring 1 | `0x3ED40000` / `0x3ED44000` | 16 KiB each | placed by Linux (`da = ANY`) |
| rpmsg buffers | `0x3ED48000` | 1 MiB | `vdev0buffer` |

Linux loads the TCM segments through its own view of R5_0's TCMs
(`0xFFE00000`, `0xFFE20000`) and the DDR segments into `rproc_0_fw_image`.
The MPU marks the shared DDR window (`0x3EC00000`, 4 MiB) non-cacheable and
the peripherals as device memory. The caches stay off, so enabling them
later keeps rpmsg coherent without cache maintenance.

## Notifications

IPI channel 1 (`0xFF310000`) is R5_0's. The host kicks it; the firmware
acknowledges by clearing `ISR`, which Linux's mailbox driver waits for. To
kick the host it writes the host's agent mask to `TRIG`:
`board::HOST_IPI_MASK` is `0x1`, the APU on channel 0 as in mainline
`zynqmp.dtsi`. Older Xilinx device trees put Linux on channel 7: use
`0x0100_0000` there.

The default build takes IPI interrupt 65 on the RPU GIC and sleeps in
`wfi`. `--features qemu` polls `ISR` instead, because AMD/Xilinx QEMU's
ZCU102 model does not instantiate the RPU GIC.

## Linux side

[`zcu102/zynqmp-zcu102-rustamp.dts`](zcu102/zynqmp-zcu102-rustamp.dts)
includes mainline `zynqmp-zcu102-rev1.0.dts`, which already enables the
cluster in split mode, and adds the vring and buffer carve-outs and the
IPI mailbox to R5_0, both wired to `r5f@0`. Copy it to
`arch/arm64/boot/dts/xilinx/` and build `xilinx/zynqmp-zcu102-rustamp.dtb`.
The R5 logs to UART1; keep its clock on with `clk_ignore_unused`.

```sh
cp target/armv7r-none-eabihf/release/zynqmp-r5-echo /lib/firmware/r5.elf
echo r5.elf > /sys/class/remoteproc/remoteproc0/firmware
echo start > /sys/class/remoteproc/remoteproc0/state
stty -F /dev/ttyRPMSG0 raw -echo
cat /dev/ttyRPMSG0 & echo hello > /dev/ttyRPMSG0   # prints "hello"
cat /sys/kernel/debug/remoteproc/remoteproc0/trace0
echo stop > /sys/class/remoteproc/remoteproc0/state  # Linux resets the R5
```

Stop and restart need nothing from the firmware: the driver powers the R5
down and boots it again.

## Files

| File | Purpose |
|---|---|
| `src/main.rs` | Resource table (vdev with `da = ANY` vrings, trace), rpmsg setup, echo loop |
| `src/hw.rs` | IPI, RPU GIC, MPU, IRQ handler |
| `src/uart.rs`, `src/trace.rs` | Debug output on UART1 and into the trace buffer |
| `src/{board,ipi,gic}.rs` | Memory map and register encodings (host-tested: `cargo test --lib --target x86_64-unknown-linux-gnu`) |
| `memory.x` | Memory regions for `aarch32-rt`'s `link.x`, and the DDR sections |
| `zcu102/` | ZCU102 device tree |
| `qemu/` | QEMU harness: AMD/Xilinx QEMU, PMU firmware, Linux, this firmware |

[openamp-system-reference]: https://github.com/OpenAMP/openamp-system-reference
