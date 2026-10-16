// SPDX-License-Identifier: BSD-3-Clause
//! [`Channel`] over an rpmsg endpoint.

use openamp_metal::sync::RawLock;
use openamp_rpmsg::{Endpoint, Error, Received, RpmsgVirtio};
use openamp_virtio::Transport;

use crate::Channel;

/// A proxy channel on an rpmsg endpoint.
///
/// While waiting for a reply, the channel drains the device: messages for
/// other endpoints and name-service events that arrive meanwhile are
/// dropped. Give the proxy its own device, or route other traffic through
/// [`on_other`](Self::on_other).
pub struct RpmsgChannel<'a, T: Transport, L: RawLock, const E: usize> {
    dev: &'a RpmsgVirtio<T, L, E>,
    ept: Endpoint,
    idle: fn(),
    on_other: Option<&'a mut dyn FnMut(&Received<'_, T, L, E>)>,
}

impl<'a, T: Transport, L: RawLock, const E: usize> RpmsgChannel<'a, T, L, E> {
    /// A channel sending from `ept` to its bound destination. `idle` runs
    /// while no reply has arrived (e.g. `wfi`, or yield).
    pub fn new(dev: &'a RpmsgVirtio<T, L, E>, ept: Endpoint, idle: fn()) -> Self {
        RpmsgChannel {
            dev,
            ept,
            idle,
            on_other: None,
        }
    }

    /// Passes traffic not meant for the proxy to `f` instead of dropping it.
    pub fn on_other(mut self, f: &'a mut dyn FnMut(&Received<'_, T, L, E>)) -> Self {
        self.on_other = Some(f);
        self
    }
}

impl<T: Transport, L: RawLock, const E: usize> Channel for RpmsgChannel<'_, T, L, E> {
    type Error = Error;

    fn request(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, Error> {
        self.dev.send(self.ept, req)?;
        loop {
            match self.dev.try_recv()? {
                Some(Received::Message(m)) if m.endpoint() == self.ept => {
                    return Ok(m.copy_to(resp));
                }
                Some(other) => {
                    if let Some(f) = self.on_other.as_mut() {
                        f(&other);
                    }
                }
                None => (self.idle)(),
            }
        }
    }
}
