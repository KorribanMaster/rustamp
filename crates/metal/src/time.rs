// SPDX-License-Identifier: BSD-3-Clause
//! Delays and timestamps (`metal_sleep_usec`, `metal_get_timestamp`).

/// A blocking microsecond delay, used while waiting for the peer.
pub trait Delay {
    /// Waits at least `us` microseconds (a busy loop is acceptable).
    fn delay_us(&self, us: u32);
}

impl<F: Fn(u32)> Delay for F {
    #[inline]
    fn delay_us(&self, us: u32) {
        self(us)
    }
}

/// A calibration-free busy wait: spins `us * spins_per_us` times.
///
/// Good enough to pace a polling loop; not a precise timer.
#[derive(Debug, Clone, Copy)]
pub struct SpinDelay {
    /// Spin-loop iterations per microsecond.
    pub spins_per_us: u32,
}

impl Default for SpinDelay {
    fn default() -> Self {
        SpinDelay { spins_per_us: 100 }
    }
}

impl Delay for SpinDelay {
    fn delay_us(&self, us: u32) {
        for _ in 0..us.saturating_mul(self.spins_per_us) {
            core::hint::spin_loop();
        }
    }
}

/// Sleeps the calling thread.
#[cfg(feature = "std")]
#[derive(Debug, Default, Clone, Copy)]
pub struct StdDelay;

#[cfg(feature = "std")]
impl Delay for StdDelay {
    fn delay_us(&self, us: u32) {
        std::thread::sleep(std::time::Duration::from_micros(us as u64));
    }
}

/// A monotonic timestamp source.
pub trait Clock {
    /// Monotonic time in microseconds.
    fn now_us(&self) -> u64;
}

#[cfg(feature = "std")]
impl Clock for std::time::Instant {
    fn now_us(&self) -> u64 {
        self.elapsed().as_micros() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    #[test]
    fn delays_and_clock() {
        // Closures are delays.
        let total = Cell::new(0);
        let d = |us: u32| total.set(total.get() + us);
        d.delay_us(3);
        Delay::delay_us(&d, 4);
        assert_eq!(total.get(), 7);

        SpinDelay::default().delay_us(10);
        // Zero iterations either way; must neither overflow nor hang.
        SpinDelay {
            spins_per_us: u32::MAX,
        }
        .delay_us(0);
        SpinDelay { spins_per_us: 0 }.delay_us(u32::MAX);

        #[cfg(feature = "std")]
        {
            let start = std::time::Instant::now();
            StdDelay.delay_us(1000);
            assert!(start.now_us() >= 1000);
        }
    }
}
