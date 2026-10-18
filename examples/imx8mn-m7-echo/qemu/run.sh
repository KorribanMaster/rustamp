#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
# Boots Linux on QEMU's imx8mp-evk (with the Cortex-M7 AMP series), starts
# the RustAmp firmware on the M7 and runs the rpmsg echo test from /init.
#
# usage: run.sh QEMU KERNEL_IMAGE DTB INIT FIRMWARE [M7_LOG]
set -eu
qemu=$1 kernel=$2 dtb=$3 init=$4 fw=$5 m7log=${6:-m7-uart4.log}
here=$(dirname "$0")
cp "$fw" rpmsg-echo.elf
python3 "$here/mkinitramfs.py" "$init" rpmsg-echo.elf initramfs.cpio.gz
exec timeout 600 "$qemu" -M imx8mp-evk -m 3G -display none \
    -serial null -serial stdio -serial null -serial "file:$m7log" \
    -kernel "$kernel" -dtb "$dtb" -initrd initramfs.cpio.gz \
    -append "console=ttymxc1,115200 rdinit=/init loglevel=7" \
    -no-reboot ${QEMU_EXTRA:-}
