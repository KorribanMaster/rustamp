// SPDX-License-Identifier: BSD-3-Clause
//! rpmsg echo firmware for the Cortex-R5F (R5_0) of a Zynq UltraScale+
//! MPSoC, with Linux on the A53s.
//!
//! Linux (`xlnx_r5_remoteproc`) loads this ELF into the TCMs and the
//! `rproc_0_fw_image` carve-out, finds `.resource_table`, allocates the
//! vrings from the `vdev0vring*` carve-outs and boots the R5 from 0. The
//! firmware announces `rpmsg-tty`, so Linux creates `/dev/ttyRPMSG0`, and
//! everything written there comes back. Notifications use IPI channel 1
//! (this core) and the host's channel; `stop` is a reset by Linux, so the
//! firmware needs no stop protocol.
#![no_std]
#![no_main]

mod hw;
mod trace;
#[macro_use]
mod uart;

#[cfg(not(feature = "qemu"))]
use core::sync::atomic::Ordering;

use rustamp_metal::IoRegion;
use rustamp_metal::sync::CsLock;
use rustamp_remoteproc::rsc::{
    FW_RSC_ADDR_ANY, FwRscTrace, FwRscVdev, FwRscVdevVring, ResourceTable,
};
use rustamp_remoteproc::{RprocVirtio, resource_table};
use rustamp_rpmsg::{ADDR_ANY, Options, Received, RpmsgVirtio, VIRTIO_RPMSG_F_NS};
use rustamp_virtio::VIRTIO_ID_RPMSG;
use zynqmp_r5_echo::board;

/// Service name; `rpmsg-tty` binds the mainline tty driver.
const SERVICE: &str = "rpmsg-tty";

// As upstream OpenAMP's zynqmp_r5 table: vrings placed by Linux (`ANY`),
// notify ids 1 and 2 (Linux reassigns them), and a trace buffer.
resource_table! {
    static RESOURCE_TABLE: RscTable = {
        vdev: FwRscVdev<2> = FwRscVdev::new(VIRTIO_ID_RPMSG, 31, 1 << VIRTIO_RPMSG_F_NS, [
            FwRscVdevVring::new(FW_RSC_ADDR_ANY, board::VRING_ALIGN, board::VRING_NUM, 1),
            FwRscVdevVring::new(FW_RSC_ADDR_ANY, board::VRING_ALIGN, board::VRING_NUM, 2),
        ]),
        trace: FwRscTrace = FwRscTrace::new("r5_trace", board::TRACE_DA, board::TRACE_LEN),
    };
}

/// Busy-wait used while waiting for the host (only paces the loop).
fn delay_us(us: u32) {
    // Without the IPI interrupt (QEMU), keep acknowledging kicks while
    // waiting, so the host's mailbox never stalls.
    #[cfg(feature = "qemu")]
    hw::poll_kick();
    for _ in 0..us.saturating_mul(100) {
        core::hint::spin_loop();
    }
}

#[aarch32_rt::entry]
fn main() -> ! {
    uart::init();
    log!("rustamp r5: up, waiting for the host (DRIVER_OK)");
    hw::init_mpu();
    hw::init_irq();
    // SAFETY: the handler only touches the IPI, the GIC and an atomic flag.
    #[cfg(not(feature = "qemu"))]
    unsafe {
        aarch32_cpu::interrupt::enable()
    };

    // SAFETY: the shared region is reserved for rpmsg in the Linux device
    // tree and only accessed through this region on the R5.
    let shm = unsafe {
        IoRegion::new(
            board::SHM_BASE as usize as *mut u8,
            board::SHM_BASE as u64,
            board::SHM_SIZE as usize,
        )
    };
    // Linux writes the table before booting us, but a restart can catch it
    // mid-write; wait for a valid one rather than giving up.
    let mut reported = false;
    let vdev = loop {
        let table = ResourceTable::new(RESOURCE_TABLE.io_region(), 0, RESOURCE_TABLE.size());
        match table.and_then(|t| t.find_vdev(0)) {
            Ok(vdev) => break vdev,
            Err(e) => {
                if !reported {
                    log!("rustamp r5: resource table not ready ({}), waiting", e);
                    reported = true;
                }
                delay_us(1000);
            }
        }
    };
    let transport = RprocVirtio::new_device(vdev, shm.clone(), |_id: u32| hw::kick_host());

    let opts = Options {
        delay_us,
        ..Options::default()
    };
    // Blocks until Linux has probed virtio_rpmsg and set DRIVER_OK.
    // CsLock keeps rpmsg correct even if it is called from an interrupt.
    let rpmsg: RpmsgVirtio<_, CsLock, 4> =
        RpmsgVirtio::new_remote(transport, shm, opts).expect("rpmsg");
    log!("rustamp r5: DRIVER_OK, ns={}", rpmsg.support_ns());
    let ept = rpmsg
        .create_endpoint(SERVICE, ADDR_ANY, ADDR_ANY)
        .expect("endpoint");
    log!("rustamp r5: announced {} at {:#x}", SERVICE, ept.addr());

    let mut echoed: u32 = 0;
    let mut bytes: u64 = 0;
    loop {
        while let Ok(Some(r)) = rpmsg.try_recv() {
            if let Received::Message(m) = r {
                if echoed == 0 {
                    log!("rustamp r5: first message from {:#x}", m.src());
                }
                // The first message also binds `ept` to the tty's address.
                if rpmsg.send_to(ept, m.src(), m.data()).is_ok() {
                    echoed += 1;
                    bytes += m.len() as u64;
                    if echoed.is_multiple_of(500) {
                        log!("rustamp r5: echoed {} messages, {} bytes", echoed, bytes);
                    }
                }
            }
        }
        // QEMU: poll the IPI instead of sleeping on its interrupt.
        #[cfg(feature = "qemu")]
        while !hw::poll_kick() {
            core::hint::spin_loop();
        }
        // Sleep until the next kick. Checking the flag with interrupts
        // masked closes the race with the handler: a kick arriving after
        // the check still ends the WFI, and is taken once `free` unmasks.
        #[cfg(not(feature = "qemu"))]
        aarch32_cpu::interrupt::free(|| {
            if !hw::KICKED.swap(false, Ordering::AcqRel) {
                aarch32_cpu::asm::dsb();
                aarch32_cpu::asm::wfi();
            }
        });
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    log!("rustamp r5: panic: {}", info);
    loop {
        aarch32_cpu::asm::wfi();
    }
}
