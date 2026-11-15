/*
 * Port of OpenJDK 25 HotSpot os_cpu/windows_x86 CPU feature detection.
 *
 * JIT backends (c1/asm, opto/asm) need to know which ISA extensions the
 * processor supports before selecting instruction encodings. Detection is
 * done once via CPUID leaves 1 / 7.0 / 0x80000001 and XGETBV(XCR0) so that
 * AVX/AVX-512 are only reported when the OS has enabled ZMM/YMM state.
 *
 * Also provides os_cpu-adjacent helpers: processor group info, the
 * safepoint-poll memory fence primitive, and is_stub_code's mapping-based
 * approximation (stubs live in MEM_PRIVATE executable regions, never in
 * MEM_IMAGE module mappings).
 */

use super::{OsWindows, MEM_COMMIT_STATE, MEM_IMAGE, MEM_PRIVATE};
use std::sync::OnceLock;

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{__cpuid, __cpuid_count};

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn xgetbv0() -> u64 {
    let eax: u32;
    let edx: u32;
    // SAFETY: caller guarantees XSAVE+OSXSAVE are present (checked in detect).
    unsafe {
        core::arch::asm!(
            "xgetbv",
            in("ecx") 0u32,
            out("eax") eax,
            out("edx") edx,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((edx as u64) << 32) | eax as u64
}

/// Snapshot of CPU capabilities relevant to HotSpot code generation.
/// Mirrors the bits read by os::win32 / hsdis / macroAssembler dispatch.
#[derive(Debug, Clone, Copy)]
pub struct CpuFeatures {
    // leaf 1 edx
    pub fpu: bool,
    pub tsc: bool,
    pub cmov: bool,
    pub cx8: bool,
    pub mmx: bool,
    pub fxsr: bool,
    pub sse: bool,
    pub sse2: bool,
    pub ht: bool,
    // leaf 1 ecx
    pub sse3: bool,
    pub pclmulqdq: bool,
    pub ssse3: bool,
    pub fma: bool,
    pub sse41: bool,
    pub sse42: bool,
    pub movbe: bool,
    pub popcnt: bool,
    pub aes: bool,
    pub xsave: bool,
    pub osxsave: bool,
    pub avx: bool,
    pub f16c: bool,
    pub rdrand: bool,
    pub hypervisor: bool,
    // leaf 7.0 ebx
    pub bmi1: bool,
    pub avx2: bool,
    pub bmi2: bool,
    pub erms: bool,
    pub rdseed: bool,
    pub adx: bool,
    pub sha: bool,
    pub avx512f: bool,
    pub avx512dq: bool,
    pub avx512cd: bool,
    pub avx512bw: bool,
    pub avx512vl: bool,
    // leaf 7.0 ecx
    pub avx512vbmi: bool,
    pub avx512vbmi2: bool,
    pub gfni: bool,
    pub vaes: bool,
    pub vpclmulqdq: bool,
    pub avx512vnni: bool,
    pub avx512bitalg: bool,
    pub avx512vpopcntdq: bool,
    pub avx512bf16: bool,
    // leaf 7.0 edx
    pub avx512vp2intersect: bool,
    // leaf 0x80000001 ecx/edx
    pub lzcnt: bool,
    pub abm: bool,
    // OS-enabled XCR0 state
    pub xcr0_sse: bool,
    pub xcr0_ymm: bool,
    pub xcr0_opmask: bool,
    pub xcr0_zmm_hi256: bool,
    pub xcr0_hi16_zmm: bool,
    // aggregate helpers
    pub avx512_any: bool,
}

#[cfg(target_arch = "x86_64")]
impl CpuFeatures {
    fn detect() -> Self {
        let mut f = CpuFeatures {
            fpu: false,
            tsc: false,
            cmov: false,
            cx8: false,
            mmx: false,
            fxsr: false,
            sse: false,
            sse2: false,
            ht: false,
            sse3: false,
            pclmulqdq: false,
            ssse3: false,
            fma: false,
            sse41: false,
            sse42: false,
            movbe: false,
            popcnt: false,
            aes: false,
            xsave: false,
            osxsave: false,
            avx: false,
            f16c: false,
            rdrand: false,
            hypervisor: false,
            bmi1: false,
            avx2: false,
            bmi2: false,
            erms: false,
            rdseed: false,
            adx: false,
            sha: false,
            avx512f: false,
            avx512dq: false,
            avx512cd: false,
            avx512bw: false,
            avx512vl: false,
            avx512vbmi: false,
            avx512vbmi2: false,
            gfni: false,
            vaes: false,
            vpclmulqdq: false,
            avx512vnni: false,
            avx512bitalg: false,
            avx512vpopcntdq: false,
            avx512bf16: false,
            avx512vp2intersect: false,
            lzcnt: false,
            abm: false,
            xcr0_sse: false,
            xcr0_ymm: false,
            xcr0_opmask: false,
            xcr0_zmm_hi256: false,
            xcr0_hi16_zmm: false,
            avx512_any: false,
        };

        // leaf 0: max supported basic leaf
        let max_leaf = __cpuid(0).eax;

        // leaf 1: feature flags
        if max_leaf >= 1 {
            let leaf1 = __cpuid(1);
            let edx = leaf1.edx;
            let ecx = leaf1.ecx;
            f.fpu = edx & (1 << 0) != 0;
            f.tsc = edx & (1 << 4) != 0;
            f.cmov = edx & (1 << 15) != 0;
            f.cx8 = edx & (1 << 8) != 0;
            f.mmx = edx & (1 << 23) != 0;
            f.fxsr = edx & (1 << 24) != 0;
            f.sse = edx & (1 << 25) != 0;
            f.sse2 = edx & (1 << 26) != 0;
            f.ht = edx & (1 << 28) != 0;

            f.sse3 = ecx & (1 << 0) != 0;
            f.pclmulqdq = ecx & (1 << 1) != 0;
            f.ssse3 = ecx & (1 << 9) != 0;
            f.fma = ecx & (1 << 12) != 0;
            f.sse41 = ecx & (1 << 19) != 0;
            f.sse42 = ecx & (1 << 20) != 0;
            f.movbe = ecx & (1 << 22) != 0;
            f.popcnt = ecx & (1 << 23) != 0;
            f.aes = ecx & (1 << 25) != 0;
            f.xsave = ecx & (1 << 26) != 0;
            f.osxsave = ecx & (1 << 27) != 0;
            f.avx = ecx & (1 << 28) != 0;
            f.f16c = ecx & (1 << 29) != 0;
            f.rdrand = ecx & (1 << 30) != 0;
            f.hypervisor = ecx & (1 << 31) != 0;
        }

        // leaf 7.0: extended features (BMI/AVX2/AVX-512)
        if max_leaf >= 7 {
            let leaf7 = __cpuid_count(7, 0);
            let ebx = leaf7.ebx;
            let ecx = leaf7.ecx;
            let edx = leaf7.edx;
            f.pclmulqdq = f.pclmulqdq || ebx & (1 << 1) != 0;
            f.avx2 = ebx & (1 << 5) != 0;
            f.bmi1 = ebx & (1 << 3) != 0;
            f.fma = f.fma || ebx & (1 << 12) != 0;
            f.avx512f = ebx & (1 << 16) != 0;
            f.avx512dq = ebx & (1 << 17) != 0;
            f.avx512cd = ebx & (1 << 28) != 0;
            f.avx512bw = ebx & (1 << 30) != 0;
            f.avx512vl = ebx & (1 << 31) != 0;
            f.bmi2 = ebx & (1 << 8) != 0;
            f.erms = ebx & (1 << 9) != 0;
            f.avx512vbmi = ecx & (1 << 1) != 0;
            f.avx512vbmi2 = ecx & (1 << 6) != 0;
            f.gfni = ecx & (1 << 8) != 0;
            f.vaes = ecx & (1 << 9) != 0;
            f.vpclmulqdq = ecx & (1 << 10) != 0;
            f.avx512vnni = ecx & (1 << 11) != 0;
            f.avx512bitalg = ecx & (1 << 12) != 0;
            f.avx512bf16 = ecx & (1 << 5) != 0;
            f.avx512vpopcntdq = ecx & (1 << 14) != 0;
            f.avx512vp2intersect = edx & (1 << 8) != 0;
            f.adx = ebx & (1 << 19) != 0;
            f.sha = ebx & (1 << 29) != 0;
            f.rdseed = ebx & (1 << 18) != 0;
        }

        // leaf 0x80000001: extended AMD features (LZCNT/ABM)
        let max_ext = __cpuid(0x8000_0000).eax;
        if max_ext >= 0x8000_0001 {
            let ext1 = __cpuid(0x8000_0001);
            f.lzcnt = ext1.ecx & (1 << 5) != 0;
            f.abm = f.lzcnt;
        }

        // XCR0: which state components the OS has enabled.
        if f.xsave && f.osxsave {
            // SAFETY: XSAVE+OSXSAVE verified above.
            let xcr0 = unsafe { xgetbv0() };
            f.xcr0_sse = xcr0 & 0x3 != 0;
            f.xcr0_ymm = xcr0 & 0x6 == 0x6;
            f.xcr0_opmask = xcr0 & (1 << 5) != 0;
            f.xcr0_zmm_hi256 = xcr0 & (1 << 6) != 0;
            f.xcr0_hi16_zmm = xcr0 & (1 << 7) != 0;
            f.avx = f.avx && f.xcr0_sse && f.xcr0_ymm;
            f.avx2 = f.avx2 && f.avx && f.xcr0_sse && f.xcr0_ymm;
            f.avx512f = f.avx512f
                && f.xcr0_opmask
                && f.xcr0_zmm_hi256
                && f.xcr0_hi16_zmm;
            f.avx512_any = f.avx512f
                || f.avx512dq
                || f.avx512cd
                || f.avx512bw
                || f.avx512vl
                || f.avx512vbmi
                || f.avx512vbmi2
                || f.avx512vnni
                || f.avx512bitalg
                || f.avx512vpopcntdq
                || f.avx512bf16
                || f.avx512vp2intersect;
            // AVX-512 subsets require AVX-512F + OS ZMM state.
            if !f.avx512f {
                f.avx512dq = false;
                f.avx512cd = false;
                f.avx512bw = false;
                f.avx512vl = false;
                f.avx512vbmi = false;
                f.avx512vbmi2 = false;
                f.avx512vnni = false;
                f.avx512bitalg = false;
                f.avx512vpopcntdq = false;
                f.avx512bf16 = false;
                f.avx512vp2intersect = false;
                f.avx512_any = false;
            }
        } else {
            f.avx = false;
            f.avx2 = false;
            f.avx512f = false;
            f.avx512_any = false;
            f.fma = false;
            f.f16c = false;
        }

        f
    }
}

#[cfg(not(target_arch = "x86_64"))]
impl CpuFeatures {
    fn detect() -> Self {
        CpuFeatures {
            fpu: false,
            tsc: false,
            cmov: false,
            cx8: false,
            mmx: false,
            fxsr: false,
            sse: false,
            sse2: false,
            ht: false,
            sse3: false,
            pclmulqdq: false,
            ssse3: false,
            fma: false,
            sse41: false,
            sse42: false,
            movbe: false,
            popcnt: false,
            aes: false,
            xsave: false,
            osxsave: false,
            avx: false,
            f16c: false,
            rdrand: false,
            hypervisor: false,
            bmi1: false,
            avx2: false,
            bmi2: false,
            erms: false,
            rdseed: false,
            adx: false,
            sha: false,
            avx512f: false,
            avx512dq: false,
            avx512cd: false,
            avx512bw: false,
            avx512vl: false,
            avx512vbmi: false,
            avx512vbmi2: false,
            gfni: false,
            vaes: false,
            vpclmulqdq: false,
            avx512vnni: false,
            avx512bitalg: false,
            avx512vpopcntdq: false,
            avx512bf16: false,
            avx512vp2intersect: false,
            lzcnt: false,
            abm: false,
            xcr0_sse: false,
            xcr0_ymm: false,
            xcr0_opmask: false,
            xcr0_zmm_hi256: false,
            xcr0_hi16_zmm: false,
            avx512_any: false,
        }
    }
}

impl CpuFeatures {
    /// os_cpu-equivalent ISA level summary for JIT dispatch tables.
    pub fn isa_level(&self) -> &'static str {
        if self.avx512_any {
            "avx512"
        } else if self.avx2 {
            "avx2"
        } else if self.avx {
            "avx"
        } else if self.sse42 {
            "sse4.2"
        } else if self.sse41 {
            "sse4.1"
        } else if self.ssse3 {
            "ssse3"
        } else if self.sse3 {
            "sse3"
        } else if self.sse2 {
            "sse2"
        } else if self.sse {
            "sse"
        } else {
            "baseline"
        }
    }
}

static FEATURES: OnceLock<CpuFeatures> = OnceLock::new();

impl OsWindows {
    /// os_cpu::cpu_features(): cached CPUID/XCR0 snapshot.
    pub fn cpu_features() -> CpuFeatures {
        *FEATURES.get_or_init(CpuFeatures::detect)
    }

    /// Safepoint polling primitive (os::safepoint_poll analogue): acquire
    /// semantics read of the poll page followed by a full fence. HotSpot's
    /// javaThread::poll_for_safepoint does the same via _ThreadLocal_poll;
    /// callers pass the address of the per-thread poll word.
    #[inline]
    pub fn safepoint_poll(poll_addr: *const u32) {
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        if !poll_addr.is_null() {
            // Volatile read: cannot be elided, must observe the latest value.
            unsafe {
                std::ptr::read_volatile(poll_addr);
            }
        }
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    }

    /// os_cpu::is_stub_code approximation: stub code lives in MEM_PRIVATE
    /// executable memory (code cache / stub cache), while real module code
    /// sits in MEM_IMAGE mappings. Returns true when `pc` belongs to a
    /// committed, private, executable region.
    pub fn is_stub_code(pc: usize) -> bool {
        match Self::virtual_query(pc) {
            Some(mi) => {
                mi.state == MEM_COMMIT_STATE
                    && mi.type_ == MEM_PRIVATE
                    && mi.protect & 0xF0 != 0 // PAGE_EXECUTE* bits
                    && mi.type_ != MEM_IMAGE
            }
            None => false,
        }
    }

    /// Inverse check used by is_stub_code callers that need the mapping kind.
    pub fn is_module_code(pc: usize) -> bool {
        match Self::virtual_query(pc) {
            Some(mi) => mi.type_ == MEM_IMAGE,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpuid_detects_baseline() {
        let f = OsWindows::cpu_features();
        // Any x86_64 CPU must have at least SSE2.
        assert!(f.sse2, "x86_64 requires SSE2");
        assert!(f.fxsr);
        assert!(f.sse);
        assert!(f.tsc);
        // AVX must imply YMM XCR0 state.
        if f.avx {
            assert!(f.xcr0_ymm);
        }
        if f.avx512_any {
            assert!(f.avx512f);
            assert!(f.xcr0_zmm_hi256);
            assert!(f.xcr0_hi16_zmm);
        }
    }

    #[test]
    fn isa_level_is_ordered() {
        let f = OsWindows::cpu_features();
        let level = f.isa_level();
        let rank = ["baseline", "sse", "sse3", "ssse3", "sse4.1", "sse4.2", "avx", "avx2", "avx512"];
        let idx = rank.iter().position(|&r| r == level).unwrap_or(0);
        assert!(idx >= 1, "isa_level={level} must be at least sse on x86_64");
    }

    #[test]
    fn safepoint_poll_does_not_fault() {
        let mut poll: u32 = 0;
        OsWindows::safepoint_poll(&mut poll);
        OsWindows::safepoint_poll(std::ptr::null());
    }
}
