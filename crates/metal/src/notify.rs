// SPDX-License-Identifier: BSD-3-Clause
//! Inter-processor notifications (the "kick").
//!
//! A [`Notifier`] raises an interrupt on the peer: a mailbox write, an IPI, an
//! MSI, a GPIO toggle. The `id` is the virtqueue or vdev notify id taken from
//! the resource table, which the peer uses to find the queue to service.

/// Raises a notification on the peer processor.
pub trait Notifier {
    /// Signals the peer that queue (or vdev) `id` needs attention.
    fn notify(&self, id: u32);
}

impl<F: Fn(u32)> Notifier for F {
    #[inline]
    fn notify(&self, id: u32) {
        self(id)
    }
}

/// A notifier that does nothing, for peers that poll.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoNotify;

impl Notifier for NoNotify {
    #[inline]
    fn notify(&self, _: u32) {}
}
