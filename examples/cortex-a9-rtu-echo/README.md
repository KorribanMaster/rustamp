# Cortex-A9 RTU rpmsg echo (Linux on CPU0, firmware on CPU1)

Bare-metal firmware for the second core of a Cortex-A9 MPCore, the classic
Zynq-7000 OpenAMP setup (Xilinx UG1186, removed from open-amp after v2023.10)
done generically: Linux runs on CPU0, and this firmware runs on CPU1 as a
real-time unit (RTU). Linux loads and starts it through `remoteproc` with the
driver in [`linux/a9-rproc`](../../linux/a9-rproc). The firmware announces
`rpmsg-tty`, so Linux creates `/dev/ttyRPMSG0`, and everything written there
comes back.

| | Zynq-7000 | Arria 10 |
|---|---|---|
| Build | `cargo build --release` | `cargo build --release --no-default-features --features arria10` |
| Debug UART (RTU) | UART0 `0xE0000000` (Cadence) | UART0 `0xFFC02000` (16550) |
| First start (driver) | `release-addr`: entry to `0xFFFFFFF0`, `sev` | `socfpga-a10`: reset manager, like Linux's own SMP code |
| Tested | QEMU `xilinx-zynq-a9` ([`qemu/`](qemu/README.md)) | Built and device tree compiled only |

Neither board has been run on real hardware yet.

## What makes it generic

* **No SoC addresses in the core code.** The GIC and SCU base (PERIPHBASE)
  comes from the CPU's CBAR register at run time. A board preset
  (`src/board.rs`) only adds the debug UART.
* **Nothing shared is touched.** Linux owns the SCU, the PL310 L2 cache and
  the GIC distributor configuration. The RTU only writes its own CP15 state,
  its L1 caches and MMU, the banked (per-CPU) GIC registers, and `GICD_SGIR`.
  `ACTLR.SMP` stays clear.
* **Caches without coherency.** The RTU's own memory is inner write-back,
  outer non-cacheable, so it never holds lines in Linux's L2. Shared memory
  (resource table, vrings, buffers) is non-cacheable on both sides, so rpmsg
  needs no cache maintenance.
* **SGIs both ways,** as in the original OpenAMP Zynq port: SGI 15 to the
  RTU, SGI 14 to Linux. Linux only allocates SGIs 0–7 for its IPIs.
* **Cooperative stop.** On `stop` the driver sets a stop request in the park
  page and kicks; the firmware turns its MMU and caches off and waits in a
  stub in that page (`src/park.rs`). A restart hands the stub the new entry,
  so stop/start reloads work on any A9, including QEMU, which ignores the
  Zynq CPU reset bits.

## Memory map

The RTU owns the top 32 MiB of the first GiB of DDR, reserved in the device
tree (`src/board.rs`, `rtu.x`):

| Region | Address | Size |
|---|---|---|
| Firmware (code, data, stacks, MMU table) | `0x3E000000` | 15 MiB |
| Resource table | `0x3EF00000` | (section) |
| Park page | `0x3EFFF000` | 4 KiB |
| vring 0 / vring 1 | `0x3F000000` / `0x3F008000` | 32 KiB each |
| rpmsg buffers (`vdev0buffer`) | `0x3F010000` | 1 MiB |

## Linux side

Boot with `maxcpus=1` (CPU1 must stay offline) and add the reserved memory
and the remoteproc node. [`qemu/zynq-zc702-rpmsg-qemu.dts`](qemu/zynq-zc702-rpmsg-qemu.dts)
(Zynq) and [`arria10/socfpga_arria10_socdk_rtu.dts`](arria10/socfpga_arria10_socdk_rtu.dts)
(Arria 10) are complete examples; the binding is
[`linux/a9-rproc/rustamp,cortex-a9-rproc.yaml`](../../linux/a9-rproc/rustamp,cortex-a9-rproc.yaml).
The RTU's UART also needs its clock left on: `clk_ignore_unused`, or a DT
clock consumer.

```sh
cp target/armv7a-none-eabi/release/cortex-a9-rtu-echo /lib/firmware/rtu.elf
echo rtu.elf > /sys/class/remoteproc/remoteproc0/firmware
echo start > /sys/class/remoteproc/remoteproc0/state
stty -F /dev/ttyRPMSG0 raw -echo
cat /dev/ttyRPMSG0 & echo hello > /dev/ttyRPMSG0   # prints "hello"
echo stop > /sys/class/remoteproc/remoteproc0/state  # RTU parks; start reloads
```

## Files

| File | Purpose |
|---|---|
| `src/rt.rs` | Vectors, reset, MMU/L1 bring-up, IRQ entry, park stub and jump |
| `src/hw.rs` | GIC: CPU interface, SGI send, IRQ acknowledge |
| `src/main.rs` | Resource table, rpmsg setup, echo loop, stop handling |
| `src/uart.rs` | Polled debug output (Cadence or 16550) |
| `src/{board,mmu,gic,park}.rs` | Memory map, MMU table, GIC encodings, park protocol (host-tested: `cargo test --lib --target x86_64-unknown-linux-gnu`) |
| `rtu.x` | Linker script |
| `qemu/` | QEMU harness: Linux + RTU on `xilinx-zynq-a9` |
| `arria10/` | Arria 10 SoCDK device tree |
