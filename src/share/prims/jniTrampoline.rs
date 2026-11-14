/*
 * Windows x64 ABI trampolines for JNI.
 *
 * HotSpot calls native methods through generated call wrappers. This module
 * does the same: it emits machine code into RWX memory (VirtualAlloc via
 * kernel32) that marshals arguments from a context block into the Win64
 * register/stack calling convention and invokes the target function.
 *
 * Two stub families exist:
 *  - call trampolines for `Ret fn(JNIEnv*, jclass/jobject, ...)` resolved from
 *    DLL exports or RegisterNatives (signature driven CallPlan);
 *  - varargs capture stubs for JNI functions declared `...` in jni.h
 *    (CallXxxMethod, NewObject, ...). They spill rcx/rdx/r8/r9 and xmm0-3 plus
 *    a pointer to the stack arguments into VarArgsCtx and tail-call a Rust
 *    handler which reconstructs the argument list from the method signature.
 */

use std::ffi::c_void;
use std::io::{self, Error};
use std::sync::OnceLock;

// Register codes (low 3 bits + extension handled by encoder)
const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RSP: u8 = 4;
const R8: u8 = 8;
const R9: u8 = 9;
const R10: u8 = 10;
const R11: u8 = 11;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgKind {
    /// Integer/pointer-sized argument (GP register or stack slot).
    Gp,
    /// IEEE single precision argument (XMM register, 4 bytes).
    Float,
    /// IEEE double precision argument (XMM register, 8 bytes).
    Double,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RetKind {
    /// void or integer/pointer result returned in RAX.
    Gp,
    /// float result returned in XMM0.
    Float,
    /// double result returned in XMM0.
    Double,
}

/// Describes how to call one native function through the Win64 ABI.
#[derive(Clone, Debug)]
pub struct CallPlan {
    /// Address of `Ret fn(JNIEnv*, jobject/jclass, jvalue...)`.
    pub func: *const c_void,
    /// Parameter kinds for every ABI slot: slot 0 is JNIEnv*, slot 1 is the
    /// class/receiver jobject, remaining slots are the Java arguments.
    pub params: Vec<ArgKind>,
    pub ret: RetKind,
    /// Number of 8-byte stack argument slots (params beyond the 4 registers).
    pub stack_args: usize,
}

impl CallPlan {
    pub fn param_count(&self) -> usize {
        self.params.len()
    }
}

/// Context block consumed by generated call trampolines.
/// Layout must match the offsets emitted by `emit_call_trampoline`.
#[repr(C)]
pub struct CallContext {
    pub func: *const c_void,
    pub env: *mut c_void,
    pub receiver: usize,
    pub args: [u64; 512],
}

/// Context captured by varargs stubs for `CallXxxMethod`-style JNI entries.
#[repr(C)]
pub struct VarArgsCtx {
    /// rcx, rdx, r8, r9 as received by the stub.
    pub gp: [u64; 4],
    /// xmm0..xmm3 raw 128-bit contents.
    pub xmm: [[u64; 2]; 4],
    /// Pointer to the first stack argument (entry rsp + 0x28).
    pub stack_args: *const u8,
    /// Encodes (call kind << 8) | ret class; see `VarCallKind`.
    pub tag: u64,
}

/// Raw context pointer type used by varargs handlers.
pub type VarArgsHandler = unsafe extern "system" fn(*const VarArgsCtx) -> u64;

// ---------------------------------------------------------------------------
// Executable memory
// ---------------------------------------------------------------------------

type VirtualAllocFn = unsafe extern "system" fn(*mut c_void, usize, u32, u32) -> *mut c_void;
type VirtualProtectFn = unsafe extern "system" fn(*mut c_void, usize, u32, *mut u32) -> i32;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE_READ: u32 = 0x20;

struct Kernel32 {
    _lib: libloading::Library,
    alloc: VirtualAllocFn,
    protect: VirtualProtectFn,
}

fn kernel32() -> io::Result<&'static Kernel32> {
    static KERNEL32: OnceLock<io::Result<Kernel32>> = OnceLock::new();
    match KERNEL32.get_or_init(|| {
        let lib = unsafe { libloading::Library::new("kernel32.dll") }.map_err(Error::other)?;
        let alloc = unsafe {
            *lib.get::<VirtualAllocFn>(b"VirtualAlloc")
                .map_err(Error::other)?
        };
        let protect = unsafe {
            *lib.get::<VirtualProtectFn>(b"VirtualProtect")
                .map_err(Error::other)?
        };
        Ok(Kernel32 {
            _lib: lib,
            alloc,
            protect,
        })
    }) {
        Ok(kernel) => Ok(kernel),
        Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
    }
}

/// Allocates RWX-capable memory: pages stay RW until `protect_executable`
/// flips them to RX after the code bytes have been written.
fn alloc_executable(size: usize) -> io::Result<*mut u8> {
    let kernel = kernel32()?;
    let raw = unsafe {
        (kernel.alloc)(
            std::ptr::null_mut(),
            size.max(1),
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if raw.is_null() {
        return Err(Error::other("VirtualAlloc failed for JNI trampoline"));
    }
    Ok(raw.cast())
}

fn protect_executable(ptr: *mut u8, size: usize) -> io::Result<()> {
    let kernel = kernel32()?;
    let mut old = 0u32;
    if unsafe { (kernel.protect)(ptr.cast(), size.max(1), PAGE_EXECUTE_READ, &mut old) } == 0 {
        return Err(Error::other("VirtualProtect failed for JNI trampoline"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// x86-64 encoder (subset needed for the stubs)
// ---------------------------------------------------------------------------

struct X64 {
    code: Vec<u8>,
}

impl X64 {
    fn new() -> Self {
        Self {
            code: Vec::with_capacity(256),
        }
    }

    fn byte(&mut self, b: u8) {
        self.code.push(b);
    }

    fn bytes(&mut self, data: &[u8]) {
        self.code.extend_from_slice(data);
    }

    fn u32(&mut self, v: u32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn rex(&mut self, w: bool, r: u8, x: u8, b: u8) {
        let mut v = 0x40;
        if w {
            v |= 0x08;
        }
        if r & 8 != 0 {
            v |= 0x04;
        }
        if x & 8 != 0 {
            v |= 0x02;
        }
        if b & 8 != 0 {
            v |= 0x01;
        }
        self.byte(v);
    }

    /// modrm + optional SIB + displacement for [base + disp].
    /// `reg_field` is the reg field of modrm (destination for loads,
    /// source for stores). base is a full register code; rsp/r12 (rm=4)
    /// and rbp/r13 (rm=5) need special encodings.
    fn mem(&mut self, reg_field: u8, base: u8, disp: i32) {
        let rm = base & 7;
        let sib = rm == 4; // rsp/r12 require a SIB byte
        if disp == 0 && rm != 5 {
            // mod = 00, register direct, no displacement
            self.byte(((reg_field & 7) << 3) | rm);
            if sib {
                self.byte(0x24);
            }
            return;
        }
        if (-128..=127).contains(&disp) {
            self.byte(0x40 | ((reg_field & 7) << 3) | rm);
            if sib {
                self.byte(0x24);
            }
            self.byte(disp as u8);
        } else {
            self.byte(0x80 | ((reg_field & 7) << 3) | rm);
            if sib {
                self.byte(0x24);
            }
            self.u32(disp as u32);
        }
    }

    /// `mov r64, [base + disp]` — loads a 64-bit value into register `dst`.
    fn load_r64(&mut self, dst: u8, base: u8, disp: i32) {
        self.rex(true, dst, 0, base);
        self.byte(0x8B);
        self.mem(dst, base, disp);
    }

    /// `mov [base + disp], r64` — stores register `src`.
    fn store_r64(&mut self, base: u8, disp: i32, src: u8) {
        self.rex(true, src, 0, base);
        self.byte(0x89);
        self.mem(src, base, disp);
    }

    /// `mov r64, r64` register move.
    fn mov_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, src, 0, dst);
        self.byte(0x89);
        self.byte(0xC0 | ((src & 7) << 3) | (dst & 7));
    }

    /// `movss xmm, [base + disp]` (4-byte float load).
    fn load_f32(&mut self, xmm: u8, base: u8, disp: i32) {
        self.byte(0xF3);
        self.rex(false, xmm, 0, base);
        self.bytes(&[0x0F, 0x10]);
        self.mem(xmm, base, disp);
    }

    /// `movsd xmm, [base + disp]` (8-byte double load).
    fn load_f64(&mut self, xmm: u8, base: u8, disp: i32) {
        self.byte(0xF2);
        self.rex(false, xmm, 0, base);
        self.bytes(&[0x0F, 0x10]);
        self.mem(xmm, base, disp);
    }

    /// `movdqu [rsp + disp], xmm` — 16-byte store.
    fn store_xmm_rsp(&mut self, disp: i32, xmm: u8) {
        self.byte(0xF3);
        self.byte(0x0F);
        self.byte(0x7F);
        self.mem(xmm, RSP, disp);
    }

    /// `movdqu xmm, [rsp + disp]` — 16-byte load.
    #[allow(dead_code)]
    fn load_xmm_rsp(&mut self, xmm: u8, disp: i32) {
        self.byte(0xF3);
        self.byte(0x0F);
        self.byte(0x6F);
        self.mem(xmm, RSP, disp);
    }

    fn sub_rsp(&mut self, imm: u32) {
        self.byte(0x48);
        self.byte(0x81);
        self.byte(0xEC);
        self.u32(imm);
    }

    fn add_rsp(&mut self, imm: u32) {
        self.byte(0x48);
        self.byte(0x81);
        self.byte(0xC4);
        self.u32(imm);
    }

    fn call_reg(&mut self, reg: u8) {
        self.rex(false, 0, 0, reg);
        self.byte(0xFF);
        self.byte(0xD0 | (reg & 7));
    }

    fn ret(&mut self) {
        self.byte(0xC3);
    }

    #[allow(dead_code)]
    fn xor_eax(&mut self) {
        self.bytes(&[0x31, 0xC0]);
    }

    /// `mov rax, imm64`.
    fn mov_rax_imm(&mut self, v: u64) {
        self.bytes(&[0x48, 0xB8]);
        self.u64(v);
    }

    /// `mov rcx, imm64`-style generic: mov r64, imm64 for rax..rdi only
    /// (extended regs need REX.B prefix handled separately).
    fn mov_r64_imm(&mut self, reg: u8, v: u64) {
        if reg & 8 != 0 {
            self.rex(true, 0, 0, reg);
            self.byte(0xB8 | (reg & 7));
        } else {
            self.byte(0x48);
            self.byte(0xB8 | (reg & 7));
        }
        self.u64(v);
    }

    /// `lea r64, [rsp + disp]`.
    fn lea_rsp(&mut self, dst: u8, disp: i32) {
        self.rex(true, dst, 0, RSP);
        self.byte(0x8D);
        self.mem(dst, RSP, disp);
    }

    /// `movd eax, xmm0` — low 32 bits of xmm0 into eax.
    fn movd_eax_xmm0(&mut self) {
        self.bytes(&[0x66, 0x0F, 0x7E, 0xC0]);
    }

    /// `movq rax, xmm0` — low 64 bits of xmm0 into rax.
    fn movq_rax_xmm0(&mut self) {
        self.bytes(&[0x66, 0x48, 0x0F, 0x7E, 0xC0]);
    }

    /// `movd xmm0, eax`.
    fn movd_xmm0_eax(&mut self) {
        self.bytes(&[0x66, 0x0F, 0x6E, 0xC0]);
    }

    /// `movq xmm0, rax`.
    fn movq_xmm0_rax(&mut self) {
        self.bytes(&[0x66, 0x48, 0x0F, 0x6E, 0xC0]);
    }
}

fn finish(code: X64) -> io::Result<*const c_void> {
    let ptr = alloc_executable(code.code.len())?;
    unsafe {
        std::ptr::copy_nonoverlapping(code.code.as_ptr(), ptr, code.code.len());
    }
    protect_executable(ptr, code.code.len())?;
    Ok(ptr.cast())
}

// ---------------------------------------------------------------------------
// Native method call trampoline
// ---------------------------------------------------------------------------

/// ABI slot -> destination register for the first four parameters.
const ARG_REGS: [u8; 4] = [RCX, RDX, R8, R9];

/// Builds a stub with signature `u64 stub(*const CallContext)` that invokes
/// `plan.func` using the Win64 calling convention.
pub fn build_call_trampoline(plan: &CallPlan) -> io::Result<*const c_void> {
    if !cfg!(windows) || !cfg!(target_arch = "x86_64") {
        return Err(Error::other("JNI trampolines require Windows x64"));
    }
    let mut asm = X64::new();

    // rcx = ctx
    asm.mov_rr(R10, RCX);
    // r11 = func
    asm.load_r64(R11, R10, 0);

    let stack = plan.stack_args;
    // frame ≡ 8 (mod 16), >= 0x20 + 8*stack
    let frame = (((0x28u32 + 8 * stack as u32 + 15) & !15) - 8) as u32;
    if frame > 0 {
        asm.sub_rsp(frame);
    }

    for (i, &kind) in plan.params.iter().enumerate() {
        // Source offset inside CallContext: env@8, receiver@16, args@24+8*j
        let src = match i {
            0 => 8i32,
            1 => 16,
            _ => 24 + 8 * (i as i32 - 2),
        };
        if i < 4 {
            match kind {
                ArgKind::Gp => asm.load_r64(ARG_REGS[i], R10, src),
                ArgKind::Float => asm.load_f32(i as u8, R10, src),
                ArgKind::Double => asm.load_f64(i as u8, R10, src),
            }
        } else {
            let slot = i - 4;
            let disp = 0x20 + 8 * slot as i32;
            asm.load_r64(RAX, R10, src);
            asm.store_r64(RSP, disp as i32, RAX);
        }
    }

    asm.call_reg(R11);

    match plan.ret {
        RetKind::Gp => {}
        RetKind::Float => asm.movd_eax_xmm0(),
        RetKind::Double => asm.movq_rax_xmm0(),
    }
    if plan.ret == RetKind::Gp && plan.params.is_empty() {
        // never happens; placeholder
    }
    if frame > 0 {
        asm.add_rsp(frame);
    }
    asm.ret();
    finish(asm)
}

// ---------------------------------------------------------------------------
// Varargs capture stubs (CallXxxMethod..., NewObject)
// ---------------------------------------------------------------------------

/// Call categories encoded in `VarArgsCtx::tag`.
pub const VAR_CALL_INSTANCE: u64 = 0;
pub const VAR_CALL_NONVIRTUAL: u64 = 1;
pub const VAR_CALL_STATIC: u64 = 2;
pub const VAR_CALL_NEWOBJECT: u64 = 3;

/// Return classes encoded in low bits of `VarArgsCtx::tag`.
pub const VAR_RET_GP: u64 = 0; // void, boolean, byte, char, short, int, long, object
pub const VAR_RET_F32: u64 = 1;
pub const VAR_RET_F64: u64 = 2;

pub fn varargs_tag(kind: u64, ret: u64) -> u64 {
    (kind << 8) | ret
}

/// Size of the register-save area written by the stub (see VarArgsCtx).
const VARARGS_FRAME: u32 = 0x88;

/// Builds a stub `u64 stub(...)` matching any `CallXxxMethod`-style variadic
/// JNI signature. It captures the argument registers and stack pointer, then
/// tail-calls `handler(ctx)`. `ret_kind` selects the xmm0 conversion applied
/// to the handler's u64 result.
pub fn build_varargs_stub(
    handler: VarArgsHandler,
    tag: u64,
    ret: RetKind,
) -> io::Result<*const c_void> {
    if !cfg!(windows) || !cfg!(target_arch = "x86_64") {
        return Err(Error::other("JNI trampolines require Windows x64"));
    }
    let mut asm = X64::new();

    // Frame layout on the new rsp (entry rsp % 16 == 8, frame % 16 == 8):
    //   +0    gp[0..4]      rcx rdx r8 r9
    //   +32   xmm save      4 * 16 bytes
    //   +96   stack_args    pointer to first stack argument
    //   +104  tag
    asm.sub_rsp(VARARGS_FRAME);

    // Save gp regs: mov rax, rXX ; mov [rsp+off], rax
    let gp = [RCX, RDX, R8, R9];
    for (i, &reg) in gp.iter().enumerate() {
        asm.mov_rr(RAX, reg);
        asm.store_r64(RSP, 8 * i as i32, RAX);
    }
    for i in 0..4u8 {
        asm.store_xmm_rsp(32 + 16 * i as i32, i);
    }
    // stack args live at entry_rsp + 0x28 == rsp + VARARGS_FRAME + 0x28
    asm.lea_rsp(RAX, (VARARGS_FRAME + 0x28) as i32);
    asm.store_r64(RSP, 96, RAX);
    asm.mov_r64_imm(RAX, tag);
    asm.store_r64(RSP, 104, RAX);

    asm.mov_rr(RCX, RSP);
    asm.mov_rax_imm(handler as usize as u64);
    asm.call_reg(RAX);

    match ret {
        RetKind::Gp => {}
        RetKind::Float => asm.movd_xmm0_eax(),
        RetKind::Double => asm.movq_xmm0_rax(),
    }
    asm.add_rsp(VARARGS_FRAME);
    asm.ret();
    finish(asm)
}
