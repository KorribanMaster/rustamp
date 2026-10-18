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

#[cfg(not(feature = "qemu-imx8mp"))]
use panic_halt as _;

/// In the QEMU build, report panics on UART4 before halting.
#[cfg(feature = "qemu-imx8mp")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log!("openamp-rs m7: {}", info);
    loop {
        cortex_m::asm::wfi();
    }
}

mod mu;
#[cfg(feature = "qemu-imx8mp")]
#[macro_use]
mod uart;

use core::sync::atomic::{AtomicBool, Ordering};

use cortex_m_rt::{entry, exception};
use openamp_metal::IoRegion;
use openamp_metal::sync::CsLock;
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
    // Without an MU interrupt (QEMU), keep draining kicks while waiting so
    // the host's mailbox TX never stalls.
    #[cfg(feature = "qemu-imx8mp")]
    while MU.try_recv().is_some() {}
    cortex_m::asm::delay(us.saturating_mul(600));
}

/// Logs to UART4 in the QEMU build; compiled out otherwise.
#[cfg(not(feature = "qemu-imx8mp"))]
macro_rules! log {
    ($($t:tt)*) => {{
        if false {
            let _ = format_args!($($t)*);
        }
    }};
}

#[entry]
fn main() -> ! {
    #[cfg(feature = "qemu-imx8mp")]
    {
        uart::init();
        log!("openamp-rs m7: up, waiting for the host (DRIVER_OK)");
    }
    #[cfg(not(feature = "qemu-imx8mp"))]
    {
        let mut p = cortex_m::Peripherals::take().unwrap();
        mpu_uncached_shared_memory(&mut p.MPU);
        MU.enable_rx_irq();
        // SAFETY: the MU handler only touches the MU and an atomic flag.
        unsafe { cortex_m::peripheral::NVIC::unmask(MU_IRQ) };
    }

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
    // CsLock keeps rpmsg correct even if it is called from an interrupt.
    let rpmsg: RpmsgVirtio<_, CsLock, 4> =
        RpmsgVirtio::new_remote(transport, shm, opts).expect("rpmsg");
    log!("openamp-rs m7: DRIVER_OK, ns={}", rpmsg.support_ns());
    let ept = rpmsg
        .create_endpoint(SERVICE, ADDR_ANY, ADDR_ANY)
        .expect("endpoint");
    log!("openamp-rs m7: announced {} at {:#x}", SERVICE, ept.addr());

    let mut echoed: u32 = 0;
    let mut bytes: u64 = 0;
    loop {
        while let Ok(Some(r)) = rpmsg.try_recv() {
            if let Received::Message(m) = r {
                if echoed == 0 {
                    log!("openamp-rs m7: first message from {:#x}", m.src());
                }
                // The first message also binds `ept` to the tty's address.
                if rpmsg.send_to(ept, m.src(), m.data()).is_ok() {
                    echoed += 1;
                    bytes += m.len() as u64;
                    if echoed.is_multiple_of(500) {
                        log!("openamp-rs m7: echoed {} messages, {} bytes", echoed, bytes);
                    }
                }
            }
        }
        #[cfg(feature = "qemu-imx8mp")]
        while MU.try_recv().is_some() {}
        // Sleep until the next kick. Checking the flag with interrupts
        // masked closes the race with the handler: a kick arriving after
        // the check still ends the WFI, and is taken once `free` unmasks.
        #[cfg(not(feature = "qemu-imx8mp"))]
        cortex_m::interrupt::free(|_| {
            if !KICKED.swap(false, Ordering::AcqRel) {
                cortex_m::asm::dsb();
                cortex_m::asm::wfi();
            }
        });
    }
}

/// Every device interrupt lands here (cortex-m-rt without a device crate).
#[exception]
unsafe fn DefaultHandler(irqn: i16) {
    if irqn == MU_IRQ.0 as i16 {
        while MU.try_recv().is_some() {}
        KICKED.store(true, Ordering::Release);
    }
}

/// Marks the 16 MiB around the rpmsg region (0x55000000..0x56000000) as
/// normal, shareable, non-cacheable, execute-never in MPU region 0, and
/// enables the MPU with the default map for everything else.
#[cfg_attr(feature = "qemu-imx8mp", allow(dead_code))]
fn mpu_uncached_shared_memory(mpu: &mut cortex_m::peripheral::MPU) {
    const XN: u32 = 1 << 28;
    const AP_FULL: u32 = 0b011 << 24;
    const TEX_NORMAL_NC: u32 = 0b001 << 19; // TEX=001, C=0, B=0
    const SHAREABLE: u32 = 1 << 18;
    const SIZE_16M: u32 = (24 - 1) << 1; // 2^(SIZE+1) bytes
    const ENABLE: u32 = 1;
    const PRIVDEFENA: u32 = 1 << 2;

    if mpu._type.read() >> 8 & 0xff == 0 {
        return; // no MPU
    }
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    // SAFETY: region 0 is not used by anything else in this firmware, and
    // the default map stays in place for everything outside it.
    unsafe {
        mpu.rnr.write(0);
        mpu.rbar.write(SHM_BASE & !0xff_ffff);
        mpu.rasr
            .write(XN | AP_FULL | TEX_NORMAL_NC | SHAREABLE | SIZE_16M | ENABLE);
        mpu.ctrl.write(PRIVDEFENA | ENABLE);
    }
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}
