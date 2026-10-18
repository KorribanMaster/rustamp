#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
# Boots Linux on CPU0 of QEMU's xilinx-zynq-a9, which starts the openamp-rs
# firmware on CPU1 and runs the rpmsg echo test from /init.
#
# Everything is placed with -device loader instead of -kernel: with -kernel
# and -smp 2, QEMU's Zynq boot code and secondary-CPU stub overlap at 0.
# CPU0 starts in boot-cpu0.S, CPU1 in boot-cpu1.S (the boot ROM's wait loop).
#
# usage: run.sh QEMU ZIMAGE DTB INIT FIRMWARE [RTU_LOG]
set -eu
qemu=$1 kernel=$2 dtb=$3 init=$4 fw=$5 rtulog=${6:-rtu-uart0.log}
here=$(cd "$(dirname "$0")" && pwd)
mkinit=$here/../../imx8mn-m7-echo/qemu/mkinitramfs.py

KERNEL_ADDR=0x01000000
DTB_ADDR=0x02000000
INITRD_ADDR=0x02100000
BOOT0_ADDR=0xfffc0000
BOOT1_ADDR=0xfffffe00

cp "$fw" rpmsg-echo.elf
python3 "$mkinit" "$init" rpmsg-echo.elf initramfs.cpio.gz

blob() {
    name=$1
    shift
    clang --target=armv7a-none-eabi -c -o "$name.o" "$here/$name.S" "$@"
    llvm-objcopy -O binary "$name.o" "$name.bin"
}
blob boot-cpu0 -DKERNEL=$KERNEL_ADDR -DDTB=$DTB_ADDR
blob boot-cpu1

# Command line and initramfs location go into the DTB's /chosen.
cp "$dtb" board.dtb
size=$(stat -c %s initramfs.cpio.gz)
fdtput -t s board.dtb /chosen bootargs \
    "console=ttyPS0,115200 maxcpus=1 clk_ignore_unused rdinit=/init loglevel=7"
fdtput -t x board.dtb /chosen linux,initrd-start $INITRD_ADDR
fdtput -t x board.dtb /chosen linux,initrd-end "$(printf '%#x' $((INITRD_ADDR + size)))"

# UART0 (first -serial) is the RTU's, UART1 the Linux console (ttyPS0).
# clk_ignore_unused keeps UART0's clock on although Linux does not use it.
exec timeout 900 "$qemu" -M xilinx-zynq-a9 -smp 2 -m 1G -display none \
    -serial "file:$rtulog" -serial stdio \
    -device loader,file="$kernel",addr=$KERNEL_ADDR,force-raw=on \
    -device loader,file=board.dtb,addr=$DTB_ADDR,force-raw=on \
    -device loader,file=initramfs.cpio.gz,addr=$INITRD_ADDR,force-raw=on \
    -device loader,file=boot-cpu0.bin,addr=$BOOT0_ADDR,force-raw=on,cpu-num=0 \
    -device loader,file=boot-cpu1.bin,addr=$BOOT1_ADDR,force-raw=on,cpu-num=1 \
    -no-reboot ${QEMU_EXTRA:-}
