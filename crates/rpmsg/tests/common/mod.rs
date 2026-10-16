// SPDX-License-Identifier: BSD-3-Clause
//! A two-sided virtio transport over one shared region, for tests.
#![allow(dead_code, unreachable_pub)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use openamp_metal::IoRegion;
use openamp_metal::shmem::ShmemPool;
use openamp_virtio::ring::VringLayout;
use openamp_virtio::{QueueConfig, Result, Role, Transport, VIRTIO_ID_RPMSG};

pub const SHM_PHYS: u64 = 0x6000_0000;
pub const SHM_SIZE: usize = 0x10_0000;
pub const VRING_NUM: u16 = 16;

/// Status, features and queue locations, as a remoteproc vdev would hold them.
pub struct Shared {
    pub io: IoRegion,
    queues: [QueueConfig; 2],
    pub kicks: [AtomicU32; 2],
}

pub struct Sim {
    role: Role,
    pub sh: Arc<Shared>,
}

const STATUS: usize = 0;
const DFEATURES: usize = 4;
const GFEATURES: usize = 8;

impl Shared {
    pub fn new(dfeatures: u32) -> Arc<Self> {
        let io = IoRegion::alloc(SHM_SIZE, SHM_PHYS);
        io.write_u32(DFEATURES, dfeatures).unwrap();
        let layout = VringLayout::new(VRING_NUM, 4096).unwrap();
        let q = |offset, notify_id| QueueConfig {
            io: io.clone(),
            offset,
            layout,
            notify_id,
        };
        let queues = [q(0x1000, 0), q(0x1000 + 0x4000, 1)];
        Arc::new(Shared {
            io,
            queues,
            kicks: [AtomicU32::new(0), AtomicU32::new(0)],
        })
    }

    /// Buffer pool for the host: everything after the rings.
    pub fn pool(&self) -> ShmemPool {
        ShmemPool::new(self.io.clone(), 0x10000, SHM_SIZE - 0x10000).unwrap()
    }

    pub fn host(self: &Arc<Self>) -> Sim {
        Sim {
            role: Role::Driver,
            sh: self.clone(),
        }
    }

    pub fn remote(self: &Arc<Self>) -> Sim {
        Sim {
            role: Role::Device,
            sh: self.clone(),
        }
    }
}

impl Transport for Sim {
    fn role(&self) -> Role {
        self.role
    }
    fn device_id(&self) -> u32 {
        VIRTIO_ID_RPMSG
    }
    fn status(&self) -> u8 {
        self.sh.io.load_u8(STATUS, Ordering::Acquire).unwrap()
    }
    fn set_status(&self, status: u8) {
        self.sh
            .io
            .store_u8(STATUS, status, Ordering::Release)
            .unwrap()
    }
    fn device_features(&self) -> u64 {
        self.sh.io.read_u32(DFEATURES).unwrap() as u64
    }
    fn driver_features(&self) -> u64 {
        self.sh.io.read_u32(GFEATURES).unwrap() as u64
    }
    fn set_driver_features(&self, f: u64) -> Result<()> {
        self.sh.io.write_u32(GFEATURES, f as u32)?;
        Ok(())
    }
    fn read_config(&self, _: usize, _: &mut [u8]) -> Result<()> {
        Ok(())
    }
    fn write_config(&self, _: usize, _: &[u8]) -> Result<()> {
        Ok(())
    }
    fn queue_count(&self) -> usize {
        2
    }
    fn queue(&self, index: usize) -> Result<QueueConfig> {
        self.sh
            .queues
            .get(index)
            .cloned()
            .ok_or(openamp_virtio::Error::NoSuchQueue)
    }
    fn notify(&self, index: usize) {
        self.sh.kicks[index].fetch_add(1, Ordering::SeqCst);
    }
}
