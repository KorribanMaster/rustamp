#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
# Boots Linux on AMD/Xilinx QEMU's ZCU102 (A53s plus the MicroBlaze PMU in a
# second QEMU process), starts the RustAmp firmware on R5_0 through
# xlnx_r5_remoteproc and runs the rpmsg echo test from /init.
#
# usage: run.sh QEMU_BUILD HW_DTB_DIR FW_DIR KERNEL_IMAGE DTB INIT FIRMWARE [R5_LOG]
#   QEMU_BUILD  directory with qemu-system-aarch64 and qemu-system-microblazeel
#   HW_DTB_DIR  qemu-devicetrees LATEST/MULTI_ARCH (zcu102-arm.dtb, zynqmp-pmu.dtb)
#   FW_DIR      pmu-rom.elf, pmufw.elf, arm-trusted-firmware.elf, u-boot.elf
set -eu
q=$1 hw=$2 fw=$3 kernel=$4 dtb=$5 init=$6 firmware=$7 r5log=${8:-r5-uart1.log}
here=$(cd "$(dirname "$0")" && pwd)

cp "$firmware" rpmsg-echo.elf
python3 "$here/../../imx8mn-m7-echo/qemu/mkinitramfs.py" "$init" rpmsg-echo.elf initramfs.cpio.gz
initrd_size=$(printf '0x%x' "$(wc -c < initramfs.cpio.gz)")
# No FSBL runs here, and the PMU boot code releases the R5s from reset, so
# they would run whatever is in TCM from the start, and run into the image
# while Linux loads it. Do what the boot loader does on a board: hold both
# R5s in reset (CRL_APB.RST_LPD_TOP bits 0 and 1) and halted with low
# vectors (RPU_0_CFG/RPU_1_CFG = 0). The PMU firmware then releases them
# when Linux asks it to boot one, after setting its boot address.
cat > boot.cmd <<CMD
mw.l 0xff9a0100 0
mw.l 0xff9a0200 0
setexpr.l rst *0xff5e023c "|" 3
mw.l 0xff5e023c \${rst}
setenv bootargs "console=ttyPS0,115200 rdinit=/init clk_ignore_unused loglevel=7 ${BOOTARGS_EXTRA:-}"
booti 0x200000 0x4000000:$initrd_size 0x100000
CMD
python3 "$here/mkbootscr.py" boot.cmd boot.scr

# The two QEMU processes talk over sockets in this directory.
mp=$(mktemp -d)
trap 'kill $pmu 2>/dev/null || true; rm -rf "$mp"' EXIT INT TERM

"$q/qemu-system-microblazeel" -M microblaze-fdt -display none \
    -serial null -serial null \
    -kernel "$fw/pmu-rom.elf" -device loader,file="$fw/pmufw.elf" \
    -hw-dtb "$hw/zynqmp-pmu.dtb" -machine-path "$mp" \
    -device loader,addr=0xfd1a0074,data=0x1011003,data-len=4 \
    -device loader,addr=0xfd1a007C,data=0x1010f03,data-len=4 \
    > pmu.log 2>&1 < /dev/null &
pmu=$!

timeout 900 "$q/qemu-system-aarch64" -M arm-generic-fdt -display none \
    -serial stdio -serial "file:$r5log" \
    -hw-dtb "$hw/zcu102-arm.dtb" -machine-path "$mp" -m 4G \
    -device loader,file="$fw/arm-trusted-firmware.elf",cpu-num=0 \
    -device loader,file="$fw/u-boot.elf" \
    -device loader,file="$dtb",addr=0x100000,force-raw=on \
    -device loader,file="$kernel",addr=0x200000,force-raw=on \
    -device loader,file=initramfs.cpio.gz,addr=0x4000000,force-raw=on \
    -device loader,file=boot.scr,addr=0x20000000,force-raw=on \
    -global xlnx,zynqmp-boot.cpu-num=0 -global xlnx,zynqmp-boot.use-pmufw=true \
    -no-reboot ${QEMU_EXTRA:-} < /dev/null || true
