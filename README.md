# RustAmp

RustAmp is a Rust port of the [OpenAMP](https://www.openampproject.org/) C
libraries [open-amp](https://github.com/OpenAMP/open-amp) and
[libmetal](https://github.com/OpenAMP/libmetal): remoteproc, rpmsg, virtio,
the I/O proxy, and the hardware abstraction they sit on. It is `no_std`,
needs no allocator and has no dependencies with default features.

**It aims for maximum compatibility with OpenAMP.** It follows OpenAMP's
design and protocol behaviour, uses the same wire formats and resource
tables, and interoperates with the Linux kernel (remoteproc, `rpmsg_tty`,
`rpmsg_char`) and with C OpenAMP in both directions; CI tests that against
the C libraries. Where it deliberately differs, it says so
([below](#behaviour-that-differs-from-c-on-purpose)).

**About the name.** RustAmp is an independent project, not part of or
endorsed by the OpenAMP project, a Linaro Community Project. It is called
RustAmp, and its crates `rustamp-*`, so that the OpenAMP name stays with
the official project.

| Crate | Replaces | What it does |
|---|---|---|
| [`rustamp-metal`](crates/metal) | libmetal | `IoRegion` (bounds-checked shared memory/MMIO), cache maintenance, locks, notifier/delay traits, shared-memory pool, Linux UIO backend |
| [`rustamp-virtio`](crates/virtio) | `lib/virtio`, `lib/virtio_mmio` | Split virtqueues in driver and device roles, event-idx, `Transport` trait, virtio-mmio driver |
| [`rustamp-rpmsg`](crates/rpmsg) | `lib/rpmsg` | Endpoints, name service, host and remote roles, zero-copy receive and send, optional `async` |
| [`rustamp-remoteproc`](crates/remoteproc) | `lib/remoteproc` | `resource_table!` for firmware, table parser, streaming ELF loader, lifecycle, resource-table virtio transport |
| [`rustamp-proxy`](crates/proxy) | `lib/proxy` | `rpmsg_retarget`-compatible file/console forwarding: `no_std` client, sandboxed host server |

```text
rustamp-proxy ──► rustamp-rpmsg ──┐
                                  ├──► rustamp-virtio ──► rustamp-metal
rustamp-remoteproc ───────────────┘          ▲
        └─────────── supplies the virtio Transport that rpmsg runs over
```

## Firmware in five lines

```rust,ignore
resource_table! {
    static RESOURCE_TABLE: RscTable = {
        vdev: FwRscVdev<2> = FwRscVdev::new(VIRTIO_ID_RPMSG, 0, 1 << VIRTIO_RPMSG_F_NS, [
            FwRscVdevVring::new(0x5500_0000, 0x1000, 256, 0),
            FwRscVdevVring::new(0x5500_8000, 0x1000, 256, 1),
        ]),
    };
}

let table = ResourceTable::new(RESOURCE_TABLE.io_region(), 0, RESOURCE_TABLE.size())?;
let transport = RprocVirtio::new_device(table.find_vdev(0)?, shm.clone(), |id| mailbox.kick(id));
let rpmsg: RpmsgVirtio<_> = RpmsgVirtio::new_remote(transport, shm, Options::default())?;
let ept = rpmsg.create_endpoint("rpmsg-tty", ADDR_ANY, ADDR_ANY)?;
while let Some(Received::Message(m)) = rpmsg.try_recv()? {
    rpmsg.send_to(ept, m.src(), m.data())?; // echo; the buffer returns when `m` drops
}
```

A complete example for the i.MX 8M Nano/Plus Cortex-M7 driven by Linux
`imx_rproc` is in [`examples/imx8mn-m7-echo`](examples/imx8mn-m7-echo). It
runs under QEMU (`imx8mp-evk` with the Cortex-M7 AMP series) via
[`qemu/reproduce.sh`](examples/imx8mn-m7-echo/qemu/reproduce.sh).

For a Cortex-A9 MPCore with Linux on CPU0 and the firmware on CPU1
(Zynq-7000, Arria 10), see [`examples/cortex-a9-rtu-echo`](examples/cortex-a9-rtu-echo)
and its Linux driver [`linux/a9-rproc`](linux/a9-rproc); it runs on
upstream QEMU's `xilinx-zynq-a9`.

## Status

Version 0.1, built against open-amp and libmetal 1.10 (main, October 2026).

| Check | Result |
|---|---|
| Unit and integration tests (`cargo test --workspace --all-features`) | 71 pass, including 100k-message threaded echoes with and without event-idx |
| Property tests (proptest) for every parser of peer-controlled data, and a model of the virtqueue pair | Part of the above; no panics on hostile rings, headers, tables or images |
| Fuzzing (`fuzz/`: ELF + resource table, device virtqueue, rpmsg receive) | Clean after five minutes per target; found one ELF offset overflow, now fixed |
| Struct layouts against the C headers (`interop-tests`) | Sizes and offsets of the resource-table and rpmsg wire structures match |
| Rust host ↔ C open-amp remote, C host ↔ Rust remote (`interop-tests`) | 20,000 checked round trips each way |
| Firmware ELF from the example, loaded by this crate's ELF loader | Entry, segments and resource table as expected |
| Compiles for `thumbv7em-none-eabihf`, `armv7a-none-eabi`, `aarch64-unknown-none` | Yes, including the cache-maintenance asm |
| Linux 7.2 `imx_rproc` + `rpmsg_tty` ↔ our firmware on QEMU `imx8mp-evk` with the Cortex-M7 AMP series ([details](examples/imx8mn-m7-echo/qemu/README.md)) | 2,000 checked round trips through `/dev/ttyRPMSG0` |
| Linux 7.2 `a9-rproc` + `rpmsg_tty` ↔ our firmware on CPU1 of QEMU `xilinx-zynq-a9` ([details](examples/cortex-a9-rtu-echo/qemu/README.md)) | 6,000 checked round trips, including two stop/start reloads |
| Cortex-A9 firmware and driver for Arria 10 | Build and device tree compile only |
| Linux kernel remoteproc on real hardware (i.MX 8M Nano/Plus, Zynq-7000, Arria 10) | Not yet run |
| Miri (whole workspace) | Clean, run in CI |
| loom (`RawSpinLock`, `IrqTable`) | Clean, run in CI; it found a race in `IrqTable`, now fixed |
| Feature combinations (`cargo hack --feature-powerset --depth 2`), `cargo deny` | Clean, run in CI |

## Design notes

* **One `unsafe` boundary.** Raw pointers to shared memory exist only inside
  `IoRegion`. Everything the peer writes — ring indices, descriptor heads,
  chain links, buffer addresses and lengths, resource-table offsets — is
  validated and turns into an error, never a panic or an out-of-bounds
  access.
* **Ownership in types.** A received message holds its ring buffer until
  dropped; a device-side descriptor (`AvailBuf`) is consumed when returned,
  so it cannot be returned twice.
* **No allocator; minimal, optional dependencies.** Sizes are const
  generics. The core path (default features) depends on nothing outside the
  workspace; optional features pull in the ecosystem standards where they
  replace hand-written unsafe code or add something: `critical-section`,
  `log`/`defmt`, `atomic-waker` (`async`), `memmap2` (Linux backend),
  `embedded-io` (proxy files).
* **Pluggable locks.** The default spin lock (`RawLock`) is right as long as
  rpmsg is never called from an interrupt handler, which the API steers
  towards (the handler just calls `on_notify` or sets a flag). If you do
  call rpmsg from an interrupt handler, use `CsLock` (feature
  `rustamp-metal/critical-section`).
* **Polled first, async on top.** `try_recv`/`send` work everywhere; the
  `async` feature adds `recv().await` and `send_async().await` for Embassy,
  RTIC or any executor.

### Behaviour that differs from C, on purpose

* Endpoint addresses outside 1024..1152 that are not fixed services are
  rejected, as in C, but clashes among fixed addresses below 1024 are also
  rejected.
* The proxy server confines file access to a root directory and translates
  newlib `open` flags (`FlagEncoding`); the C demo passes both through.
* `RpmsgVirtio::try_recv` returns unbound name-service announcements as
  events instead of calling a `ns_bind_cb`.

### Interop findings

* The v2 QEMU Cortex-M7 AMP series' MU model compares byte offsets with
  register indices, which breaks Linux's `imx-mailbox`. A fix is in
  `examples/imx8mn-m7-echo/qemu/patches`; see that README for the other
  QEMU findings.

* C open-amp's remoteproc vdev never sets `vdev->features`, so a C remote
  that advertises `VIRTIO_RING_F_EVENT_IDX` in its resource table will not
  actually use event-idx. Rust peers negotiate it whenever it is offered;
  pair them with C remotes that do not offer it.

## Repository layout

```text
crates/          the five library crates
interop-tests/   Rust ↔ C tests; builds pinned C libmetal + open-amp with cmake
fuzz/            cargo-fuzz targets for the parsers of peer-controlled data
examples/        imx8mn-m7-echo: Cortex-M7 firmware on cortex-m-rt
                 cortex-a9-rtu-echo: Cortex-A9 CPU1 firmware (Zynq-7000, Arria 10)
linux/           a9-rproc: remoteproc driver for a spare Cortex-A9 core (GPL-2.0)
.github/         CI: tests, lints, embedded cross-builds, interop; nightly QEMU runs
```

## Testing

```sh
cargo test --workspace --all-features
./interop-tests/fetch-c-sources.sh
cargo test --manifest-path interop-tests/Cargo.toml --release
RUSTFLAGS="--cfg loom" cargo test -p rustamp-metal --test loom --release
cargo +nightly miri test --workspace --all-features   # MIRIFLAGS=-Zmiri-disable-isolation
(cd fuzz && cargo +nightly fuzz run rpmsg_recv -- -detect_leaks=0)
```

`PROPTEST_CASES` raises the number of cases the property tests run.

## License

BSD-3-Clause, like the C libraries this is derived from; see
[LICENSE](LICENSE) and [NOTICE](NOTICE). Exceptions: the Linux driver in
`linux/a9-rproc` is GPL-2.0-only (its binding GPL-2.0-only OR BSD-2-Clause),
and the example device trees, which include Linux's, are GPL-2.0.

OpenAMP is a Linaro Community Project. RustAmp is independent of it; the
OpenAMP name is used only to say what this is a port of and compatible
with.
