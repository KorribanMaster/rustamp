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

## Features

None are on by default.

| Feature | Adds |
|---|---|
| `async` | `recv().await`, `send_async().await` (uses `atomic-waker`) |
| `log` | Diagnostics through the [`log`](https://docs.rs/log) facade |
| `defmt` | Diagnostics through [`defmt`](https://docs.rs/defmt), and `defmt::Format` on the error types (if both are on, `log` is used) |

To call rpmsg from an interrupt handler, use `openamp_metal::sync::CsLock` as
the lock type (feature `openamp-metal/critical-section`).
