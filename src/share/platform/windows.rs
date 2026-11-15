/*
 * Port of OpenJDK 25 HotSpot os::win32 / os_windows.cpp (Windows x64)
 *
 * Real Win32 bindings for the HotSpot platform layer:
 *   - VirtualAlloc/VirtualFree/VirtualProtect/VirtualQuery memory management
 *     (reserve / commit / uncommit / release / protect / guard pages)
 *   - memory-mapped files (CreateFileMappingW + MapViewOfFileEx)
 *   - timers: QueryPerformanceCounter elapsed time and javaTimeNanos,
 *     GetSystemTimeAsFileTime javaTimeMillis
 *   - processor count (GetSystemInfo + GetActiveProcessorCount),
 *     physical memory (GlobalMemoryStatusEx), page sizes
 *   - thread priority / affinity / CPU quota detection
 *   - vm_version via ntdll!RtlGetVersion (no GetVersionEx lie)
 *   - date/time conversion (FILETIME <-> java epoch)
 *   - unhandled exception filter installation
 *
 * Mirrors os_windows.cpp semantics: MEM_COMMIT is page-granular, exec
 * protection is applied with a second VirtualProtect to PAGE_EXECUTE_READWRITE,
 * and release walks VirtualQuery regions releasing each mapping.
 */

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

pub mod cpu_features;
pub mod file_io;
pub mod thread_sync;

pub type Handle = *mut c_void;
pub type Address = usize;

// ---------------------------------------------------------------------------
// Win32 constants (mirroring winnt.h / memoryapi.h)
// ---------------------------------------------------------------------------

pub const MEM_COMMIT: u32 = 0x0000_1000;
pub const MEM_RESERVE: u32 = 0x0000_2000;
pub const MEM_DECOMMIT: u32 = 0x0000_4000;
pub const MEM_RELEASE: u32 = 0x0000_8000;
pub const MEM_RESET: u32 = 0x0008_0000;
pub const MEM_TOP_DOWN: u32 = 0x0010_0000;
pub const MEM_LARGE_PAGES: u32 = 0x2000_0000;
pub const MEM_PHYSICAL: u32 = 0x0040_0000;

pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_WRITECOPY: u32 = 0x08;
pub const PAGE_EXECUTE: u32 = 0x10;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const PAGE_GUARD: u32 = 0x100;
pub const PAGE_NOCACHE: u32 = 0x200;
pub const PAGE_WRITECOMBINE: u32 = 0x400;

pub const MEM_FREE: u32 = 0x10000;
pub const MEM_RESERVE_STATE: u32 = 0x2000;
pub const MEM_COMMIT_STATE: u32 = 0x1000;
pub const MEM_PRIVATE: u32 = 0x20000;
pub const MEM_MAPPED: u32 = 0x40000;
pub const MEM_IMAGE: u32 = 0x1000000;

const FILE_MAP_READ: u32 = 0x0000_0004;
const FILE_MAP_WRITE: u32 = 0x0000_0002;
const PAGE_READONLY_MAP: u32 = 0x02;
const PAGE_READWRITE_MAP: u32 = 0x04;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

// Thread priorities (winbase.h)
pub const THREAD_PRIORITY_LOWEST: i32 = 1;
pub const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
pub const THREAD_PRIORITY_NORMAL: i32 = 0;
pub const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
pub const THREAD_PRIORITY_HIGHEST: i32 = 2;
pub const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
pub const THREAD_PRIORITY_IDLE: i32 = -15;

// Process priority classes (winbase.h)
pub const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
pub const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
pub const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;
pub const REALTIME_PRIORITY_CLASS: u32 = 0x0000_0100;
pub const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
pub const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

const ALL_PROCESSOR_GROUPS: u16 = 0xFFFF;

pub const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
pub const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
pub const EXCEPTION_EXECUTE_HANDLER: i32 = 1;

// ---------------------------------------------------------------------------
// Win32 structures (x64 layouts)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Default)]
pub struct SystemInfo {
    pub oem_id: u32, // union { dwOemId } / { wProcessorArchitecture, wReserved }
    pub page_size: u32,
    pub min_app_address: usize,
    pub max_app_address: usize,
    pub active_processor_mask: usize,
    pub number_of_processors: u32,
    pub processor_type: u32,
    pub allocation_granularity: u32,
    pub processor_level: u16,
    pub processor_revision: u16,
}

#[repr(C)]
#[derive(Default)]
pub struct MemoryStatusEx {
    pub length: u32,
    pub memory_load: u32,
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_page_file: u64,
    pub avail_page_file: u64,
    pub total_virtual: u64,
    pub avail_virtual: u64,
    pub avail_extended_virtual: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FileTime {
    pub low: u32,
    pub high: u32,
}

impl FileTime {
    pub fn ticks(self) -> i64 {
        ((self.high as i64) << 32) | self.low as i64
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SystemTime {
    pub year: u16,
    pub month: u16,
    pub day_of_week: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub milliseconds: u16,
}

#[repr(C)]
#[derive(Default)]
pub struct MemoryBasicInformation {
    pub base_address: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub partition_id: u16,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub type_: u32,
}

#[repr(C)]
pub struct OsVersionInfoExW {
    pub size: u32,
    pub major_version: u32,
    pub minor_version: u32,
    pub build_number: u32,
    pub platform_id: u32,
    pub csd_version: [u16; 128],
    pub service_pack_major: u16,
    pub service_pack_minor: u16,
    pub suite_mask: u16,
    pub product_type: u8,
    pub reserved: u8,
}

impl Default for OsVersionInfoExW {
    fn default() -> Self {
        Self {
            size: 0,
            major_version: 0,
            minor_version: 0,
            build_number: 0,
            platform_id: 0,
            csd_version: [0; 128],
            service_pack_major: 0,
            service_pack_minor: 0,
            suite_mask: 0,
            product_type: 0,
            reserved: 0,
        }
    }
}

impl OsVersionInfoExW {
    pub fn is_windows_11_or_greater(&self) -> bool {
        self.build_number >= 22000
    }

    pub fn is_windows_server_2022_or_greater(&self) -> bool {
        self.build_number >= 20348 && (self.product_type == 2 || self.product_type == 3)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ExceptionRecord {
    pub code: u32,
    pub flags: u32,
    pub record: usize,
    pub address: usize,
    pub number_parameters: u32,
    pub exception_information: [usize; 15],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ExceptionPointers {
    pub exception_record: usize,
    pub context_record: usize,
}

pub type TopLevelExceptionFilter =
    unsafe extern "system" fn(*const ExceptionPointers) -> i32;

/// LPTHREAD_START_ROUTINE (winbase.h).
pub type ThreadStartRoutine = unsafe extern "system" fn(*mut c_void) -> u32;

/// BY_HANDLE_FILE_INFORMATION (fileapi.h).
#[repr(C)]
#[derive(Default)]
pub struct ByHandleFileInformation {
    pub file_attributes: u32,
    pub creation_time: FileTime,
    pub last_access_time: FileTime,
    pub last_write_time: FileTime,
    pub volume_serial_number: u32,
    pub file_size_high: u32,
    pub file_size_low: u32,
    pub number_of_links: u32,
    pub file_index_high: u32,
    pub file_index_low: u32,
}

/// GROUP_AFFINITY (winnt.h, x64: KAFFINITY mask + group + reserved).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GroupAffinity {
    pub mask: usize,
    pub group: u16,
    pub reserved: [u16; 3],
}

/// HotSpot os::ProtType.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prot {
    None,
    Read,
    ReadWrite,
    ReadWriteExecute,
}

impl Prot {
    fn page_flag(self) -> u32 {
        match self {
            Self::None => PAGE_NOACCESS,
            Self::Read => PAGE_READONLY,
            Self::ReadWrite => PAGE_READWRITE,
            Self::ReadWriteExecute => PAGE_EXECUTE_READWRITE,
        }
    }

    /// Decode a Windows protection constant back to a Prot.
    pub fn from_page_protect(protect: u32) -> Self {
        match protect & !(PAGE_GUARD | PAGE_NOCACHE | PAGE_WRITECOMBINE) {
            PAGE_NOACCESS => Self::None,
            PAGE_READONLY => Self::Read,
            PAGE_READWRITE => Self::ReadWrite,
            PAGE_EXECUTE | PAGE_EXECUTE_READ => Self::Read,
            PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY => Self::ReadWriteExecute,
            _ => Self::ReadWrite,
        }
    }
}

/// One row of a VirtualQuery walk (os::win32::mapping_info_t analogue).
#[derive(Debug, Clone)]
pub struct MemoryMapping {
    pub base: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub type_: u32,
}

// ---------------------------------------------------------------------------
// FFI declarations
// ---------------------------------------------------------------------------

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(
        lp_address: *mut c_void,
        dw_size: usize,
        fl_allocation_type: u32,
        fl_protect: u32,
    ) -> *mut c_void;
    fn VirtualFree(lp_address: *mut c_void, dw_size: usize, dw_free_type: u32) -> i32;
    fn VirtualProtect(
        lp_address: *mut c_void,
        dw_size: usize,
        fl_new_protect: u32,
        lp_fl_old_protect: *mut u32,
    ) -> i32;
    fn VirtualQuery(
        lp_address: *const c_void,
        lp_buffer: *mut MemoryBasicInformation,
        dw_length: usize,
    ) -> usize;
    fn GetSystemInfo(lp_system_info: *mut SystemInfo);
    fn GetNativeSystemInfo(lp_system_info: *mut SystemInfo);
    fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
    fn QueryPerformanceFrequency(lp_frequency: *mut i64) -> i32;
    fn QueryPerformanceCounter(lp_performance_count: *mut i64) -> i32;
    fn GetTickCount64() -> u64;
    fn GetSystemTimeAsFileTime(lp_system_time_as_file_time: *mut FileTime);
    fn SystemTimeToFileTime(
        lp_system_time: *const SystemTime,
        lp_file_time: *mut FileTime,
    ) -> i32;
    fn FileTimeToSystemTime(lp_file_time: *const FileTime, lp_system_time: *mut SystemTime)
        -> i32;
    fn GetSystemTime(lp_system_time: *mut SystemTime);
    fn GetLocalTime(lp_local_time: *mut SystemTime);
    fn GetThreadTimes(
        h_thread: Handle,
        lp_creation_time: *mut FileTime,
        lp_exit_time: *mut FileTime,
        lp_kernel_time: *mut FileTime,
        lp_user_time: *mut FileTime,
    ) -> i32;
    fn GetProcessTimes(
        h_process: Handle,
        lp_creation_time: *mut FileTime,
        lp_exit_time: *mut FileTime,
        lp_kernel_time: *mut FileTime,
        lp_user_time: *mut FileTime,
    ) -> i32;
    fn SetThreadPriority(h_thread: Handle, n_priority: i32) -> i32;
    fn GetThreadPriority(h_thread: Handle) -> i32;
    fn GetCurrentThread() -> Handle;
    fn SetThreadAffinityMask(h_thread: Handle, dw_thread_affinity_mask: usize) -> usize;
    fn GetProcessAffinityMask(
        h_process: Handle,
        lp_process_affinity_mask: *mut usize,
        lp_system_affinity_mask: *mut usize,
    ) -> i32;
    fn SetPriorityClass(h_process: Handle, dw_priority_class: u32) -> i32;
    fn GetPriorityClass(h_process: Handle) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn Sleep(dw_milliseconds: u32);
    fn SwitchToThread() -> i32;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetLargePageMinimum() -> usize;
    fn CreateFileMappingW(
        h_file: Handle,
        lp_file_mapping_attributes: *mut c_void,
        fl_protect: u32,
        dw_maximum_size_high: u32,
        dw_maximum_size_low: u32,
        lp_name: *const u16,
    ) -> Handle;
    fn MapViewOfFileEx(
        h_file_mapping_object: Handle,
        dw_desired_access: u32,
        dw_file_offset_high: u32,
        dw_file_offset_low: u32,
        dw_number_of_bytes_to_map: usize,
        lp_base_address: *mut c_void,
    ) -> *mut c_void;
    fn UnmapViewOfFile(lp_base_address: *const c_void) -> i32;
    fn CloseHandle(h_object: Handle) -> i32;
    fn SetUnhandledExceptionFilter(
        lp_top_level_exception_filter: TopLevelExceptionFilter,
    ) -> TopLevelExceptionFilter;
    fn GetLastError() -> u32;
    fn SetLastError(dw_err_code: u32);
    fn FormatMessageW(
        dw_flags: u32,
        lp_source: *const c_void,
        dw_message_id: u32,
        dw_language_id: u32,
        lp_buffer: *mut u16,
        n_size: u32,
        arguments: *mut c_void,
    ) -> u32;
    fn GetActiveProcessorCount(group_number: u16) -> u32;
    fn SetProcessWorkingSetSize(
        h_process: Handle,
        dw_minimum_working_set_size: usize,
        dw_maximum_working_set_size: usize,
    ) -> i32;
    fn CreateFileW(
        lp_file_name: *const u16,
        dw_desired_access: u32,
        dw_share_mode: u32,
        lp_security_attributes: *mut c_void,
        dw_creation_disposition: u32,
        dw_flags_and_attributes: u32,
        h_template_file: Handle,
    ) -> Handle;
    fn ReadFile(
        h_file: Handle,
        lp_buffer: *mut u8,
        n_number_of_bytes_to_read: u32,
        lp_number_of_bytes_read: *mut u32,
        lp_overlapped: *mut c_void,
    ) -> i32;
    fn WriteFile(
        h_file: Handle,
        lp_buffer: *const u8,
        n_number_of_bytes_to_write: u32,
        lp_number_of_bytes_written: *mut u32,
        lp_overlapped: *mut c_void,
    ) -> i32;
    fn GetFileSizeEx(h_file: Handle, lp_file_size: *mut i64) -> i32;
    fn SetFilePointerEx(
        h_file: Handle,
        li_distance_to_move: i64,
        lp_new_file_pointer: *mut i64,
        dw_move_method: u32,
    ) -> i32;
    fn GetFileInformationByHandle(
        h_file: Handle,
        lp_file_information: *mut ByHandleFileInformation,
    ) -> i32;
    fn SetEndOfFile(h_file: Handle) -> i32;
    fn MoveFileExW(
        lp_existing_file_name: *const u16,
        lp_new_file_name: *const u16,
        dw_flags: u32,
    ) -> i32;
    fn DeleteFileW(lp_file_name: *const u16) -> i32;
    fn GetFileAttributesW(lp_file_name: *const u16) -> u32;
    fn CreateThread(
        lp_thread_attributes: *mut c_void,
        dw_stack_size: usize,
        lp_start_address: ThreadStartRoutine,
        lp_parameter: *mut c_void,
        dw_creation_flags: u32,
        lp_thread_id: *mut u32,
    ) -> Handle;
    fn OpenThread(
        dw_desired_access: u32,
        b_inherit_handle: i32,
        dw_thread_id: u32,
    ) -> Handle;
    fn SuspendThread(h_thread: Handle) -> u32;
    fn ResumeThread(h_thread: Handle) -> u32;
    fn TerminateThread(h_thread: Handle, dw_exit_code: u32) -> i32;
    fn WaitForSingleObject(h_handle: Handle, dw_milliseconds: u32) -> u32;
    fn GetExitCodeThread(h_thread: Handle, lp_exit_code: *mut u32) -> i32;
    fn CreateEventW(
        lp_event_attributes: *mut c_void,
        b_manual_reset: i32,
        b_initial_state: i32,
        lp_name: *const u16,
    ) -> Handle;
    fn SetEvent(h_event: Handle) -> i32;
    fn ResetEvent(h_event: Handle) -> i32;
    fn PulseEvent(h_event: Handle) -> i32;
    fn CreateMutexW(
        lp_mutex_attributes: *mut c_void,
        b_initial_owner: i32,
        lp_name: *const u16,
    ) -> Handle;
    fn ReleaseMutex(h_mutex: Handle) -> i32;
    fn CreateSemaphoreW(
        lp_semaphore_attributes: *mut c_void,
        l_initial_count: i32,
        l_maximum_count: i32,
        lp_name: *const u16,
    ) -> Handle;
    fn ReleaseSemaphore(
        h_semaphore: Handle,
        l_release_count: i32,
        lp_previous_count: *mut i32,
    ) -> i32;
    fn GetThreadGroupAffinity(
        h_thread: Handle,
        lp_group_affinity: *mut GroupAffinity,
    ) -> i32;
    fn SetThreadGroupAffinity(
        h_thread: Handle,
        group_affinity: *mut GroupAffinity,
        previous_group_affinity: *mut GroupAffinity,
    ) -> i32;
    fn GetMaximumProcessorGroupCount() -> u16;
    fn GetCurrentProcessorNumber() -> u32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    // RtlGenRandom (a.k.a. SystemFunction036), advapi32.dll export.
    fn SystemFunction036(random_buffer: *mut u8, random_buffer_length: u32) -> u8;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(lp_os_version_info: *mut OsVersionInfoExW) -> i32;
}

// ---------------------------------------------------------------------------
// OsWindows — the platform singleton
// ---------------------------------------------------------------------------

/// Cached, immutable platform state filled by `OsWindows::init()`.
#[derive(Debug, Clone, Copy)]
pub struct PlatformInfo {
    pub vm_page_size: usize,
    pub vm_allocation_granularity: usize,
    pub processor_count: u32,
    pub processor_type: u32,
    pub processor_level: u16,
    pub processor_revision: u16,
    pub physical_memory: u64,
    pub available_physical_memory: u64,
    pub total_page_file: u64,
    pub available_page_file: u64,
    pub min_app_address: usize,
    pub max_app_address: usize,
    pub large_page_minimum: usize,
}

static PLATFORM: OnceLock<PlatformInfo> = OnceLock::new();
static PERF_FREQUENCY: OnceLock<i64> = OnceLock::new();
static INITIAL_PERF_COUNTER: OnceLock<i64> = OnceLock::new();

/// os::_rand_seed (os.cpp:96) — Park-Miller LCG state behind os::random().
static RAND_SEED: AtomicU32 = AtomicU32::new(1234567);

const FILETIME_UNIX_OFFSET_TICKS: i64 = 116_444_736_000_000_000;

/// Port of os::win32 / os_* implementation for Windows x64.
pub struct OsWindows;

impl OsWindows {
    /// os::win32::initialize_system_info() + initialize_performance_counter().
    pub fn init() -> io::Result<()> {
        unsafe {
            let mut si = SystemInfo::default();
            GetSystemInfo(&mut si);

            let mut ms = MemoryStatusEx {
                length: std::mem::size_of::<MemoryStatusEx>() as u32,
                ..MemoryStatusEx::default()
            };
            if GlobalMemoryStatusEx(&mut ms) == 0 {
                return Err(last_error_io("GlobalMemoryStatusEx"));
            }

            // os::win32::initialize_system_info: prefer GetActiveProcessorCount
            // when the OS schedules all processor groups (Win11 / Server 2022+).
            let mut processors = 0u32;
            if let Some(ver) = vm_version() {
                if ver.is_windows_11_or_greater() || ver.is_windows_server_2022_or_greater() {
                    processors = GetActiveProcessorCount(ALL_PROCESSOR_GROUPS);
                }
            }
            if processors == 0 {
                processors = si.number_of_processors;
            }

            let info = PlatformInfo {
                vm_page_size: si.page_size as usize,
                vm_allocation_granularity: si.allocation_granularity as usize,
                processor_count: processors,
                processor_type: si.processor_type,
                processor_level: si.processor_level,
                processor_revision: si.processor_revision,
                physical_memory: ms.total_phys,
                available_physical_memory: ms.avail_phys,
                total_page_file: ms.total_page_file,
                available_page_file: ms.avail_page_file,
                min_app_address: si.min_app_address,
                max_app_address: si.max_app_address,
                large_page_minimum: GetLargePageMinimum(),
            };
            let _ = PLATFORM.set(info);

            let mut freq: i64 = 0;
            if QueryPerformanceFrequency(&mut freq) == 0 || freq <= 0 {
                return Err(last_error_io("QueryPerformanceFrequency"));
            }
            let _ = PERF_FREQUENCY.set(freq);
            let mut counter: i64 = 0;
            QueryPerformanceCounter(&mut counter);
            let _ = INITIAL_PERF_COUNTER.set(counter);
        }
        Ok(())
    }

    /// Fallback for callers that never ran init (tests, early boot).
    fn info() -> PlatformInfo {
        *PLATFORM.get_or_init(|| unsafe {
            let mut si = SystemInfo::default();
            GetSystemInfo(&mut si);
            let mut ms = MemoryStatusEx {
                length: std::mem::size_of::<MemoryStatusEx>() as u32,
                ..MemoryStatusEx::default()
            };
            GlobalMemoryStatusEx(&mut ms);
            PlatformInfo {
                vm_page_size: si.page_size as usize,
                vm_allocation_granularity: si.allocation_granularity as usize,
                processor_count: si.number_of_processors,
                processor_type: si.processor_type,
                processor_level: si.processor_level,
                processor_revision: si.processor_revision,
                physical_memory: ms.total_phys,
                available_physical_memory: ms.avail_phys,
                total_page_file: ms.total_page_file,
                available_page_file: ms.avail_page_file,
                min_app_address: si.min_app_address,
                max_app_address: si.max_app_address,
                large_page_minimum: GetLargePageMinimum(),
            }
        })
    }

    // -- page / memory geometry ------------------------------------------------

    #[inline]
    pub fn vm_page_size() -> usize {
        Self::info().vm_page_size
    }

    #[inline]
    pub fn vm_allocation_granularity() -> usize {
        Self::info().vm_allocation_granularity
    }

    #[inline]
    pub fn large_page_minimum() -> usize {
        Self::info().large_page_minimum
    }

    pub fn is_large_pages_available() -> bool {
        Self::info().large_page_minimum > 0
    }

    #[inline]
    pub fn processor_count() -> u32 {
        Self::info().processor_count
    }

    /// os::active_processor_count(): processors available to this process.
    /// Falls back to GetProcessAffinityMask / dwNumberOfProcessors like
    /// os_windows.cpp lines ~1040-1160.
    pub fn active_processor_count() -> u32 {
        unsafe {
            let proc = GetCurrentProcess();
            let mut proc_mask: usize = 0;
            let mut sys_mask: usize = 0;
            if GetProcessAffinityMask(proc, &mut proc_mask, &mut sys_mask) != 0 {
                let count = proc_mask.count_ones();
                if count > 0 {
                    return count;
                }
            }
            let mut si = SystemInfo::default();
            GetSystemInfo(&mut si);
            si.number_of_processors
        }
    }

    #[inline]
    pub fn physical_memory() -> u64 {
        Self::info().physical_memory
    }

    #[inline]
    pub fn available_physical_memory() -> u64 {
        Self::info().available_physical_memory
    }

    #[inline]
    pub fn total_page_file() -> u64 {
        Self::info().total_page_file
    }

    #[inline]
    pub fn available_page_file() -> u64 {
        Self::info().available_page_file
    }

    /// Live GlobalMemoryStatusEx refresh (os::refresh_memory_info equivalent).
    pub fn refresh_memory_status() -> io::Result<MemoryStatusEx> {
        unsafe {
            let mut ms = MemoryStatusEx {
                length: std::mem::size_of::<MemoryStatusEx>() as u32,
                ..MemoryStatusEx::default()
            };
            if GlobalMemoryStatusEx(&mut ms) == 0 {
                return Err(last_error_io("GlobalMemoryStatusEx"));
            }
            Ok(ms)
        }
    }

    // -- timers -----------------------------------------------------------------

    /// os::elapsedTime(): seconds since VM start (QPC based).
    pub fn elapsed_time() -> f64 {
        unsafe {
            let freq = *PERF_FREQUENCY.get().unwrap_or(&1);
            let initial = *INITIAL_PERF_COUNTER.get().unwrap_or(&0);
            let mut count: i64 = 0;
            QueryPerformanceCounter(&mut count);
            let elapsed_counts = count.saturating_sub(initial);
            if freq > 0 {
                elapsed_counts as f64 / freq as f64
            } else {
                0.0
            }
        }
    }

    /// os::elapsedCounter() raw ticks.
    pub fn elapsed_counter() -> i64 {
        unsafe {
            let mut count: i64 = 0;
            QueryPerformanceCounter(&mut count);
            count.saturating_sub(*INITIAL_PERF_COUNTER.get().unwrap_or(&0))
        }
    }

    #[inline]
    pub fn performance_frequency() -> i64 {
        *PERF_FREQUENCY.get().unwrap_or(&0)
    }

    /// os::javaTimeNanos(): QPC-derived nanos.
    pub fn java_time_nanos() -> i64 {
        unsafe {
            let freq = *PERF_FREQUENCY.get().unwrap_or(&0);
            let mut count: i64 = 0;
            QueryPerformanceCounter(&mut count);
            if freq > 0 {
                // (count * 1e9) / freq, done in f64 like os_windows.cpp
                (count as f64 * 1_000_000_000.0 / freq as f64) as i64
            } else {
                0
            }
        }
    }

    /// os::javaTimeMillis(): FILETIME → Unix epoch ms.
    pub fn java_time_millis() -> i64 {
        unsafe {
            let mut wt = FileTime::default();
            GetSystemTimeAsFileTime(&mut wt);
            crate::share::platform::neutral::filetime_to_unix_ms(wt.low, wt.high)
        }
    }

    /// os::javaTimeSystemUTC.
    pub fn java_time_system_utc() -> (i64, i64) {
        unsafe {
            let mut wt = FileTime::default();
            GetSystemTimeAsFileTime(&mut wt);
            let raw = wt.ticks() - FILETIME_UNIX_OFFSET_TICKS;
            let millis = raw / 10_000;
            let ticks_in_ms = raw - millis * 10_000;
            let secs = millis / 1000;
            let nanos = (millis - secs * 1000) * 1_000_000 + ticks_in_ms * 100;
            (secs, nanos)
        }
    }

    /// os::getHostName-adjacent wall clock: FILETIME → SYSTEMTIME (UTC).
    pub fn system_time_utc() -> SystemTime {
        unsafe {
            let mut wt = FileTime::default();
            GetSystemTimeAsFileTime(&mut wt);
            let mut st = SystemTime::default();
            FileTimeToSystemTime(&wt, &mut st);
            st
        }
    }

    /// Local wall-clock time (GetLocalTime) for date/time natives.
    pub fn local_time() -> SystemTime {
        unsafe {
            let mut st = SystemTime::default();
            GetLocalTime(&mut st);
            st
        }
    }

    /// UTC wall-clock time via GetSystemTime (uncached SYSTEMTIME).
    pub fn system_time() -> SystemTime {
        unsafe {
            let mut st = SystemTime::default();
            GetSystemTime(&mut st);
            st
        }
    }

    /// FILETIME → unix ms for an arbitrary FILETIME (java_to_windows_time
    /// inverse; used by date/time natives).
    pub fn filetime_to_unix_ms(ft: FileTime) -> i64 {
        crate::share::platform::neutral::filetime_to_unix_ms(ft.low, ft.high)
    }

    /// unix ms → FILETIME ticks (java_to_windows_time equivalent).
    pub fn unix_ms_to_filetime_ticks(ms: i64) -> i64 {
        crate::share::platform::neutral::unix_ms_to_filetime(ms)
    }

    /// GetTickCount64 — ms since system boot (os::current_elapsed_time ms).
    pub fn tick_count64() -> u64 {
        unsafe { GetTickCount64() }
    }

    /// Process CPU time (kernel+user) via GetProcessTimes — os::elapsedVTime
    /// for the whole VM process rather than just this thread.
    pub fn process_vtime() -> f64 {
        unsafe {
            let mut created = FileTime::default();
            let mut exited = FileTime::default();
            let mut kernel = FileTime::default();
            let mut user = FileTime::default();
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            ) != 0
            {
                (kernel.ticks() + user.ticks()) as f64 / 10_000_000.0
            } else {
                Self::elapsed_vtime()
            }
        }
    }

    /// SYSTEMTIME → unix epoch ms (SystemTimeToFileTime path).
    pub fn system_time_to_unix_ms(st: &SystemTime) -> io::Result<i64> {
        unsafe {
            let mut ft = FileTime::default();
            if SystemTimeToFileTime(st, &mut ft) == 0 {
                return Err(last_error_io("SystemTimeToFileTime"));
            }
            Ok(Self::filetime_to_unix_ms(ft))
        }
    }

    /// GetSystemTimeAsFileTime raw (for callers needing FILETIME directly).
    pub fn system_time_as_filetime() -> FileTime {
        unsafe {
            let mut wt = FileTime::default();
            GetSystemTimeAsFileTime(&mut wt);
            wt
        }
    }

    /// os::elapsedVTime(): process CPU time (kernel+user) in seconds.
    pub fn elapsed_vtime() -> f64 {
        unsafe {
            let mut created = FileTime::default();
            let mut exited = FileTime::default();
            let mut kernel = FileTime::default();
            let mut user = FileTime::default();
            if GetThreadTimes(
                GetCurrentThread(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            ) != 0
            {
                // FILETIME ticks are 100ns units; CPU time has no epoch offset.
                let cpu_100ns = kernel.ticks() + user.ticks();
                cpu_100ns as f64 / 10_000_000.0
            } else {
                Self::elapsed_time()
            }
        }
    }

    pub fn current_time_ticks() -> i64 {
        unsafe {
            let mut wt = FileTime::default();
            GetSystemTimeAsFileTime(&mut wt);
            wt.ticks() - FILETIME_UNIX_OFFSET_TICKS
        }
    }

    // -- virtual memory (os::pd_* / Memory::reserve_memory) --------------------

    /// os::pd_reserve_memory: MEM_RESERVE a range (PAGE_READWRITE reservation).
    /// `exec` is advisory only — real exec protection is applied at commit.
    pub fn reserve_memory(addr: usize, bytes: usize) -> io::Result<usize> {
        if bytes == 0 {
            return Ok(addr);
        }
        unsafe {
            let p = VirtualAlloc(
                addr as *mut c_void,
                bytes,
                MEM_RESERVE,
                PAGE_READWRITE,
            );
            if p.is_null() {
                return Err(last_error_io("VirtualAlloc(MEM_RESERVE)"));
            }
            Ok(p as usize)
        }
    }

    /// os::reserve_memory_aligned: reserve size+alignment, find aligned subrange.
    pub fn reserve_memory_aligned(addr: usize, size: usize, alignment: usize) -> io::Result<usize> {
        let alignment = alignment.max(1);
        let granularity = Self::vm_allocation_granularity();
        let alloc_size = size + alignment + granularity;
        // Reserve a larger block, then try to commit-aligned inside it by
        // re-reserving: Windows cannot reserve subranges of a reservation, so
        // we release and retry at a top-down hint instead.
        let mut candidate = addr;
        for _ in 0..8 {
            let base = Self::reserve_memory(candidate, alloc_size)?;
            let aligned = (base + alignment - 1) & !(alignment - 1);
            if aligned + size <= base + alloc_size {
                // Release the reservation and re-reserve exactly at aligned.
                unsafe {
                    VirtualFree(base as *mut c_void, 0, MEM_RELEASE);
                }
                match Self::reserve_memory(aligned, size) {
                    Ok(p) if p == aligned => return Ok(p),
                    Ok(p) => {
                        unsafe {
                            VirtualFree(p as *mut c_void, 0, MEM_RELEASE);
                        }
                        candidate = p.wrapping_add(granularity);
                    }
                    Err(_) => {
                        candidate = aligned.wrapping_add(granularity);
                    }
                }
            } else {
                unsafe {
                    VirtualFree(base as *mut c_void, 0, MEM_RELEASE);
                }
                candidate = base.wrapping_add(granularity);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::OutOfMemory,
            "reserve_memory_aligned: no aligned region found",
        ))
    }

    /// os::pd_commit_memory: MEM_COMMIT within an existing reservation, then
    /// VirtualProtect to RWX when exec is requested.
    pub fn commit_memory(addr: usize, bytes: usize, exec: bool) -> io::Result<()> {
        if bytes == 0 {
            return Ok(());
        }
        unsafe {
            let p = VirtualAlloc(
                addr as *mut c_void,
                bytes,
                MEM_COMMIT,
                PAGE_READWRITE,
            );
            if p.is_null() {
                return Err(last_error_io("VirtualAlloc(MEM_COMMIT)"));
            }
            if exec {
                let mut old: u32 = 0;
                if VirtualProtect(
                    addr as *mut c_void,
                    bytes,
                    PAGE_EXECUTE_READWRITE,
                    &mut old,
                ) == 0
                {
                    return Err(last_error_io("VirtualProtect(PAGE_EXECUTE_READWRITE)"));
                }
            }
        }
        Ok(())
    }

    /// os::pd_commit_memory_or_exit equivalent.
    pub fn commit_memory_or_exit(addr: usize, bytes: usize, exec: bool, msg: &str) {
        if let Err(e) = Self::commit_memory(addr, bytes, exec) {
            // Matches vm_exit_out_of_memory path: report and abort the process.
            eprintln!("fatal: {}: commit_memory(0x{:x}, {}, exec={}) failed: {}", msg, addr, bytes, exec, e);
            std::process::abort();
        }
    }

    /// os::pd_uncommit_memory: MEM_DECOMMIT keeps the reservation.
    pub fn uncommit_memory(addr: usize, bytes: usize) -> io::Result<()> {
        if bytes == 0 {
            return Ok(());
        }
        unsafe {
            if VirtualFree(addr as *mut c_void, bytes, MEM_DECOMMIT) == 0 {
                return Err(last_error_io("VirtualFree(MEM_DECOMMIT)"));
            }
        }
        Ok(())
    }

    /// os::pd_release_memory: walk VirtualQuery mappings and MEM_RELEASE each.
    pub fn release_memory(addr: usize, bytes: usize) -> io::Result<()> {
        let start = addr;
        let end = start.saturating_add(bytes);
        let mut p = start;
        while p < end {
            let Some(mi) = Self::virtual_query(p) else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "release_memory: no mapping found",
                ));
            };
            if mi.region_size == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "release_memory: zero-length mapping",
                ));
            }
            unsafe {
                if VirtualFree(mi.allocation_base as *mut c_void, 0, MEM_RELEASE) == 0 {
                    return Err(last_error_io("VirtualFree(MEM_RELEASE)"));
                }
            }
            p = mi.base.saturating_add(mi.region_size);
        }
        Ok(())
    }

    /// os::protect_memory(addr, bytes, prot, is_committed).
    pub fn protect_memory(
        addr: usize,
        bytes: usize,
        prot: Prot,
        is_committed: bool,
    ) -> io::Result<()> {
        if !is_committed {
            Self::commit_memory(addr, bytes, prot == Prot::ReadWriteExecute)?;
        }
        unsafe {
            let mut old: u32 = 0;
            if VirtualProtect(addr as *mut c_void, bytes, prot.page_flag(), &mut old) == 0 {
                return Err(last_error_io("VirtualProtect"));
            }
        }
        Ok(())
    }

    /// os::guard_memory: PAGE_GUARD one-shot access alarm.
    pub fn guard_memory(addr: usize, bytes: usize) -> io::Result<()> {
        unsafe {
            let mut old: u32 = 0;
            if VirtualProtect(
                addr as *mut c_void,
                bytes,
                PAGE_READWRITE | PAGE_GUARD,
                &mut old,
            ) == 0
            {
                return Err(last_error_io("VirtualProtect(PAGE_GUARD)"));
            }
        }
        Ok(())
    }

    /// os::unguard_memory.
    pub fn unguard_memory(addr: usize, bytes: usize) -> io::Result<()> {
        unsafe {
            let mut old: u32 = 0;
            if VirtualProtect(addr as *mut c_void, bytes, PAGE_READWRITE, &mut old) == 0 {
                return Err(last_error_io("VirtualProtect(unguard)"));
            }
        }
        Ok(())
    }

    /// os::allocate_memory: single VirtualAlloc MEM_COMMIT|MEM_RESERVE.
    pub fn allocate_memory(addr: usize, bytes: usize, exec: bool) -> io::Result<usize> {
        unsafe {
            let prot = if exec {
                PAGE_EXECUTE_READWRITE
            } else {
                PAGE_READWRITE
            };
            let p = VirtualAlloc(
                addr as *mut c_void,
                bytes,
                MEM_COMMIT | MEM_RESERVE,
                prot,
            );
            if p.is_null() {
                return Err(last_error_io("VirtualAlloc(MEM_COMMIT|MEM_RESERVE)"));
            }
            Ok(p as usize)
        }
    }

    /// VirtualQuery wrapper returning region state for `addr`.
    pub fn virtual_query(addr: usize) -> Option<MemoryMapping> {
        unsafe {
            let mut mbi = MemoryBasicInformation::default();
            let got = VirtualQuery(
                addr as *const c_void,
                &mut mbi,
                std::mem::size_of::<MemoryBasicInformation>(),
            );
            if got == 0 {
                None
            } else {
                Some(MemoryMapping {
                    base: mbi.base_address,
                    allocation_base: mbi.allocation_base,
                    allocation_protect: mbi.allocation_protect,
                    region_size: mbi.region_size,
                    state: mbi.state,
                    protect: mbi.protect,
                    type_: mbi.type_,
                })
            }
        }
    }

    /// os::find_mapping: first mapping at-or-before `addr`.
    pub fn find_mapping(mut addr: usize) -> Option<MemoryMapping> {
        // Walk up to allocation granularity looking for a committed/reserved
        // region whose allocation base is <= addr.
        for _ in 0..(Self::vm_allocation_granularity() * 2 / Self::vm_page_size()) {
            if let Some(mi) = Self::virtual_query(addr) {
                if mi.state != MEM_FREE && mi.allocation_base <= addr {
                    return Some(mi);
                }
                addr = mi.base.wrapping_sub(Self::vm_page_size());
            } else {
                break;
            }
        }
        None
    }

    /// os::pd_pretouch_memory: commit pages by touching them (real commit).
    pub fn pretouch_memory(first: usize, last: usize) -> usize {
        let page = Self::vm_page_size();
        let mut touched = 0usize;
        let mut addr = first & !(page - 1);
        unsafe {
            while addr < last {
                // Write a byte to force the OS to materialize the page.
                std::ptr::write_volatile(addr as *mut u8, 0);
                addr += page;
                touched += 1;
            }
        }
        touched
    }

    // -- memory-mapped files ----------------------------------------------------

    /// os::map_memory_to_file: map `file` (offset,size) at `addr` (0 = anywhere).
    /// Returns mapped base address.
    pub fn map_file_memory(
        file: &File,
        offset: u64,
        size: u64,
        addr: usize,
        writable: bool,
    ) -> io::Result<usize> {
        unsafe {
            let handle = file.as_raw_handle() as Handle;
            let prot = if writable {
                PAGE_READWRITE_MAP
            } else {
                PAGE_READONLY_MAP
            };
            let mapping = CreateFileMappingW(
                handle,
                std::ptr::null_mut(),
                prot,
                (size >> 32) as u32,
                size as u32,
                std::ptr::null(),
            );
            if mapping.is_null() || mapping == INVALID_HANDLE_VALUE {
                return Err(last_error_io("CreateFileMappingW"));
            }
            let access = if writable {
                FILE_MAP_WRITE
            } else {
                FILE_MAP_READ
            };
            let view = MapViewOfFileEx(
                mapping,
                access,
                (offset >> 32) as u32,
                offset as u32,
                size as usize,
                addr as *mut c_void,
            );
            let close_err = {
                // Mapping object handle can be closed once the view exists.
                let r = CloseHandle(mapping);
                if r == 0 {
                    Some(last_error())
                } else {
                    None
                }
            };
            if view.is_null() {
                return Err(match close_err {
                    Some(e) => io::Error::new(io::ErrorKind::Other, format!("MapViewOfFileEx (err {e})")),
                    None => last_error_io("MapViewOfFileEx"),
                });
            }
            Ok(view as usize)
        }
    }

    /// Unmap a view created by map_file_memory.
    pub fn unmap_file_memory(addr: usize, _size: usize) -> io::Result<()> {
        unsafe {
            if UnmapViewOfFile(addr as *const c_void) == 0 {
                return Err(last_error_io("UnmapViewOfFile"));
            }
        }
        Ok(())
    }

    /// os::pd_flush_memory: FlushViewOfFile + FlushInstructionCache equivalent.
    pub fn flush_icache(addr: usize, size: usize) -> io::Result<()> {
        unsafe {
            // FlushInstructionCache(GetCurrentProcess(), addr, size)
            unsafe extern "system" {
                fn FlushInstructionCache(
                    h_process: Handle,
                    lp_base_address: *const c_void,
                    dw_size: usize,
                ) -> i32;
            }
            if FlushInstructionCache(
                GetCurrentProcess(),
                addr as *const c_void,
                size,
            ) == 0
            {
                return Err(last_error_io("FlushInstructionCache"));
            }
        }
        Ok(())
    }

    // -- threads -----------------------------------------------------------------

    pub fn set_thread_priority(priority: i32) -> io::Result<()> {
        unsafe {
            if SetThreadPriority(GetCurrentThread(), priority) == 0 {
                return Err(last_error_io("SetThreadPriority"));
            }
        }
        Ok(())
    }

    pub fn get_thread_priority() -> i32 {
        unsafe { GetThreadPriority(GetCurrentThread()) }
    }

    /// os::set_native_priority / set_thread_priority mapping.
    pub fn set_native_priority(priority: i32) -> io::Result<()> {
        // Java priorities 1..10 map to Windows thread priorities.
        let mapped = match priority {
            0..=4 => THREAD_PRIORITY_BELOW_NORMAL,
            5 => THREAD_PRIORITY_NORMAL,
            6..=8 => THREAD_PRIORITY_ABOVE_NORMAL,
            _ => THREAD_PRIORITY_HIGHEST,
        };
        Self::set_thread_priority(mapped)
    }

    pub fn set_process_priority_class(class: u32) -> io::Result<()> {
        unsafe {
            if SetPriorityClass(GetCurrentProcess(), class) == 0 {
                return Err(last_error_io("SetPriorityClass"));
            }
        }
        Ok(())
    }

    pub fn get_process_priority_class() -> u32 {
        unsafe { GetPriorityClass(GetCurrentProcess()) }
    }

    /// os::set_thread_affinity_mask.
    pub fn set_thread_affinity_mask(mask: usize) -> io::Result<usize> {
        unsafe {
            let prev = SetThreadAffinityMask(GetCurrentThread(), mask);
            if prev == 0 {
                return Err(last_error_io("SetThreadAffinityMask"));
            }
            Ok(prev)
        }
    }

    pub fn thread_affinity_mask() -> io::Result<usize> {
        unsafe {
            let mut proc_mask: usize = 0;
            let mut sys_mask: usize = 0;
            if GetProcessAffinityMask(GetCurrentProcess(), &mut proc_mask, &mut sys_mask) == 0 {
                return Err(last_error_io("GetProcessAffinityMask"));
            }
            Ok(sys_mask)
        }
    }

    /// os::current_thread_cpu_time / yield / sleep.
    pub fn yield_cpu() {
        unsafe {
            SwitchToThread();
        }
    }

    pub fn sleep_millis(ms: u32) {
        unsafe {
            Sleep(ms);
        }
    }

    #[inline]
    pub fn current_process_id() -> u32 {
        unsafe { GetCurrentProcessId() }
    }

    #[inline]
    pub fn current_thread_id() -> u32 {
        unsafe { GetCurrentThreadId() }
    }

    // Note: GetThreadAffinityMask does not exist on x64 Windows; thread
    // affinity is managed via processor groups (GetThreadGroupAffinity).

    /// os::trim_memory: lower working set to let OS reclaim pages.
    pub fn trim_memory() -> io::Result<()> {
        unsafe {
            if SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) == 0 {
                return Err(last_error_io("SetProcessWorkingSetSize"));
            }
        }
        Ok(())
    }

    // -- OS version ---------------------------------------------------------------

    /// RtlGetVersion — real version, unaffected by manifest shims.
    pub fn os_version() -> Option<OsVersionInfoExW> {
        vm_version()
    }

    pub fn is_windows_11_or_greater() -> bool {
        vm_version()
            .map(|v| v.build_number >= 22000)
            .unwrap_or(false)
    }

    pub fn is_windows_server_2022_or_greater() -> bool {
        // Server 2022 = build 20348, Server 2025 = build 26100.
        vm_version()
            .map(|v| {
                v.build_number >= 20348
                    && (v.product_type == 2 || v.product_type == 3) // VER_NT_SERVER
            })
            .unwrap_or(false)
    }

    /// os::win32::is_64bit etc. — compile-time facts for x86_64 target.
    pub fn is_64bit() -> bool {
        cfg!(target_pointer_width = "64")
    }

    // -- error reporting ------------------------------------------------------------

    pub fn get_last_error() -> u32 {
        unsafe { GetLastError() }
    }

    pub fn set_last_error(code: u32) {
        unsafe {
            SetLastError(code);
        }
    }

    /// os::lasterror: FormatMessageW rendering of the last Win32 error.
    pub fn lasterror() -> String {
        let code = Self::get_last_error();
        format_error_message(code)
    }

    pub fn lasterror_for(code: u32) -> String {
        format_error_message(code)
    }

    /// os::get_host_name / environment — thin wrappers.
    pub fn getenv(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    pub fn get_host_name() -> Option<String> {
        // HOSTNAME env var is always set on Windows; fall back to COMPUTERNAME.
        Self::getenv("COMPUTERNAME").or_else(|| Self::getenv("HOSTNAME"))
    }

    /// os::setenv(name, value): process environment mutation.
    pub fn setenv(name: &str, value: &str) -> io::Result<()> {
        // SAFETY: Rust 2024 marks set_var unsafe because concurrent getenv in
        // other threads may observe torn state; HotSpot calls setenv during
        // single-threaded VM init only, same contract applies here.
        unsafe { std::env::set_var(name, value) };
        Ok(())
    }

    /// os::unsetenv.
    pub fn unsetenv(name: &str) -> io::Result<()> {
        unsafe { std::env::remove_var(name) };
        Ok(())
    }

    /// Cryptographically secure random bytes via advapi32!SystemFunction036
    /// (RtlGenRandom) — used to seed the os::random LCG and any
    /// java.util.Random / ThreadLocalRandom entropy needs.
    pub fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
        unsafe {
            let mut done = 0usize;
            while done < buf.len() {
                let chunk = (buf.len() - done).min(u32::MAX as usize) as u32;
                if SystemFunction036(buf.as_mut_ptr().add(done), chunk) == 0 {
                    return Err(last_error_io("SystemFunction036"));
                }
                done += chunk as usize;
            }
        }
        Ok(())
    }

    // -- os::random (os.cpp Park-Miller LCG over AtomicU32) ---------------------

    /// os::init_random(initval).
    pub fn init_random(initval: u32) {
        RAND_SEED.store(initval, Ordering::Relaxed);
    }

    /// os::next_random(seed): (16807 * seed) mod (2^31 - 1), Park-Miller
    /// "minimal standard" generator (Schrage decomposition, 32-bit safe).
    pub fn next_random(rand_seed: u32) -> u32 {
        const A: u32 = 16807;
        const M: u32 = 2147483647;
        let mut lo = A.wrapping_mul(rand_seed & 0xFFFF);
        let hi = A.wrapping_mul(rand_seed >> 16);
        lo = lo.wrapping_add((hi & 0x7FFF) << 16);
        if lo > M {
            lo &= M;
            lo = lo.wrapping_add(1);
        }
        lo = lo.wrapping_add(hi >> 15);
        if lo > M {
            lo &= M;
            lo = lo.wrapping_add(1);
        }
        lo
    }

    /// os::random(): thread-safe draw; cmpxchg loop over the shared seed.
    pub fn random() -> i32 {
        loop {
            let seed = RAND_SEED.load(Ordering::Relaxed);
            let rand = Self::next_random(seed);
            if RAND_SEED
                .compare_exchange_weak(seed, rand, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return rand as i32;
            }
        }
    }

    /// Native (non-WoW64) system info — GetNativeSystemInfo.
    pub fn native_system_info() -> SystemInfo {
        unsafe {
            let mut si = SystemInfo::default();
            GetNativeSystemInfo(&mut si);
            si
        }
    }

    /// os::win32::processor_group_count / max groups on the machine.
    pub fn maximum_processor_group_count() -> u32 {
        unsafe { GetMaximumProcessorGroupCount() as u32 }
    }

    /// Processor number (within its group) of the calling thread —
    /// os::processor_id equivalent on Windows.
    pub fn current_processor_number() -> u32 {
        unsafe { GetCurrentProcessorNumber() }
    }

    // -- crash handling -------------------------------------------------------------

    /// os::win32 crash support: install the top-level unhandled exception
    /// filter. `filter` receives ExceptionPointers and returns
    /// EXCEPTION_CONTINUE_SEARCH / EXCEPTION_CONTINUE_EXECUTION.
    pub fn install_crash_filter(filter: TopLevelExceptionFilter) {
        unsafe {
            SetUnhandledExceptionFilter(filter);
        }
    }

    /// Write a minidump for the current process (dbghelp!MiniDumpWriteDump —
    /// the same mechanism HotSpot's os::win32::dump_memory uses for crash
    /// logs). Returns bytes written path on success.
    #[cfg(windows)]
    pub fn write_minidump(path: &std::path::Path) -> io::Result<u64> {
        use std::os::windows::ffi::OsStrExt;
        #[repr(C)]
        struct MinidumpExceptionInformation {
            thread_id: u32,
            exception_pointers: usize,
            client_pointers: i32,
        }
        const MINIDUMP_TYPE_FULL: u32 = 0x0000_0000;
        #[link(name = "dbghelp")]
        unsafe extern "system" {
            fn CreateFileW(
                name: *const u16,
                access: u32,
                share: u32,
                security: *mut c_void,
                creation: u32,
                flags: u32,
                template: Handle,
            ) -> Handle;
            fn MiniDumpWriteDump(
                process: Handle,
                process_id: u32,
                file: Handle,
                dump_type: u32,
                exception_param: *mut MinidumpExceptionInformation,
                user_stream_param: *mut c_void,
                callback_param: *mut c_void,
            ) -> i32;
        }
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const CREATE_ALWAYS: u32 = 2;
        const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let file = CreateFileW(
                wide.as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            if file == INVALID_HANDLE_VALUE || file.is_null() {
                return Err(last_error_io("CreateFileW(minidump)"));
            }
            let ok = MiniDumpWriteDump(
                GetCurrentProcess(),
                Self::current_process_id(),
                file,
                MINIDUMP_TYPE_FULL,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            CloseHandle(file);
            if ok == 0 {
                return Err(last_error_io("MiniDumpWriteDump"));
            }
        }
        Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
    }

    /// Default unhandled-exception filter body (os::win32::topLevelExceptionFilter
    /// analogue): dump then continue search so WER/JVM handlers still run.
    pub unsafe extern "system" fn default_exception_filter(
        info: *const ExceptionPointers,
    ) -> i32 {
        if !info.is_null() {
            unsafe {
                let ep = &*info;
                if ep.exception_record != 0 {
                    let rec = &*(ep.exception_record as *const ExceptionRecord);
                    // Log the exception code; write a minidump next to the log.
                    eprintln!(
                        "fatal: unhandled exception 0x{:08x} at 0x{:x}",
                        rec.code, rec.address
                    );
                    let _ = Self::write_minidump(std::path::Path::new("hs_err_pid.dmp"));
                }
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    /// os::get_last_error_string for display.
    pub fn describe_last_error() -> String {
        let code = Self::get_last_error();
        format!("GetLastError={} ({})", code, format_error_message(code))
    }
}

// ---------------------------------------------------------------------------
// Free functions / helpers
// ---------------------------------------------------------------------------

fn vm_version() -> Option<OsVersionInfoExW> {
    unsafe {
        let mut info = OsVersionInfoExW {
            size: std::mem::size_of::<OsVersionInfoExW>() as u32,
            ..OsVersionInfoExW::default()
        };
        if RtlGetVersion(&mut info) == 0 {
            Some(info)
        } else {
            None
        }
    }
}

pub fn vm_version_info() -> Option<OsVersionInfoExW> {
    vm_version()
}

fn last_error() -> u32 {
    unsafe { GetLastError() }
}

fn last_error_io(context: &str) -> io::Error {
    let code = last_error();
    io::Error::new(
        io::ErrorKind::Other,
        format!("{}: Win32 error {} ({})", context, code, format_error_message(code)),
    )
}

fn format_error_message(code: u32) -> String {
    const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x0000_1000;
    const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x0000_0200;
    unsafe {
        let mut buf: Vec<u16> = vec![0u16; 512];
        let flags = FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;
        let n = FormatMessageW(
            flags,
            std::ptr::null(),
            code,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null_mut(),
        );
        if n == 0 {
            return format!("<error {code}>");
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len]).trim().to_string()
    }
}

/// Public raw version query (os::win32::get_nt_version equivalent).
pub fn nt_version() -> Option<(u32, u32, u32)> {
    vm_version().map(|v| (v.major_version, v.minor_version, v.build_number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_and_geometry() {
        OsWindows::init().unwrap();
        assert!(OsWindows::vm_page_size() >= 4096);
        assert!(OsWindows::vm_allocation_granularity() >= OsWindows::vm_page_size());
        assert!(OsWindows::processor_count() >= 1);
        assert!(OsWindows::physical_memory() > 0);
    }

    #[test]
    fn timers_advance() {
        OsWindows::init().unwrap();
        let t0 = OsWindows::java_time_nanos();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let t1 = OsWindows::java_time_nanos();
        assert!(t1 > t0);
        let (s, n) = OsWindows::java_time_system_utc();
        assert!(s > 1_700_000_000);
        assert!((0..1_000_000_000).contains(&n));
    }

    #[test]
    fn reserve_commit_release() {
        OsWindows::init().unwrap();
        let page = OsWindows::vm_page_size();
        let addr = OsWindows::reserve_memory(0, page * 4).unwrap();
        assert_ne!(addr, 0);
        // Reserved but not committed.
        let mi = OsWindows::virtual_query(addr).unwrap();
        assert_eq!(mi.state, MEM_RESERVE_STATE);
        OsWindows::commit_memory(addr, page, false).unwrap();
        let mi = OsWindows::virtual_query(addr).unwrap();
        assert_eq!(mi.state, MEM_COMMIT_STATE);
        unsafe {
            *(addr as *mut u8) = 0xAB;
            assert_eq!(*((addr) as *const u8), 0xAB);
        }
        OsWindows::uncommit_memory(addr, page).unwrap();
        OsWindows::release_memory(addr, page * 4).unwrap();
        let mi = OsWindows::virtual_query(addr);
        assert!(mi.is_none() || mi.unwrap().state == MEM_FREE);
    }

    #[test]
    fn protect_and_guard() {
        OsWindows::init().unwrap();
        let page = OsWindows::vm_page_size();
        let addr = OsWindows::allocate_memory(0, page, false).unwrap();
        OsWindows::protect_memory(addr, page, Prot::Read, true).unwrap();
        let mi = OsWindows::virtual_query(addr).unwrap();
        assert_eq!(Prot::from_page_protect(mi.protect), Prot::Read);
        OsWindows::protect_memory(addr, page, Prot::ReadWrite, true).unwrap();
        OsWindows::guard_memory(addr, page).unwrap();
        OsWindows::unguard_memory(addr, page).unwrap();
        OsWindows::release_memory(addr, page).unwrap();
    }

    #[test]
    fn version_and_errors() {
        OsWindows::init().unwrap();
        assert!(nt_version().is_some());
        assert!(!OsWindows::lasterror().is_empty() || OsWindows::get_last_error() == 0);
    }
}
