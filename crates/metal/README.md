# openamp-metal

Hardware and OS abstraction for [openamp-rs](../../README.md), a port of
[libmetal](https://github.com/OpenAMP/libmetal). `no_std`, no dependencies.

* `IoRegion`: bounds-checked volatile, atomic and block access to shared
  memory or MMIO, with physical/device address translation.
* `cache`: `Cache` trait; Cortex-M7 (`cortex-m7`), ARMv7-A (`armv7a`) and
  AArch64 implementations.
* `sync`: `Mutex` over a pluggable `RawLock`, interrupt-safe `AtomicWaker`.
* `notify`, `time`, `irq`: platform hooks (`Notifier`, `Delay`,
  `IrqController`, `IrqTable`).
* `shmem::ShmemPool`: bump allocator for message buffers and vrings.
* `log`: pluggable handler (`metal_log`).
* `linux` (feature): UIO devices and shared-memory files via `mmap`.

Features: `alloc`, `std`, `linux`, `cortex-m7`, `armv7a`.
