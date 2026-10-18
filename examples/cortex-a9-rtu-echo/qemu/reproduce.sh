#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
#
# Builds everything from public sources and runs the rpmsg echo test on
# QEMU's xilinx-zynq-a9 (two Cortex-A9 cores): Linux on CPU0, the
# openamp-rs RTU firmware on CPU1.
#
#   QEMU   upstream release, unpatched (arm-softmmu)
#   Linux  mainline, multi_v7 with the rpmsg/remoteproc path built in and
#          linux/a9-rproc added as drivers/remoteproc/a9_rproc.c
#   RTU    examples/cortex-a9-rtu-echo (zynq7000 preset)
#   CPU0   /init from examples/imx8mn-m7-echo/qemu/init.c (32-bit ARM)
#
# Needs: git, curl, xz, python3, ninja, pkg-config, glib/pixman dev files, clang, lld,
# llvm, flex, bison, bc, rustup with armv7a-none-eabi.
#
# usage: reproduce.sh [WORKDIR]     (default: ./qemu-work)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
work=$(mkdir -p "${1:-qemu-work}" && cd "${1:-qemu-work}" && pwd)
jobs=$(nproc)
QEMU_TAG=v11.1.2
LINUX_VERSION=7.2
RELOAD_CYCLES=${RELOAD_CYCLES:-2}

log() { printf '\n==> %s\n' "$*"; }

# ---------------------------------------------------------------- QEMU ----
if [ ! -x "$work/qemu-build/qemu-system-arm" ]; then
    log "QEMU: fetching $QEMU_TAG"
    [ -d "$work/qemu" ] ||
        git -c advice.detachedHead=false clone -q --depth 1 --branch "$QEMU_TAG" \
            https://github.com/qemu/qemu.git "$work/qemu"
    # Subprojects from GitHub mirrors (the wraps point at gitlab.com).
    sub() {
        [ -d "$work/qemu/subprojects/$1" ] && return
        git clone -q "$2" "$work/qemu/subprojects/$1"
        if [ -n "${3:-}" ]; then
            git -C "$work/qemu/subprojects/$1" checkout -q "$3" 2>/dev/null || true
        fi
        if [ -d "$work/qemu/subprojects/packagefiles/$1" ]; then
            cp -r "$work/qemu/subprojects/packagefiles/$1/." "$work/qemu/subprojects/$1/"
        fi
    }
    rev() { sed -n 's/^revision = //p' "$work/qemu/subprojects/$1.wrap"; }
    sub dtc https://github.com/dgibson/dtc.git "$(rev dtc)"
    sub keycodemapdb https://github.com/qemu/keycodemapdb "$(rev keycodemapdb)"
    sub berkeley-softfloat-3 https://github.com/qemu/berkeley-softfloat-3 "$(rev berkeley-softfloat-3)"
    sub berkeley-testfloat-3 https://github.com/qemu/berkeley-testfloat-3 "$(rev berkeley-testfloat-3)"
    log "QEMU: building arm-softmmu"
    mkdir -p "$work/qemu-build"
    (cd "$work/qemu-build" &&
        "$work/qemu/configure" --target-list=arm-softmmu --disable-docs \
            --disable-werror --disable-rust --disable-gtk --disable-sdl \
            --disable-vnc --disable-opengl --disable-slirp --disable-tools \
            --disable-guest-agent --disable-download --enable-fdt=internal \
            > configure.log &&
        ninja -j "$jobs" qemu-system-arm > build.log)
fi

# --------------------------------------------------------------- Linux ----
kimg="$work/linux-build/arch/arm/boot/zImage"
dtb="$work/linux-build/arch/arm/boot/dts/xilinx/zynq-zc702-rpmsg-qemu.dtb"
drv="$work/linux/drivers/remoteproc"
if [ ! -f "$kimg" ] || [ ! -f "$dtb" ] ||
    ! cmp -s "$repo/linux/a9-rproc/a9_rproc.c" "$drv/a9_rproc.c" ||
    ! cmp -s "$here/zynq-zc702-rpmsg-qemu.dts" \
        "$work/linux/arch/arm/boot/dts/xilinx/zynq-zc702-rpmsg-qemu.dts"; then
    if [ ! -d "$work/linux" ]; then
        log "Linux: fetching $LINUX_VERSION"
        mkdir -p "$work/linux"
        curl -sSfL "https://cdn.kernel.org/pub/linux/kernel/v${LINUX_VERSION%%.*}.x/linux-$LINUX_VERSION.tar.xz" |
            tar -xJ -C "$work/linux" --strip-components=1
    fi
    # The driver, added in-tree (the same source builds out of tree).
    cp "$repo/linux/a9-rproc/a9_rproc.c" "$drv/"
    grep -q A9_REMOTEPROC "$drv/Kconfig" || {
        sed -i '/^endmenu/d' "$drv/Kconfig"
        cat "$repo/linux/a9-rproc/Kconfig" >> "$drv/Kconfig"
        echo endmenu >> "$drv/Kconfig"
        echo 'obj-$(CONFIG_A9_REMOTEPROC) += a9_rproc.o' >> "$drv/Makefile"
    }
    cp "$here/zynq-zc702-rpmsg-qemu.dts" "$work/linux/arch/arm/boot/dts/xilinx/"
    grep -q zynq-zc702-rpmsg-qemu "$work/linux/arch/arm/boot/dts/xilinx/Makefile" ||
        echo 'dtb-$(CONFIG_ARCH_ZYNQ) += zynq-zc702-rpmsg-qemu.dtb' \
            >> "$work/linux/arch/arm/boot/dts/xilinx/Makefile"
    kmake() { make -s -C "$work/linux" ARCH=arm LLVM=1 O="$work/linux-build" "$@"; }
    kmake multi_v7_defconfig
    cfg="$work/linux/scripts/config --file $work/linux-build/.config"
    # Zynq only, small, with the rpmsg/remoteproc path built in.
    for p in $(sed -n 's/^config \(ARCH_[A-Z0-9_]*\)$/\1/p' "$work/linux/arch/arm/Kconfig.platforms" \
        "$work/linux/arch/arm"/mach-*/Kconfig); do
        [ "$p" = ARCH_ZYNQ ] || [ "$p" = ARCH_MULTI_V7 ] || $cfg -d "$p"
    done
    $cfg -d MODULES -d NET -d DRM -d SOUND -d SND -d WLAN -d BT -d USB_SUPPORT \
        -d MEDIA_SUPPORT -d HID -d BPF_SYSCALL -d SECURITY -d FTRACE \
        -d DEBUG_INFO_DWARF_TOOLCHAIN_DEFAULT -e DEBUG_INFO_NONE -d PCI \
        -d NFS_FS -d EXT4_FS -d MTD -d SCSI -d ATA -d STAGING -d IIO -d FB \
        -d INFINIBAND -d EFI -d PROFILING -d KPROBES -d CPU_FREQ -d CPU_IDLE \
        -d THERMAL -d HWMON -d MMC -d DMADEVICES -d SPI -d I2C -d PWM \
        -d CGROUPS -d NAMESPACES -d AUDIT -d SWAP -d SYSVIPC -d VIRTIO_MENU \
        -e REMOTEPROC -e A9_REMOTEPROC -e RPMSG_VIRTIO -e RPMSG_TTY \
        -e RPMSG_CTRL -e BLK_DEV_INITRD -e DEVTMPFS -e DEVTMPFS_MOUNT \
        -e SERIAL_XILINX_PS_UART -e SERIAL_XILINX_PS_UART_CONSOLE
    kmake olddefconfig
    for k in ARCH_ZYNQ SMP A9_REMOTEPROC RPMSG_VIRTIO RPMSG_TTY; do
        grep -q "^CONFIG_$k=y" "$work/linux-build/.config" ||
            { echo "CONFIG_$k is not enabled" >&2; exit 1; }
    done
    log "Linux: building zImage and DTB"
    kmake -j "$jobs" zImage xilinx/zynq-zc702-rpmsg-qemu.dtb
fi

log "Arria 10: compiling the SoCDK device tree with the RTU node (not run)"
a10=$work/linux/arch/arm/boot/dts/intel/socfpga
cpp -nostdinc -undef -x assembler-with-cpp -I "$work/linux/include" -I "$a10" \
    "$here/../arria10/socfpga_arria10_socdk_rtu.dts" |
    dtc -q -I dts -O dtb -o "$work/socfpga_arria10_socdk_rtu.dtb" -

# ----------------------------------------------------------- firmware ----
log "RTU firmware: cargo build (zynq7000)"
(cd "$here/.." && cargo build --release)
fw="$here/../target/armv7a-none-eabi/release/cortex-a9-rtu-echo"

log "CPU0 init"
clang --target=armv7a-linux-gnueabi -ffreestanding -fno-builtin -nostdlib -static \
    -fno-stack-protector -fno-pic -O2 -fuse-ld=lld -DRELOAD_CYCLES="$RELOAD_CYCLES" -DEXIT_BY_RESTART \
    -o "$work/init" "$repo/examples/imx8mn-m7-echo/qemu/init.c"

# ---------------------------------------------------------------- run ----
log "Running (console below; RTU UART0 in $work/rtu-uart0.log)"
cd "$work"
"$here/run.sh" "$work/qemu-build/qemu-system-arm" "$kimg" "$dtb" "$work/init" "$fw" \
    "$work/rtu-uart0.log" | tee console.log
echo
echo "--- RTU UART0 ---"
cat rtu-uart0.log
grep -q '^\[init\] PASS' console.log
