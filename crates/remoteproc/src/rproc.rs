// SPDX-License-Identifier: BSD-3-Clause
//! Remote processor lifecycle (`remoteproc.c`).

use openamp_metal::notify::Notifier;
use openamp_metal::shmem::ShmemPool;
use openamp_metal::{IoRegion, metal_warn};
use openamp_virtio::ring::VringLayout;

use crate::elf::{self, ImageStore};
use crate::rsc::{Entry, FW_RSC_ADDR_ANY, RSC_NOTIFY_ID_ANY, ResourceTable, RscType};
use crate::{Error, Result, RprocVirtio};

/// Platform operations for one remote processor (`struct remoteproc_ops`).
pub trait RemoteprocOps {
    /// Maps `size` bytes at device address `da` (the remote's view) for the
    /// host to access, e.g. a TCM alias or a DDR carve-out.
    fn mmap(&mut self, da: u64, size: usize) -> Result<IoRegion>;

    /// Releases the remote from reset at `entry`.
    fn start(&mut self, entry: u64) -> Result<()>;

    /// Halts the remote.
    fn stop(&mut self) -> Result<()>;

    /// Halts the remote and releases platform resources.
    fn shutdown(&mut self) -> Result<()> {
        self.stop()
    }

    /// Prepares the platform before loading (clocks, power domains).
    fn config(&mut self) -> Result<()> {
        Ok(())
    }

    /// Handles a vendor resource (types 128..=512).
    fn handle_vendor_rsc(&mut self, table: &ResourceTable, entry: &Entry) -> Result<()> {
        let _ = table;
        match entry.rsc_type {
            RscType::Vendor(t) | RscType::Unknown(t) => Err(Error::UnsupportedResource(t)),
            _ => Ok(()),
        }
    }
}

/// Lifecycle state (`enum remoteproc_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Powered off, nothing loaded.
    Offline,
    /// Platform configured.
    Configured,
    /// Firmware loaded, ready to start.
    Ready,
    /// Running.
    Running,
    /// Suspended.
    Suspended,
    /// Crashed; needs recovery.
    Error,
    /// Stopped after running.
    Stopped,
}

/// What [`Remoteproc::load`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadInfo {
    /// Entry point.
    pub entry: u64,
    /// Device address and size of the resource table, if present.
    pub resource_table: Option<(u64, usize)>,
}

/// A remote processor managed from this side.
pub struct Remoteproc<O: RemoteprocOps> {
    ops: O,
    state: State,
    rsc: Option<ResourceTable>,
    entry: u64,
    next_notifyid: u32,
}

impl<O: RemoteprocOps> Remoteproc<O> {
    /// A remote in the `Offline` state (`remoteproc_init`).
    pub fn new(ops: O) -> Self {
        Remoteproc {
            ops,
            state: State::Offline,
            rsc: None,
            entry: 0,
            next_notifyid: 0,
        }
    }

    /// Current state.
    pub fn state(&self) -> State {
        self.state
    }

    /// The platform operations.
    pub fn ops(&self) -> &O {
        &self.ops
    }

    /// The platform operations, mutably.
    pub fn ops_mut(&mut self) -> &mut O {
        &mut self.ops
    }

    /// The parsed resource table.
    pub fn resource_table(&self) -> Option<&ResourceTable> {
        self.rsc.as_ref()
    }

    /// Configures the platform (`remoteproc_config`).
    pub fn config(&mut self) -> Result<()> {
        if self.state != State::Offline {
            return Err(Error::InvalidState);
        }
        self.ops.config()?;
        self.state = State::Configured;
        Ok(())
    }

    /// Loads an ELF image and its resource table (`remoteproc_load`).
    pub fn load(&mut self, img: &mut impl ImageStore) -> Result<LoadInfo> {
        if !matches!(
            self.state,
            State::Offline | State::Configured | State::Stopped
        ) {
            return Err(Error::InvalidState);
        }
        let h = elf::header(img)?;
        let ops = &mut self.ops;
        elf::load_segments(img, &h, |da, size| ops.mmap(da, size))?;
        let rsc = elf::find_section(img, &h, b".resource_table")?;
        let resource_table = match rsc {
            Some(s) => {
                let len = usize::try_from(s.size).map_err(|_| Error::InvalidImage)?;
                let io = self.ops.mmap(s.addr, len)?;
                self.set_resource_table(io, 0, len)?;
                Some((s.addr, len))
            }
            None => {
                self.rsc = None;
                None
            }
        };
        self.entry = h.entry;
        self.state = State::Ready;
        Ok(LoadInfo {
            entry: h.entry,
            resource_table,
        })
    }

    /// Uses a resource table that is already in place (firmware loaded by a
    /// boot loader, or `remoteproc_set_rsc_table`), handling its entries.
    pub fn set_resource_table(&mut self, io: IoRegion, base: usize, len: usize) -> Result<()> {
        let t = ResourceTable::new(io, base, len)?;
        self.handle_resources(&t)?;
        self.rsc = Some(t);
        if matches!(self.state, State::Offline | State::Configured) {
            self.state = State::Ready;
        }
        Ok(())
    }

    fn alloc_notifyid(&mut self) -> u32 {
        let id = self.next_notifyid;
        self.next_notifyid += 1;
        id
    }

    fn handle_resources(&mut self, t: &ResourceTable) -> Result<()> {
        for e in t.entries() {
            let e = e?;
            match e.rsc_type {
                RscType::Carveout | RscType::Devmem => {
                    let c = t.carveout(&e)?;
                    if c.da == FW_RSC_ADDR_ANY {
                        // Host-allocated carve-outs need an allocator this
                        // crate does not own; let the platform handle them.
                        self.ops.handle_vendor_rsc(t, &e)?;
                    } else {
                        self.ops.mmap(c.da as u64, c.len as usize)?;
                    }
                }
                RscType::Trace => {}
                RscType::Vdev => {
                    let v = t.vdev(&e)?;
                    if v.notifyid() == RSC_NOTIFY_ID_ANY {
                        let id = self.alloc_notifyid();
                        v.set_notifyid(id)?;
                    }
                    for i in 0..v.num_vrings() {
                        if v.vring(i)?.notifyid == RSC_NOTIFY_ID_ANY {
                            let id = self.alloc_notifyid();
                            v.set_vring_notifyid(i, id)?;
                        }
                    }
                }
                RscType::Vendor(_) => self.ops.handle_vendor_rsc(t, &e)?,
                RscType::Unknown(ty) => {
                    // Linux warns and skips unknown types; so do we.
                    metal_warn!("remoteproc: skipping unknown resource type {}", ty);
                }
            }
        }
        Ok(())
    }

    /// Prepares the `n`th vdev for the host side: allocates vrings the
    /// firmware left to the host (`da == FW_RSC_ADDR_ANY`) from `pool`,
    /// writes their addresses into the table, and returns the driver-side
    /// transport.
    ///
    /// `shm` must cover the vrings and buffers, addressed in the remote's
    /// device-address space.
    pub fn create_vdev<N: Notifier>(
        &mut self,
        n: usize,
        shm: IoRegion,
        mut pool: Option<&mut ShmemPool>,
        notifier: N,
    ) -> Result<RprocVirtio<N>> {
        let t = self.rsc.as_ref().ok_or(Error::NoResourceTable)?;
        let v = t.find_vdev(n)?;
        for i in 0..v.num_vrings() {
            let r = v.vring(i)?;
            if r.da != FW_RSC_ADDR_ANY {
                continue;
            }
            let pool = pool.as_deref_mut().ok_or(Error::NoMemory)?;
            let num = u16::try_from(r.num).map_err(|_| Error::InvalidTable)?;
            let layout = VringLayout::new(num, r.align as usize)?;
            let off = pool
                .alloc(layout.size(), layout.align().max(16))
                .map_err(|_| Error::NoMemory)?;
            pool.io().fill(off, 0, layout.size())?;
            let phys = pool.io().offset_to_phys(off).ok_or(Error::NoMemory)?;
            shm.phys_range_to_offset(phys, layout.size())?;
            let da = u32::try_from(phys).map_err(|_| Error::NoMemory)?;
            v.set_vring_da(i, da)?;
        }
        Ok(RprocVirtio::new_driver(v, shm, notifier))
    }

    /// Starts the loaded firmware (`remoteproc_start`).
    pub fn start(&mut self) -> Result<()> {
        if self.state != State::Ready {
            return Err(Error::InvalidState);
        }
        self.ops.start(self.entry)?;
        self.state = State::Running;
        Ok(())
    }

    /// Stops the remote (`remoteproc_stop`).
    pub fn stop(&mut self) -> Result<()> {
        if !matches!(
            self.state,
            State::Running | State::Suspended | State::Error
        ) {
            return Err(Error::InvalidState);
        }
        self.ops.stop()?;
        self.state = State::Stopped;
        Ok(())
    }

    /// Stops the remote if needed, shuts it down and forgets the loaded
    /// firmware (`remoteproc_shutdown`).
    pub fn shutdown(&mut self) -> Result<()> {
        if self.state == State::Offline {
            return Ok(());
        }
        self.ops.shutdown()?;
        self.rsc = None;
        self.state = State::Offline;
        Ok(())
    }

    /// Records that the remote crashed; [`stop`](Self::stop) then recovers.
    pub fn report_crash(&mut self) {
        if self.state == State::Running {
            self.state = State::Error;
        }
    }
}
