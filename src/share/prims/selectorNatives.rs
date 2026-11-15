//! Native methods for the Windows selector stack (`sun.nio.ch`):
//! `WindowsSelectorImpl`/`SubSelector.poll0`, `NativeSocketAddress` layout
//! constants, and the `UnixDomainSockets` capability probe.
//!
//! Readiness is computed against the VM's socket table in `netNatives`
//! (non-blocking `std::net` sockets), matching what `select(2)` would
//! report: a descriptor lands in the read set when `POLL_IN` (or an error)
//! is pending, in the write set for `POLL_OUT`/`POLL_CONN`, and in the
//! except set on `POLL_ERR`/`POLL_HUP`/`POLL_NVAL`.

use std::io;
use std::time::{Duration, Instant};

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;

use super::netNatives;
use super::offHeapMemory::OffHeapMemory;

const POLLFD_SIZE: u64 = 8;
const POLLFD_FD_OFFSET: u64 = 0;
const POLLFD_EVENTS_OFFSET: u64 = 4;
const MAX_POLL_FDS: i32 = 4096;

pub(crate) fn is_owner(owner: &[u8]) -> bool {
    matches!(
        owner,
        b"sun/nio/ch/WindowsSelectorImpl"
            | b"sun/nio/ch/WindowsSelectorImpl$SubSelector"
            | b"sun/nio/ch/NativeSocketAddress"
            | b"sun/nio/ch/UnixDomainSockets"
    )
}

/// `sun/nio/ch/NativeSocketAddress` layout constants and
/// `sun/nio/ch/UnixDomainSockets.init`.
pub(crate) fn invoke_static<'a>(
    _loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    _arguments: &[Value<'a>],
    _output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    let result = match (name, descriptor) {
        (b"AFINET", b"()I") => Value::Int(2),
        (b"AFINET6", b"()I") => Value::Int(23),
        (b"sizeofSockAddr4", b"()I") => Value::Int(16),
        (b"sizeofSockAddr6", b"()I") => Value::Int(28),
        (b"sizeofFamily", b"()I") => Value::Int(2),
        (b"offsetFamily", b"()I") => Value::Int(0),
        (b"offsetSin4Port", b"()I") => Value::Int(2),
        (b"offsetSin4Addr", b"()I") => Value::Int(4),
        (b"offsetSin6Port", b"()I") => Value::Int(2),
        (b"offsetSin6Addr", b"()I") => Value::Int(8),
        (b"offsetSin6ScopeId", b"()I") => Value::Int(24),
        (b"offsetSin6FlowInfo", b"()I") => Value::Int(4),
        (b"init", b"()Z") => Value::Int(0),
        _ => return Ok(None),
    };
    Ok(Some(Some(result)))
}

fn int_arg(arguments: &[Value<'_>], index: usize) -> i32 {
    match arguments.get(index) {
        Some(Value::Int(v)) => *v,
        Some(Value::Long(v)) => *v as i32,
        _ => 0,
    }
}

fn long_arg(arguments: &[Value<'_>], index: usize) -> i64 {
    match arguments.get(index) {
        Some(Value::Long(v)) => *v,
        Some(Value::Int(v)) => i64::from(*v),
        _ => 0,
    }
}

fn write_int_array<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    values: &[i32],
) -> io::Result<()> {
    let mut arrays = loader.arrays.borrow_mut();
    if let Some(TypeArrayOop::Int(ints)) = arrays.get_mut(handle) {
        let n = values.len().min(ints.len());
        ints[..n].copy_from_slice(&values[..n]);
    }
    Ok(())
}

/// `WindowsSelectorImpl$SubSelector.poll0(long,int,int[],int[],int[],long,long)`.
///
/// Reads `numfds` pollfd entries ({ i32 fd, i16 events } stride 8) from
/// off-heap `pollAddress`, computes readiness for each descriptor and stores
/// the selected fds into `readFds`/`writeFds`/`exceptFds` as
/// `[count, fd...]` int arrays, matching the JDK `select(2)` contract.
fn poll0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let poll_address = u64::try_from(long_arg(arguments, 1))
        .map_err(|_| io::Error::other("Invalid poll address"))?;
    let numfds = int_arg(arguments, 2).clamp(0, MAX_POLL_FDS) as usize;
    let read_handle = match arguments.get(3) {
        Some(Value::PrimitiveArray(h)) => *h,
        _ => return Err(io::Error::other("Expected int[] readFds")),
    };
    let write_handle = match arguments.get(4) {
        Some(Value::PrimitiveArray(h)) => *h,
        _ => return Err(io::Error::other("Expected int[] writeFds")),
    };
    let except_handle = match arguments.get(5) {
        Some(Value::PrimitiveArray(h)) => *h,
        _ => return Err(io::Error::other("Expected int[] exceptFds")),
    };
    let timeout = long_arg(arguments, 6);
    let mem = OffHeapMemory::global();
    let deadline = if timeout < 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout.max(0) as u64))
    };

    loop {
        let mut read_fds: Vec<i32> = Vec::new();
        let mut write_fds: Vec<i32> = Vec::new();
        let mut except_fds: Vec<i32> = Vec::new();
        for i in 0..numfds {
            let base = poll_address + i as u64 * POLLFD_SIZE;
            let fd = match mem.get_int(base + POLLFD_FD_OFFSET) {
                Ok(fd) => fd,
                Err(_) => continue,
            };
            if fd < 0 {
                continue;
            }
            let events = mem
                .get_short(base + POLLFD_EVENTS_OFFSET)
                .unwrap_or(0) as i32
                & 0xffff;
            let ready = netNatives::sock_ready_bits(fd as u32 as u64);
            let want_read = events & netNatives::POLL_IN != 0;
            let want_write = events & (netNatives::POLL_OUT | netNatives::POLL_CONN) != 0;
            let exceptional = ready
                & (netNatives::POLL_ERR | netNatives::POLL_HUP | netNatives::POLL_NVAL)
                != 0;
            if want_read && (ready & (netNatives::POLL_IN | netNatives::POLL_ERR | netNatives::POLL_HUP | netNatives::POLL_NVAL) != 0) {
                read_fds.push(fd);
            }
            if want_write && (ready & (netNatives::POLL_OUT | netNatives::POLL_ERR | netNatives::POLL_HUP | netNatives::POLL_NVAL) != 0 || exceptional) {
                write_fds.push(fd);
            }
            if exceptional {
                except_fds.push(fd);
            }
        }
        if !read_fds.is_empty() || !write_fds.is_empty() || !except_fds.is_empty() || timeout == 0 {
            let mut read_out = Vec::with_capacity(read_fds.len() + 1);
            read_out.push(read_fds.len() as i32);
            read_out.extend_from_slice(&read_fds);
            let mut write_out = Vec::with_capacity(write_fds.len() + 1);
            write_out.push(write_fds.len() as i32);
            write_out.extend_from_slice(&write_fds);
            let mut except_out = Vec::with_capacity(except_fds.len() + 1);
            except_out.push(except_fds.len() as i32);
            except_out.extend_from_slice(&except_fds);
            write_int_array(loader, read_handle, &read_out)?;
            write_int_array(loader, write_handle, &write_out)?;
            write_int_array(loader, except_handle, &except_out)?;
            return Ok(Value::Int(0));
        }
        if let Some(dl) = deadline {
            let now = Instant::now();
            if now >= dl {
                write_int_array(loader, read_handle, &[0])?;
                write_int_array(loader, write_handle, &[0])?;
                write_int_array(loader, except_handle, &[0])?;
                return Ok(Value::Int(0));
            }
            std::thread::sleep((dl - now).min(Duration::from_millis(2)));
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Instance natives: `arguments[0]` is the selector/channel receiver.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    match (name, descriptor) {
        (b"setWakeupSocket0", b"(I)V") => {
            let fd = int_arg(arguments, 1) as u32 as u64;
            netNatives::sock_write_byte(fd, 1)?;
            Ok(Some(None))
        }
        (b"resetWakeupSocket0", b"(I)V") => {
            let fd = int_arg(arguments, 1) as u32 as u64;
            netNatives::sock_drain(fd)?;
            Ok(Some(None))
        }
        (b"poll0", b"(JI[I[I[IJJ)I") => {
            poll0(loader, arguments).map(|value| Some(Some(value)))
        }
        _ => Ok(None),
    }
}
