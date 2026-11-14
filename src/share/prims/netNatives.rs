//! Native methods for `java.net` (`InetAddress`, `Inet4AddressImpl`,
//! `Inet6AddressImpl`, `NetworkInterface`).
//!
//! Name resolution uses the host resolver via `std::net` (getaddrinfo on
//! Windows), so lookups are real. Interface enumeration is minimal: an empty
//! interface list, matching hosts where only the loopback pseudo-interface
//! matters for class initialization.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::{JavaThrowable, Value};
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::objArrayOop::ObjArrayOop;

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

fn throw_java<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: &str) -> io::Error {
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

/// Allocates an `Inet4Address` with the given host name and address.
/// `address` is stored big-endian, matching `InetAddressHolder.address`
/// (`Inet4Address.getAddress` extracts octets via `>>> 24`, `>>> 16`, ...).
fn inet4_address<'a>(
    loader: &ClassLoader<'a>,
    host_name: &str,
    address: Ipv4Addr,
) -> io::Result<Value<'a>> {
    let holder = raw_instance(loader, b"java/net/InetAddress$InetAddressHolder")?;
    set_field(
        loader,
        holder,
        b"originalHostName",
        b"Ljava/lang/String;",
        host_string(loader, host_name),
    )?;
    set_field(
        loader,
        holder,
        b"hostName",
        b"Ljava/lang/String;",
        host_string(loader, host_name),
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
        | (b"java/net/Inet6AddressImpl", b"getLocalHostName", b"()Ljava/lang/String;") => {
            let name = std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "localhost".to_string());
            Some(Some(host_string(loader, &name)))
        }
        (b"java/net/Inet4AddressImpl", b"lookupAllHostAddr", b"(Ljava/lang/String;)[Ljava/net/InetAddress;")
        | (b"java/net/Inet6AddressImpl", b"lookupAllHostAddr", b"(Ljava/lang/String;I)[Ljava/net/InetAddress;") => {
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
        | (b"java/net/Inet6AddressImpl", b"getHostByAddr", b"([B)Ljava/lang/String;") => {
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
        (b"java/net/Inet4AddressImpl", b"isReachable0", b"([BII[BI)Z")
        | (b"java/net/Inet6AddressImpl", b"isReachable0", b"([BIII[BI)Z") => {
            Some(Some(Value::Int(0)))
        }
        _ => return Ok(None),
    };
    Ok(result)
}
