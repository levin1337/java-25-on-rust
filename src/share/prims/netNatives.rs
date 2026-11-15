//! Native methods for `java.net` (`InetAddress`, `Inet4AddressImpl`,
//! `Inet6AddressImpl`, `NetworkInterface`).
//!
//! Name resolution uses the host resolver via `std::net` (getaddrinfo on
//! Windows), so lookups are real. Interface enumeration is minimal: an empty
//! interface list, matching hosts where only the loopback pseudo-interface
//! matters for class initialization.

use std::io;
use std::net::{
    Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};
use std::sync::Mutex;

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::{JavaThrowable, Value};
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::objArrayOop::ObjArrayOop;
use super::offHeapMemory::OffHeapMemory;

// ---------------------------------------------------------------------------
// Blocking-capable socket layer (`sun.nio.ch.Net`, `SocketDispatcher`,
// `DatagramDispatcher`, `DatagramChannelImpl` send/receive).
//
// Streams are always non-blocking, matching the JDK design: Java code polls
// readiness via `Net.poll`/`pollConnect` and retries on `IOStatus.UNAVAILABLE`.
// ---------------------------------------------------------------------------

pub(crate) enum Sock {
    Fresh { stream: bool, bound: Option<SocketAddr> },
    Listener {
        listener: TcpListener,
        pending: Option<(TcpStream, SocketAddr)>,
    },
    Connected { stream: TcpStream, local: SocketAddr, remote: SocketAddr },
    Datagram {
        sock: UdpSocket,
        bound: Option<SocketAddr>,
        peer: Option<SocketAddr>,
    },
}

static SOCKETS: Mutex<Option<std::collections::HashMap<u64, Sock>>> = Mutex::new(None);
static NEXT_SOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x4000_0100);
static SOCK_OPTS: Mutex<Option<std::collections::HashMap<(u64, i32, i32), i32>>> =
    Mutex::new(None);

// WEPoll instances: epoll fd -> (watched fdVal -> requested epoll events).
static WE_POLLS: Mutex<Option<std::collections::HashMap<u64, std::collections::HashMap<u32, i32>>>> =
    Mutex::new(None);
static NEXT_EPOLL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x5000_0001);

fn we_polls() -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<u64, std::collections::HashMap<u32, i32>>>> {
    let mut guard = WE_POLLS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(std::collections::HashMap::new());
    }
    guard
}

const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_MOD: i32 = 2;
const EPOLL_CTL_DEL: i32 = 3;
const EPOLLIN: i32 = 1;
const EPOLLOUT: i32 = 4;
const EPOLLERR: i32 = 8;
const EPOLLHUP: i32 = 16;
const EPOLL_EVENT_SIZE: i64 = 16;
const EPOLL_EVENTS_OFFSET: i64 = 8;
const EPOLL_SOCK_OFFSET: i64 = 0;

/// Readiness of `id` expressed in wepoll event bits.
fn epoll_ready(id: u64) -> i32 {
    let mut map = sockets();
    let Some(sock) = map.as_mut().unwrap().get_mut(&id) else {
        return EPOLLERR | EPOLLHUP;
    };
    let ready = sock_ready(sock);
    let mut events = 0;
    if ready & POLL_IN != 0 {
        events |= EPOLLIN;
    }
    if ready & POLL_OUT != 0 {
        events |= EPOLLOUT;
    }
    if ready & POLL_ERR != 0 {
        events |= EPOLLERR;
    }
    if ready & POLL_HUP != 0 {
        events |= EPOLLHUP;
    }
    events
}

pub(crate) fn sockets() -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<u64, Sock>>> {
    let mut guard = SOCKETS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(std::collections::HashMap::new());
    }
    guard
}

fn sock_opts() -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<(u64, i32, i32), i32>>> {
    let mut guard = SOCK_OPTS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(std::collections::HashMap::new());
    }
    guard
}

fn alloc_sock(state: Sock) -> u64 {
    let id = NEXT_SOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    sockets().as_mut().unwrap().insert(id, state);
    id
}

fn sock_id(handle: u64) -> io::Result<u64> {
    if handle >= 0x4000_0100 {
        Ok(handle)
    } else {
        Err(io::Error::other("Invalid socket handle"))
    }
}

fn fd_id<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<u64> {
    super::nioFileNatives::file_descriptor_handle(loader, value).and_then(sock_id)
}

fn set_fd_fields<'a>(loader: &ClassLoader<'a>, value: Value<'a>, id: u64) -> io::Result<()> {
    let Value::Object(handle) = value else {
        return Err(io::Error::other("Expected FileDescriptor"));
    };
    set_field(loader, handle, b"fd", b"I", Value::Int(-1))?;
    set_field(loader, handle, b"handle", b"J", Value::Long(id as i64))
}

fn inet_addr<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<SocketAddr> {
    let Value::Object(handle) = value else {
        return Err(io::Error::other("Expected InetAddress"));
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| io::Error::other("Invalid InetAddress"))?;
    // InetAddressHolder.address is a big-endian int for IPv4.
    let slot = object
        .klass
        .instanceField(b"holder", b"Ljava/net/InetAddress$InetAddressHolder;")?;
    let holder = match object.fields.get(slot).copied().unwrap_or(Value::Null) {
        Value::Object(h) => h,
        _ => return Err(io::Error::other("InetAddress has no holder")),
    };
    drop(objects);
    let objects = loader.borrow_objects();
    let holder_obj = objects
        .get(holder)
        .ok_or_else(|| io::Error::other("Invalid InetAddressHolder"))?;
    let slot = holder_obj
        .klass
        .instanceField(b"address", b"I")?;
    let ip = match holder_obj.fields.get(slot).copied().unwrap_or(Value::Null) {
        Value::Int(v) => Ipv4Addr::from(v as u32),
        _ => return Err(io::Error::other("Unsupported address family")),
    };
    Ok(SocketAddr::new(std::net::IpAddr::V4(ip), 0))
}

pub(crate) fn with_sock<R>(id: u64, f: impl FnOnce(&mut Sock) -> io::Result<R>) -> io::Result<R> {
    let mut map = sockets();
    let map = map.as_mut().unwrap();
    let sock = map
        .get_mut(&id)
        .ok_or_else(|| io::Error::other("Socket is closed"))?;
    f(sock)
}

fn sock_addr_parts(addr: SocketAddr) -> (Ipv4Addr, u16) {
    match addr.ip() {
        std::net::IpAddr::V4(v4) => (v4, addr.port()),
        std::net::IpAddr::V6(_) => (Ipv4Addr::UNSPECIFIED, addr.port()),
    }
}

fn sock_io_err(_loader: &ClassLoader<'_>, e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::WouldBlock {
        return e;
    }
    throw_java_no_loader(b"java/net/SocketException", &e.to_string())
}

fn throw_java_no_loader(class_name: &[u8], message: &str) -> io::Error {
    io::Error::other(format!(
        "{}: {}",
        String::from_utf8_lossy(class_name).replace('/', "."),
        message
    ))
}

/// Reads `len` bytes from the direct/off-heap buffer at `address`.
pub(crate) fn read_addr(address: i64, len: usize) -> io::Result<Vec<u8>> {
    let base = u64::try_from(address).map_err(|_| io::Error::other("Invalid address"))?;
    let mem = OffHeapMemory::global();
    let mut out = vec![0u8; len];
    for (i, b) in out.iter_mut().enumerate() {
        *b = mem.get_byte(base + i as u64)? as u8;
    }
    Ok(out)
}

/// Writes bytes to the direct/off-heap buffer at `address`.
pub(crate) fn write_addr(address: i64, data: &[u8]) -> io::Result<()> {
    let base = u64::try_from(address).map_err(|_| io::Error::other("Invalid address"))?;
    let mem = OffHeapMemory::global();
    for (i, &b) in data.iter().enumerate() {
        mem.put_byte(base + i as u64, b as i8)?;
    }
    Ok(())
}

/// Parses the native iovec array at `address`: `count` entries of
/// `{ u64 base, u64 len }` (16 bytes each on 64-bit).
fn read_iovecs(address: i64, count: i32) -> io::Result<Vec<(u64, usize)>> {
    let base_addr = u64::try_from(address).map_err(|_| io::Error::other("Invalid address"))?;
    let count = count.max(0).min(1024) as usize;
    let mem = OffHeapMemory::global();
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let entry = base_addr + i as u64 * 16;
        let base = mem.get_long(entry)? as u64;
        let len = mem.get_long(entry + 8)?.max(0) as usize;
        out.push((base, len));
    }
    Ok(out)
}

/// Scatter-read for stream sockets: fills each iovec in order.
fn tcp_readv(mut stream: &TcpStream, address: i64, count: i32) -> io::Result<i64> {
    let iovs = read_iovecs(address, count)?;
    let mut total = 0i64;
    let mut eof = false;
    for (base, len) in iovs {
        if len == 0 {
            continue;
        }
        let mut buf = vec![0u8; len];
        match stream.read(&mut buf) {
            Ok(0) => {
                eof = true;
                break;
            }
            Ok(n) => {
                write_addr(base as i64, &buf[..n])?;
                total += n as i64;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Ok(if total > 0 { total } else { -2 });
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                if total > 0 {
                    return Ok(total);
                }
                return Ok(-3);
            }
            Err(e) => return Err(e),
        }
    }
    if total == 0 && eof {
        return Ok(-1);
    }
    Ok(total)
}

/// Gather-write for stream sockets: sends each iovec in order.
fn tcp_writev(mut stream: &TcpStream, address: i64, count: i32) -> io::Result<i64> {
    let iovs = read_iovecs(address, count)?;
    let mut total = 0i64;
    for (base, len) in iovs {
        if len == 0 {
            continue;
        }
        let data = read_addr(base as i64, len)?;
        match stream.write(&data) {
            Ok(n) => {
                total += n as i64;
                if n < data.len() {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Ok(if total > 0 { total } else { -2 });
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                if total > 0 {
                    return Ok(total);
                }
                return Ok(-3);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

fn tcp_read(mut stream: &TcpStream, address: i64, len: i32) -> io::Result<i64> {
    let len = len.max(0) as usize;
    if len == 0 {
        return Ok(0);
    }
    let mut buf = vec![0u8; len];
    match stream.read(&mut buf) {
        Ok(0) => Ok(-1),
        Ok(n) => {
            write_addr(address, &buf[..n])?;
            Ok(n as i64)
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(-2),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(-3),
        Err(e) => Err(e),
    }
}

fn tcp_write(mut stream: &TcpStream, address: i64, len: i32) -> io::Result<i64> {
    let data = read_addr(address, len.max(0) as usize)?;
    if data.is_empty() {
        return Ok(0);
    }
    match stream.write(&data) {
        Ok(n) => Ok(n as i64),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(-2),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(-3),
        Err(e) => Err(e),
    }
}

use std::io::{Read, Write};

/// Poll constants; both producer (`Net.poll*Value`) and consumer
/// (`SocketChannelImpl.translateAndSet`) live in this VM, so any consistent
/// non-zero bit assignment works.
pub(crate) const POLL_IN: i32 = 1;
pub(crate) const POLL_OUT: i32 = 2;
pub(crate) const POLL_ERR: i32 = 4;
pub(crate) const POLL_HUP: i32 = 8;
pub(crate) const POLL_NVAL: i32 = 16;
pub(crate) const POLL_CONN: i32 = 32;

pub(crate) fn sock_ready(sock: &mut Sock) -> i32 {
    match sock {
        Sock::Connected { stream, .. } => {
            let mut probe = [0u8; 1];
            let mut ready = POLL_OUT;
            match stream.peek(&mut probe) {
                Ok(0) => ready |= POLL_IN | POLL_HUP,
                Ok(_) => ready |= POLL_IN,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => ready |= POLL_ERR | POLL_HUP,
            }
            ready
        }
        Sock::Listener { listener, pending } => {
            // A listening socket is readable only while a connection is
            // pending; probe with try-accept and stash the stream so a
            // later `Net.accept` returns it instead of re-accepting.
            if pending.is_none() {
                if let Ok((stream, remote)) = listener.accept() {
                    stream.set_nonblocking(true).ok();
                    *pending = Some((stream, remote));
                }
            }
            if pending.is_some() {
                POLL_IN | POLL_OUT
            } else {
                POLL_OUT
            }
        }
        Sock::Datagram { .. } => POLL_IN | POLL_OUT,
        Sock::Fresh { .. } => POLL_OUT,
    }
}

/// Readiness bits (`POLL_*`) for a socket id; `POLL_NVAL` when unknown.
pub(crate) fn sock_ready_bits(id: u64) -> i32 {
    let mut map = sockets();
    match map.as_mut().unwrap().get_mut(&id) {
        Some(sock) => sock_ready(sock),
        None => POLL_NVAL,
    }
}

/// Writes a single byte to a connected stream socket (selector wakeup).
pub(crate) fn sock_write_byte(id: u64, byte: u8) -> io::Result<()> {
    with_sock(id, |s| match s {
        Sock::Connected { stream, .. } => (&*stream).write_all(&[byte]),
        _ => Err(io::Error::other("Not a connected socket")),
    })
}

/// Discards all pending input on a connected stream socket.
pub(crate) fn sock_drain(id: u64) -> io::Result<()> {
    with_sock(id, |s| match s {
        Sock::Connected { stream, .. } => {
            let mut buf = [0u8; 512];
            loop {
                match (&*stream).read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            Ok(())
        }
        _ => Ok(()),
    })
}

/// Creates an `InetSocketAddress` object wrapping `ip:port`.
fn inet_socket_address<'a>(
    loader: &ClassLoader<'a>,
    ip: Ipv4Addr,
    port: u16,
) -> io::Result<Value<'a>> {
    let addr = inet4_address(loader, &ip.to_string(), ip)?;
    let holder = raw_instance(loader, b"java/net/InetSocketAddress$InetSocketAddressHolder")?;
    set_field(
        loader,
        holder,
        b"addr",
        b"Ljava/net/InetAddress;",
        addr,
    )?;
    set_field(
        loader,
        holder,
        b"hostname",
        b"Ljava/lang/String;",
        Value::Null,
    )?;
    set_field(loader, holder, b"port", b"I", Value::Int(port as i32))?;
    let object = raw_instance(loader, b"java/net/InetSocketAddress")?;
    set_field(
        loader,
        object,
        b"holder",
        b"Ljava/net/InetSocketAddress$InetSocketAddressHolder;",
        Value::Object(holder),
    )?;
    Ok(Value::Object(object))
}

fn sockaddr_of<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<SocketAddr> {
    // Accepts InetSocketAddress or InetAddress(+port already resolved by caller).
    let Value::Object(handle) = value else {
        return Err(io::Error::other("Expected SocketAddress"));
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| io::Error::other("Invalid SocketAddress"))?;
    let slot = object
        .klass
        .instanceField(b"holder", b"Ljava/net/InetSocketAddress$InetSocketAddressHolder;")?;
    let holder = match object.fields.get(slot).copied().unwrap_or(Value::Null) {
        Value::Object(h) => h,
        _ => return Err(io::Error::other("Not an InetSocketAddress")),
    };
    drop(objects);
    let objects = loader.borrow_objects();
    let holder_obj = objects
        .get(holder)
        .ok_or_else(|| io::Error::other("Invalid holder"))?;
    let addr_slot = holder_obj
        .klass
        .instanceField(b"addr", b"Ljava/net/InetAddress;")?;
    let port_slot = holder_obj
        .klass
        .instanceField(b"port", b"I")?;
    let addr_val = holder_obj.fields.get(addr_slot).copied().unwrap_or(Value::Null);
    let port = match holder_obj.fields.get(port_slot).copied().unwrap_or(Value::Null) {
        Value::Int(p) => p as u16,
        _ => 0,
    };
    drop(objects);
    let base = inet_addr(loader, addr_val)?;
    Ok(SocketAddr::new(base.ip(), port))
}

fn socket_dispatch<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let fd = || -> io::Result<u64> {
        arguments
            .first()
            .copied()
            .ok_or_else(|| io::Error::other("Missing FileDescriptor"))
            .and_then(|v| fd_id(loader, v))
    };
    match (name, descriptor) {
        (b"initIDs", b"()V") => Ok(Some(Value::Null)),
        (b"read0", b"(Ljava/io/FileDescriptor;JI)I") => {
            let id = fd()?;
            let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
            let len = match arguments.get(2) { Some(Value::Int(l)) => *l, _ => 0 };
            let n = with_sock(id, |s| match s {
                Sock::Connected { stream, .. } => tcp_read(stream, address, len),
                Sock::Datagram { sock, peer, .. } => {
                    let mut buf = vec![0u8; len.max(0) as usize];
                    match peer {
                        Some(_) => match sock.recv(&mut buf) {
                            Ok(n) => { write_addr(address, &buf[..n])?; Ok(n as i64) }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(-2),
                            Err(e) => Err(e),
                        },
                        None => match sock.recv_from(&mut buf) {
                            Ok((n, _)) => { write_addr(address, &buf[..n])?; Ok(n as i64) }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(-2),
                            Err(e) => Err(e),
                        },
                    }
                }
                _ => Err(io::Error::other("Socket is not connected")),
            })?;
            Ok(Some(Value::Int(n as i32)))
        }
        (b"write0", b"(Ljava/io/FileDescriptor;JI)I") => {
            let id = fd()?;
            let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
            let len = match arguments.get(2) { Some(Value::Int(l)) => *l, _ => 0 };
            let n = with_sock(id, |s| match s {
                Sock::Connected { stream, .. } => tcp_write(stream, address, len),
                Sock::Datagram { sock, peer, .. } => {
                    let data = read_addr(address, len.max(0) as usize)?;
                    match peer {
                        Some(p) => sock.send_to(&data, *p).map(|n| n as i64),
                        None => sock.send(&data).map(|n| n as i64),
                    }
                    .or_else(|e| {
                        if e.kind() == io::ErrorKind::WouldBlock { Ok(-2) } else { Err(e) }
                    })
                }
                _ => Err(io::Error::other("Socket is not connected")),
            })?;
            Ok(Some(Value::Int(n as i32)))
        }
        (b"readv0", b"(Ljava/io/FileDescriptor;JI)J") => {
            let id = fd()?;
            let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
            let count = match arguments.get(2) { Some(Value::Int(c)) => *c, _ => 0 };
            let n = with_sock(id, |s| match s {
                Sock::Connected { stream, .. } => tcp_readv(stream, address, count),
                Sock::Datagram { sock, peer, .. } => {
                    let iovs = read_iovecs(address, count)?;
                    let capacity: usize = iovs.iter().map(|(_, l)| *l).sum::<usize>().max(1);
                    let mut buf = vec![0u8; capacity];
                    let received = match peer {
                        Some(_) => sock.recv(&mut buf),
                        None => sock.recv_from(&mut buf).map(|(n, _)| n),
                    };
                    match received {
                        Ok(n) => {
                            let mut offset = 0usize;
                            for (base, len) in &iovs {
                                if offset >= n {
                                    break;
                                }
                                let take = (*len).min(n - offset);
                                if take > 0 {
                                    write_addr(*base as i64, &buf[offset..offset + take])?;
                                    offset += take;
                                }
                            }
                            Ok(n as i64)
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(-2),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(-3),
                        Err(e) => Err(e),
                    }
                }
                _ => Err(io::Error::other("Socket is not connected")),
            })?;
            Ok(Some(Value::Long(n)))
        }
        (b"writev0", b"(Ljava/io/FileDescriptor;JI)J") => {
            let id = fd()?;
            let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
            let count = match arguments.get(2) { Some(Value::Int(c)) => *c, _ => 0 };
            let n = with_sock(id, |s| match s {
                Sock::Connected { stream, .. } => tcp_writev(stream, address, count),
                Sock::Datagram { sock, peer, .. } => {
                    let iovs = read_iovecs(address, count)?;
                    let mut data = Vec::new();
                    for (base, len) in iovs {
                        if len > 0 {
                            data.extend_from_slice(&read_addr(base as i64, len)?);
                        }
                    }
                    if data.is_empty() {
                        return Ok(0i64);
                    }
                    match peer {
                        Some(p) => sock.send_to(&data, *p).map(|n| n as i64),
                        None => sock.send(&data).map(|n| n as i64),
                    }
                    .or_else(|e| {
                        if e.kind() == io::ErrorKind::WouldBlock { Ok(-2) }
                        else if e.kind() == io::ErrorKind::Interrupted { Ok(-3) }
                        else { Err(e) }
                    })
                }
                _ => Err(io::Error::other("Socket is not connected")),
            })?;
            Ok(Some(Value::Long(n)))
        }
        (b"close0", b"(I)V") => {
            let fd_val = match arguments.first() { Some(Value::Int(v)) => *v as u32 as u64, _ => 0 };
            if fd_val >= 0x4000_0100 {
                sockets().as_mut().unwrap().remove(&fd_val);
            }
            Ok(Some(Value::Null))
        }
        _ => Ok(None),
    }
}

fn datagram_dispatch<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    match (name, descriptor) {
        (b"disconnect0", b"(Ljava/io/FileDescriptor;Z)V") => {
            let id = fd_id(loader, arguments[0])?;
            with_sock(id, |s| {
                if let Sock::Datagram { peer, .. } = s {
                    *peer = None;
                }
                Ok(())
            })?;
            Ok(None)
        }
        (b"receive0", b"(Ljava/io/FileDescriptor;JI[BI)I") => {
            // Returns: address bytes written? JDK packs sender sockaddr into
            // the byte[] and returns source port via out param.
            let id = fd_id(loader, arguments[0])?;
            let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
            let len = match arguments.get(2) { Some(Value::Int(l)) => *l, _ => 0 };
            let Value::PrimitiveArray(arr) = arguments.get(3).copied().unwrap_or(Value::Null) else {
                return Ok(Some(Value::Int(-1)));
            };
            let mut buf = vec![0u8; len.max(0) as usize];
            let mut sender: Option<SocketAddr> = None;
            let n = with_sock(id, |s| match s {
                Sock::Datagram { sock, .. } => match sock.recv_from(&mut buf) {
                    Ok((n, from)) => { sender = Some(from); Ok(n) }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(usize::MAX),
                    Err(e) => Err(e),
                },
                _ => Err(io::Error::other("Not a datagram socket")),
            })?;
            if n == usize::MAX {
                return Ok(Some(Value::Int(-2)));
            }
            write_addr(address, &buf[..n])?;
            // Write sender IPv4 address into the byte[] and source port into
            // the int[] out param, matching the JDK sockaddr-in-buffer layout.
            if let Some(from) = sender {
                if let std::net::IpAddr::V4(v4) = from.ip() {
                    let _ = write_java_bytes(loader, arr, &v4.octets());
                }
                if let Some(Value::PrimitiveArray(port_arr)) = arguments.get(4) {
                    let _ = write_java_int(loader, *port_arr, from.port() as i32);
                }
            }
            Ok(Some(Value::Int(n as i32)))
        }
        (b"send0", b"(Ljava/io/FileDescriptor;JI[BI)I") => {
            let id = fd_id(loader, arguments[0])?;
            let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
            let len = match arguments.get(2) { Some(Value::Int(l)) => *l, _ => 0 };
            let Value::PrimitiveArray(_arr) = arguments.get(3).copied().unwrap_or(Value::Null) else {
                return Ok(Some(Value::Int(-1)));
            };
            let data = read_addr(address, len.max(0) as usize)?;
            let n = with_sock(id, |s| match s {
                Sock::Datagram { sock, .. } => sock.send(&data).map_err(|e| {
                    if e.kind() == io::ErrorKind::WouldBlock {
                        io::Error::other("__WOULDBLOCK__")
                    } else {
                        e
                    }
                }),
                _ => Err(io::Error::other("Not a datagram socket")),
            });
            match n {
                Ok(n) => Ok(Some(Value::Int(n as i32))),
                Err(e) if e.to_string().contains("__WOULDBLOCK__") => Ok(Some(Value::Int(-2))),
                Err(e) => Err(e),
            }
        }
        _ => Ok(None),
    }
}

fn write_java_bytes<'a>(loader: &ClassLoader<'a>, arr: usize, data: &[u8]) -> io::Result<()> {
    let mut arrays = loader.arrays.borrow_mut();
    if let Some(crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes)) = arrays.get_mut(arr) {
        let n = data.len().min(bytes.len());
        for (b, &d) in bytes[..n].iter_mut().zip(&data[..n]) {
            *b = d as i8;
        }
    }
    Ok(())
}

fn write_java_int<'a>(loader: &ClassLoader<'a>, arr: usize, value: i32) -> io::Result<()> {
    let mut arrays = loader.arrays.borrow_mut();
    match arrays.get_mut(arr) {
        Some(crate::share::oops::typeArrayOop::TypeArrayOop::Int(ints)) if !ints.is_empty() => {
            ints[0] = value;
        }
        _ => {}
    }
    Ok(())
}


fn sink() -> io::Sink {
    io::sink()
}

/// Builds a real throwable object without running its constructor.
fn throwable<'a>(
    loader: &ClassLoader<'a>,
    class_name: &[u8],
    message: &str,
) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
        loader,
        &klass,
        &mut sink(),
        0,
    );
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    if let Ok(slot) = klass.instanceField(b"detailMessage", b"Ljava/lang/String;") {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle) {
            object.fields[slot] = Value::HostString(loader.storage.alloc_str(message));
        }
    }
    Ok(handle)
}

pub(crate) fn throw_java<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: &str) -> io::Error {
    match throwable(loader, class_name, message) {
        Ok(handle) => io::Error::other(JavaThrowable(handle)),
        Err(_) => io::Error::other(format!(
            "{}: {}",
            String::from_utf8_lossy(class_name).replace('/', "."),
            message
        )),
    }
}

fn unknown_host<'a>(loader: &ClassLoader<'a>, host: &str) -> io::Error {
    throw_java(loader, b"java/net/UnknownHostException", host)
}

fn raw_instance<'a>(loader: &ClassLoader<'a>, class_name: &[u8]) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let object = InstanceOop::new(klass)?;
    Ok(loader.alloc_object(object))
}

fn set_field<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    name: &[u8],
    descriptor: &[u8],
    value: Value<'a>,
) -> io::Result<()> {
    let slot = {
        let objects = loader.borrow_objects();
        let object = objects
            .get(handle)
            .ok_or_else(|| io::Error::other("Invalid object reference"))?;
        object.klass.instanceField(name, descriptor)?
    };
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(handle)
        .ok_or_else(|| io::Error::other("Invalid object reference"))?;
    if slot < object.fields.len() {
        object.fields[slot] = value;
    }
    Ok(())
}

fn host_string<'a>(loader: &ClassLoader<'a>, text: &str) -> Value<'a> {
    Value::HostString(loader.storage.alloc_str(text))
}

/// Creates a real `java.lang.String` heap object (unlike `HostString`, which
/// JDK bytecode cannot invoke methods on).
fn real_string<'a>(loader: &ClassLoader<'a>, text: &str) -> io::Result<Value<'a>> {
    let value = host_string(loader, text);
    let mut sink = io::sink();
    match super::stringNatives::materialize(loader, value, &mut sink, 0)? {
        Some(object) => Ok(object),
        None => Ok(value),
    }
}

/// Allocates an `Inet4Address` with the given host name and address.
/// `address` is stored big-endian, matching `InetAddressHolder.address`
/// (`Inet4Address.getAddress` extracts octets via `>>> 24`, `>>> 16`, ...).
fn inet4_address<'a>(
    loader: &ClassLoader<'a>,
    host_name: &str,
    address: Ipv4Addr,
) -> io::Result<Value<'a>> {
    let holder = raw_instance(loader, b"java/net/InetAddress$InetAddressHolder")?;
    let name_string = real_string(loader, host_name)?;
    set_field(
        loader,
        holder,
        b"originalHostName",
        b"Ljava/lang/String;",
        name_string,
    )?;
    set_field(
        loader,
        holder,
        b"hostName",
        b"Ljava/lang/String;",
        name_string,
    )?;
    set_field(
        loader,
        holder,
        b"address",
        b"I",
        Value::Int(u32::from(address) as i32),
    )?;
    set_field(loader, holder, b"family", b"I", Value::Int(1))?;

    let object = raw_instance(loader, b"java/net/Inet4Address")?;
    set_field(
        loader,
        object,
        b"holder",
        b"Ljava/net/InetAddress$InetAddressHolder;",
        Value::Object(holder),
    )?;
    Ok(Value::Object(object))
}

/// Resolves `host` to IPv4 addresses using the platform resolver.
/// `"localhost"` and numeric literals are handled directly.
fn resolve_ipv4<'a>(
    loader: &ClassLoader<'a>,
    host: &str,
) -> io::Result<Vec<(String, Ipv4Addr)>> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Ok(vec![(host.to_string(), ip)]);
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(vec![(
            host.to_string(),
            Ipv4Addr::LOCALHOST,
        )]);
    }
    let addresses: Vec<SocketAddr> = format!("{host}:0")
        .to_socket_addrs()
        .map_err(|_| unknown_host(loader, host))?
        .collect();
    let mut result: Vec<(String, Ipv4Addr)> = Vec::new();
    for address in addresses {
        if let std::net::IpAddr::V4(ip) = address.ip()
            && !result.iter().any(|(_, seen)| *seen == ip)
        {
            result.push((host.to_string(), ip));
        }
    }
    if result.is_empty() {
        return Err(unknown_host(loader, host));
    }
    Ok(result)
}

fn inet4_array<'a>(
    loader: &ClassLoader<'a>,
    entries: &[(String, Ipv4Addr)],
) -> io::Result<Value<'a>> {
    let component = loader.storage.alloc_slice_copy(b"java/net/InetAddress");
    let mut array = ObjArrayOop::withComponent(entries.len() as i32, component)?;
    for (index, (host, ip)) in entries.iter().enumerate() {
        array.elements[index] = inet4_address(loader, host, *ip)?;
    }
    Ok(Value::ObjectArray(loader.alloc_obj_array(array)))
}

fn dotted(ip: Ipv4Addr) -> String {
    ip.to_string()
}

/// Dispatches `java.net` statics and instance-less natives.
pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    let result = match (owner, name, descriptor) {
        (b"java/net/InetAddress", b"isIPv4Available", b"()Z") => {
            Some(Some(Value::Int(1)))
        }
        (b"java/net/InetAddress", b"isIPv6Supported", b"()Z") => {
            Some(Some(Value::Int(1)))
        }
        (b"java/net/InetAddress", b"init", b"()V")
        | (b"java/net/Inet4Address", b"init", b"()V")
        | (b"java/net/Inet6Address", b"init", b"()V")
        | (b"java/net/NetworkInterface", b"init", b"()V") => Some(None),

        (b"java/net/NetworkInterface", b"getAll", b"()[Ljava/net/NetworkInterface;") => {
            let component = loader.storage.alloc_slice_copy(b"java/net/NetworkInterface");
            let array = ObjArrayOop::withComponent(0, component)?;
            Some(Some(Value::ObjectArray(loader.alloc_obj_array(array))))
        }
        (b"java/net/NetworkInterface", b"getByName0", b"(Ljava/lang/String;)Ljava/net/NetworkInterface;")
        | (b"java/net/NetworkInterface", b"getByIndex0", b"(I)Ljava/net/NetworkInterface;")
        | (b"java/net/NetworkInterface", b"getByInetAddress0", b"(Ljava/net/InetAddress;)Ljava/net/NetworkInterface;") => {
            Some(Some(Value::Null))
        }
        (b"java/net/NetworkInterface", b"boundInetAddress0", b"(Ljava/net/InetAddress;)Z") => {
            Some(Some(Value::Int(0)))
        }
        (b"java/net/NetworkInterface", b"isUp0", b"(Ljava/lang/String;I)Z") => {
            Some(Some(Value::Int(1)))
        }
        (b"java/net/NetworkInterface", b"isLoopback0", b"(Ljava/lang/String;I)Z")
        | (b"java/net/NetworkInterface", b"supportsMulticast0", b"(Ljava/lang/String;I)Z")
        | (b"java/net/NetworkInterface", b"isP2P0", b"(Ljava/lang/String;I)Z") => {
            Some(Some(Value::Int(0)))
        }
        (b"java/net/NetworkInterface", b"getMacAddr0", b"([BLjava/lang/String;I)[B") => {
            Some(Some(arguments.first().copied().unwrap_or(Value::Null)))
        }
        (b"java/net/NetworkInterface", b"getMTU0", b"(Ljava/lang/String;I)I") => {
            Some(Some(Value::Int(1500)))
        }

        (b"java/net/Inet4AddressImpl", b"getLocalHostName", b"()Ljava/lang/String;")
        | (b"java/net/Inet6AddressImpl", b"getLocalHostName", b"()Ljava/lang/String;")
        | (b"java/net/InetAddressImpl", b"getLocalHostName", b"()Ljava/lang/String;") => {
            let name = std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "localhost".to_string());
            Some(Some(host_string(loader, &name)))
        }
        (b"java/net/Inet4AddressImpl", b"lookupAllHostAddr", b"(Ljava/lang/String;)[Ljava/net/InetAddress;")
        | (b"java/net/Inet6AddressImpl", b"lookupAllHostAddr", b"(Ljava/lang/String;I)[Ljava/net/InetAddress;")
        | (b"java/net/InetAddressImpl", b"lookupAllHostAddr", b"(Ljava/lang/String;)[Ljava/net/InetAddress;")
        | (b"java/net/InetAddressImpl", b"lookupAllHostAddr", b"(Ljava/lang/String;I)[Ljava/net/InetAddress;") => {
            let host = match arguments.first() {
                Some(value) => super::stringNatives::string(loader, *value)
                    .ok()
                    .flatten()
                    .map(|cow| cow.into_owned())
                    .unwrap_or_default(),
                None => String::new(),
            };
            let entries = resolve_ipv4(loader, &host)?;
            Some(Some(inet4_array(loader, &entries)?))
        }
        (b"java/net/Inet4AddressImpl", b"getHostByAddr", b"([B)Ljava/lang/String;")
        | (b"java/net/Inet6AddressImpl", b"getHostByAddr", b"([B)Ljava/lang/String;")
        | (b"java/net/InetAddressImpl", b"getHostByAddr", b"([B)Ljava/lang/String;") => {
            let text = match arguments.first() {
                Some(Value::PrimitiveArray(handle)) => {
                    let arrays = loader.arrays.borrow();
                    match arrays.get(*handle) {
                        Some(crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes))
                            if bytes.len() == 4 =>
                        {
                            dotted(Ipv4Addr::new(
                                bytes[0] as u8,
                                bytes[1] as u8,
                                bytes[2] as u8,
                                bytes[3] as u8,
                            ))
                        }
                        _ => String::new(),
                    }
                }
                _ => String::new(),
            };
            if text.is_empty() {
                return Err(unknown_host(loader, "address"));
            }
            Some(Some(host_string(loader, &text)))
        }
        (b"java/net/Inet4AddressImpl", b"isReachable0", b"([BI[BI)Z")
        | (b"java/net/Inet4AddressImpl", b"isReachable0", b"([BII[BI)Z")
        | (b"java/net/Inet6AddressImpl", b"isReachable0", b"([BII[BII)Z")
        | (b"java/net/Inet6AddressImpl", b"isReachable0", b"([BIII[BI)Z")
        | (b"java/net/InetAddressImpl", b"isReachable0", b"([BI[BI)Z")
        | (b"java/net/InetAddressImpl", b"isReachable0", b"([BII[BI)Z")
        | (b"java/net/InetAddressImpl", b"isReachable0", b"([BII[BII)Z")
        | (b"java/net/InetAddressImpl", b"isReachable0", b"([BIII[BI)Z") => {
            Some(Some(Value::Int(0)))
        }

        // AF_UNIX socket support probe. Windows 10+ supports AF_UNIX in
        // Winsock, but this runtime's socket table only models TCP/UDP
        // streams; report unsupported so JDK callers (e.g. PipeImpl's
        // loopback connector) take the genuine TCP fallback path.
        (b"sun/nio/ch/UnixDomainSockets", b"init", b"()Z") => {
            Some(Some(Value::Int(0)))
        }
        (b"sun/nio/ch/UnixDomainSockets", b"socket0", b"()I")
        | (b"sun/nio/ch/UnixDomainSockets", b"bind0", b"(Ljava/io/FileDescriptor;[B)V")
        | (b"sun/nio/ch/UnixDomainSockets", b"connect0", b"(Ljava/io/FileDescriptor;[B)I")
        | (b"sun/nio/ch/UnixDomainSockets", b"accept0", b"(Ljava/io/FileDescriptor;Ljava/io/FileDescriptor;[Ljava/lang/Object;)I")
        | (b"sun/nio/ch/UnixDomainSockets", b"localAddress0", b"(Ljava/io/FileDescriptor;)[B") => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unix domain sockets are not supported",
            ));
        }
        _ => return net_channel(loader, owner, name, descriptor, arguments),
    };
    Ok(result)
}

const SA_FAMILY_INET: i32 = 2;
const SA_FAMILY_INET6: i32 = 23;

/// Parses a `NativeSocketAddress` (sockaddr/sockaddr_in/sockaddr_in6) stored
/// in off-heap memory. Layout matches `sun.nio.ch.NativeSocketAddress`:
/// family u16 native @0, port big-endian @2, addr @4 (v4) / @8 (v6).
fn sockaddr_target(address: u64) -> io::Result<SocketAddr> {
    let mem = OffHeapMemory::global();
    let raw = mem.read_bytes(address, 28)?;
    let family = i16::from_ne_bytes([raw[0], raw[1]]) as u16 as i32;
    let port = ((raw[2] as u16) << 8) | raw[3] as u16;
    match family {
        SA_FAMILY_INET => Ok(SocketAddr::new(
            std::net::IpAddr::V4(Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7])),
            port,
        )),
        SA_FAMILY_INET6 => {
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&raw[8..24]);
            let v6 = std::net::Ipv6Addr::from(bytes);
            if let Some(v4) = v6.to_ipv4_mapped() {
                return Ok(SocketAddr::new(std::net::IpAddr::V4(v4), port));
            }
            Ok(SocketAddr::new(std::net::IpAddr::V6(v6), port))
        }
        _ => Err(io::Error::other("Unsupported socket address family")),
    }
}

/// Serializes a `SocketAddr` into the `NativeSocketAddress` layout
/// (28-byte union): family u16 native @0, port network order @2,
/// IPv4 addr @4, IPv6 addr @8 with scope id @24.
fn sockaddr_bytes(addr: SocketAddr) -> [u8; 28] {
    let mut out = [0u8; 28];
    let port = addr.port();
    match addr.ip() {
        std::net::IpAddr::V4(v4) => {
            out[0] = SA_FAMILY_INET as u8;
            out[2] = (port >> 8) as u8;
            out[3] = port as u8;
            out[4..8].copy_from_slice(&v4.octets());
        }
        std::net::IpAddr::V6(v6) => {
            out[0] = SA_FAMILY_INET6 as u8;
            out[2] = (port >> 8) as u8;
            out[3] = port as u8;
            let bytes = if let Some(v4) = v6.to_ipv4_mapped() {
                let mut mapped = [0u8; 16];
                mapped[10] = 0xff;
                mapped[11] = 0xff;
                mapped[12..16].copy_from_slice(&v4.octets());
                mapped
            } else {
                v6.octets()
            };
            out[8..24].copy_from_slice(&bytes);
        }
    }
    out
}

/// `DatagramChannelImpl.receive0(FileDescriptor, long, int, long, boolean)`.
fn datagram_channel_receive0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let id = fd_id(loader, arguments.first().copied().unwrap_or(Value::Null))?;
    let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
    let len = match arguments.get(2) { Some(Value::Int(l)) => *l, _ => 0 };
    let sender_address = match arguments.get(3) { Some(Value::Long(a)) => *a, _ => 0 };
    let connected = matches!(arguments.get(4), Some(Value::Int(v)) if *v != 0);
    let (n, sender) = with_sock(id, |s| match s {
        Sock::Datagram { sock, peer, .. } => {
            let mut buf = vec![0u8; len.max(0) as usize];
            if connected || peer.is_some() {
                match sock.recv(&mut buf) {
                    Ok(n) => {
                        write_addr(address, &buf[..n])?;
                        Ok((n as i64, None))
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok((-2i64, None)),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok((-3i64, None)),
                    Err(e) => Err(e),
                }
            } else {
                match sock.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        write_addr(address, &buf[..n])?;
                        Ok((n as i64, Some(from)))
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok((-2i64, None)),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok((-3i64, None)),
                    Err(e) => Err(e),
                }
            }
        }
        _ => Err(io::Error::other("Not a datagram socket")),
    })?;
    if let Some(from) = sender
        && sender_address != 0
    {
        write_addr(sender_address, &sockaddr_bytes(from))?;
    }
    Ok(Value::Int(n as i32))
}

/// `DatagramChannelImpl.send0(FileDescriptor, long, int, long, int)`.
fn datagram_channel_send0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let id = fd_id(loader, arguments.first().copied().unwrap_or(Value::Null))?;
    let address = match arguments.get(1) { Some(Value::Long(a)) => *a, _ => 0 };
    let len = match arguments.get(2) { Some(Value::Int(l)) => *l, _ => 0 };
    let target_address = match arguments.get(3) { Some(Value::Long(a)) => *a, _ => 0 };
    let data = read_addr(address, len.max(0) as usize)?;
    let dest = if target_address != 0 {
        Some(sockaddr_target(target_address as u64)?)
    } else {
        None
    };
    let n = with_sock(id, |s| match s {
        Sock::Datagram { sock, peer, .. } => {
            let target = dest.or(*peer);
            let result = match target {
                Some(p) => sock.send_to(&data, p),
                None => sock.send(&data),
            };
            match result {
                Ok(n) => Ok(n as i64),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(-2),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(-3),
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => Err(throw_java(
                    loader,
                    b"java/net/PortUnreachableException",
                    "Port unreachable",
                )),
                Err(e) => Err(e),
            }
        }
        _ => Err(io::Error::other("Not a datagram socket")),
    })?;
    Ok(Value::Int(n as i32))
}

/// `sun.nio.ch` socket natives: `Net`, `SocketDispatcher`, `DatagramDispatcher`,
/// `DatagramChannelImpl`, `IOUtil`.
fn net_channel<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    if std::env::var_os("JVM25_TRACE_SOCK").is_some() {
        eprintln!(
            "[sock-dispatch] {}.{}{} args={}",
            String::from_utf8_lossy(owner),
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor),
            arguments.len()
        );
    }
    // Dispatchers take FileDescriptor as first arg.
    match owner {
        b"sun/nio/ch/SocketDispatcher" => {
            return match socket_dispatch(loader, name, descriptor, arguments)? {
                Some(v) => Ok(Some(Some(v))),
                None => Ok(None),
            };
        }
        b"sun/nio/ch/DatagramDispatcher" => {
            let handled = matches!(
                (name, descriptor),
                (b"disconnect0", b"(Ljava/io/FileDescriptor;Z)V")
                    | (b"receive0", b"(Ljava/io/FileDescriptor;JI[BI)I")
                    | (b"send0", b"(Ljava/io/FileDescriptor;JI[BI)I")
            );
            let result = if handled {
                datagram_dispatch(loader, name, descriptor, arguments)?
            } else {
                socket_dispatch(loader, name, descriptor, arguments)?
            };
            return match result {
                Some(v) => Ok(Some(Some(v))),
                None => Ok(None),
            };
        }
        _ => {}
    }
    match (owner, name, descriptor) {
        (b"sun/nio/ch/IOUtil", b"setfdVal", b"(Ljava/io/FileDescriptor;I)V") => {
            let id = match arguments.get(1) {
                Some(Value::Int(v)) => *v as u32 as u64,
                _ => 0,
            };
            if let Some(v) = arguments.first().copied() {
                set_fd_fields(loader, v, id)?;
            }
            return Ok(Some(None));
        }
        (b"sun/nio/ch/IOUtil", b"randomBytes", b"([B)Z") => {
            return Ok(super::nioFileNatives::io_util(loader, name, descriptor, arguments)?
                .map(Some));
        }
        (b"sun/nio/ch/IOUtil", b"initIDs", b"()V") => {
            return Ok(Some(None));
        }
        (b"sun/nio/ch/IOUtil", b"fdLimit", b"()I") => {
            return Ok(Some(Some(Value::Int(1024))));
        }
        (b"sun/nio/ch/IOUtil", b"iovMax", b"()I") => {
            return Ok(Some(Some(Value::Int(16))));
        }
        (b"sun/nio/ch/IOUtil", b"writevMax", b"()J") => {
            return Ok(Some(Some(Value::Long(i64::MAX))));
        }
        (b"sun/nio/ch/IOUtil", b"fdVal", b"(Ljava/io/FileDescriptor;)I") => {
            let id = match arguments.first().copied() {
                Some(v) => super::nioFileNatives::file_descriptor_handle(loader, v)
                    .unwrap_or(0) as u32 as i32,
                None => 0,
            };
            return Ok(Some(Some(Value::Int(id))));
        }
        (b"sun/nio/ch/IOUtil", b"configureBlocking", b"(Ljava/io/FileDescriptor;Z)V") => {
            return Ok(Some(None));
        }
        (b"sun/nio/ch/IOUtil", b"makePipe", b"(Z)J") => {
            // Real socketpair over the loopback interface: bind a listener,
            // connect, accept, and register both ends as connected sockets.
            let listener = TcpListener::bind(SocketAddr::new(
                std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
                0,
            ))
            .map_err(|e| sock_io_err(loader, e))?;
            listener.set_nonblocking(true).ok();
            let addr = listener.local_addr().map_err(|e| sock_io_err(loader, e))?;
            let client = TcpStream::connect(addr).map_err(|e| sock_io_err(loader, e))?;
            client.set_nonblocking(true).ok();
            let (server, remote) = listener
                .accept()
                .map_err(|e| sock_io_err(loader, e))?;
            server.set_nonblocking(true).ok();
            let local = server.local_addr().unwrap_or(addr);
            let c_id = alloc_sock(Sock::Connected {
                stream: client,
                local: addr,
                remote,
            });
            let s_id = alloc_sock(Sock::Connected {
                stream: server,
                local,
                remote: addr,
            });
            return Ok(Some(Some(Value::Long(
                (((c_id as u64) << 32) | (s_id as u64 & 0xffff_ffff)) as i64,
            ))));
        }
        (b"sun/nio/ch/IOUtil", b"write1", b"(II)I") | (b"sun/nio/ch/IOUtil", b"write1", b"(IB)I") => {
            // write1(fdVal, byte): used by Selector.wakeup() to poke the
            // interrupt pipe. fdVal is the raw socket id.
            let fd = match arguments.first() {
                Some(Value::Int(v)) => *v as u32 as u64,
                _ => 0,
            };
            let byte = match arguments.get(1) {
                Some(Value::Int(v)) => *v as u8,
                _ => 0,
            };
            let n = with_sock(fd, |s| match s {
                Sock::Connected { stream, .. } => {
                    (&*stream).write_all(&[byte]).map(|_| 1i64)
                }
                _ => Err(io::Error::other("Not a connected socket")),
            })
            .unwrap_or(-1);
            return Ok(Some(Some(Value::Int(n as i32))));
        }
        (b"sun/nio/ch/IOUtil", b"drain", b"(I)Z") | (b"sun/nio/ch/IOUtil", b"drain1", b"(I)I") => {
            // drain(fdVal): discard pending input (selector interrupt pipe).
            let fd = match arguments.first() {
                Some(Value::Int(v)) => *v as u32 as u64,
                _ => 0,
            };
            let mut total = 0i64;
            let _ = with_sock(fd, |s| match s {
                Sock::Connected { stream, .. } => {
                    let mut buf = [0u8; 256];
                    loop {
                        match stream.peek(&mut buf) {
                            Ok(0) => break,
                            Ok(_) => {
                                if let Ok(n) = (&*stream).read(&mut buf) {
                                    if n == 0 { break; }
                                    total += n as i64;
                                } else {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    Ok(())
                }
                _ => Ok(()),
            });
            if name == b"drain1" {
                return Ok(Some(Some(Value::Int(total.min(i32::MAX as i64) as i32))));
            }
            return Ok(Some(Some(Value::Int(i32::from(total > 0)))));
        }
        (b"sun/nio/ch/WEPoll", b"eventSize", b"()I") => {
            return Ok(Some(Some(Value::Int(EPOLL_EVENT_SIZE as i32))));
        }
        (b"sun/nio/ch/WEPoll", b"eventsOffset", b"()I") => {
            return Ok(Some(Some(Value::Int(EPOLL_EVENTS_OFFSET as i32))));
        }
        (b"sun/nio/ch/WEPoll", b"dataOffset", b"()I") => {
            return Ok(Some(Some(Value::Int(EPOLL_SOCK_OFFSET as i32))));
        }
        (b"sun/nio/ch/WEPoll", b"create", b"()J") => {
            let id = NEXT_EPOLL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            we_polls().as_mut().unwrap().insert(id, std::collections::HashMap::new());
            return Ok(Some(Some(Value::Long(id as i64))));
        }
        (b"sun/nio/ch/WEPoll", b"close", b"(J)V") => {
            if let Some(v) = arguments.first() {
                if let Value::Long(id) = v {
                    we_polls().as_mut().unwrap().remove(&(*id as u64));
                }
            }
            return Ok(Some(None));
        }
        (b"sun/nio/ch/WEPoll", b"ctl", b"(JIJI)I") => {
            let (eph, op, sock, events) = match arguments {
                [Value::Long(e), Value::Int(o), Value::Long(s), Value::Int(ev)] =>
                    (*e as u64, *o, *s, *ev),
                _ => return Err(io::Error::other("Bad WEPoll.ctl arguments")),
            };
            let mut polls = we_polls();
            let polls = polls.as_mut().unwrap();
            if let Some(watch) = polls.get_mut(&eph) {
                match op {
                    EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
                        watch.insert(sock as u32, events);
                    }
                    EPOLL_CTL_DEL => {
                        watch.remove(&(sock as u32));
                    }
                    _ => {}
                }
            }
            return Ok(Some(Some(Value::Int(0))));
        }
        (b"sun/nio/ch/WEPoll", b"wait", b"(JJII)I") => {
            let (eph, arr, max_events, timeout) = match arguments {
                [Value::Long(e), Value::Long(a), Value::Int(m), Value::Int(t)] =>
                    (*e as u64, *a, *m, *t),
                _ => return Err(io::Error::other("Bad WEPoll.wait arguments")),
            };
            let max_events = max_events.max(0).min(4096) as usize;
            let deadline = if timeout < 0 {
                None
            } else {
                Some(std::time::Instant::now() + std::time::Duration::from_millis(timeout as u64))
            };
            let mem = OffHeapMemory::global();
            loop {
                // Snapshot watched fds under the lock, then probe readiness
                // without holding it (sock_ready takes the socket lock).
                let watched: Vec<(u32, i32)> = {
                    let polls = we_polls();
                    polls
                        .as_ref()
                        .and_then(|p| p.get(&eph))
                        .map(|w| w.iter().map(|(&fd, &ev)| (fd, ev)).collect())
                        .unwrap_or_default()
                };
                let mut ready: Vec<(u32, i32)> = Vec::new();
                for (fd, requested) in &watched {
                    let got = epoll_ready(*fd as u64) & (requested | EPOLLERR | EPOLLHUP);
                    if got != 0 {
                        ready.push((*fd, got));
                    }
                }
                if !ready.is_empty() || timeout == 0 {
                    let count = ready.len().min(max_events);
                    for (i, (fd, events)) in ready.iter().take(count).enumerate() {
                        let base = arr as u64 + i as u64 * EPOLL_EVENT_SIZE as u64;
                        let _ = mem.put_long(base + EPOLL_SOCK_OFFSET as u64, *fd as i64);
                        let _ = mem.put_int(base + EPOLL_EVENTS_OFFSET as u64, *events);
                    }
                    return Ok(Some(Some(Value::Int(count as i32))));
                }
                if let Some(dl) = deadline {
                    let now = std::time::Instant::now();
                    if now >= dl {
                        return Ok(Some(Some(Value::Int(0))));
                    }
                    let remain = dl - now;
                    std::thread::sleep(remain.min(std::time::Duration::from_millis(2)));
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
        (b"sun/nio/ch/Net", b"initIDs", b"()V") => return Ok(Some(None)),
        (b"sun/nio/ch/Net", b"isIPv6Available0", b"()Z")
        | (b"sun/nio/ch/Net", b"isReusePortAvailable0", b"()Z")
        | (b"sun/nio/ch/Net", b"shouldShutdownWriteBeforeClose0", b"()Z")
        | (b"sun/nio/ch/Net", b"shouldSetBothIPv4AndIPv6Options0", b"()Z")
        | (b"sun/nio/ch/Net", b"canIPv6SocketJoinIPv4Group0", b"()Z")
        | (b"sun/nio/ch/Net", b"canJoin6WithIPv4Group0", b"()Z")
        | (b"sun/nio/ch/Net", b"canUseIPv6OptionsWithIPv4LocalAddress0", b"()Z") => {
            return Ok(Some(Some(Value::Int(0))));
        }
        (b"sun/nio/ch/Net", b"isExclusiveBindAvailable", b"()I") => {
            return Ok(Some(Some(Value::Int(-1))));
        }
        (b"sun/nio/ch/Net", b"socket0", b"(ZZZZ)I") => {
            let stream = arguments.get(1).copied() == Some(Value::Int(1));
            let id = alloc_sock(Sock::Fresh { stream, bound: None });
            return Ok(Some(Some(Value::Int(id as i32))));
        }
        (b"sun/nio/ch/Net", b"bind0", b"(Ljava/io/FileDescriptor;ZZLjava/net/InetAddress;I)V") => {
            let id = fd_id(loader, arguments[0])?;
            let addr_val = arguments.get(3).copied().unwrap_or(Value::Null);
            let port = match arguments.get(4) { Some(Value::Int(p)) => *p as u16, _ => 0 };
            let base = inet_addr(loader, addr_val)?;
            let addr = SocketAddr::new(base.ip(), port);
            with_sock(id, |s| {
                match s {
                    Sock::Fresh { stream: true, bound } => {
                        // std has no bind-without-listen; create the listener
                        // now so the OS assigns the port immediately.
                        let listener = TcpListener::bind(addr)
                            .map_err(|e| sock_io_err(loader, e))?;
                        listener.set_nonblocking(true).ok();
                        *s = Sock::Listener { listener, pending: None };
                        Ok(())
                    }
                    Sock::Fresh { stream: false, .. } => {
                        let sock = UdpSocket::bind(addr).map_err(|e| sock_io_err(loader, e))?;
                        sock.set_nonblocking(true).ok();
                        *s = Sock::Datagram { sock, bound: Some(addr), peer: None };
                        Ok(())
                    }
                    Sock::Datagram { sock, bound, .. } => {
                        // Rebind is not supported; treat as no-op success.
                        let _ = (sock, bound);
                        Ok(())
                    }
                    _ => Err(io::Error::other("Socket is already bound")),
                }
            })?;
            return Ok(Some(None));
        }
        (b"sun/nio/ch/Net", b"listen", b"(Ljava/io/FileDescriptor;I)V") => {
            let id = fd_id(loader, arguments[0])?;
            let backlog = match arguments.get(1) { Some(Value::Int(b)) => *b as i32, _ => 50 };
            let _ = backlog;
            with_sock(id, |s| {
                match s {
                    Sock::Listener { .. } => Ok(()), // already listening
                    Sock::Fresh { stream: true, bound } => {
                        let addr = bound.unwrap_or(SocketAddr::new(
                            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                            0,
                        ));
                        let listener = TcpListener::bind(addr)
                            .map_err(|e| sock_io_err(loader, e))?;
                        listener.set_nonblocking(true).ok();
                        *s = Sock::Listener { listener, pending: None };
                        Ok(())
                    }
                    _ => Err(io::Error::other("Socket is not bound")),
                }
            })?;
            return Ok(Some(None));
        }
        (b"sun/nio/ch/Net", b"connect0", b"(ZLjava/io/FileDescriptor;Ljava/net/InetAddress;I)I") => {
            let id = fd_id(loader, arguments[1])?;
            let addr_val = arguments.get(2).copied().unwrap_or(Value::Null);
            let port = match arguments.get(3) { Some(Value::Int(p)) => *p as u16, _ => 0 };
            let base = inet_addr(loader, addr_val)?;
            let addr = SocketAddr::new(base.ip(), port);
            return with_sock(id, |s| {
                let bound = match s {
                    Sock::Fresh { stream: true, bound } => bound.take(),
                    Sock::Connected { .. } | Sock::Listener { .. } => {
                        return Err(io::Error::other("Already connected"));
                    }
                    _ => return Err(io::Error::other("Not a stream socket")),
                };
                let stream = match bound {
                    Some(b) => TcpStream::connect(addr).map_err(|e| sock_io_err(loader, e))?,
                    None => TcpStream::connect(addr).map_err(|e| sock_io_err(loader, e))?,
                };
                stream.set_nonblocking(true).ok();
                let local = stream.local_addr().unwrap_or(addr);
                *s = Sock::Connected { stream, local, remote: addr };
                Ok(0i64)
            })
            .map(|n| Some(Some(Value::Int(n as i32))))
        }
        (b"sun/nio/ch/Net", b"accept", b"(Ljava/io/FileDescriptor;Ljava/io/FileDescriptor;[Ljava/net/InetSocketAddress;)I") => {
            let id = fd_id(loader, arguments[0])?;
            let new_fd = arguments.get(1).copied().unwrap_or(Value::Null);
            // Accept under the socket lock, then do all Java-object work
            // (alloc_sock re-locks SOCKETS; inet_socket_address runs the
            // interpreter) outside it to avoid deadlock.
            let accepted = with_sock(id, |s| match s {
                Sock::Listener { listener, pending } => {
                    // A readiness probe may already have accepted a pending
                    // connection; hand it out before touching the listener.
                    if let Some((stream, remote)) = pending.take() {
                        let local = stream.local_addr().unwrap_or(remote);
                        return Ok(Some((stream, local, remote)));
                    }
                    match listener.accept() {
                        Ok((stream, remote)) => {
                            stream.set_nonblocking(true).ok();
                            let local = stream.local_addr().unwrap_or(remote);
                            Ok(Some((stream, local, remote)))
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
                        Err(e) => Err(sock_io_err(loader, e)),
                    }
                }
                _ => Err(io::Error::other("Not a server socket")),
            })?;
            let retval = match accepted {
                None => -2i64,
                Some((stream, local, remote)) => {
                    let new_id = alloc_sock(Sock::Connected { stream, local, remote });
                    set_fd_fields(loader, new_fd, new_id)?;
                    // Fill inetSocketAddress[0] with the remote address.
                    if let Some(Value::ObjectArray(arr)) = arguments.get(2).copied()
                        && let std::net::IpAddr::V4(v4) = remote.ip()
                    {
                        let isa = inet_socket_address(loader, v4, remote.port())?;
                        let mut arrays = loader.obj_arrays.borrow_mut();
                        if let Some(slot) = arrays.get_mut(arr)
                            && !slot.elements.is_empty()
                        {
                            slot.elements[0] = isa;
                        }
                    }
                    1i64
                }
            };
            return Ok(Some(Some(Value::Int(retval as i32))));
        }
        (b"sun/nio/ch/Net", b"shutdown", b"(Ljava/io/FileDescriptor;I)V") => {
            let id = fd_id(loader, arguments[0])?;
            let how = match arguments.get(1) { Some(Value::Int(h)) => *h, _ => 2 };
            with_sock(id, |s| {
                if let Sock::Connected { stream, .. } = s {
                    let sd = match how {
                        0 => Shutdown::Read,
                        1 => Shutdown::Write,
                        _ => Shutdown::Both,
                    };
                    stream.shutdown(sd).ok();
                }
                Ok(())
            })?;
            return Ok(Some(None));
        }
        (b"sun/nio/ch/Net", b"localPort", b"(Ljava/io/FileDescriptor;)I")
        | (b"sun/nio/ch/Net", b"remotePort", b"(Ljava/io/FileDescriptor;)I") => {
            let id = fd_id(loader, arguments[0])?;
            let is_local = name == b"localPort";
            let port = with_sock(id, |s| match s {
                Sock::Connected { local, remote, .. } => {
                    Ok((if is_local { local } else { remote }).port() as i64)
                }
                Sock::Listener { listener, .. } => {
                    Ok(listener.local_addr().map(|a| a.port()).unwrap_or(0) as i64)
                }
                Sock::Datagram { sock, bound, .. } => Ok(bound
                    .or_else(|| sock.local_addr().ok())
                    .map(|a| a.port())
                    .unwrap_or(0) as i64),
                Sock::Fresh { bound, .. } => Ok(bound.map(|a| a.port()).unwrap_or(0) as i64),
                _ => Ok(0),
            })?;
            return Ok(Some(Some(Value::Int(port as i32))));
        }
        (b"sun/nio/ch/Net", b"localInetAddress", b"(Ljava/io/FileDescriptor;)Ljava/net/InetAddress;")
        | (b"sun/nio/ch/Net", b"remoteInetAddress", b"(Ljava/io/FileDescriptor;)Ljava/net/InetAddress;") => {
            let id = fd_id(loader, arguments[0])?;
            let is_local = name == b"localInetAddress";
            let ip = with_sock(id, |s| match s {
                Sock::Connected { local, remote, .. } => {
                    let a = if is_local { local } else { remote };
                    match a.ip() { std::net::IpAddr::V4(v) => Ok(v), _ => Ok(Ipv4Addr::UNSPECIFIED) }
                }
                Sock::Listener { listener, .. } => Ok(match listener.local_addr().ok().map(|a| a.ip()) {
                    Some(std::net::IpAddr::V4(v)) => v,
                    _ => Ipv4Addr::UNSPECIFIED,
                }),
                Sock::Datagram { sock, bound, .. } => Ok(match bound
                    .or_else(|| sock.local_addr().ok())
                    .map(|a| a.ip())
                {
                    Some(std::net::IpAddr::V4(v)) => v,
                    _ => Ipv4Addr::UNSPECIFIED,
                }),
                Sock::Fresh { bound, .. } => Ok(match bound.map(|a| a.ip()) {
                    Some(std::net::IpAddr::V4(v)) => v,
                    _ => Ipv4Addr::UNSPECIFIED,
                }),
                _ => Ok(Ipv4Addr::UNSPECIFIED),
            })?;
            return Ok(Some(Some(inet4_address(loader, &ip.to_string(), ip)?)));
        }
        (b"sun/nio/ch/Net", b"getIntOption0", b"(Ljava/io/FileDescriptor;ZII)I") => {
            let id = fd_id(loader, arguments[0])?;
            let level = match arguments.get(2) { Some(Value::Int(v)) => *v, _ => 0 };
            let opt = match arguments.get(3) { Some(Value::Int(v)) => *v, _ => 0 };
            let val = sock_opts().as_ref().unwrap().get(&(id, level, opt)).copied().unwrap_or(0);
            return Ok(Some(Some(Value::Int(val))));
        }
        (b"sun/nio/ch/Net", b"setIntOption0", b"(Ljava/io/FileDescriptor;ZIIIZ)V")
        | (b"sun/nio/ch/Net", b"setIntOption0", b"(Ljava/io/FileDescriptor;ZIIII)V") => {
            let id = fd_id(loader, arguments[0])?;
            let level = match arguments.get(2) { Some(Value::Int(v)) => *v, _ => 0 };
            let opt = match arguments.get(3) { Some(Value::Int(v)) => *v, _ => 0 };
            let arg = match arguments.get(4) { Some(Value::Int(v)) => *v, _ => 0 };
            sock_opts().as_mut().unwrap().insert((id, level, opt), arg);
            return Ok(Some(None));
        }
        (b"sun/nio/ch/Net", b"poll", b"(Ljava/io/FileDescriptor;IJ)I") => {
            let id = fd_id(loader, arguments[0])?;
            let events = match arguments.get(1) { Some(Value::Int(v)) => *v, _ => 0 };
            let timeout = match arguments.get(2) { Some(Value::Long(v)) => *v, _ => 0 };
            let _ = timeout;
            let ready = with_sock(id, |s| Ok(sock_ready(s)))? & events;
            return Ok(Some(Some(Value::Int(ready))));
        }
        (b"sun/nio/ch/Net", b"pollConnect", b"(Ljava/io/FileDescriptor;J)Z") => {
            let id = fd_id(loader, arguments[0])?;
            let connected = with_sock(id, |s| {
                Ok(matches!(s, Sock::Connected { .. } | Sock::Listener { .. }))
            })?;
            return Ok(Some(Some(Value::Int(connected as i32))));
        }
        (b"sun/nio/ch/Net", b"available", b"(Ljava/io/FileDescriptor;)I") => {
            return Ok(Some(Some(Value::Int(0))));
        }
        (b"sun/nio/ch/Net", b"sendOOB", b"(Ljava/io/FileDescriptor;B)I") => {
            return Ok(Some(Some(Value::Int(-1))));
        }
        (b"sun/nio/ch/Net", b"discardOOB", b"(Ljava/io/FileDescriptor;)Z") => {
            return Ok(Some(Some(Value::Int(0))));
        }
        (b"sun/nio/ch/Net", b"pollinValue", b"()S")
        | (b"sun/nio/ch/Net", b"polloutValue", b"()S")
        | (b"sun/nio/ch/Net", b"pollerrValue", b"()S")
        | (b"sun/nio/ch/Net", b"pollhupValue", b"()S")
        | (b"sun/nio/ch/Net", b"pollnvalValue", b"()S")
        | (b"sun/nio/ch/Net", b"pollconnValue", b"()S") => {
            let v = match name {
                b"pollinValue" => POLL_IN,
                b"polloutValue" => POLL_OUT,
                b"pollerrValue" => POLL_ERR,
                b"pollhupValue" => POLL_HUP,
                b"pollconnValue" => 32,
                _ => POLL_NVAL,
            };
            return Ok(Some(Some(Value::Int(v))));
        }
        (b"sun/nio/ch/Net", b"joinOrDrop4", b"(ZLjava/io/FileDescriptor;III)I")
        | (b"sun/nio/ch/Net", b"blockOrUnblock4", b"(ZLjava/io/FileDescriptor;II)I")
        | (b"sun/nio/ch/Net", b"joinOrDrop6", b"(ZLjava/io/FileDescriptor;[BI[B)I")
        | (b"sun/nio/ch/Net", b"blockOrUnblock6", b"(ZLjava/io/FileDescriptor;[BI[B)I")
        | (b"sun/nio/ch/Net", b"setInterface4", b"(Ljava/io/FileDescriptor;I)V")
        | (b"sun/nio/ch/Net", b"setInterface6", b"(Ljava/io/FileDescriptor;I)V") => {
            if descriptor.ends_with(b")I") {
                return Ok(Some(Some(Value::Int(0))));
            }
            return Ok(Some(None));
        }
        (b"sun/nio/ch/Net", b"getInterface4", b"(Ljava/io/FileDescriptor;)I")
        | (b"sun/nio/ch/Net", b"getInterface6", b"(Ljava/io/FileDescriptor;)I") => {
            return Ok(Some(Some(Value::Int(0))));
        }
        (b"jdk/net/WindowsSocketOptions", _, b"()Z") => {
            return Ok(Some(Some(Value::Int(0))));
        }
        (b"jdk/net/WindowsSocketOptions", _, b"()V")
        | (b"jdk/net/WindowsSocketOptions", _, b"(I)V") => {
            return Ok(Some(None));
        }
        (b"sun/nio/ch/DatagramChannelImpl", b"receive0", b"(Ljava/io/FileDescriptor;JI[BI)I")
        | (b"sun/nio/ch/DatagramChannelImpl", b"send0", b"(Ljava/io/FileDescriptor;JI[BI)I")
        | (b"sun/nio/ch/DatagramChannelImpl", b"disconnect0", b"(Ljava/io/FileDescriptor;Z)V") => {
            return datagram_dispatch(loader, name, descriptor, arguments)
                .map(|r| r.map(Some));
        }
        (b"sun/nio/ch/DatagramChannelImpl", b"receive0", b"(Ljava/io/FileDescriptor;JIJZ)I") => {
            return datagram_channel_receive0(loader, arguments)
                .map(|v| Some(Some(v)));
        }
        (b"sun/nio/ch/DatagramChannelImpl", b"send0", b"(Ljava/io/FileDescriptor;JIJI)I") => {
            return datagram_channel_send0(loader, arguments)
                .map(|v| Some(Some(v)));
        }
        _ => {}
    }
    Ok(None)
}
