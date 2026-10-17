#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
#
# Builds everything from public sources and runs the rpmsg echo test on
# QEMU's imx8mp-evk with the Cortex-M7 AMP series:
#
#   QEMU   patchew v2 of "hw/arm: Adding Cortex-M7 Asymmetric Multiprocessing
#          boot support for i.MX8MP" (tag from github.com/patchew-project/qemu)
#          + patches/0001 (MU register-offset fix, equivalent to v4)
#   Linux  mainline v7.2, clang/LLVM, i.MX-only config with rpmsg_tty built in
#   M7     this example built with --features qemu-imx8mp
#   A53    /init from init.c (freestanding, clang)
#
# Needs: git, python3, ninja, pkg-config, glib/pixman dev files, clang, lld,
# llvm, flex, bison, bc, rustup with thumbv7em-none-eabihf.
#
# usage: reproduce.sh [WORKDIR]     (default: ./qemu-work)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
work=$(mkdir -p "${1:-qemu-work}" && cd "${1:-qemu-work}" && pwd)
jobs=$(nproc)
SERIES_TAG="patchew/20260706162859.3361063-1-gaurav.sharma._5F7@nxp.com"
LINUX_TAG=v7.2

log() { printf '\n==> %s\n' "$*"; }

# ---------------------------------------------------------------- QEMU ----
if [ ! -x "$work/qemu-build/qemu-system-aarch64" ]; then
    log "QEMU: fetching $SERIES_TAG"
    if [ ! -d "$work/qemu" ]; then
        git init -q "$work/qemu"
        git -C "$work/qemu" fetch -q --depth 10 \
            https://github.com/patchew-project/qemu "refs/tags/$SERIES_TAG:refs/tags/series"
        git -C "$work/qemu" -c advice.detachedHead=false checkout -q series
        git -C "$work/qemu" -c user.name=repro -c user.email=repro@localhost \
            am -q "$here"/patches/*.patch
    fi
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
    log "QEMU: building aarch64-softmmu"
    mkdir -p "$work/qemu-build"
    (cd "$work/qemu-build" &&
        "$work/qemu/configure" --target-list=aarch64-softmmu --disable-docs \
            --disable-werror --disable-rust --disable-gtk --disable-sdl \
            --disable-vnc --disable-opengl --disable-slirp --disable-tools \
            --disable-guest-agent --disable-download --enable-fdt=internal \
            > configure.log &&
        ninja -j "$jobs" qemu-system-aarch64 > build.log)
fi

# --------------------------------------------------------------- Linux ----
kimg="$work/linux-build/arch/arm64/boot/Image"
dtb="$work/linux-build/arch/arm64/boot/dts/freescale/imx8mp-evk-rpmsg-qemu.dtb"
if [ ! -f "$kimg" ] || [ ! -f "$dtb" ]; then
    log "Linux: fetching $LINUX_TAG"
    [ -d "$work/linux" ] ||
        git clone -q --depth 1 --branch "$LINUX_TAG" https://github.com/torvalds/linux.git "$work/linux"
    cp "$here/imx8mp-evk-rpmsg-qemu.dts" "$work/linux/arch/arm64/boot/dts/freescale/"
    kmake() { make -s -C "$work/linux" ARCH=arm64 LLVM=1 O="$work/linux-build" "$@"; }
    kmake defconfig
    cfg="$work/linux/scripts/config --file $work/linux-build/.config"
    for p in $(sed -n 's/^config \(ARCH_[A-Z0-9_]*\)$/\1/p' "$work/linux/arch/arm64/Kconfig.platforms"); do
        [ "$p" = ARCH_MXC ] || $cfg -d "$p"
    done
    # Small, i.MX-only kernel with the rpmsg/remoteproc path built in.
    $cfg -d MODULES -d NET -d DRM -d SOUND -d SND -d VIRTUALIZATION -d KVM \
        -d WLAN -d BT -d USB_SUPPORT -d MEDIA_SUPPORT -d HID -d BPF_SYSCALL \
        -d SECURITY -d FTRACE -d DEBUG_INFO_DWARF_TOOLCHAIN_DEFAULT \
        -e DEBUG_INFO_NONE -d PCI -d NFS_FS -d BTRFS_FS -d XFS_FS -d EXT4_FS \
        -d MTD -d SCSI -d ATA -d STAGING -d IIO -d FB -d SOUNDWIRE \
        -d INFINIBAND -d ACPI -d EFI -d NUMA -d HUGETLBFS -d PROFILING \
        -d KPROBES -d CPU_FREQ -d CPU_IDLE -d THERMAL -d HWMON -d MMC \
        -d NVME_CORE -d DMADEVICES -d SPI -d I2C -d PWM -d CGROUPS \
        -d NAMESPACES -d AUDIT -d SWAP -d SYSVIPC -d VIRTIO_MENU -d VHOST_MENU \
        -e REMOTEPROC -e IMX_REMOTEPROC -e RPMSG_VIRTIO -e RPMSG_TTY \
        -e RPMSG_CTRL -e IMX_MBOX -e BLK_DEV_INITRD -e DEVTMPFS \
        -e DEVTMPFS_MOUNT -e SERIAL_IMX -e SERIAL_IMX_CONSOLE -e MFD_SYSCON
    kmake olddefconfig
    for k in ARCH_MXC IMX_REMOTEPROC IMX_MBOX RPMSG_VIRTIO RPMSG_TTY; do
        grep -q "^CONFIG_$k=y" "$work/linux-build/.config" ||
            { echo "CONFIG_$k is not enabled" >&2; exit 1; }
    done
    log "Linux: building Image and DTB"
    kmake -j "$jobs" Image freescale/imx8mp-evk-rpmsg-qemu.dtb
fi

# ----------------------------------------------------------- firmware ----
log "M7 firmware: cargo build --features qemu-imx8mp"
(cd "$here/.." && cargo build --release --features qemu-imx8mp)
fw="$here/../target/thumbv7em-none-eabihf/release/imx8mn-m7-echo"

log "A53 init"
clang --target=aarch64-linux-gnu -ffreestanding -fno-builtin -nostdlib -static \
    -fno-stack-protector -fno-pic -O2 -fuse-ld=lld -o "$work/init" "$here/init.c"

# ---------------------------------------------------------------- run ----
log "Running (console below; M7 UART4 in $work/m7-uart4.log)"
cd "$work"
"$here/run.sh" "$work/qemu-build/qemu-system-aarch64" "$kimg" "$dtb" "$work/init" "$fw" \
    "$work/m7-uart4.log" | tee console.log
echo
echo "--- M7 UART4 ---"
cat m7-uart4.log
grep -q '^\[init\] PASS' console.log
