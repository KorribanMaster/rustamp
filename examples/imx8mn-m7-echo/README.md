# i.MX 8M Nano Cortex-M7 rpmsg echo

Bare-metal firmware for the Cortex-M7 of an i.MX 8M Nano, started by Linux
through `remoteproc` (`imx_rproc`). It announces `rpmsg-tty`, so mainline
Linux creates `/dev/ttyRPMSG0`; everything written there is echoed back.

The firmware runs on `cortex-m-rt`, with `cortex-m` for the NVIC, MPU and
`wfi`, and `panic-halt`. The memory layout is in `memory-tcm.x` (or
`memory-qemu.x`), which `build.rs` hands to `cortex-m-rt`'s `link.x` as
`memory.x`. It builds and links to about 10 KB of code. It
has not been run on hardware yet. It does run under QEMU's `imx8mp-evk`
(i.MX 8M Plus, same M7 memory map and MU): see [`qemu/`](qemu/README.md),
where Linux 7.2 completes 2,000 echo round trips through `/dev/ttyRPMSG0`. Register addresses (MU1 B side `0x30AB0000`,
IRQ 97) follow NXP's SDK headers; check them against your reference manual
and SDK.

## Build

```sh
rustup target add thumbv7em-none-eabihf
cargo build --release
```

The ELF is `target/thumbv7em-none-eabihf/release/imx8mn-m7-echo`.

## Linux side

The device tree needs the M7 node with mailboxes and the reserved memory the
firmware uses (as in NXP's `imx8mn-evk-rpmsg.dts`):

```dts
reserved-memory {
    vdev0vring0: vdev0vring0@55000000 { reg = <0 0x55000000 0 0x8000>; no-map; };
    vdev0vring1: vdev0vring1@55008000 { reg = <0 0x55008000 0 0x8000>; no-map; };
    vdevbuffer: vdevbuffer@55400000 {
        compatible = "shared-dma-pool";
        reg = <0 0x55400000 0 0x100000>;
        no-map;
    };
};

imx8mn-cm7 {
    compatible = "fsl,imx8mn-cm7";
    clocks = <&clk IMX8MN_CLK_M7_DIV>;
    mboxes = <&mu 0 1>, <&mu 1 1>, <&mu 3 1>;
    mbox-names = "tx", "rx", "rxdb";
    memory-region = <&vdevbuffer>, <&vdev0vring0>, <&vdev0vring1>;
};
```

Then, on the target:

```sh
cp imx8mn-m7-echo /lib/firmware/
echo imx8mn-m7-echo > /sys/class/remoteproc/remoteproc0/firmware
echo start > /sys/class/remoteproc/remoteproc0/state
stty -F /dev/ttyRPMSG0 raw -echo
cat /dev/ttyRPMSG0 & echo hello > /dev/ttyRPMSG0   # prints "hello"
```

## How it fits together

| Piece | Where |
|---|---|
| Runtime | `cortex-m-rt` (no `device` feature, so every interrupt reaches `DefaultHandler(irqn)`); `memory-tcm.x` (ITCM code, DTCM data), `memory-qemu.x` with `--features qemu-imx8mp` (DDR, sets VTOR). Panics halt (`panic-halt`); the QEMU build logs them on UART4 first |
| Resource table (vdev, 2 × 256-entry vrings at fixed addresses) | `resource_table!` in `src/main.rs`, its own section at `0x2001F000` (`memory-tcm.x`) |
| Locking | `CsLock` (feature `rustamp-metal/critical-section`, implemented by `cortex-m`'s `critical-section-single-core`), so rpmsg stays correct even if called from an interrupt |
| Virtio transport | `rustamp_remoteproc::RprocVirtio::new_device` |
| Kick Linux | MU channel 1 (`src/mu.rs`), value `notifyid << 16` |
| Kicked by Linux | MU interrupt → `DefaultHandler` sets a flag → main loop polls `try_recv`, sleeps in `wfi` |
| Caches | Off (reset state); MPU marks `0x55000000` uncached in case you enable them |
