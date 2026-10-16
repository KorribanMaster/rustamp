# openamp-virtio

Virtio for [openamp-rs](../../README.md), a port of open-amp's `lib/virtio`
and `lib/virtio_mmio`.

* `ring::VringLayout`: the split-ring layout (identical to Linux's legacy
  vring used by remoteproc).
* `DriverQueue` / `DeviceQueue`: both roles, chained descriptors, event-idx
  and flag-based notification suppression. Indices and descriptors written
  by the peer are validated.
* `Transport`: status, features, config space, queue discovery, notify.
* `mmio::MmioTransport` (feature `mmio`, default): virtio-mmio v1 and v2,
  driver side.
