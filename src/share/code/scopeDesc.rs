/*
 * Port of OpenJDK 25 HotSpot ScopeDesc (share/code/scopeDesc.hpp / scopeDesc.cpp)
 *
 * ScopeDesc describes debug and deoptimization information for JIT-compiled frames:
 * method identity, bytecode indices (BCI), inlining call tree, locals, expressions,
 * and lock monitors.
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeDesc {
    method_holder: String,
    method_name: String,
    method_signature: String,
    bci: i32,
    is_inlined: bool,
    sender_decode_offset: i32,
    locals: Vec<String>,
    expressions: Vec<String>,
    monitors: Vec<String>,
    reexecute: bool,
    return_oop: bool,
    rethrow_exception: bool,
}

impl ScopeDesc {
    pub const NO_SENDER_DECODE_OFFSET: i32 = -1;

    pub fn new(
        method_holder: String,
        method_name: String,
        method_signature: String,
        bci: i32,
        is_inlined: bool,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            bci,
            is_inlined,
            sender_decode_offset: Self::NO_SENDER_DECODE_OFFSET,
            locals: Vec::new(),
            expressions: Vec::new(),
            monitors: Vec::new(),
            reexecute: false,
            return_oop: false,
            rethrow_exception: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_details(
        method_holder: String,
        method_name: String,
        method_signature: String,
        bci: i32,
        is_inlined: bool,
        sender_decode_offset: i32,
        locals: Vec<String>,
        expressions: Vec<String>,
        monitors: Vec<String>,
        reexecute: bool,
        return_oop: bool,
        rethrow_exception: bool,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            bci,
            is_inlined,
            sender_decode_offset,
            locals,
            expressions,
            monitors,
            reexecute,
            return_oop,
            rethrow_exception,
        }
    }

    #[inline]
    pub fn method_holder(&self) -> &str {
        &self.method_holder
    }

    #[inline]
    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    #[inline]
    pub fn method_signature(&self) -> &str {
        &self.method_signature
    }

    #[inline]
    pub fn bci(&self) -> i32 {
        self.bci
    }

    #[inline]
    pub fn is_inlined(&self) -> bool {
        self.is_inlined
    }

    #[inline]
    pub fn sender_decode_offset(&self) -> i32 {
        self.sender_decode_offset
    }

    #[inline]
    pub fn is_top(&self) -> bool {
        self.sender_decode_offset == Self::NO_SENDER_DECODE_OFFSET
    }

    #[inline]
    pub fn locals(&self) -> &[String] {
        &self.locals
    }

    #[inline]
    pub fn expressions(&self) -> &[String] {
        &self.expressions
    }

    #[inline]
    pub fn monitors(&self) -> &[String] {
        &self.monitors
    }

    #[inline]
    pub fn should_reexecute(&self) -> bool {
        self.reexecute
    }

    #[inline]
    pub fn return_oop(&self) -> bool {
        self.return_oop
    }

    #[inline]
    pub fn rethrow_exception(&self) -> bool {
        self.rethrow_exception
    }
}
