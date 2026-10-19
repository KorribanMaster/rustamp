# Running the example on QEMU (ZCU102)

Linux runs on the A53s of AMD/Xilinx QEMU's ZCU102 model and starts the
firmware on R5_0 with mainline `xlnx_r5_remoteproc`, which talks to the
platform's PMU firmware for power, reset and boot address. Upstream QEMU
does not model the PMU, so this uses the AMD/Xilinx fork with its MicroBlaze
PMU running in a second, co-simulated QEMU process.

```sh
./reproduce.sh            # builds QEMU, Linux, firmware and init, then runs
```

Expected end of the console:

```text
[init] PASS: 6000 round trips, 770136 bytes echoed by the RustAmp firmware across 2 stop/start reloads
```

and on R5_0's UART1 (`r5-uart1.log`), per start:

```text
rustamp r5: up, waiting for the host (DRIVER_OK)
rustamp r5: DRIVER_OK, ns=true
rustamp r5: announced rpmsg-tty at 0x400
rustamp r5: first message from 0x400
rustamp r5: echoed 2000 messages, 256712 bytes
```

## What the test does

1. The PMU process boots the PMU ROM and the PMU firmware. The A53 process
   boots ATF and U-Boot, which runs `boot.scr` (loaded at `0x20000000`, the
   address Xilinx U-Boot looks for a JTAG boot script) and `booti`s Linux
   7.2 with `zynqmp-zcu102-rustamp.dtb` and an initramfs.
2. `/init` (`../../imx8mn-m7-echo/qemu/init.c`) starts remoteproc0. The
   driver loads the ELF into R5_0's TCMs and `rproc_0_fw_image`, allocates
   the vrings from the `vdev0vring*` carve-outs, and asks the PMU firmware
   to boot R5_0 from 0.
3. The firmware waits for `DRIVER_OK` and announces `rpmsg-tty`; Linux
   creates `/dev/ttyRPMSG0`.
4. `/init` sends 2,000 messages of 1–256 bytes and checks each echo, stops
   the remote (the driver powers R5_0 down) and starts it again, twice.

## Where QEMU differs from a board

* **No RPU GIC.** The model's RPU interrupt controller is disabled, so the
  firmware is built with `--features qemu`: it polls IPI channel 1 instead
  of taking interrupt 65. Writes to `0xF9000000` (the GIC) break the run.
* **No FSBL, so the R5s start out running.** The PMU's boot code releases
  both R5s from reset, and nothing puts them back: they run whatever is in
  TCM from power-on, and run into the image while Linux loads it. On a
  board the boot loader holds them in reset. `boot.scr` does the same before
  booting Linux: R5_0 and R5_1 in reset (`CRL_APB.RST_LPD_TOP` bits 0 and
  1), halted, with low vectors (`RPU_0_CFG`/`RPU_1_CFG`). (Those registers
  sit behind the platform's access protection, so `-device loader` writes
  from QEMU's side do not reach them; U-Boot's own writes do.)
* **Restarts can catch the image mid-load.** On a stop/start reload the
  model can let R5_0 run briefly before Linux has written the new image and
  resource table, until the boot request restarts it. The firmware
  therefore waits for a valid resource table instead of failing.
* **Prebuilt firmware.** PMU ROM, PMU firmware, ATF and U-Boot come from
  AMD/Xilinx (`xlnx_rel_v2025.2`), downloaded at run time and never
  committed. The PMU ROM is under a Xilinx binary licence that permits
  unmodified redistribution (`fw/PMU_ROM-LICENSE.txt`).
* **Build notes.** This QEMU is older than some host libraries (`libnfs`),
  so it is built with `--without-default-features` plus `libgcrypt` (its
  crypto models need it); its meson only finds libgcrypt via
  `libgcrypt-config`, which `reproduce.sh` provides from pkg-config.
