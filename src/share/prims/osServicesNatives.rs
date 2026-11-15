//! OS service natives: `sun.net.dns.ResolverConfigurationImpl`,
//! `sun.net.spi.DefaultProxySelector`, `jdk.internal.vm.vector.VectorSupport`
//! and `sun.nio.fs.RegistryFileTypeDetector`.
//!
//! Ports the Windows implementations from JDK `src/java.base/windows/native/
//! libnet` (`ResolverConfigurationImpl.c`, `DefaultProxySelector.c`) and
//! `libnio` (`RegistryFileTypeDetector.c`) onto real Win32 APIs, plus the
//! POSIX fallbacks (`/etc/resolv.conf`, proxy environment variables).

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    oops::objArrayOop::ObjArrayOop,
    runtime::javaCalls::{JavaCallArguments, JavaCalls},
};
use std::io;

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

fn host_string<'a>(loader: &ClassLoader<'a>, text: &str) -> Value<'a> {
    Value::HostString(loader.storage.alloc_str(text))
}

fn string_arg<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
) -> io::Result<Option<String>> {
    Ok(super::stringNatives::string(loader, value)?.map(|cow| cow.into_owned()))
}

fn set_static_string<'a>(
    loader: &ClassLoader<'a>,
    class: &[u8],
    field: &[u8],
    text: &str,
) -> io::Result<()> {
    if let Ok(klass) = loader.load(class)
        && let Ok(cell) = klass.staticField(field, b"Ljava/lang/String;")
    {
        cell.set(host_string(loader, text));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// sun.net.dns.ResolverConfigurationImpl
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod dns_windows {
    use super::append_token;
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetAdaptersAddresses(
            family: u32,
            flags: u32,
            reserved: *mut c_void,
            adapters: *mut u8,
            size: *mut u32,
        ) -> u32;
        fn NotifyAddrChange(handle: *mut *mut c_void, overlapped: *mut Overlapped) -> u32;
        fn GetOverlappedResult(
            handle: *mut c_void,
            overlapped: *mut Overlapped,
            transferred: *mut u32,
            wait: i32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateEventW(
            attributes: *const c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn WSAAddressToStringA(
            sockaddr: *const c_void,
            length: i32,
            format: *const c_void,
            buffer: *mut u8,
            buffer_length: *mut u32,
        ) -> i32;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            key: *mut c_void,
            sub_key: *const u16,
            options: u32,
            access: u32,
            result: *mut *mut c_void,
        ) -> u32;
        fn RegQueryValueExW(
            key: *mut c_void,
            name: *const u16,
            reserved: *mut c_void,
            kind: *mut u32,
            data: *mut u8,
            size: *mut u32,
        ) -> u32;
        fn RegCloseKey(key: *mut c_void) -> i32;
    }

    #[repr(C)]
    pub struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut c_void,
    }

    const ERROR_SUCCESS: u32 = 0;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const ERROR_IO_PENDING: u32 = 997;
    const ERROR_NO_DATA: u32 = 232;
    const AF_UNSPEC: u32 = 0;
    const GAA_FLAG_SKIP_UNICAST: u32 = 0x0001;
    const GAA_FLAG_SKIP_ANYCAST: u32 = 0x0002;
    const GAA_FLAG_SKIP_MULTICAST: u32 = 0x0004;
    const GAA_FLAG_SKIP_FRIENDLY_NAME: u32 = 0x0020;
    const IF_OPER_STATUS_UP: u32 = 1;
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u64 as isize;
    const KEY_READ: u32 = 0x20019;

    /// Offsets into `IP_ADAPTER_ADDRESSES` / `IP_ADAPTER_DNS_SERVER_ADDRESS`
    /// (x64 layout, stable across Windows versions).
    const ADAPTER_NEXT: usize = 8;
    const ADAPTER_DNS_SERVER: usize = 48;
    const ADAPTER_DNS_SUFFIX: usize = 56;
    const ADAPTER_OPER_STATUS: usize = 104;
    const DNS_NEXT: usize = 8;
    const DNS_SOCKADDR: usize = 16;
    const DNS_SOCKADDR_LEN: usize = 24;
    const SOCKADDR_IN6_SCOPE_ID: usize = 24;

    fn read_ptr(base: *const u8, offset: usize) -> *const u8 {
        unsafe { *(base.add(offset) as *const *const u8) }
    }

    fn read_u32(base: *const u8, offset: usize) -> u32 {
        unsafe { *(base.add(offset) as *const u32) }
    }

    /// Collects (search_suffixes, nameserver_addresses) from the global
    /// Tcpip registry SearchList and from every up adapter's DNS servers,
    /// mirroring `loadConfig` in `ResolverConfigurationImpl.c`.
    pub fn load_config() -> (String, String) {
        let mut search = String::new();
        let mut servers = String::new();
        let key_path: Vec<u16> = "SYSTEM\\CurrentControlSet\\Services\\Tcpip\\Parameters"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let value_name: Vec<u16> = "SearchList".encode_utf16().chain(std::iter::once(0)).collect();
        let mut key: *mut c_void = ptr::null_mut();
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE as *mut c_void,
                key_path.as_ptr(),
                0,
                KEY_READ,
                &mut key,
            )
        };
        if opened == ERROR_SUCCESS && !key.is_null() {
            let mut buf = [0u16; 512];
            let mut size = (buf.len() * 2) as u32;
            let mut kind = 0u32;
            let queried = unsafe {
                RegQueryValueExW(
                    key,
                    value_name.as_ptr(),
                    ptr::null_mut(),
                    &mut kind,
                    buf.as_mut_ptr() as *mut u8,
                    &mut size,
                )
            };
            unsafe {
                RegCloseKey(key);
            }
            if queried == ERROR_SUCCESS {
                let chars = size as usize / 2;
                let text: String = buf[..chars.min(buf.len())]
                    .iter()
                    .map(|&c| char::from_u32(u32::from(c)).unwrap_or('\u{fffd}'))
                    .collect();
                let text = text.trim_end_matches('\0');
                if !text.is_empty() {
                    append_token(&mut search, text);
                }
            }
        }

        let flags = GAA_FLAG_SKIP_UNICAST
            | GAA_FLAG_SKIP_ANYCAST
            | GAA_FLAG_SKIP_MULTICAST
            | GAA_FLAG_SKIP_FRIENDLY_NAME;
        let mut size = 15360u32;
        let mut buffer = vec![0u8; size as usize];
        let mut ret = unsafe {
            GetAdaptersAddresses(AF_UNSPEC, flags, ptr::null_mut(), buffer.as_mut_ptr(), &mut size)
        };
        let mut tries = 0;
        while ret == ERROR_BUFFER_OVERFLOW && tries < 3 {
            buffer.resize(size as usize, 0);
            ret = unsafe {
                GetAdaptersAddresses(
                    AF_UNSPEC,
                    flags,
                    ptr::null_mut(),
                    buffer.as_mut_ptr(),
                    &mut size,
                )
            };
            tries += 1;
        }
        if ret != ERROR_SUCCESS && ret != ERROR_NO_DATA {
            return (search, servers);
        }

        let mut adapter = buffer.as_ptr();
        while !adapter.is_null() {
            let oper_status = unsafe { *(adapter.add(ADAPTER_OPER_STATUS) as *const u32) };
            if oper_status == IF_OPER_STATUS_UP {
                let mut dns = read_ptr(adapter, ADAPTER_DNS_SERVER);
                while !dns.is_null() {
                    let sockaddr = read_ptr(dns, DNS_SOCKADDR);
                    let sockaddr_len = read_u32(dns, DNS_SOCKADDR_LEN) as i32;
                    if !sockaddr.is_null() && sockaddr_len > 0 {
                        let family = unsafe { *(sockaddr as *const u16) };
                        // sockaddr_in6: skip link/site-local (scope id != 0).
                        let scoped_v6 = family == 23
                            && unsafe { *(sockaddr.add(SOCKADDR_IN6_SCOPE_ID) as *const u32) } != 0;
                        if !scoped_v6 {
                            let mut text = [0u8; 64];
                            let mut len = text.len() as u32;
                            let ok = unsafe {
                                WSAAddressToStringA(
                                    sockaddr as *const c_void,
                                    sockaddr_len,
                                    ptr::null(),
                                    text.as_mut_ptr(),
                                    &mut len,
                                )
                            };
                            if ok == 0 {
                                let end = text
                                    .iter()
                                    .position(|&b| b == 0)
                                    .unwrap_or(text.len());
                                if let Ok(addr) = std::str::from_utf8(&text[..end]) {
                                    append_token(&mut servers, addr);
                                }
                            }
                        }
                    }
                    dns = read_ptr(dns, DNS_NEXT);
                }
                let suffix = read_ptr(adapter, ADAPTER_DNS_SUFFIX);
                if !suffix.is_null() {
                    let wide: Vec<u16> = unsafe {
                        let mut out = Vec::new();
                        let mut p = suffix as *const u16;
                        while *p != 0 && out.len() < 256 {
                            out.push(*p);
                            p = p.add(1);
                        }
                        out
                    };
                    let text: String = wide.iter().map(|&c| char::from_u32(u32::from(c)).unwrap_or('\u{fffd}')).collect();
                    if !text.is_empty() {
                        append_token(&mut search, &text);
                    }
                }
            }
            adapter = read_ptr(adapter, ADAPTER_NEXT);
        }
        (search, servers)
    }

    /// `NotifyAddrChange` + blocking `GetOverlappedResult`, mirroring
    /// `notifyAddrChange0` in `ResolverConfigurationImpl.c`.
    pub fn notify_addr_change() -> i32 {
        let event = unsafe {
            CreateEventW(ptr::null(), 1, 0, ptr::null())
        };
        if event.is_null() {
            return -1;
        }
        let mut overlapped = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event,
        };
        let mut handle: *mut c_void = ptr::null_mut();
        let rc = unsafe { NotifyAddrChange(&mut handle, &mut overlapped) };
        let result = if rc == ERROR_IO_PENDING {
            let mut transferred = 0u32;
            let ok = unsafe {
                GetOverlappedResult(handle, &mut overlapped, &mut transferred, 1)
            };
            i32::from(ok == 0) * -1
        } else if rc == ERROR_SUCCESS {
            // Already signaled: treat as a pending change notification.
            0
        } else {
            -1
        };
        unsafe {
            CloseHandle(event);
        }
        result
    }
}

#[cfg(not(windows))]
mod dns_unix {
    use super::append_token;
    /// Minimal `/etc/resolv.conf` parser for `nameserver` and `search` lines.
    pub fn load_config() -> (String, String) {
        let mut search = String::new();
        let mut servers = String::new();
        let Ok(text) = std::fs::read_to_string("/etc/resolv.conf") else {
            return (search, servers);
        };
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("nameserver") {
                let value = rest.trim();
                if !value.is_empty() {
                    append_token(&mut servers, value);
                }
            } else if let Some(rest) = line.strip_prefix("search") {
                for token in rest.split_whitespace() {
                    append_token(&mut search, token);
                }
            } else if let Some(rest) = line.strip_prefix("domain") {
                let value = rest.trim();
                if !value.is_empty() {
                    append_token(&mut search, value);
                }
            }
        }
        (search, servers)
    }

    /// No address-change notification mechanism; report an error so the
    /// listener thread exits instead of spinning.
    pub fn notify_addr_change() -> i32 {
        -1
    }
}

#[cfg(windows)]
use dns_windows as platform_dns;
#[cfg(not(windows))]
use dns_unix as platform_dns;

fn append_token(buffer: &mut String, token: &str) {
    if token.is_empty() {
        return;
    }
    if !buffer.is_empty() {
        buffer.push(',');
    }
    if buffer.len() + token.len() + 1 > 1024 {
        return;
    }
    buffer.push_str(token);
}

// ---------------------------------------------------------------------------
// sun.net.spi.DefaultProxySelector
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod proxy_windows {
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(
            agent: *const u16,
            access_type: u32,
            proxy: *const u16,
            proxy_bypass: *const u16,
            flags: u32,
        ) -> *mut c_void;
        fn WinHttpCloseHandle(handle: *mut c_void) -> i32;
        fn WinHttpGetIEProxyConfigForCurrentUser(config: *mut IeProxyConfig) -> i32;
        fn WinHttpGetProxyForUrl(
            session: *mut c_void,
            url: *const u16,
            options: *const AutoProxyOptions,
            info: *mut ProxyInfo,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalFree(memory: *mut c_void) -> *mut c_void;
    }

    const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
    const WINHTTP_AUTOPROXY_AUTO_DETECT: u32 = 0x0000_0001;
    const WINHTTP_AUTOPROXY_CONFIG_URL: u32 = 0x0000_0002;
    const WINHTTP_AUTO_DETECT_TYPE_DHCP: u32 = 0x0000_0001;
    const WINHTTP_AUTO_DETECT_TYPE_DNS_A: u32 = 0x0000_0002;

    #[repr(C)]
    pub struct IeProxyConfig {
        pub flags: u32,
        pub auto_detect: i32,
        pub auto_config_url: *mut u16,
        pub proxy: *mut u16,
        pub proxy_bypass: *mut u16,
    }

    #[repr(C)]
    pub struct AutoProxyOptions {
        pub flags: u32,
        pub auto_detect_flags: u32,
        pub auto_config_url: *const u16,
        pub reserved: *mut c_void,
        pub reserved2: u32,
    }

    #[repr(C)]
    pub struct ProxyInfo {
        pub access_type: u32,
        pub proxy: *mut u16,
        pub proxy_bypass: *mut u16,
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn read_wide(ptr: *const u16) -> Option<String> {
        unsafe {
            if ptr.is_null() {
                return None;
            }
            let mut out = Vec::new();
            let mut p = ptr;
            while *p != 0 && out.len() < 4096 {
                out.push(*p);
                p = p.add(1);
            }
            Some(
                out.iter()
                    .map(|&c| char::from_u32(u32::from(c)).unwrap_or('\u{fffd}'))
                    .collect(),
            )
        }
    }

    unsafe fn free_wide(ptr: *mut u16) {
        unsafe {
            if !ptr.is_null() {
                GlobalFree(ptr as *mut c_void);
            }
        }
    }

    /// Mirrors `Java_sun_net_spi_DefaultProxySelector_getSystemProxies`
    /// from `DefaultProxySelector.c`: IE proxy config (manual or WPAD/PAC
    /// via `WinHttpGetProxyForUrl`), bypass-list matching, then the
    /// scheme-filtered proxy list.
    ///
    /// Returns `(proxy_list, bypass_list)`; either may be empty.
    pub fn system_proxies(proto: &str, host: &str) -> Option<(String, String)> {
        let agent = wide("Java/25.0");
        let session = unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                WINHTTP_NO_PROXY_NAME,
                WINHTTP_NO_PROXY_BYPASS,
                0,
            )
        };
        if session.is_null() {
            return None;
        }
        let mut config = IeProxyConfig {
            flags: 0,
            auto_detect: 0,
            auto_config_url: ptr::null_mut(),
            proxy: ptr::null_mut(),
            proxy_bypass: ptr::null_mut(),
        };
        let result = unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut config) };
        if result == 0 {
            unsafe {
                WinHttpCloseHandle(session);
            }
            return None;
        }
        let mut options = AutoProxyOptions {
            flags: 0,
            auto_detect_flags: 0,
            auto_config_url: ptr::null(),
            reserved: ptr::null_mut(),
            reserved2: 0,
        };
        let mut use_auto = false;
        if config.auto_detect != 0 {
            options.auto_detect_flags = WINHTTP_AUTO_DETECT_TYPE_DHCP
                | WINHTTP_AUTO_DETECT_TYPE_DNS_A;
            options.flags |= WINHTTP_AUTOPROXY_AUTO_DETECT;
            use_auto = true;
        }
        if !config.auto_config_url.is_null() {
            options.auto_config_url = config.auto_config_url;
            options.flags |= WINHTTP_AUTOPROXY_CONFIG_URL;
            use_auto = true;
        }
        let mut win_proxy: *mut u16 = ptr::null_mut();
        let mut win_bypass: *mut u16 = ptr::null_mut();
        let mut info = ProxyInfo {
            access_type: 0,
            proxy: ptr::null_mut(),
            proxy_bypass: ptr::null_mut(),
        };
        if use_auto {
            let url = wide(&format!("{proto}://{host}"));
            let resolved = unsafe {
                WinHttpGetProxyForUrl(session, url.as_ptr(), &options, &mut info)
            };
            if resolved != 0 {
                win_proxy = info.proxy;
                win_bypass = info.proxy_bypass;
            }
        }
        if win_proxy.is_null() && !config.proxy.is_null() {
            win_bypass = config.proxy_bypass;
            win_proxy = config.proxy;
        }
        let proxy_list = unsafe { read_wide(win_proxy) }.unwrap_or_default();
        let bypass_list = unsafe { read_wide(win_bypass) }.unwrap_or_default();
        unsafe {
            if !info.proxy.is_null() {
                GlobalFree(info.proxy as *mut c_void);
            }
            if !info.proxy_bypass.is_null() {
                GlobalFree(info.proxy_bypass as *mut c_void);
            }
            free_wide(config.auto_config_url);
            free_wide(config.proxy);
            free_wide(config.proxy_bypass);
            WinHttpCloseHandle(session);
        }
        Some((proxy_list, bypass_list))
    }

    const WINHTTP_NO_PROXY_NAME: *const u16 = 1usize as *const u16;
    const WINHTTP_NO_PROXY_BYPASS: *const u16 = 2usize as *const u16;
}

#[cfg(not(windows))]
mod proxy_unix {
    /// POSIX proxy configuration from the standard environment variables.
    pub fn system_proxies(proto: &str, _host: &str) -> Option<(String, String)> {
        let upper = proto.to_uppercase();
        let key = format!("{}_proxy", upper);
        let value = std::env::var(&key)
            .ok()
            .or_else(|| std::env::var("all_proxy").ok())
            .or_else(|| std::env::var("ALL_PROXY").ok())?;
        let bypass = std::env::var(format!("{}_no_proxy", upper))
            .ok()
            .or_else(|| std::env::var("no_proxy").ok())
            .unwrap_or_default();
        Some((value, bypass))
    }
}

#[cfg(windows)]
use proxy_windows as platform_proxy;
#[cfg(not(windows))]
use proxy_unix as platform_proxy;

/// Tokenizes on the separator set used by `createProxyList`
/// (`\t\r\n ;` in the C source).
fn proxy_tokens(list: &str) -> Vec<String> {
    list.split(['\t', '\r', '\n', ' ', ';'])
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Parses one proxy-list entry the way `createProxyList` does: optional
/// `scheme=` filter, optional `scheme://` prefix, optional `:port`.
/// Returns `(host, port)` with port 0 when absent/invalid.
fn parse_proxy_entry(entry: &str, proto: &str) -> Option<(String, i32)> {
    let mut current = entry.to_string();
    if let Some(eq) = current.find('=') {
        let (scheme, rest) = current.split_at(eq);
        if scheme != proto {
            return None;
        }
        current = rest[1..].to_string();
    }
    if let Some(pos) = current.find("://") {
        current = current[pos + 3..].to_string();
    }
    let (host, port) = match current.find(':') {
        Some(colon) => {
            let port_text = current[colon + 1..].trim_start();
            let digits: String = port_text
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
                .collect();
            let port = digits.parse::<i32>().unwrap_or(0);
            (current[..colon].to_string(), port)
        }
        None => (current, 0),
    };
    Some((host, port))
}

/// Bypass check from `DefaultProxySelector.c`: each `;`/space-separated
/// token is a host prefix; `<local>` (or any prefix of it, matching the
/// C `wcsncmp` semantics) bypasses hosts without a dot.
fn bypassed(host: &str, bypass: &str) -> bool {
    for token in bypass.split([';', ' ']).filter(|t| !t.is_empty()) {
        if host.starts_with(token) {
            return true;
        }
        if token.len() <= "<local>".len() && "<local>".starts_with(token) && !host.contains('.') {
            return true;
        }
    }
    false
}

fn default_port(proto: &str) -> i32 {
    match proto {
        "http" | "ftp" => 80,
        "https" => 443,
        "socks" => 1080,
        _ => 0,
    }
}

/// Builds one `java.net.Proxy` wrapping `InetSocketAddress.createUnresolved`.
fn proxy_object<'a>(
    loader: &ClassLoader<'a>,
    proto: &str,
    host: &str,
    port: i32,
    output: &mut dyn std::io::Write,
) -> io::Result<Value<'a>> {
    let port = if port == 0 { default_port(proto) } else { port };
    let is_socks = proto == "socks";
    let address = loader.load(b"java/net/InetSocketAddress")?;
    let host_value = host_string(loader, host);
    let mut isa_args = JavaCallArguments::new();
    isa_args.push_oop(host_value);
    isa_args.push_int(port);
    let socket_address = JavaCalls::call_static(
        loader,
        &address,
        b"createUnresolved",
        b"(Ljava/lang/String;I)Ljava/net/InetSocketAddress;",
        &isa_args,
        output,
    )?
    .unwrap_or(Value::Null);
    let proxy_type = loader.load(b"java/net/Proxy$Type")?;
    let type_name: &[u8] = if is_socks { b"SOCKS" } else { b"HTTP" };
    let type_value = proxy_type
        .staticField(type_name, b"Ljava/net/Proxy$Type;")?
        .get();
    let proxy = loader.load(b"java/net/Proxy")?;
    let object = crate::share::oops::instanceOop::InstanceOop::new(proxy.clone())?;
    let handle = loader.alloc_object(object);
    let mut ctor_args = JavaCallArguments::new();
    ctor_args.push_oop(Value::Object(handle));
    ctor_args.push_oop(type_value);
    ctor_args.push_oop(socket_address);
    JavaCalls::call_special(
        loader,
        &proxy,
        b"<init>",
        b"(Ljava/net/Proxy$Type;Ljava/net/SocketAddress;)V",
        &ctor_args,
        output,
    )?;
    Ok(Value::Object(handle))
}

fn get_system_proxies<'a>(
    loader: &ClassLoader<'a>,
    proto: &str,
    host: &str,
    output: &mut dyn std::io::Write,
) -> io::Result<Value<'a>> {
    let Some((proxy_list, bypass_list)) = platform_proxy::system_proxies(proto, host) else {
        return Ok(Value::Null);
    };
    if bypassed(host, &bypass_list) {
        return Ok(Value::Null);
    }
    let mut proxies = Vec::new();
    for token in proxy_tokens(&proxy_list) {
        let Some((proxy_host, port)) = parse_proxy_entry(&token, proto) else {
            continue;
        };
        proxies.push(proxy_object(loader, proto, &proxy_host, port, output)?);
    }
    if proxies.is_empty() {
        return Ok(Value::Null);
    }
    let mut array = ObjArrayOop::withComponent(proxies.len() as i32, b"java/net/Proxy")?;
    for (slot, proxy) in array.elements.iter_mut().zip(proxies) {
        *slot = proxy;
    }
    Ok(Value::ObjectArray(loader.alloc_obj_array(array)))
}

// ---------------------------------------------------------------------------
// sun.nio.fs.RegistryFileTypeDetector
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn query_registry_string(sub_key: &str, name: &str) -> Option<String> {
    use std::ffi::c_void;
    use std::ptr;
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            key: *mut c_void,
            sub_key: *const u16,
            options: u32,
            access: u32,
            result: *mut *mut c_void,
        ) -> u32;
        fn RegQueryValueExW(
            key: *mut c_void,
            name: *const u16,
            reserved: *mut c_void,
            kind: *mut u32,
            data: *mut u8,
            size: *mut u32,
        ) -> u32;
        fn RegCloseKey(key: *mut c_void) -> i32;
    }
    const HKEY_CLASSES_ROOT: isize = 0x8000_0000u64 as isize;
    const KEY_READ: u32 = 0x20019;
    let wide = |text: &str| -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    };
    let sub = wide(sub_key);
    let val = wide(name);
    let mut key: *mut c_void = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CLASSES_ROOT as *mut c_void,
            sub.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if opened != 0 || key.is_null() {
        return None;
    }
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let mut kind = 0u32;
    let queried = unsafe {
        RegQueryValueExW(
            key,
            val.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            buf.as_mut_ptr() as *mut u8,
            &mut size,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if queried != 0 {
        return None;
    }
    let chars = (size as usize / 2).min(buf.len());
    let text: String = buf[..chars]
        .iter()
        .map(|&c| char::from_u32(u32::from(c)).unwrap_or('\u{fffd}'))
        .collect();
    let text = text.trim_end_matches('\0').to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Reads a NUL-terminated UTF-16LE string from off-heap memory.
fn read_wide_string(address: i64) -> Option<String> {
    let base = u64::try_from(address).ok()?;
    let memory = super::offHeapMemory::OffHeapMemory::global();
    let mut units = Vec::new();
    for index in 0..512u64 {
        let lo = memory.get_byte(base + index * 2).ok()? as u8;
        let hi = memory.get_byte(base + index * 2 + 1).ok()? as u8;
        let unit = u16::from_le_bytes([lo, hi]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    Some(
        units
            .iter()
            .map(|&c| char::from_u32(u32::from(c)).unwrap_or('\u{fffd}'))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub(crate) fn is_owner(owner: &[u8]) -> bool {
    owner == b"sun/net/dns/ResolverConfigurationImpl"
        || owner == b"sun/net/spi/DefaultProxySelector"
        || owner == b"jdk/internal/vm/vector/VectorSupport"
        || owner == b"sun/nio/fs/RegistryFileTypeDetector"
}

/// Instance natives: `sun.net.spi.DefaultProxySelector.getSystemProxies`.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
    output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    if !(name == b"getSystemProxies"
        && descriptor == b"(Ljava/lang/String;Ljava/lang/String;)[Ljava/net/Proxy;")
    {
        return Ok(None);
    }
    let proto = match arguments.first() {
        Some(value) => string_arg(loader, *value)?.unwrap_or_default(),
        None => String::new(),
    };
    let host = match arguments.get(1) {
        Some(value) => string_arg(loader, *value)?.unwrap_or_default(),
        None => String::new(),
    };
    get_system_proxies(loader, &proto, &host, output).map(|v| Some(Some(v)))
}

pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
    output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    let _ = descriptor;
    let result = match (owner, name) {
        (b"sun/net/dns/ResolverConfigurationImpl", b"init0") => None,
        (b"sun/net/dns/ResolverConfigurationImpl", b"loadDNSconfig0") => {
            let (search, servers) = platform_dns::load_config();
            set_static_string(
                loader,
                b"sun/net/dns/ResolverConfigurationImpl",
                b"os_searchlist",
                &search,
            )?;
            set_static_string(
                loader,
                b"sun/net/dns/ResolverConfigurationImpl",
                b"os_nameservers",
                &servers,
            )?;
            None
        }
        (b"sun/net/dns/ResolverConfigurationImpl", b"notifyAddrChange0") => {
            return Ok(Some(Some(Value::Int(platform_dns::notify_addr_change()))));
        }
        (b"sun/net/spi/DefaultProxySelector", b"init") => {
            return Ok(Some(Some(Value::Int(1))));
        }
        (b"sun/net/spi/DefaultProxySelector", b"getSystemProxies") => {
            let proto = match arguments.first() {
                Some(value) => string_arg(loader, *value)?.unwrap_or_default(),
                None => String::new(),
            };
            let host = match arguments.get(1) {
                Some(value) => string_arg(loader, *value)?.unwrap_or_default(),
                None => String::new(),
            };
            return get_system_proxies(loader, &proto, &host, output).map(|v| Some(Some(v)));
        }
        (b"jdk/internal/vm/vector/VectorSupport", b"getCPUFeatures") => {
            // No vectorizing backend exists in this interpreter VM, so the
            // honest feature set is empty (Vector API then uses scalar
            // fallback shapes, same as HotSpot without C2).
            return Ok(Some(Some(host_string(loader, ""))));
        }
        (b"jdk/internal/vm/vector/VectorSupport", b"getMaxLaneCount") => {
            // HotSpot returns -1 without C2; VectorShape then defaults to
            // the 64-bit shape.
            return Ok(Some(Some(Value::Int(-1))));
        }
        (b"sun/nio/fs/RegistryFileTypeDetector", b"queryStringValue") => {
            let (sub_key, name) = match (
                arguments.first().copied().unwrap_or(Value::Null),
                arguments.get(1).copied().unwrap_or(Value::Null),
            ) {
                (Value::Long(sub), Value::Long(name)) => (sub, name),
                (Value::Int(sub), Value::Int(name)) => (i64::from(sub), i64::from(name)),
                _ => return Err(invalid("Invalid registry query arguments")),
            };
            let sub_key = read_wide_string(sub_key).unwrap_or_default();
            let name = read_wide_string(name).unwrap_or_default();
            #[cfg(windows)]
            {
                return Ok(Some(Some(match query_registry_string(&sub_key, &name) {
                    Some(text) => host_string(loader, &text),
                    None => Value::Null,
                })));
            }
            #[cfg(not(windows))]
            {
                let _ = (sub_key, name);
                return Ok(Some(Some(Value::Null)));
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::{append_token, bypassed, default_port, parse_proxy_entry, proxy_tokens};

    #[test]
    fn proxy_list_tokenization_matches_c_separators() {
        let tokens = proxy_tokens("http=proxy1:80\thttps=proxy2:443\r\nsocks=s:1080 ;ftp=f:21");
        assert_eq!(
            tokens,
            vec![
                "http=proxy1:80",
                "https=proxy2:443",
                "socks=s:1080",
                "ftp=f:21"
            ]
        );
    }

    #[test]
    fn proxy_entry_scheme_filter_and_prefix() {
        assert_eq!(
            parse_proxy_entry("http=corp:8080", "http"),
            Some(("corp".to_string(), 8080))
        );
        assert_eq!(parse_proxy_entry("http=corp:8080", "https"), None);
        assert_eq!(
            parse_proxy_entry("https://secure:444", "https"),
            Some(("secure".to_string(), 444))
        );
        assert_eq!(
            parse_proxy_entry("plain:3128", "http"),
            Some(("plain".to_string(), 3128))
        );
        assert_eq!(
            parse_proxy_entry("noport", "http"),
            Some(("noport".to_string(), 0))
        );
    }

    #[test]
    fn bypass_prefix_and_local_rules() {
        assert!(bypassed("internal.example.com", "internal.example.com;*.local"));
        assert!(bypassed("intranet", "<local>"));
        assert!(!bypassed("example.com", "<local>"));
        assert!(!bypassed("other.com", "allowed.com"));
    }

    #[test]
    fn default_ports_match_hotspot_table() {
        assert_eq!(default_port("http"), 80);
        assert_eq!(default_port("https"), 443);
        assert_eq!(default_port("ftp"), 80);
        assert_eq!(default_port("socks"), 1080);
        assert_eq!(default_port("socket"), 0);
    }

    #[test]
    fn dns_tokens_are_comma_joined_and_capped() {
        let mut buffer = String::new();
        append_token(&mut buffer, "8.8.8.8");
        append_token(&mut buffer, "1.1.1.1");
        assert_eq!(buffer, "8.8.8.8,1.1.1.1");
        append_token(&mut buffer, "");
        assert_eq!(buffer, "8.8.8.8,1.1.1.1");
    }
}
