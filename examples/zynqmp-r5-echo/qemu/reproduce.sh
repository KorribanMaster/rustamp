#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
#
# Builds everything from public sources and runs the rpmsg echo test on
# AMD/Xilinx QEMU's ZCU102: Linux on the A53s starts this firmware on R5_0
# through mainline xlnx_r5_remoteproc.
#
#   QEMU      AMD/Xilinx fork (A53s plus the MicroBlaze PMU, co-simulated)
#   firmware  PMU ROM, PMU firmware, ATF and U-Boot, prebuilt by AMD/Xilinx
#   Linux     mainline v7.2, clang/LLVM, ZynqMP-only config, built in:
#             xlnx_r5_remoteproc, zynqmp-ipi-mbox, rpmsg_tty
#   R5        this example built with --features qemu
#   A53       /init from examples/imx8mn-m7-echo/qemu/init.c
#
# The prebuilt binaries are downloaded at run time and never committed:
# PMU ROM (Xilinx binary licence, redistribution permitted), PMU firmware,
# ATF and U-Boot from Xilinx/soc-prebuilt-firmware.
#
# Needs: git, python3, ninja, pkg-config, glib/pixman/libgcrypt dev files,
# dtc, cpp, make, curl, clang, lld, llvm, flex, bison, bc, rustup with
# armv7r-none-eabihf.
#
# usage: reproduce.sh [WORKDIR]     (default: ./qemu-work)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
work=$(mkdir -p "${1:-qemu-work}" && cd "${1:-qemu-work}" && pwd)
jobs=$(nproc)
RELOAD_CYCLES=${RELOAD_CYCLES:-2}
# AMD/Xilinx release xlnx_rel_v2025.2.
QEMU_REV=1c85619702e00e3591bddf19bc5a738c0617b2bb
QEMU_DT_REV=075383c583d3c4ed7ed19b8dc45cd12e391eca1b
PREBUILT_REV=7319a74d19ff09ed470476fd721534d1603f04da
PMU_ROM_SHA256=f9a450ef960979463ea0a87a35fafb4a5b62d3a741de30cbcef04c8edc22a7cf
LINUX_TAG=v7.2

log() { printf '\n==> %s\n' "$*"; }
fetch_rev() { # dir url rev
    [ -d "$1/.git" ] && return
    git init -q "$1"
    git -C "$1" fetch -q --depth 1 "$2" "$3"
    git -C "$1" -c advice.detachedHead=false checkout -q FETCH_HEAD
}

# ---------------------------------------------------------------- QEMU ----
if [ ! -x "$work/qemu-build/qemu-system-aarch64" ] ||
   [ ! -x "$work/qemu-build/qemu-system-microblazeel" ]; then
    log "QEMU: AMD/Xilinx fork $QEMU_REV"
    fetch_rev "$work/qemu" https://github.com/Xilinx/qemu "$QEMU_REV"
    sub() { # Subprojects from GitHub mirrors (the wraps point at gitlab.com).
        [ -d "$work/qemu/subprojects/$1" ] && return
        git clone -q "$2" "$work/qemu/subprojects/$1"
        git -C "$work/qemu/subprojects/$1" checkout -q "$3" 2>/dev/null || true
        if [ -d "$work/qemu/subprojects/packagefiles/$1" ]; then
            cp -r "$work/qemu/subprojects/packagefiles/$1/." "$work/qemu/subprojects/$1/"
        fi
    }
    rev() { sed -n 's/^revision = //p' "$work/qemu/subprojects/$1.wrap"; }
    sub dtc https://github.com/dgibson/dtc.git "$(rev dtc)"
    sub keycodemapdb https://github.com/qemu/keycodemapdb "$(rev keycodemapdb)"
    sub berkeley-softfloat-3 https://github.com/qemu/berkeley-softfloat-3 "$(rev berkeley-softfloat-3)"
    sub berkeley-testfloat-3 https://github.com/qemu/berkeley-testfloat-3 "$(rev berkeley-testfloat-3)"
    # This QEMU's meson only finds libgcrypt through libgcrypt-config, which
    # newer distributions no longer ship; answer it from pkg-config.
    mkdir -p "$work/bin"
    cat > "$work/bin/libgcrypt-config" <<'SH'
#!/bin/sh
for a in "$@"; do
    case "$a" in
    --version) pkg-config --modversion libgcrypt ;;
    --cflags) pkg-config --cflags libgcrypt ;;
    --libs) pkg-config --libs libgcrypt ;;
    esac
done
SH
    chmod +x "$work/bin/libgcrypt-config"
    # No optional host libraries: this QEMU predates some (libnfs) and needs
    # only libgcrypt (its crypto models) and the bundled libfdt.
    mkdir -p "$work/qemu-build"
    (cd "$work/qemu-build" &&
        PATH="$work/bin:$PATH" "$work/qemu/configure" \
            --target-list=aarch64-softmmu,microblazeel-softmmu \
            --without-default-features --disable-werror --disable-download \
            --enable-fdt=internal --enable-gcrypt > configure.log &&
        ninja -j "$jobs" qemu-system-aarch64 qemu-system-microblazeel > build.log)
fi

hw="$work/qemu-devicetrees/LATEST/MULTI_ARCH"
if [ ! -f "$hw/zcu102-arm.dtb" ]; then
    log "QEMU hardware device trees $QEMU_DT_REV"
    fetch_rev "$work/qemu-devicetrees" https://github.com/Xilinx/qemu-devicetrees "$QEMU_DT_REV"
    make -s -C "$work/qemu-devicetrees" -j "$jobs" > "$work/qemu-devicetrees.log"
fi

# ------------------------------------------------------------ firmware ----
fw="$work/fw"
mkdir -p "$fw"
for f in pmufw.elf arm-trusted-firmware.elf u-boot.elf; do
    [ -f "$fw/$f" ] && continue
    log "prebuilt $f"
    curl -sfL -o "$fw/$f" \
        "https://raw.githubusercontent.com/Xilinx/soc-prebuilt-firmware/$PREBUILT_REV/zcu102-zynqmp/$f"
done
if [ ! -f "$fw/pmu-rom.elf" ]; then
    log "PMU ROM (Xilinx binary licence, see PMU_ROM-LICENSE.txt)"
    curl -sfL -o "$fw/PMU_ROM.tar.gz" \
        "https://www.xilinx.com/bin/public/openDownload?filename=PMU_ROM.tar.gz"
    echo "$PMU_ROM_SHA256  $fw/PMU_ROM.tar.gz" | sha256sum -c --quiet
    tar -xzf "$fw/PMU_ROM.tar.gz" -C "$fw"
    cp "$fw/PMU_ROM/pmu-rom.elf" "$fw/PMU_ROM/PMU_ROM-LICENSE.txt" "$fw/"
fi

# --------------------------------------------------------------- Linux ----
kimg="$work/linux-build/arch/arm64/boot/Image"
dtb="$work/linux-build/arch/arm64/boot/dts/xilinx/zynqmp-zcu102-rustamp.dtb"
if [ ! -f "$kimg" ] || [ ! -f "$dtb" ] ||
   ! cmp -s "$here/../zcu102/zynqmp-zcu102-rustamp.dts" \
            "$work/linux/arch/arm64/boot/dts/xilinx/zynqmp-zcu102-rustamp.dts"; then
    log "Linux: $LINUX_TAG"
    [ -d "$work/linux" ] ||
        git clone -q --depth 1 --branch "$LINUX_TAG" https://github.com/torvalds/linux.git "$work/linux"
    cp "$here/../zcu102/zynqmp-zcu102-rustamp.dts" "$work/linux/arch/arm64/boot/dts/xilinx/"
    kmake() { make -s -C "$work/linux" ARCH=arm64 LLVM=1 O="$work/linux-build" "$@"; }
    if [ ! -f "$work/linux-build/.config" ]; then
        kmake defconfig
        cfg="$work/linux/scripts/config --file $work/linux-build/.config"
        for p in $(sed -n 's/^config \(ARCH_[A-Z0-9_]*\)$/\1/p' "$work/linux/arch/arm64/Kconfig.platforms"); do
            [ "$p" = ARCH_ZYNQMP ] || $cfg -d "$p"
        done
        # Small, ZynqMP-only kernel with the R5 remoteproc path built in.
        $cfg -d MODULES -d NET -d DRM -d SOUND -d SND -d VIRTUALIZATION -d KVM \
            -d WLAN -d BT -d USB_SUPPORT -d MEDIA_SUPPORT -d HID -d BPF_SYSCALL \
            -d SECURITY -d FTRACE -d DEBUG_INFO_DWARF_TOOLCHAIN_DEFAULT \
            -e DEBUG_INFO_NONE -d PCI -d NFS_FS -d BTRFS_FS -d XFS_FS -d EXT4_FS \
            -d MTD -d SCSI -d ATA -d STAGING -d IIO -d FB -d SOUNDWIRE \
            -d INFINIBAND -d ACPI -d EFI -d NUMA -d HUGETLBFS -d PROFILING \
            -d KPROBES -d CPU_FREQ -d CPU_IDLE -d THERMAL -d HWMON -d MMC \
            -d NVME_CORE -d DMADEVICES -d SPI -d I2C -d PWM -d CGROUPS \
            -d NAMESPACES -d AUDIT -d SWAP -d SYSVIPC -d VIRTIO_MENU -d VHOST_MENU \
            -e ARCH_ZYNQMP -e ZYNQMP_FIRMWARE -e ZYNQMP_POWER -e ZYNQMP_IPI_MBOX \
            -e REMOTEPROC -e XLNX_R5_REMOTEPROC -e RPMSG_VIRTIO -e RPMSG_TTY \
            -e RPMSG_CTRL -e MAILBOX -e BLK_DEV_INITRD -e DEVTMPFS -e DEVTMPFS_MOUNT \
            -e SERIAL_XILINX_PS_UART -e SERIAL_XILINX_PS_UART_CONSOLE -e PM \
            -e DEBUG_FS
        kmake olddefconfig
        for k in ARCH_ZYNQMP ZYNQMP_FIRMWARE ZYNQMP_IPI_MBOX XLNX_R5_REMOTEPROC \
                 RPMSG_VIRTIO RPMSG_TTY SERIAL_XILINX_PS_UART_CONSOLE; do
            grep -q "^CONFIG_$k=y" "$work/linux-build/.config" ||
                { echo "CONFIG_$k is not enabled" >&2; exit 1; }
        done
    fi
    log "Linux: building Image and DTB"
    kmake -j "$jobs" Image xilinx/zynqmp-zcu102-rustamp.dtb
fi

# ----------------------------------------------------------- R5 and /init ----
log "R5 firmware: cargo build --release --features qemu"
(cd "$here/.." && cargo build --release --features qemu)
r5="$here/../target/armv7r-none-eabihf/release/zynqmp-r5-echo"

log "A53 init"
clang --target=aarch64-linux-gnu -ffreestanding -fno-builtin -nostdlib -static \
    -fno-stack-protector -fno-pic -O2 -fuse-ld=lld -DRELOAD_CYCLES="$RELOAD_CYCLES" \
    -o "$work/init" "$repo/examples/imx8mn-m7-echo/qemu/init.c"

# ---------------------------------------------------------------- run ----
log "Running (console below; R5 UART1 in $work/r5-uart1.log)"
cd "$work"
"$here/run.sh" "$work/qemu-build" "$hw" "$fw" "$kimg" "$dtb" "$work/init" "$r5" \
    "$work/r5-uart1.log" | tee console.log
echo
echo "--- R5 UART1 ---"
cat r5-uart1.log
grep -q '^\[init\] PASS' console.log
