# openamp-proxy

File and console forwarding for [openamp-rs](../../README.md), a port of
open-amp's `lib/proxy` (`rpmsg_retarget`), wire-compatible with the C
implementation and `rpmsg-proxy-app`.

* `ProxyClient` (remote, `no_std`): `open`, `read`, `write`, `close`, and
  `stdout()` implementing `core::fmt::Write`.
* `RpmsgChannel` (feature `rpmsg`, default): runs the client over an rpmsg
  endpoint.
* `server::ProxyServer` (feature `std`): executes requests inside a root
  directory, translating newlib `open` flags.
