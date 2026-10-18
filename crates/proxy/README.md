# rustamp-proxy

File and console forwarding for [RustAmp](../../README.md), a port of
open-amp's `lib/proxy` (`rpmsg_retarget`), wire-compatible with the C
implementation and `rpmsg-proxy-app`.

* `ProxyClient` (remote, `no_std`): `open`, `read`, `write`, `close`, and
  `stdout()` implementing `core::fmt::Write`.
* `RpmsgChannel` (feature `rpmsg`, default): runs the client over an rpmsg
  endpoint.
* `server::ProxyServer` (feature `std`): executes requests inside a root
  directory, translating newlib `open` flags.
* `ProxyFile` (feature `embedded-io`): a host file implementing
  `embedded_io::{Read, Write}`, from `ProxyClient::open_file`.

## Features

| Feature | Adds |
|---|---|
| `rpmsg` (default) | `RpmsgChannel` |
| `std` | The host server |
| `embedded-io` | `ProxyFile` (uses `embedded-io`) |
| `log`, `defmt` | Forwarded to the rpmsg stack; `defmt::Format` on `Error` |
