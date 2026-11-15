/*
 * Port of OpenJDK 25 HotSpot os:: file I/O primitives (os_windows.cpp
 * os::open / os::read / os::write / os::close / os::stat / os::rename /
 * os::remove / os::lseek / os::ftruncate) over raw Win32 HANDLEs.
 *
 * HotSpot routes these through the CRT (_wopen/_lseeki64); the Rust port
 * uses CreateFileW + ReadFile/WriteFile/SetFilePointerEx directly so no CRT
 * fd table is needed. Path handling uses wide strings; callers pass UTF-8
 * paths which are converted with to_wide().
 */

use super::{last_error_io, ByHandleFileInformation, Handle, OsWindows, INVALID_HANDLE_VALUE};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

// CreateFileW access / share / disposition / attribute constants.
pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const GENERIC_EXECUTE: u32 = 0x2000_0000;

pub const FILE_SHARE_READ: u32 = 0x0000_0001;
pub const FILE_SHARE_WRITE: u32 = 0x0000_0002;
pub const FILE_SHARE_DELETE: u32 = 0x0000_0004;

pub const CREATE_NEW: u32 = 1;
pub const CREATE_ALWAYS: u32 = 2;
pub const OPEN_EXISTING: u32 = 3;
pub const OPEN_ALWAYS: u32 = 4;
pub const TRUNCATE_EXISTING: u32 = 5;

pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
pub const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
pub const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
pub const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x0000_0100;
pub const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
pub const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
pub const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
pub const FILE_FLAG_RANDOM_ACCESS: u32 = 0x1000_0000;
pub const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;

pub const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;

// os::open() oflag bits (winnt/msvcrt compatible subset).
pub const O_RDONLY: i32 = 0x0000;
pub const O_WRONLY: i32 = 0x0001;
pub const O_RDWR: i32 = 0x0002;
pub const O_CREAT: i32 = 0x0100;
pub const O_EXCL: i32 = 0x0400;
pub const O_TRUNC: i32 = 0x0200;
pub const O_APPEND: i32 = 0x0008;
pub const O_BINARY: i32 = 0x8000;
pub const O_TEMPORARY: i32 = 0x0040;
pub const O_NOINHERIT: i32 = 0x0080;

// os::lseek() whence (os::SEEK_*).
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

const FILE_BEGIN: u32 = 0;
const FILE_CURRENT: u32 = 1;
const FILE_END: u32 = 2;

const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
const MOVEFILE_COPY_ALLOWED: u32 = 0x2;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x0000_0102;
const INFINITE: u32 = 0xFFFF_FFFF;

/// os::stat() result — stat(2)-like metadata for a path.
#[derive(Debug, Clone, Copy)]
pub struct FileStat {
    pub attributes: u32,
    pub creation_time: super::FileTime,
    pub last_access_time: super::FileTime,
    pub last_write_time: super::FileTime,
    pub volume_serial_number: u32,
    pub size: u64,
    pub number_of_links: u32,
    pub file_index: u64,
}

impl FileStat {
    pub fn is_directory(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    pub fn is_readonly(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_READONLY != 0
    }
}

fn to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Convert an msvcrt-style oflag word into (dwDesiredAccess, dwCreationDisposition).
fn open_flags(oflag: i32) -> (u32, u32) {
    let access = match oflag & 0x3 {
        O_RDONLY => GENERIC_READ,
        O_WRONLY => GENERIC_WRITE,
        _ => GENERIC_READ | GENERIC_WRITE,
    };
    let disposition = if oflag & O_CREAT != 0 {
        if oflag & O_EXCL != 0 {
            CREATE_NEW
        } else if oflag & O_TRUNC != 0 {
            CREATE_ALWAYS
        } else {
            OPEN_ALWAYS
        }
    } else if oflag & O_TRUNC != 0 {
        TRUNCATE_EXISTING
    } else {
        OPEN_EXISTING
    };
    (access, disposition)
}

fn open_attributes(oflag: i32) -> u32 {
    let mut attrs = FILE_ATTRIBUTE_NORMAL;
    if oflag & O_TEMPORARY != 0 {
        attrs = FILE_ATTRIBUTE_TEMPORARY;
    }
    attrs
}

impl OsWindows {
    /// os::open(path, oflag, mode): open a file, returning a raw HANDLE.
    /// `mode` is advisory on Windows (ACLs come from the security descriptor).
    pub fn open(path: &Path, oflag: i32, _mode: u32) -> io::Result<Handle> {
        let wide = to_wide(path);
        let (access, disposition) = open_flags(oflag);
        let attrs = open_attributes(oflag);
        unsafe {
            let h = super::CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null_mut(),
                disposition,
                attrs,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE || h.is_null() {
                return Err(last_error_io("CreateFileW"));
            }
            Ok(h)
        }
    }

    /// os::open on a path string (UTF-8).
    pub fn open_str(path: &str, oflag: i32, mode: u32) -> io::Result<Handle> {
        Self::open(Path::new(path), oflag, mode)
    }

    /// os::read(fd, buf): bytes read, 0 at EOF.
    pub fn read(handle: Handle, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut done = 0usize;
        while done < buf.len() {
            let want = (buf.len() - done).min(u32::MAX as usize) as u32;
            let mut got: u32 = 0;
            unsafe {
                if super::ReadFile(
                    handle,
                    buf.as_mut_ptr().add(done),
                    want,
                    &mut got,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(last_error_io("ReadFile"));
                }
            }
            if got == 0 {
                break;
            }
            done += got as usize;
        }
        Ok(done)
    }

    /// os::write(fd, buf): bytes written (partial writes retried).
    pub fn write(handle: Handle, buf: &[u8]) -> io::Result<usize> {
        let mut done = 0usize;
        while done < buf.len() {
            let want = (buf.len() - done).min(u32::MAX as usize) as u32;
            let mut put: u32 = 0;
            unsafe {
                if super::WriteFile(
                    handle,
                    buf.as_ptr().add(done),
                    want,
                    &mut put,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(last_error_io("WriteFile"));
                }
            }
            if put == 0 {
                break;
            }
            done += put as usize;
        }
        Ok(done)
    }

    /// os::close(fd).
    pub fn close(handle: Handle) -> io::Result<()> {
        unsafe {
            if super::CloseHandle(handle) == 0 {
                return Err(last_error_io("CloseHandle"));
            }
        }
        Ok(())
    }

    /// os::lseek(fd, offset, whence): new absolute position.
    pub fn lseek(handle: Handle, offset: i64, whence: i32) -> io::Result<i64> {
        let method = match whence {
            SEEK_SET => FILE_BEGIN,
            SEEK_CUR => FILE_CURRENT,
            SEEK_END => FILE_END,
            _ => {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad whence"));
            }
        };
        unsafe {
            let mut pos: i64 = 0;
            if super::SetFilePointerEx(handle, offset, &mut pos, method) == 0 {
                return Err(last_error_io("SetFilePointerEx"));
            }
            Ok(pos)
        }
    }

    /// os::file_size(fd).
    pub fn file_size(handle: Handle) -> io::Result<u64> {
        unsafe {
            let mut size: i64 = 0;
            if super::GetFileSizeEx(handle, &mut size) == 0 {
                return Err(last_error_io("GetFileSizeEx"));
            }
            Ok(size as u64)
        }
    }

    /// os::stat(path): metadata via GetFileAttributesExW (does not follow
    /// the CRT symlink dance; reparse points report link attributes).
    pub fn stat(path: &Path) -> io::Result<FileStat> {
        let wide = to_wide(path);
        #[repr(C)]
        struct Win32FileAttributeData {
            file_attributes: u32,
            creation_time: super::FileTime,
            last_access_time: super::FileTime,
            last_write_time: super::FileTime,
            file_size_high: u32,
            file_size_low: u32,
        }
        const GET_FILEEX_INFO_STANDARD: i32 = 0;
        unsafe extern "system" {
            fn GetFileAttributesExW(
                lp_file_name: *const u16,
                info_level: i32,
                lp_file_information: *mut Win32FileAttributeData,
            ) -> i32;
        }
        unsafe {
            let mut fad = Win32FileAttributeData {
                file_attributes: 0,
                creation_time: super::FileTime::default(),
                last_access_time: super::FileTime::default(),
                last_write_time: super::FileTime::default(),
                file_size_high: 0,
                file_size_low: 0,
            };
            if GetFileAttributesExW(
                wide.as_ptr(),
                GET_FILEEX_INFO_STANDARD,
                &mut fad,
            ) == 0
            {
                return Err(last_error_io("GetFileAttributesExW"));
            }
            Ok(FileStat {
                attributes: fad.file_attributes,
                creation_time: fad.creation_time,
                last_access_time: fad.last_access_time,
                last_write_time: fad.last_write_time,
                volume_serial_number: 0,
                size: ((fad.file_size_high as u64) << 32) | fad.file_size_low as u64,
                number_of_links: 1,
                file_index: 0,
            })
        }
    }

    /// os::stat equivalent on an open handle: GetFileInformationByHandle
    /// gives volume/index/link data GetFileAttributesExW lacks.
    pub fn stat_handle(handle: Handle) -> io::Result<FileStat> {
        unsafe {
            let mut info = ByHandleFileInformation::default();
            if super::GetFileInformationByHandle(handle, &mut info) == 0 {
                return Err(last_error_io("GetFileInformationByHandle"));
            }
            Ok(FileStat {
                attributes: info.file_attributes,
                creation_time: info.creation_time,
                last_access_time: info.last_access_time,
                last_write_time: info.last_write_time,
                volume_serial_number: info.volume_serial_number,
                size: ((info.file_size_high as u64) << 32) | info.file_size_low as u64,
                number_of_links: info.number_of_links,
                file_index: ((info.file_index_high as u64) << 32)
                    | info.file_index_low as u64,
            })
        }
    }

    /// os::rename(old, new): MoveFileExW with replace+write-through, matching
    /// os_windows.cpp os::rename semantics (MOVEFILE_REPLACE_EXISTING |
    /// MOVEFILE_WRITE_THROUGH | MOVEFILE_COPY_ALLOWED).
    pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
        let old_w = to_wide(old);
        let new_w = to_wide(new);
        unsafe {
            if super::MoveFileExW(
                old_w.as_ptr(),
                new_w.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH | MOVEFILE_COPY_ALLOWED,
            ) == 0
            {
                return Err(last_error_io("MoveFileExW"));
            }
        }
        Ok(())
    }

    /// os::remove(path): DeleteFileW (directories must be empty and are
    /// handled by os::remove_directory instead).
    pub fn remove(path: &Path) -> io::Result<()> {
        let wide = to_wide(path);
        unsafe {
            if super::DeleteFileW(wide.as_ptr()) == 0 {
                return Err(last_error_io("DeleteFileW"));
            }
        }
        Ok(())
    }

    /// os::remove_directory.
    pub fn remove_directory(path: &Path) -> io::Result<()> {
        let wide = to_wide(path);
        unsafe extern "system" {
            fn RemoveDirectoryW(lp_path_name: *const u16) -> i32;
        }
        unsafe {
            if RemoveDirectoryW(wide.as_ptr()) == 0 {
                return Err(last_error_io("RemoveDirectoryW"));
            }
        }
        Ok(())
    }

    /// os::ftruncate(fd, length): seek to length then SetEndOfFile.
    pub fn ftruncate(handle: Handle, length: i64) -> io::Result<()> {
        Self::lseek(handle, length, SEEK_SET)?;
        unsafe {
            if super::SetEndOfFile(handle) == 0 {
                return Err(last_error_io("SetEndOfFile"));
            }
        }
        Ok(())
    }

    /// os::file_exists(path): GetFileAttributesW != INVALID_FILE_ATTRIBUTES.
    pub fn file_exists(path: &Path) -> bool {
        let wide = to_wide(path);
        unsafe {
            super::GetFileAttributesW(wide.as_ptr()) != INVALID_FILE_ATTRIBUTES
        }
    }

    /// os::is_directory(path).
    pub fn is_directory(path: &Path) -> io::Result<bool> {
        Ok(Self::stat(path)?.is_directory())
    }

    /// Convenience: open + write + close whole buffer (os::write_file).
    pub fn write_file(path: &Path, data: &[u8]) -> io::Result<()> {
        let h = Self::open(path, O_WRONLY | O_CREAT | O_TRUNC, 0o666)?;
        let result = Self::write(h, data).map(|_| ());
        let close_result = Self::close(h);
        result.and(close_result)
    }

    /// Convenience: open + read whole file to end (os::read_file).
    pub fn read_file(path: &Path) -> io::Result<Vec<u8>> {
        let h = Self::open(path, O_RDONLY, 0)?;
        let mut data = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match Self::read(h, &mut buf) {
                Ok(0) => break,
                Ok(n) => data.extend_from_slice(&buf[..n]),
                Err(e) => {
                    let _ = Self::close(h);
                    return Err(e);
                }
            }
        }
        Self::close(h)?;
        Ok(data)
    }

    /// Blocking wait on a file handle with timeout — used by os::sleep-like
    /// waits and overlapped I/O completion.
    pub fn wait_handle(handle: Handle, timeout_ms: u32) -> io::Result<u32> {
        let r = unsafe { super::WaitForSingleObject(handle, timeout_ms) };
        match r {
            WAIT_OBJECT_0 => Ok(r),
            WAIT_TIMEOUT => Ok(WAIT_TIMEOUT),
            other => Err(io::Error::new(
                io::ErrorKind::Other,
                format!("WaitForSingleObject: status {other}"),
            )),
        }
    }

    /// INFINITE wait helper.
    pub fn wait_handle_infinite(handle: Handle) -> io::Result<u32> {
        Self::wait_handle(handle, INFINITE)
    }
}
