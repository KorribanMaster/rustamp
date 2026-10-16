// SPDX-License-Identifier: BSD-3-Clause
//! The rpmsg virtio device (`rpmsg_virtio.c` and `rpmsg.c`).

use core::fmt;

use openamp_metal::cache::{Cache, NoCache};
use openamp_metal::shmem::ShmemPool;
use openamp_metal::sync::{Mutex, RawLock, RawSpinLock};
use openamp_metal::time::{Delay, SpinDelay};
use openamp_metal::{IoRegion, metal_warn};
use openamp_virtio::{
    AvailBuf, Buffer, DeviceQueue, DriverQueue, Role, Transport, features, status,
};

use crate::wire::{HDR_SIZE, Header, NS_CREATE, NS_DESTROY, NS_MSG_SIZE, NsMsg};
use crate::{
    ADDR_ANY, ADDR_BMP_SIZE, DEFAULT_BUFFER_SIZE, Error, NS_EPT_ADDR, Name, RESERVED_ADDRESSES,
    Result, VIRTIO_RPMSG_F_NS,
};

/// Buffer sizes for the host (driver) role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostConfig {
    /// Size of each host-to-remote (transmit) buffer, header included.
    pub h2r_buf_size: u32,
    /// Size of each remote-to-host (receive) buffer, header included.
    pub r2h_buf_size: u32,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            h2r_buf_size: DEFAULT_BUFFER_SIZE,
            r2h_buf_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

/// Platform hooks and timing.
#[derive(Clone, Copy)]
pub struct Options {
    /// Cache maintenance for message buffers (not rings). Flushes before a
    /// buffer is handed to the peer, invalidates after one comes back.
    pub cache: &'static dyn Cache,
    /// Sleep used while waiting for the peer.
    pub delay_us: fn(u32),
    /// How long a blocking send waits for a free buffer (C: 15 s).
    pub tx_timeout_us: u32,
    /// Poll interval while waiting (C: 1 ms).
    pub poll_interval_us: u32,
}

fn spin_delay(us: u32) {
    SpinDelay::default().delay_us(us)
}

impl Default for Options {
    fn default() -> Self {
        Options {
            cache: &NoCache,
            delay_us: spin_delay,
            tx_timeout_us: 15_000_000,
            poll_interval_us: 1000,
        }
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("tx_timeout_us", &self.tx_timeout_us)
            .field("poll_interval_us", &self.poll_interval_us)
            .finish_non_exhaustive()
    }
}

/// Handle to a local endpoint.
///
/// A handle is a slot index plus the endpoint's address; using a handle
/// after [`RpmsgVirtio::destroy_endpoint`] fails with
/// [`Error::InvalidEndpoint`] unless the slot was reused for the same
/// address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Endpoint {
    slot: u8,
    addr: u32,
}

impl Endpoint {
    /// Local address of the endpoint.
    pub fn addr(&self) -> u32 {
        self.addr
    }
}

/// Kind of a name-service announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsKind {
    /// The peer created a service.
    Create,
    /// The peer destroyed a service.
    Destroy,
}

/// A name-service announcement for which no local endpoint was bound
/// (`ns_bind_cb`), or a destruction (`ns_unbind_cb`).
///
/// To accept a created service, create an endpoint with the same name and
/// `dst = event.addr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NsEvent {
    /// Service name.
    pub name: Name,
    /// Remote address of the service.
    pub addr: u32,
    /// Created or destroyed.
    pub kind: NsKind,
}

#[derive(Clone, Copy)]
struct EptEntry {
    name: Name,
    addr: u32,
    dest: u32,
}

enum Queue {
    Driver(DriverQueue),
    Device(DeviceQueue),
}

impl Queue {
    fn kick_needed(&mut self) -> Result<bool> {
        Ok(match self {
            Queue::Driver(q) => q.kick_needed()?,
            Queue::Device(q) => q.kick_needed()?,
        })
    }
}

/// Ownership of one ring buffer.
enum Token {
    /// A buffer the host owns (address and size).
    Host { addr: u64, len: u32 },
    /// A descriptor taken from the avail ring (remote role).
    Device(AvailBuf),
}

impl Token {
    fn addr_len(&self) -> (u64, u32) {
        match self {
            Token::Host { addr, len } => (*addr, *len),
            Token::Device(b) => (b.desc().addr, b.desc().len),
        }
    }
}

const RECLAIM: usize = 16;

struct State<const E: usize> {
    rvq: Queue,
    svq: Queue,
    epts: [Option<EptEntry>; E],
    bitmap: [u32; (ADDR_BMP_SIZE / 32) as usize],
    bitnext: u32,
    pool: Option<ShmemPool>,
    tx_allocated: u16,
    reclaim: [Option<Token>; RECLAIM],
}

impl<const E: usize> State<E> {
    fn ept(&mut self, ep: Endpoint) -> Result<&mut EptEntry> {
        match self.epts.get_mut(ep.slot as usize) {
            Some(Some(e)) if e.addr == ep.addr => Ok(e),
            _ => Err(Error::InvalidEndpoint),
        }
    }

    fn bit(&self, b: u32) -> bool {
        self.bitmap[(b / 32) as usize] & (1 << (b % 32)) != 0
    }

    fn set_bit(&mut self, b: u32, v: bool) {
        let w = &mut self.bitmap[(b / 32) as usize];
        if v {
            *w |= 1 << (b % 32);
        } else {
            *w &= !(1 << (b % 32));
        }
    }

    fn push_reclaim(&mut self, t: Token) -> core::result::Result<(), Token> {
        match self.reclaim.iter_mut().find(|s| s.is_none()) {
            Some(slot) => {
                *slot = Some(t);
                Ok(())
            }
            None => Err(t),
        }
    }

    fn pop_reclaim(&mut self) -> Option<Token> {
        self.reclaim.iter_mut().find_map(|s| s.take())
    }
}

/// An rpmsg device over a virtio transport.
///
/// * `T`: the virtio transport (a remoteproc vdev, virtio-mmio, ...).
/// * `L`: the lock protecting internal state; see
///   [`openamp_metal::sync`] for when to replace the default spin lock.
/// * `E`: maximum number of local endpoints.
///
/// All methods take `&self`, so the device can be shared between threads or
/// tasks.
pub struct RpmsgVirtio<T: Transport, L: RawLock = RawSpinLock, const E: usize = 8> {
    transport: T,
    shm: IoRegion,
    role: Role,
    support_ns: bool,
    h2r_buf_size: u32,
    opts: Options,
    state: Mutex<L, State<E>>,
    #[cfg(feature = "async")]
    rx_waker: openamp_metal::sync::AtomicWaker,
    #[cfg(feature = "async")]
    tx_waker: openamp_metal::sync::AtomicWaker,
}

impl<T: Transport, L: RawLock, const E: usize> fmt::Debug for RpmsgVirtio<T, L, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpmsgVirtio")
            .field("role", &self.role)
            .field("support_ns", &self.support_ns)
            .field("shm", &self.shm)
            .finish_non_exhaustive()
    }
}

impl<T: Transport, L: RawLock, const E: usize> RpmsgVirtio<T, L, E> {
    /// Sets up the host (virtio driver) side.
    ///
    /// Negotiates features, initialises both rings, carves one receive
    /// buffer per receive descriptor out of `pool` and posts them, then sets
    /// `DRIVER_OK`. Transmit buffers are carved lazily, up to the transmit
    /// ring size. Every buffer in `pool` must lie inside `shm`, the region
    /// buffer addresses are translated through.
    pub fn new_host(
        transport: T,
        shm: IoRegion,
        mut pool: ShmemPool,
        cfg: HostConfig,
        opts: Options,
    ) -> Result<Self> {
        if transport.role() != Role::Driver {
            return Err(Error::InvalidParam);
        }
        let min = HDR_SIZE as u32 + 1;
        if cfg.h2r_buf_size < min || cfg.r2h_buf_size < min {
            return Err(Error::InvalidParam);
        }
        transport.set_status(status::ACKNOWLEDGE);
        transport.set_status(status::ACKNOWLEDGE | status::DRIVER);
        let wanted = (1u64 << VIRTIO_RPMSG_F_NS) | (1u64 << features::RING_EVENT_IDX);
        let accepted = transport.device_features() & wanted;
        transport.set_driver_features(accepted)?;
        let event_idx = features::has(accepted, features::RING_EVENT_IDX);

        let mut rvq = DriverQueue::new(&transport.queue(0)?, event_idx)?;
        let mut svq = DriverQueue::new(&transport.queue(1)?, event_idx)?;
        let r2h = cfg.r2h_buf_size;
        for _ in 0..rvq.size() {
            let off = pool
                .alloc(r2h as usize, 16)
                .map_err(|_| Error::NoBuffer)?;
            pool.io().fill(off, 0, r2h as usize)?;
            let addr = pool.io().offset_to_phys(off).ok_or(Error::InvalidBuffer)?;
            shm.phys_range_to_offset(addr, r2h as usize)
                .map_err(|_| Error::InvalidBuffer)?;
            rvq.add(&[Buffer {
                addr,
                len: r2h,
                device_writable: true,
            }])?;
        }
        // Transmit completions are collected by polling (as in C).
        svq.disable_interrupts()?;

        let dev = Self::build(
            transport,
            shm,
            Role::Driver,
            features::has(accepted, VIRTIO_RPMSG_F_NS),
            cfg.h2r_buf_size,
            opts,
            Queue::Driver(rvq),
            Queue::Driver(svq),
            Some(pool),
        );
        dev.transport
            .set_status(status::ACKNOWLEDGE | status::DRIVER | status::DRIVER_OK);
        {
            let mut st = dev.state.lock();
            if st.rvq.kick_needed()? {
                dev.transport.notify(0);
            }
        }
        Ok(dev)
    }

    /// Sets up the remote (virtio device) side, waiting until the host has
    /// set `DRIVER_OK` (`rpmsg_virtio_wait_remote_ready`).
    pub fn new_remote(transport: T, shm: IoRegion, opts: Options) -> Result<Self> {
        loop {
            let s = transport.status();
            if s & status::NEEDS_RESET != 0 {
                transport.set_status(0);
            } else if s & status::DRIVER_OK != 0 {
                break;
            }
            (opts.delay_us)(opts.poll_interval_us);
        }
        Self::try_new_remote(transport, shm, opts)
    }

    /// Like [`new_remote`](Self::new_remote) but fails with
    /// [`Error::NotReady`] instead of waiting.
    pub fn try_new_remote(transport: T, shm: IoRegion, opts: Options) -> Result<Self> {
        if transport.role() != Role::Device {
            return Err(Error::InvalidParam);
        }
        if transport.status() & status::DRIVER_OK == 0 {
            return Err(Error::NotReady);
        }
        let negotiated = transport.features();
        let event_idx = features::has(negotiated, features::RING_EVENT_IDX);
        let mut svq = DeviceQueue::new(&transport.queue(0)?, event_idx)?;
        let rvq = DeviceQueue::new(&transport.queue(1)?, event_idx)?;
        // Don't ask the host to notify us of new transmit buffers; a blocked
        // sender polls (as in C).
        svq.disable_notifications()?;
        Ok(Self::build(
            transport,
            shm,
            Role::Device,
            features::has(negotiated, VIRTIO_RPMSG_F_NS),
            0,
            opts,
            Queue::Device(rvq),
            Queue::Device(svq),
            None,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        transport: T,
        shm: IoRegion,
        role: Role,
        support_ns: bool,
        h2r_buf_size: u32,
        opts: Options,
        rvq: Queue,
        svq: Queue,
        pool: Option<ShmemPool>,
    ) -> Self {
        RpmsgVirtio {
            transport,
            shm,
            role,
            support_ns,
            h2r_buf_size,
            opts,
            state: Mutex::new(State {
                rvq,
                svq,
                epts: [None; E],
                bitmap: [0; (ADDR_BMP_SIZE / 32) as usize],
                bitnext: 0,
                pool,
                tx_allocated: 0,
                reclaim: [const { None }; RECLAIM],
            }),
            #[cfg(feature = "async")]
            rx_waker: openamp_metal::sync::AtomicWaker::new(),
            #[cfg(feature = "async")]
            tx_waker: openamp_metal::sync::AtomicWaker::new(),
        }
    }

    /// Our virtio role.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Whether name-service announcements were negotiated.
    pub fn support_ns(&self) -> bool {
        self.support_ns
    }

    // Queue indices: queue 0 carries remote→host messages, queue 1
    // host→remote, whichever side we are.
    fn rvq_index(&self) -> usize {
        match self.role {
            Role::Driver => 0,
            Role::Device => 1,
        }
    }

    fn svq_index(&self) -> usize {
        1 - self.rvq_index()
    }

    fn kick(&self, q: &mut Queue, index: usize) -> Result<()> {
        if q.kick_needed()? {
            self.transport.notify(index);
        }
        Ok(())
    }

    fn ptr(&self, offset: usize) -> *const u8 {
        self.shm.offset_to_virt(offset).unwrap_or(core::ptr::null_mut())
    }

    // ---- endpoints -------------------------------------------------------

    /// Creates a local endpoint (`rpmsg_create_ept`).
    ///
    /// * `src`: local address, or [`ADDR_ANY`] to allocate one from
    ///   1024..1152.
    /// * `dst`: remote address, or [`ADDR_ANY`] if not yet known. Messages
    ///   from the peer bind it on first receipt.
    ///
    /// If the endpoint is named, name service is negotiated and `dst` is
    /// [`ADDR_ANY`], the name is announced to the peer.
    pub fn create_endpoint(&self, name: &str, src: u32, dst: u32) -> Result<Endpoint> {
        let name = Name::new(name)?;
        let ep = {
            let mut st = self.state.lock();
            let slot = st
                .epts
                .iter()
                .position(Option::is_none)
                .ok_or(Error::NoEndpointSlot)?;
            let addr = self.alloc_addr(&mut st, src)?;
            st.epts[slot] = Some(EptEntry {
                name,
                addr,
                dest: dst,
            });
            Endpoint {
                slot: slot as u8,
                addr,
            }
        };
        if !name.is_empty() && self.support_ns && dst == ADDR_ANY {
            if let Err(e) = self.send_ns(&name, ep.addr, NS_CREATE) {
                self.unregister(ep);
                return Err(e);
            }
        }
        Ok(ep)
    }

    fn alloc_addr(&self, st: &mut State<E>, src: u32) -> Result<u32> {
        if src == ADDR_ANY {
            for i in 0..ADDR_BMP_SIZE {
                let b = (st.bitnext + i) % ADDR_BMP_SIZE;
                if !st.bit(b) {
                    st.set_bit(b, true);
                    st.bitnext = (b + 1) % ADDR_BMP_SIZE;
                    return Ok(RESERVED_ADDRESSES + b);
                }
            }
            Err(Error::InvalidAddress)
        } else if src >= RESERVED_ADDRESSES {
            let b = src - RESERVED_ADDRESSES;
            if b >= ADDR_BMP_SIZE || st.bit(b) {
                return Err(Error::InvalidAddress);
            }
            st.set_bit(b, true);
            Ok(src)
        } else {
            // Fixed services: trust the author, but refuse obvious clashes.
            let clash = (self.support_ns && src == NS_EPT_ADDR)
                || st.epts.iter().flatten().any(|e| e.addr == src);
            if clash {
                return Err(Error::InvalidAddress);
            }
            Ok(src)
        }
    }

    fn unregister(&self, ep: Endpoint) -> Option<EptEntry> {
        let mut st = self.state.lock();
        let e = *st.ept(ep).ok()?;
        st.epts[ep.slot as usize] = None;
        if e.addr >= RESERVED_ADDRESSES && e.addr - RESERVED_ADDRESSES < ADDR_BMP_SIZE {
            st.set_bit(e.addr - RESERVED_ADDRESSES, false);
        }
        Some(e)
    }

    /// Destroys a local endpoint (`rpmsg_destroy_ept`), announcing it to the
    /// peer if it was announced on creation.
    pub fn destroy_endpoint(&self, ep: Endpoint) -> Result<()> {
        let e = *self.state.lock().ept(ep)?;
        if !e.name.is_empty() && self.support_ns && e.addr >= RESERVED_ADDRESSES {
            // Best effort, as in C: the endpoint goes away regardless.
            let _ = self.send_ns(&e.name, e.addr, NS_DESTROY);
        }
        self.unregister(ep);
        Ok(())
    }

    /// Remote address bound to the endpoint, if any.
    pub fn endpoint_dest(&self, ep: Endpoint) -> Result<Option<u32>> {
        let d = self.state.lock().ept(ep)?.dest;
        Ok((d != ADDR_ANY).then_some(d))
    }

    /// Binds the endpoint to remote address `dst`.
    pub fn set_endpoint_dest(&self, ep: Endpoint, dst: u32) -> Result<()> {
        self.state.lock().ept(ep)?.dest = dst;
        Ok(())
    }

    /// Whether the endpoint knows its remote address (`is_rpmsg_ept_ready`).
    pub fn is_ready(&self, ep: Endpoint) -> bool {
        matches!(self.endpoint_dest(ep), Ok(Some(_)))
    }

    /// Name of the endpoint.
    pub fn endpoint_name(&self, ep: Endpoint) -> Result<Name> {
        Ok(self.state.lock().ept(ep)?.name)
    }

    fn send_ns(&self, name: &Name, addr: u32, flags: u32) -> Result<()> {
        let m = NsMsg {
            name: *name,
            addr,
            flags,
        };
        self.send_raw(addr, NS_EPT_ADDR, &m.to_bytes(), true)
            .map(|_| ())
    }

    // ---- transmit --------------------------------------------------------

    fn take_tx(&self, st: &mut State<E>) -> Result<Option<Token>> {
        if let Some(t) = st.pop_reclaim() {
            return Ok(Some(t));
        }
        match &mut st.svq {
            Queue::Driver(q) => {
                if let Some(u) = q.pop_used()? {
                    return Ok(Some(Token::Host {
                        addr: u.addr,
                        len: u.buf_len,
                    }));
                }
                if q.num_free() > 0 && st.tx_allocated < q.size() {
                    if let Some(pool) = st.pool.as_mut() {
                        if let Ok(off) = pool.alloc(self.h2r_buf_size as usize, 16) {
                            let addr =
                                pool.io().offset_to_phys(off).ok_or(Error::InvalidBuffer)?;
                            st.tx_allocated += 1;
                            return Ok(Some(Token::Host {
                                addr,
                                len: self.h2r_buf_size,
                            }));
                        }
                    }
                }
                Ok(None)
            }
            Queue::Device(q) => Ok(q.pop_avail()?.map(Token::Device)),
        }
    }

    /// Takes a transmit buffer to fill in place
    /// (`rpmsg_get_tx_payload_buffer`). With `wait`, polls for up to
    /// [`Options::tx_timeout_us`].
    pub fn get_tx_buffer(&self, wait: bool) -> Result<TxBuffer<'_, T, L, E>> {
        if self.transport.status() & status::DRIVER_OK == 0 {
            return Err(Error::NotReady);
        }
        let mut ticks = if wait {
            self.opts.tx_timeout_us / self.opts.poll_interval_us.max(1)
        } else {
            0
        };
        loop {
            let token = {
                let mut st = self.state.lock();
                self.take_tx(&mut st)?
            };
            if let Some(token) = token {
                return TxBuffer::new(self, token);
            }
            if ticks == 0 {
                return Err(if wait { Error::Timeout } else { Error::NoBuffer });
            }
            ticks -= 1;
            (self.opts.delay_us)(self.opts.poll_interval_us);
        }
    }

    fn enqueue_tx(&self, token: Token) -> Result<()> {
        let mut st = self.state.lock();
        let idx = self.svq_index();
        match (&mut st.svq, token) {
            (Queue::Driver(q), Token::Host { addr, len }) => {
                q.add(&[Buffer {
                    addr,
                    len,
                    device_writable: false,
                }])?;
            }
            (Queue::Device(q), Token::Device(b)) => {
                let len = b.desc().len;
                q.push_used(b, len)?;
            }
            _ => return Err(Error::InvalidParam),
        }
        self.kick(&mut st.svq, idx)
    }

    fn send_raw(&self, src: u32, dst: u32, data: &[u8], wait: bool) -> Result<usize> {
        if data.len() > u16::MAX as usize {
            return Err(Error::MessageTooLarge);
        }
        let mut tx = self.get_tx_buffer(wait)?;
        if data.len() > tx.capacity() {
            return Err(Error::MessageTooLarge);
        }
        tx.write(0, data)?;
        tx.send_offchannel(src, dst, data.len())
    }

    fn dest_of(&self, ep: Endpoint) -> Result<(u32, u32)> {
        let mut st = self.state.lock();
        let e = st.ept(ep)?;
        if e.dest == ADDR_ANY {
            return Err(Error::NotReady);
        }
        Ok((e.addr, e.dest))
    }

    /// Sends `data` to the endpoint's bound destination, waiting for a
    /// buffer if necessary (`rpmsg_send`).
    pub fn send(&self, ep: Endpoint, data: &[u8]) -> Result<usize> {
        let (src, dst) = self.dest_of(ep)?;
        self.send_raw(src, dst, data, true)
    }

    /// Sends without waiting (`rpmsg_trysend`).
    pub fn try_send(&self, ep: Endpoint, data: &[u8]) -> Result<usize> {
        let (src, dst) = self.dest_of(ep)?;
        self.send_raw(src, dst, data, false)
    }

    /// Sends `data` from `ep` to address `dst` (`rpmsg_sendto`).
    pub fn send_to(&self, ep: Endpoint, dst: u32, data: &[u8]) -> Result<usize> {
        let src = self.state.lock().ept(ep)?.addr;
        self.send_raw(src, dst, data, true)
    }

    /// Like [`send_to`](Self::send_to) without waiting.
    pub fn try_send_to(&self, ep: Endpoint, dst: u32, data: &[u8]) -> Result<usize> {
        let src = self.state.lock().ept(ep)?.addr;
        self.send_raw(src, dst, data, false)
    }

    /// Sends with explicit source and destination addresses
    /// (`rpmsg_send_offchannel_raw`).
    pub fn send_offchannel(&self, src: u32, dst: u32, data: &[u8], wait: bool) -> Result<usize> {
        self.send_raw(src, dst, data, wait)
    }

    // ---- receive ---------------------------------------------------------

    fn release_rx(&self, st: &mut State<E>, token: Token) -> Result<()> {
        let (addr, len) = token.addr_len();
        if let Ok(off) = self.shm.phys_range_to_offset(addr, len as usize) {
            self.opts.cache.invalidate(self.ptr(off), len as usize);
        }
        match (&mut st.rvq, token) {
            (Queue::Driver(q), Token::Host { addr, len }) => {
                q.add(&[Buffer {
                    addr,
                    len,
                    device_writable: true,
                }])?;
            }
            (Queue::Device(q), Token::Device(b)) => {
                let len = b.desc().len;
                q.push_used(b, len)?;
            }
            _ => return Err(Error::InvalidParam),
        }
        self.kick(&mut st.rvq, self.rvq_index())
    }

    /// Takes the next received message or name-service event, if any.
    ///
    /// Messages for unknown endpoints are dropped (their buffers are
    /// returned at once), as in C. A message's buffer is returned to the
    /// peer when the [`RxMessage`] is dropped.
    pub fn try_recv(&self) -> Result<Option<Received<'_, T, L, E>>> {
        loop {
            let mut st = self.state.lock();
            let token = match &mut st.rvq {
                Queue::Driver(q) => q.pop_used()?.map(|u| Token::Host {
                    addr: u.addr,
                    len: u.buf_len,
                }),
                Queue::Device(q) => q.pop_avail()?.map(Token::Device),
            };
            let Some(token) = token else {
                return Ok(None);
            };
            let (addr, blen) = token.addr_len();
            let off = match self.shm.phys_range_to_offset(addr, blen as usize) {
                Ok(o) if blen as usize >= HDR_SIZE => o,
                _ => {
                    self.release_rx(&mut st, token)?;
                    return Err(Error::InvalidBuffer);
                }
            };
            self.opts.cache.invalidate(self.ptr(off), blen as usize);
            let mut hb = [0u8; HDR_SIZE];
            self.shm.read_bytes(off, &mut hb)?;
            let hdr = Header::from_bytes(&hb);
            if hdr.len as usize > blen as usize - HDR_SIZE {
                metal_warn!("rpmsg: dropping message with bad length {}", hdr.len);
                self.release_rx(&mut st, token)?;
                continue;
            }

            if hdr.dst == NS_EPT_ADDR && self.support_ns {
                let mut nb = [0u8; NS_MSG_SIZE];
                let msg = if hdr.len as usize == NS_MSG_SIZE {
                    self.shm.read_bytes(off + HDR_SIZE, &mut nb)?;
                    NsMsg::from_bytes(&nb)
                } else {
                    None
                };
                self.release_rx(&mut st, token)?;
                let Some(m) = msg else { continue };
                let kind = if m.flags & NS_DESTROY != 0 {
                    NsKind::Destroy
                } else {
                    NsKind::Create
                };
                let bound = st.epts.iter_mut().flatten().find(|e| e.name == m.name);
                match (kind, bound) {
                    (NsKind::Create, Some(e)) => {
                        e.dest = m.addr;
                        continue;
                    }
                    (NsKind::Destroy, Some(e)) => e.dest = ADDR_ANY,
                    _ => {}
                }
                return Ok(Some(Received::NameService(NsEvent {
                    name: m.name,
                    addr: m.addr,
                    kind,
                })));
            }

            let found = st
                .epts
                .iter_mut()
                .enumerate()
                .find_map(|(i, e)| e.as_mut().filter(|e| e.addr == hdr.dst).map(|e| (i, e)));
            let Some((slot, e)) = found else {
                self.release_rx(&mut st, token)?;
                continue;
            };
            if e.dest == ADDR_ANY {
                e.dest = hdr.src;
            }
            let ept = Endpoint {
                slot: slot as u8,
                addr: e.addr,
            };
            drop(st);
            return Ok(Some(Received::Message(RxMessage {
                dev: self,
                token: Some(token),
                offset: off + HDR_SIZE,
                len: hdr.len,
                src: hdr.src,
                dst: hdr.dst,
                ept,
            })));
        }
    }

    /// Whether received messages are waiting.
    pub fn has_pending(&self) -> bool {
        let st = self.state.lock();
        match &st.rvq {
            Queue::Driver(q) => q.has_used().unwrap_or(false),
            Queue::Device(q) => q.has_avail().unwrap_or(false),
        }
    }

    /// Call from the notification interrupt (or after polling it): wakes
    /// tasks blocked in the `async` API. Without the `async` feature this is
    /// a no-op, so it can be called unconditionally.
    pub fn on_notify(&self) {
        #[cfg(feature = "async")]
        {
            self.rx_waker.wake();
            self.tx_waker.wake();
        }
    }
}

#[cfg(feature = "async")]
impl<T: Transport, L: RawLock, const E: usize> RpmsgVirtio<T, L, E> {
    /// Waits for the next message or name-service event.
    pub async fn recv(&self) -> Result<Received<'_, T, L, E>> {
        core::future::poll_fn(|cx| {
            self.rx_waker.register(cx.waker());
            match self.try_recv() {
                Ok(Some(r)) => core::task::Poll::Ready(Ok(r)),
                Ok(None) => core::task::Poll::Pending,
                Err(e) => core::task::Poll::Ready(Err(e)),
            }
        })
        .await
    }

    fn set_tx_notifications(&self, on: bool) -> Result<bool> {
        let mut st = self.state.lock();
        Ok(match (&mut st.svq, on) {
            (Queue::Driver(q), true) => q.enable_interrupts()?,
            (Queue::Driver(q), false) => {
                q.disable_interrupts()?;
                false
            }
            (Queue::Device(q), true) => q.enable_notifications()?,
            (Queue::Device(q), false) => {
                q.disable_notifications()?;
                false
            }
        })
    }

    /// Sends to `dst` (or the bound destination when `dst` is `None`),
    /// waiting asynchronously for a free buffer.
    pub async fn send_async(&self, ep: Endpoint, dst: Option<u32>, data: &[u8]) -> Result<usize> {
        let (src, bound) = {
            let mut st = self.state.lock();
            let e = st.ept(ep)?;
            (e.addr, e.dest)
        };
        let dst = match dst {
            Some(d) => d,
            None if bound != ADDR_ANY => bound,
            None => return Err(Error::NotReady),
        };
        let mut armed = false;
        let r = core::future::poll_fn(|cx| {
            self.tx_waker.register(cx.waker());
            match self.send_raw(src, dst, data, false) {
                Err(Error::NoBuffer) => {
                    match self.set_tx_notifications(true) {
                        Ok(pending) => {
                            armed = true;
                            if pending {
                                cx.waker().wake_by_ref();
                            }
                        }
                        Err(e) => return core::task::Poll::Ready(Err(e)),
                    }
                    core::task::Poll::Pending
                }
                other => core::task::Poll::Ready(other),
            }
        })
        .await;
        if armed {
            self.set_tx_notifications(false)?;
        }
        r
    }
}

/// What [`RpmsgVirtio::try_recv`] returns.
pub enum Received<'a, T: Transport, L: RawLock = RawSpinLock, const E: usize = 8> {
    /// A message for a local endpoint.
    Message(RxMessage<'a, T, L, E>),
    /// A name-service announcement to act on.
    NameService(NsEvent),
}

impl<T: Transport, L: RawLock, const E: usize> fmt::Debug for Received<'_, T, L, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Received::Message(m) => f.debug_tuple("Message").field(m).finish(),
            Received::NameService(e) => f.debug_tuple("NameService").field(e).finish(),
        }
    }
}

/// A received message, held in its ring buffer until dropped
/// (`rpmsg_hold_rx_buffer` / `rpmsg_release_rx_buffer`).
pub struct RxMessage<'a, T: Transport, L: RawLock = RawSpinLock, const E: usize = 8> {
    dev: &'a RpmsgVirtio<T, L, E>,
    token: Option<Token>,
    offset: usize,
    len: u16,
    src: u32,
    dst: u32,
    ept: Endpoint,
}

impl<T: Transport, L: RawLock, const E: usize> RxMessage<'_, T, L, E> {
    /// Sender address.
    pub fn src(&self) -> u32 {
        self.src
    }

    /// Destination address (the local endpoint's address).
    pub fn dst(&self) -> u32 {
        self.dst
    }

    /// The local endpoint the message is for.
    pub fn endpoint(&self) -> Endpoint {
        self.ept
    }

    /// Payload length.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Whether the payload is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The payload, in place in shared memory.
    pub fn data(&self) -> &[u8] {
        // SAFETY: we own the descriptor (taken from the ring, not yet
        // returned), so by the virtio protocol the peer does not access the
        // buffer until `Drop` gives it back; the bounds were checked against
        // the buffer when the message was taken.
        unsafe { self.dev.shm.as_slice(self.offset, self.len as usize) }.unwrap_or(&[])
    }

    /// Copies the payload into `out`; returns the number of bytes copied.
    pub fn copy_to(&self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len as usize);
        if self.dev.shm.read_bytes(self.offset, &mut out[..n]).is_err() {
            return 0;
        }
        n
    }
}

impl<T: Transport, L: RawLock, const E: usize> fmt::Debug for RxMessage<'_, T, L, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RxMessage")
            .field("src", &self.src)
            .field("dst", &self.dst)
            .field("len", &self.len)
            .finish()
    }
}

impl<T: Transport, L: RawLock, const E: usize> Drop for RxMessage<'_, T, L, E> {
    fn drop(&mut self) {
        if let Some(t) = self.token.take() {
            let mut st = self.dev.state.lock();
            if let Err(e) = self.dev.release_rx(&mut st, t) {
                metal_warn!("rpmsg: failed to release rx buffer: {}", e);
            }
        }
    }
}

/// A transmit buffer owned by this side until sent or dropped.
pub struct TxBuffer<'a, T: Transport, L: RawLock = RawSpinLock, const E: usize = 8> {
    dev: &'a RpmsgVirtio<T, L, E>,
    token: Option<Token>,
    offset: usize,
    len: u32,
}

impl<'a, T: Transport, L: RawLock, const E: usize> TxBuffer<'a, T, L, E> {
    fn new(dev: &'a RpmsgVirtio<T, L, E>, token: Token) -> Result<Self> {
        let (addr, len) = token.addr_len();
        match dev.shm.phys_range_to_offset(addr, len as usize) {
            Ok(offset) if len as usize > HDR_SIZE => Ok(TxBuffer {
                dev,
                token: Some(token),
                offset,
                len,
            }),
            _ => {
                // A peer-supplied buffer we cannot use: hand it straight back.
                if let Token::Device(b) = token {
                    let mut st = dev.state.lock();
                    if let Queue::Device(q) = &mut st.svq {
                        let _ = q.push_used(b, 0);
                    }
                }
                Err(Error::InvalidBuffer)
            }
        }
    }

    /// Payload capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.len as usize - HDR_SIZE
    }

    /// Writes `data` at payload offset `at`.
    pub fn write(&mut self, at: usize, data: &[u8]) -> Result<()> {
        match at.checked_add(data.len()) {
            Some(end) if end <= self.capacity() => {}
            _ => return Err(Error::MessageTooLarge),
        }
        Ok(self.dev.shm.write_bytes(self.offset + HDR_SIZE + at, data)?)
    }

    /// The payload area, for building a message in place.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        let cap = self.capacity();
        // SAFETY: we own the descriptor and have not handed it to the peer;
        // `&mut self` makes this the only view of the buffer.
        unsafe { self.dev.shm.as_mut_slice(self.offset + HDR_SIZE, cap) }.unwrap_or(&mut [])
    }

    /// Sends the first `len` payload bytes to the endpoint's destination.
    pub fn send(self, ep: Endpoint, len: usize) -> Result<usize> {
        let (src, dst) = self.dev.dest_of(ep)?;
        self.send_offchannel(src, dst, len)
    }

    /// Sends the first `len` payload bytes from `ep` to `dst`.
    pub fn send_to(self, ep: Endpoint, dst: u32, len: usize) -> Result<usize> {
        let src = self.dev.state.lock().ept(ep)?.addr;
        self.send_offchannel(src, dst, len)
    }

    /// Sends the first `len` payload bytes with explicit addresses
    /// (`rpmsg_send_offchannel_nocopy`).
    pub fn send_offchannel(mut self, src: u32, dst: u32, len: usize) -> Result<usize> {
        if len > self.capacity() || len > u16::MAX as usize {
            return Err(Error::MessageTooLarge);
        }
        let hdr = Header {
            src,
            dst,
            reserved: 0,
            len: len as u16,
            flags: 0,
        };
        self.dev.shm.write_bytes(self.offset, &hdr.to_bytes())?;
        self.dev
            .opts
            .cache
            .flush(self.dev.ptr(self.offset), HDR_SIZE + len);
        let token = self.token.take().ok_or(Error::InvalidParam)?;
        self.dev.enqueue_tx(token)?;
        Ok(len)
    }
}

impl<T: Transport, L: RawLock, const E: usize> Drop for TxBuffer<'_, T, L, E> {
    fn drop(&mut self) {
        if let Some(t) = self.token.take() {
            // `rpmsg_release_tx_buffer`: keep it for the next send.
            if self.dev.state.lock().push_reclaim(t).is_err() {
                metal_warn!("rpmsg: reclaim list full, leaking a tx buffer");
            }
        }
    }
}
