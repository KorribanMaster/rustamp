# openamp-rpmsg

RPMsg over virtio for [openamp-rs](../../README.md), a port of open-amp's
`lib/rpmsg`. Wire-compatible with Linux `virtio_rpmsg_bus` and C OpenAMP.

* Host (driver) and remote (device) roles: `RpmsgVirtio::new_host`,
  `RpmsgVirtio::new_remote`.
* Endpoints with automatic address allocation and name-service announce/
  destroy.
* Zero-copy receive: an `RxMessage` holds its ring buffer until dropped.
* Copying sends (`send`, `send_to`, `try_send`) and zero-copy sends
  (`get_tx_buffer` → `TxBuffer::send`).
* Feature `async`: `recv().await` and `send_async().await`, woken by
  `on_notify()` from the notification interrupt.
