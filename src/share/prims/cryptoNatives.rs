use std::io;

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;

#[cfg(windows)]
fn os_random(bytes: &mut [u8]) -> bool {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut core::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    if bytes.is_empty() {
        return true;
    }
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk = (bytes.len() - offset).min(u32::MAX as usize) as u32;
        let status = unsafe {
            BCryptGenRandom(
                core::ptr::null_mut(),
                bytes[offset..].as_mut_ptr(),
                chunk,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return false;
        }
        offset += chunk as usize;
    }
    true
}

#[cfg(not(windows))]
fn os_random(bytes: &mut [u8]) -> bool {
    use std::io::Read;
    match std::fs::File::open("/dev/urandom") {
        Ok(mut file) => file.read_exact(bytes).is_ok(),
        Err(_) => weak_random(bytes),
    }
}

#[cfg(not(windows))]
fn weak_random(bytes: &mut [u8]) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ ((std::process::id() as u64) << 32);
    let mut state = seed | 1;
    let mut buffer = [0u8; 8];
    for chunk in bytes.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        buffer[..8].copy_from_slice(&z.to_le_bytes());
        chunk.copy_from_slice(&buffer[..chunk.len()]);
    }
}

pub(crate) fn is_owner(owner: &[u8]) -> bool {
    owner == b"sun/security/provider/NativeSeedGenerator"
}

pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
    _output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    match (name, descriptor) {
        (b"nativeGenerateSeed", b"([B)Z") => {
            let filled = match arguments.first().copied() {
                Some(Value::PrimitiveArray(handle)) => {
                    let mut arrays = loader.arrays.borrow_mut();
                    match arrays.get_mut(handle) {
                        Some(TypeArrayOop::Byte(bytes)) => {
                            let mut buf = vec![0u8; bytes.len()];
                            let ok = os_random(&mut buf);
                            for (slot, &byte) in bytes.iter_mut().zip(buf.iter()) {
                                *slot = byte as i8;
                            }
                            ok
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            Ok(Some(Some(Value::Int(i32::from(filled)))))
        }
        _ => Ok(None),
    }
}
