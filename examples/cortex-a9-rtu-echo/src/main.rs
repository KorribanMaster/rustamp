// SPDX-License-Identifier: BSD-3-Clause
//! rpmsg echo firmware for the second core (CPU1) of a Cortex-A9 MPCore,
//! with Linux on CPU0.
//!
//! Linux (`linux/a9-rproc`) loads this ELF into the RTU carve-out, starts
//! CPU1 at `_start`, finds `.resource_table`, sets up the vrings and binds
//! the announced `rpmsg-tty` service to `/dev/ttyRPMSG*`. Everything
//! written to that tty comes back. Notifications are SGIs both ways: SGI 15
//! to this core, SGI 14 to Linux. On `stop`, Linux sets the park page's
//! stop request and this core parks itself (see `park.rs`), so the image
//! can be replaced and restarted without a reset.
#![no_std]
#![no_main]

mod hw;
mod rt;
#[macro_use]
mod uart;

use core::sync::atomic::Ordering;

use cortex_a9_rtu_echo::board::{self, BOARD};
use rustamp_metal::IoRegion;
use rustamp_metal::sync::RawSpinLock;
use rustamp_remoteproc::rsc::{FwRscVdev, FwRscVdevVring, ResourceTable};
use rustamp_remoteproc::{RprocVirtio, resource_table};
use rustamp_rpmsg::{ADDR_ANY, Options, Received, RpmsgVirtio, VIRTIO_RPMSG_F_NS};
use rustamp_virtio::{Transport, VIRTIO_ID_RPMSG, status};

/// Service name; `rpmsg-tty` binds the mainline tty driver.
const SERVICE: &str = "rpmsg-tty";

resource_table! {
    static RESOURCE_TABLE: RscTable = {
        vdev: FwRscVdev<2> = FwRscVdev::new(VIRTIO_ID_RPMSG, 0, 1 << VIRTIO_RPMSG_F_NS, [
            FwRscVdevVring::new(board::VRING0, board::VRING_ALIGN, board::VRING_NUM, 0),
            FwRscVdevVring::new(board::VRING1, board::VRING_ALIGN, board::VRING_NUM, 1),
        ]),
    };
}

/// Busy-wait used while polling the vdev status (only paces the loop).
fn delay_us(us: u32) {
    for _ in 0..us.saturating_mul(100) {
        core::hint::spin_loop();
    }
}

/// Called by the reset code with the MMU and caches on.
pub(crate) fn main() -> ! {
    uart::init();
    let pb = rt::periphbase();
    log!(
        "rustamp a9: up on {} (PERIPHBASE {:#x}), waiting for the host",
        BOARD.name,
        pb
    );
    hw::init(pb);
    rt::enable_irqs();

    // SAFETY: the shared region is reserved for rpmsg in the Linux device
    // tree and only accessed through this region on the RTU side.
    let shm = unsafe {
        IoRegion::new(
            board::SHM_BASE as usize as *mut u8,
            board::SHM_BASE as u64,
            board::SHM_SIZE as usize,
        )
    };
    let table = ResourceTable::new(RESOURCE_TABLE.io_region(), 0, RESOURCE_TABLE.size())
        .expect("resource table");
    let vdev = table.find_vdev(0).expect("vdev");
    let transport = RprocVirtio::new_device(vdev, shm.clone(), |_id: u32| hw::kick_host());

    // Wait for virtio_rpmsg to set DRIVER_OK, parking if Linux stops us first.
    loop {
        if hw::stop_requested() {
            log!("rustamp a9: stop requested before DRIVER_OK, parking");
            rt::park();
        }
        let s = transport.status();
        if s & status::NEEDS_RESET != 0 {
            transport.set_status(0);
        } else if s & status::DRIVER_OK != 0 {
            break;
        }
        delay_us(1000);
    }

    let opts = Options {
        delay_us,
        ..Options::default()
    };
    let rpmsg: RpmsgVirtio<_, RawSpinLock, 4> =
        RpmsgVirtio::try_new_remote(transport, shm, opts).expect("rpmsg");
    log!("rustamp a9: DRIVER_OK, ns={}", rpmsg.support_ns());
    let ept = rpmsg
        .create_endpoint(SERVICE, ADDR_ANY, ADDR_ANY)
        .expect("endpoint");
    log!("rustamp a9: announced {} at {:#x}", SERVICE, ept.addr());

    let mut echoed: u32 = 0;
    let mut bytes: u64 = 0;
    loop {
        while let Ok(Some(r)) = rpmsg.try_recv() {
            if let Received::Message(m) = r {
                if echoed == 0 {
                    log!("rustamp a9: first message from {:#x}", m.src());
                }
                // The first message also binds `ept` to the tty's address.
                if rpmsg.send_to(ept, m.src(), m.data()).is_ok() {
                    echoed += 1;
                    bytes += m.len() as u64;
                    if echoed.is_multiple_of(500) {
                        log!("rustamp a9: echoed {} messages, {} bytes", echoed, bytes);
                    }
                }
            }
        }
        if hw::stop_requested() {
            log!(
                "rustamp a9: stop requested after {} messages, parking",
                echoed
            );
            rt::park();
        }
        // Sleep until the next SGI; checking with IRQs masked closes the
        // race with the handler.
        rt::sleep_unless(|| hw::KICKED.swap(false, Ordering::AcqRel) || hw::stop_requested());
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    log!("rustamp a9: panic: {}", info);
    loop {
        // SAFETY: halts this core.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
