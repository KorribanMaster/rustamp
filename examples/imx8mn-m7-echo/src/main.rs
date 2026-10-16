// SPDX-License-Identifier: BSD-3-Clause
//! rpmsg echo firmware for the i.MX 8M Nano Cortex-M7.
//!
//! Linux loads this ELF with `imx_rproc`, finds `.resource_table`, sets up
//! the vrings at the addresses below and binds the announced `rpmsg-tty`
//! service to `/dev/ttyRPMSG*` (mainline `rpmsg_tty` driver). Everything
//! written to that tty comes back.
//!
//! Shared memory (from NXP's `imx8mn-evk-rpmsg` device tree):
//!
//! | region       | address      | size     |
//! |--------------|--------------|----------|
//! | vdev0vring0  | `0x55000000` | 32 KiB   |
//! | vdev0vring1  | `0x55008000` | 32 KiB   |
//! | vdevbuffer   | `0x55400000` | 1 MiB    |
//!
//! The M7 caches stay disabled (their reset state), so shared memory needs
//! no maintenance. If you enable them, keep the region uncached:
//! [`mpu_uncached_shared_memory`] does that.
#![no_std]
#![no_main]

mod mu;
mod rt;

use core::sync::atomic::{AtomicBool, Ordering};

use openamp_metal::IoRegion;
use openamp_metal::sync::RawSpinLock;
use openamp_remoteproc::rsc::{FwRscVdev, FwRscVdevVring, ResourceTable};
use openamp_remoteproc::{RprocVirtio, resource_table};
use openamp_rpmsg::{ADDR_ANY, Options, Received, RpmsgVirtio, VIRTIO_RPMSG_F_NS};
use openamp_virtio::VIRTIO_ID_RPMSG;

use mu::{MU_B_BASE, MU_IRQ, Mu};

const SHM_BASE: u32 = 0x5500_0000;
const SHM_SIZE: usize = 0x50_0000;
const VRING0: u32 = SHM_BASE;
const VRING1: u32 = SHM_BASE + 0x8000;
const VRING_ALIGN: u32 = 0x1000;
const VRING_NUM: u32 = 256;
/// Service name; `rpmsg-tty` binds the mainline tty driver, `rpmsg-raw`
/// the char driver (`/dev/rpmsgN` via `rpmsg_ctrl`).
const SERVICE: &str = "rpmsg-tty";

resource_table! {
    static RESOURCE_TABLE: RscTable = {
        vdev: FwRscVdev<2> = FwRscVdev::new(VIRTIO_ID_RPMSG, 0, 1 << VIRTIO_RPMSG_F_NS, [
            FwRscVdevVring::new(VRING0, VRING_ALIGN, VRING_NUM, 0),
            FwRscVdevVring::new(VRING1, VRING_ALIGN, VRING_NUM, 1),
        ]),
    };
}

static KICKED: AtomicBool = AtomicBool::new(false);
static MU: Mu = Mu::new(MU_B_BASE);

/// Busy-wait delay at the M7's 600 MHz (roughly; only paces polling).
fn delay_us(us: u32) {
    rt::delay_cycles(us.saturating_mul(600));
}

/// Called by the reset code once memory is initialised.
pub(crate) extern "C" fn main() -> ! {
    mpu_uncached_shared_memory();
    MU.enable_rx_irq();
    // SAFETY: the MU handler only touches an atomic flag.
    unsafe { nvic_unmask(MU_IRQ) };

    // SAFETY: the shared region is reserved for rpmsg in the device tree
    // and only accessed through this region on the M7 side.
    let shm = unsafe { IoRegion::new(SHM_BASE as usize as *mut u8, SHM_BASE as u64, SHM_SIZE) };
    let table = ResourceTable::new(RESOURCE_TABLE.io_region(), 0, RESOURCE_TABLE.size())
        .expect("resource table");
    let vdev = table.find_vdev(0).expect("vdev");
    let transport = RprocVirtio::new_device(vdev, shm.clone(), |id: u32| MU.send(id << 16));

    let opts = Options {
        delay_us,
        ..Options::default()
    };
    // Blocks until Linux has probed virtio_rpmsg and set DRIVER_OK.
    let rpmsg: RpmsgVirtio<_, RawSpinLock, 4> =
        RpmsgVirtio::new_remote(transport, shm, opts).expect("rpmsg");
    let ept = rpmsg
        .create_endpoint(SERVICE, ADDR_ANY, ADDR_ANY)
        .expect("endpoint");

    loop {
        while let Ok(Some(r)) = rpmsg.try_recv() {
            if let Received::Message(m) = r {
                // The first message also binds `ept` to the tty's address.
                let _ = rpmsg.send_to(ept, m.src(), m.data());
            }
        }
        // Sleep until the next kick; checking the flag with interrupts
        // masked closes the race with the handler.
        rt::sleep_unless(|| KICKED.swap(false, Ordering::AcqRel));
    }
}

/// Device interrupt `irq` fired (called from the vector table).
pub(crate) fn on_irq(irq: u16) {
    if irq == MU_IRQ {
        while MU.try_recv().is_some() {}
        KICKED.store(true, Ordering::Release);
    }
}

/// Unmasks NVIC interrupt `irq` (ISER registers).
///
/// # Safety
///
/// The interrupt's handler must be sound to run at any time.
unsafe fn nvic_unmask(irq: u16) {
    let iser = (0xE000_E100 + 4 * (irq as usize / 32)) as *mut u32;
    // SAFETY: architecturally defined NVIC register; writes of 1 set only
    // the given enable bit.
    unsafe { iser.write_volatile(1 << (irq % 32)) };
}

/// Marks the 16 MiB around the rpmsg region (0x55000000..0x56000000) as
/// normal, shareable, non-cacheable, execute-never in MPU region 0, and
/// enables the MPU with the default map for everything else.
fn mpu_uncached_shared_memory() {
    const MPU_TYPE: *const u32 = 0xE000_ED90 as *const u32;
    const MPU_CTRL: *mut u32 = 0xE000_ED94 as *mut u32;
    const MPU_RNR: *mut u32 = 0xE000_ED98 as *mut u32;
    const MPU_RBAR: *mut u32 = 0xE000_ED9C as *mut u32;
    const MPU_RASR: *mut u32 = 0xE000_EDA0 as *mut u32;
    const XN: u32 = 1 << 28;
    const AP_FULL: u32 = 0b011 << 24;
    const TEX_NORMAL_NC: u32 = 0b001 << 19; // TEX=001, C=0, B=0
    const SHAREABLE: u32 = 1 << 18;
    const SIZE_16M: u32 = (24 - 1) << 1; // 2^(SIZE+1) bytes
    const ENABLE: u32 = 1;

    // SAFETY: architecturally defined MPU registers; region 0 is not used
    // by anything else in this firmware.
    unsafe {
        if MPU_TYPE.read_volatile() >> 8 & 0xff == 0 {
            return; // no MPU
        }
        rt::barrier();
        MPU_RNR.write_volatile(0);
        MPU_RBAR.write_volatile(SHM_BASE & !0xff_ffff);
        MPU_RASR.write_volatile(XN | AP_FULL | TEX_NORMAL_NC | SHAREABLE | SIZE_16M | ENABLE);
        MPU_CTRL.write_volatile(1 << 2 | 1); // PRIVDEFENA | ENABLE
        rt::barrier();
    }
}
