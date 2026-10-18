# openamp-metal

Hardware and OS abstraction for [openamp-rs](../../README.md), a port of
[libmetal](https://github.com/OpenAMP/libmetal). `no_std`; no dependencies
with default features.

* `IoRegion`: bounds-checked volatile, atomic and block access to shared
  memory or MMIO, with physical/device address translation.
* `cache`: `Cache` trait; Cortex-M7 (`cortex-m7`), ARMv7-A (`armv7a`) and
  AArch64 implementations.
* `sync`: `Mutex` over a pluggable `RawLock` (spin lock, or `CsLock` with the
  `critical-section` feature).
* `notify`, `time`, `irq`: platform hooks (`Notifier`, `Delay`,
  `IrqController`, `IrqTable`).
* `shmem::ShmemPool`: bump allocator for message buffers and vrings.
* `linux` (feature): UIO devices and shared-memory files via `mmap`.

## Features

None are on by default.

| Feature | Adds | Dependencies |
|---|---|---|
| `alloc` | `IoRegion::alloc`, for host-side tests | |
| `std` | `alloc`, thread-sleeping delay, `std` error impls | |
| `linux` | `std`, the `linux` backend (UIO, shared-memory files) | `memmap2`, `libc` |
| `critical-section` | `sync::CsLock`, for rpmsg used from interrupt handlers | `critical-section` |
| `cortex-m7`, `armv7a` | Cache maintenance for that core | |
| `log`, `defmt` | Forwarded to by the higher crates; `defmt::Format` on `Error` | `defmt` |
