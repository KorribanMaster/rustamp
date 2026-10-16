# openamp-remoteproc

Remote processor support for [openamp-rs](../../README.md), a port of
open-amp's `lib/remoteproc`.

* Firmware: `resource_table!` declares the table in `.resource_table` with
  compiler-computed offsets; `RprocVirtio::new_device` turns its vdev into a
  virtio transport.
* Hosts: `Remoteproc` loads ELF32/ELF64 images through `ImageStore`
  (streaming, no full-image buffer), parses and validates the resource
  table, assigns notify ids, allocates vrings and returns the driver-side
  `RprocVirtio`; lifecycle `config` → `load` → `start` → `stop` →
  `shutdown`.
* `rsc::ResourceTable`: a bounds-checked parser usable on both sides.
