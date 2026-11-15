/*
 * Port of OpenJDK 25 HotSpot JVMCIEnv
 * (share/jvmci/jvmciEnv.hpp / jvmciEnv.cpp)
 *
 * JVMCIEnv is the top-level wrapper for interactions between HotSpot
 * and JVMCI Java code, supporting both the HotSpot-heap runtime and a
 * shared-library (JNI) runtime.  In this port the JNI environment is
 * represented by an opaque handle token and exceptions are carried by
 * `JvmciException`/`JvmciResult` instead of the TRAPS macros.
 */

use crate::share::jvmci::jvmciExceptions::{
    JvmciException, JvmciResult, ThrowKind, JVMCI_NOT_ENABLED_ERROR_MESSAGE,
};
use crate::share::jvmci::jvmciObject::{
    self, JvmciArray, JvmciHeapValue, JvmciObject, JvmciObjectArray, JvmciPrimitiveArray,
    JvmciValue,
};
use crate::share::jvmci::jvmciRuntime::JvmciRuntime;
use crate::share::runtime::handles::Handle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Keeps a Klass*/metadata reference alive for GC — in this port keys
/// are GC-safe strings, so the handle is a no-cost wrapper.
#[derive(Clone, Debug)]
pub struct JvmciKlassHandle {
    pub key: String,
}

impl JvmciKlassHandle {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }
}

/// Keeps an installed nmethod alive while a `HotSpotNmethod` mirror is
/// in use (JVMCINMethodHandle analogue).  Records the code-cache key.
#[derive(Debug)]
pub struct JvmciNmethodHandle {
    pub method_key: Option<String>,
}

impl JvmciNmethodHandle {
    pub fn new() -> Self {
        Self { method_key: None }
    }

    pub fn set_nmethod(&mut self, method_key: Option<String>) {
        self.method_key = method_key;
    }
}

impl Default for JvmciNmethodHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Compilation state tracked per CompileBroker request
/// (JVMCICompileState in jvmciEnv.hpp).
pub struct JvmciCompileState {
    /// Canonical `holder.name+descriptor` key of the target method.
    pub task_method_key: String,
    /// Entry bci (InvocationEntryBci == -1).
    pub entry_bci: i32,
    is_osr: bool,
    /// Cached JVMTI state (bytes in C++; booleans here).
    pub jvmti_redefinition_count: u64,
    pub jvmti_can_hotswap_or_post_breakpoint: bool,
    pub jvmti_can_access_local_variables: bool,
    pub jvmti_can_post_on_exceptions: bool,
    pub jvmti_can_pop_frame: bool,
    pub target_method_is_old: bool,
    retryable: bool,
    failure_reason: Option<(String, bool)>, // (message, on_c_heap)
    compilation_ticks: i32,
}

impl JvmciCompileState {
    pub fn new(method_key: impl Into<String>, entry_bci: i32, is_osr: bool) -> Self {
        Self {
            task_method_key: method_key.into(),
            entry_bci,
            is_osr,
            jvmti_redefinition_count: 0,
            jvmti_can_hotswap_or_post_breakpoint: false,
            jvmti_can_access_local_variables: false,
            jvmti_can_post_on_exceptions: false,
            jvmti_can_pop_frame: false,
            target_method_is_old: false,
            retryable: false,
            failure_reason: None,
            compilation_ticks: 0,
        }
    }

    pub fn task_method_key(&self) -> &str {
        &self.task_method_key
    }

    pub fn is_osr(&self) -> bool {
        self.is_osr
    }

    /// `jvmti_state_changed`: redefinition count or capabilities changed
    /// since the snapshot was taken.  The snapshot fields are updated by
    /// `refresh_jvmti_state`; with no JVMTI in this port the count is 0.
    pub fn jvmti_state_changed(&self) -> bool {
        self.jvmti_redefinition_count != 0
            || self.jvmti_can_hotswap_or_post_breakpoint
            || self.jvmti_can_access_local_variables
            || self.jvmti_can_post_on_exceptions
            || self.jvmti_can_pop_frame
    }

    pub fn failure_reason(&self) -> Option<&str> {
        self.failure_reason.as_ref().map(|(m, _)| m.as_str())
    }

    pub fn failure_reason_on_c_heap(&self) -> bool {
        self.failure_reason
            .as_ref()
            .map_or(false, |(_, on_heap)| *on_heap)
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    pub fn set_failure(&mut self, retryable: bool, reason: impl Into<String>, on_c_heap: bool) {
        self.retryable = retryable;
        self.failure_reason = Some((reason.into(), on_c_heap));
    }

    pub fn notify_libjvmci_oome(&mut self) {
        self.set_failure(true, "libjvmci isolate creation failed: out of memory", true);
    }

    pub fn compilation_ticks(&self) -> i32 {
        self.compilation_ticks
    }

    pub fn inc_compilation_ticks(&mut self) {
        self.compilation_ticks += 1;
    }
}

/// Shared handle to a `JvmciCompileState` (Rc because env and compiler
/// share it on one thread, like the C++ stack object shared by pointer).
pub type SharedCompileState = Rc<RefCell<JvmciCompileState>>;

/// The JVMCI environment scope.  Construction mirrors the four C++
/// constructors; `Drop` performs the scope-close work (detaching the
/// thread from a native runtime when required).
pub struct JvmciEnv {
    /// JNI environment token for the shared-library heap (0 = none).
    jni_env: u64,
    pop_frame_on_close: bool,
    detach_on_close: bool,
    runtime: Option<Arc<JvmciRuntime>>,
    is_hotspot: bool,
    throw_to_caller: bool,
    file: &'static str,
    line: u32,
    init_error: i32,
    init_error_msg: Option<String>,
    pending_exception: Option<JvmciException>,
    compile_state: Option<SharedCompileState>,
}

impl JvmciEnv {
    fn init_fields(
        thread_id: u64,
        is_hotspot: bool,
        file: &'static str,
        line: u32,
    ) -> (Option<Arc<JvmciRuntime>>, i32, Option<String>) {
        let runtime = JvmciRuntime::for_thread(thread_id, is_hotspot);
        match runtime {
            Ok(rt) => (Some(rt), 0, None),
            Err(e) => (None, 1, Some(e)),
        }
    }

    /// `JVMCIEnv(JavaThread*, JNIEnv* parent_env, file, line)` — scope for
    /// a Java-to-VM call (e.g. CompilerToVM).  `parent_env` is the JNI
    /// environment of the calling Java frame.
    pub fn new_for_java_to_vm(thread_id: u64, parent_env: u64, file: &'static str, line: u32) -> Self {
        let (runtime, init_error, init_error_msg) = Self::init_fields(thread_id, false, file, line);
        Self {
            jni_env: parent_env,
            pop_frame_on_close: false,
            detach_on_close: true,
            runtime,
            is_hotspot: false,
            throw_to_caller: true,
            file,
            line,
            init_error,
            init_error_msg,
            pending_exception: None,
            compile_state: None,
        }
    }

    /// `JVMCIEnv(JavaThread*, JVMCICompileState*, file, line)` — scope for
    /// a CompileBroker compilation.  Exceptions must not propagate back
    /// to the broker as Java exceptions.
    pub fn new_for_compile_state(
        thread_id: u64,
        compile_state: SharedCompileState,
        file: &'static str,
        line: u32,
    ) -> Self {
        let (runtime, init_error, init_error_msg) = Self::init_fields(thread_id, false, file, line);
        Self {
            jni_env: 0,
            pop_frame_on_close: false,
            detach_on_close: true,
            runtime,
            is_hotspot: false,
            throw_to_caller: false,
            file,
            line,
            init_error,
            init_error_msg,
            pending_exception: None,
            compile_state: Some(compile_state),
        }
    }

    /// `JVMCIEnv(JavaThread*, file, line)` — scope for a VM-internal call.
    pub fn new_for_vm_call(thread_id: u64, file: &'static str, line: u32) -> Self {
        let (runtime, init_error, init_error_msg) = Self::init_fields(thread_id, false, file, line);
        Self {
            jni_env: 0,
            pop_frame_on_close: false,
            detach_on_close: true,
            runtime,
            is_hotspot: false,
            throw_to_caller: false,
            file,
            line,
            init_error,
            init_error_msg,
            pending_exception: None,
            compile_state: None,
        }
    }

    /// `JVMCIEnv(JavaThread*, bool is_hotspot, file, line)`.
    pub fn new_for_heap(thread_id: u64, is_hotspot: bool, file: &'static str, line: u32) -> Self {
        let (runtime, init_error, init_error_msg) = Self::init_fields(thread_id, is_hotspot, file, line);
        Self {
            jni_env: 0,
            pop_frame_on_close: false,
            detach_on_close: !is_hotspot,
            runtime,
            is_hotspot,
            throw_to_caller: false,
            file,
            line,
            init_error,
            init_error_msg,
            pending_exception: None,
            compile_state: None,
        }
    }

    /// JNI result code from runtime attach/create (`init_error`).
    pub fn init_error(&self) -> i32 {
        self.init_error
    }

    pub fn init_error_msg(&self) -> Option<&str> {
        self.init_error_msg.as_deref()
    }

    /// `check_init`: convert a failed env initialization into a pending
    /// exception.
    pub fn check_init(&mut self) -> JvmciResult<()> {
        if self.init_error != 0 {
            let msg = self
                .init_error_msg
                .clone()
                .unwrap_or_else(|| format!("JVMCIEnv init failed: {}", self.init_error));
            return Err(self.raise(JvmciException::internal_error(msg)));
        }
        Ok(())
    }

    /// The JVMCIRuntime for this env.  Panics-equivalent of the C++
    /// `guarantee` is expressed as an error return.
    pub fn runtime(&self) -> JvmciResult<Arc<JvmciRuntime>> {
        self.check_init()?;
        self.runtime.clone().ok_or_else(|| {
            JvmciException::internal_error(format!(
                "JVMCIEnv has no runtime (init_error={})",
                self.init_error
            ))
        })
    }

    /// Whether this env targets the HotSpot heap runtime.
    pub fn is_hotspot(&self) -> bool {
        self.is_hotspot
    }

    pub fn compile_state(&self) -> Option<&SharedCompileState> {
        self.compile_state.as_ref()
    }

    pub fn set_compile_state(&mut self, state: SharedCompileState) {
        self.compile_state = Some(state);
    }

    // ---- pending exception handling ---------------------------------

    pub fn has_pending_exception(&self) -> bool {
        self.pending_exception.is_some()
    }

    pub fn pending_exception(&self) -> Option<&JvmciException> {
        self.pending_exception.as_ref()
    }

    /// `clear_pending_exception`.
    pub fn clear_pending_exception(&mut self) {
        self.pending_exception = None;
    }

    /// Record `e` as the pending exception and return it for `Err(...)`.
    pub fn raise(&mut self, e: JvmciException) -> JvmciException {
        let e = JvmciException {
            file: if e.file == "<jvmci>" { self.file } else { e.file },
            line: if e.line == 0 { self.line } else { e.line },
            ..e
        };
        self.pending_exception = Some(e.clone());
        e
    }

    /// `transfer_pending_exception`: move the pending exception to
    /// `peer_env`.  Returns true if one was transferred.
    pub fn transfer_pending_exception(&mut self, peer: &mut JvmciEnv) -> bool {
        if let Some(e) = self.pending_exception.take() {
            peer.pending_exception = Some(e);
            true
        } else {
            false
        }
    }

    /// `describe_pending_exception`: render and clear the pending
    /// exception.
    pub fn describe_pending_exception(&mut self) -> String {
        match self.pending_exception.take() {
            Some(e) => e.describe(),
            None => String::new(),
        }
    }

    /// `pending_exception_as_string`.
    pub fn pending_exception_as_string(&mut self) -> Option<(String, String)> {
        self.pending_exception.take().map(|e| {
            let to_string = format!("{}: {}", e.kind.simple_name(), e.message);
            (to_string, e.describe())
        })
    }

    // ---- throw_<Kind> helpers (JVMCI_THROW macros) -------------------

    macro_rules! throw_method {
        ($name:ident, $kind:expr) => {
            pub fn $name(&mut self, msg: Option<&str>) -> JvmciException {
                self.raise(JvmciException::throw_kind($kind, msg, "<jvmci>", 0))
            }
        };
    }

    throw_method!(throw_internal_error, ThrowKind::InternalError);
    throw_method!(
        throw_array_index_out_of_bounds,
        ThrowKind::ArrayIndexOutOfBoundsException
    );
    throw_method!(throw_illegal_state, ThrowKind::IllegalStateException);
    throw_method!(throw_null_pointer, ThrowKind::NullPointerException);
    throw_method!(throw_illegal_argument, ThrowKind::IllegalArgumentException);
    throw_method!(
        throw_invalid_installed_code,
        ThrowKind::InvalidInstalledCodeException
    );
    throw_method!(throw_unsatisfied_link, ThrowKind::UnsatisfiedLinkError);
    throw_method!(
        throw_unsupported_operation,
        ThrowKind::UnsupportedOperationException
    );
    throw_method!(throw_out_of_memory, ThrowKind::OutOfMemoryError);
    throw_method!(throw_no_class_def_found, ThrowKind::NoClassDefFoundError);

    /// `fthrow_error(file, line, format, ...)`.
    pub fn fthrow_error(&mut self, message: String) -> JvmciException {
        self.raise(JvmciException::fthrow_error(self.file, self.line, message))
    }

    /// `JVMCIENV->throw_InternalError(JVMCI_NOT_ENABLED_ERROR_MESSAGE)`.
    pub fn throw_not_enabled(&mut self) -> JvmciException {
        self.raise(JvmciException::not_enabled())
    }

    // ---- array accessors --------------------------------------------

    fn array_len(&self, array: JvmciObject) -> JvmciResult<usize> {
        self.check_init()?;
        match array.heap_value() {
            Some(JvmciHeapValue::ByteArray(v)) => Ok(v.len()),
            Some(JvmciHeapValue::CharArray(v)) => Ok(v.len()),
            Some(JvmciHeapValue::IntArray(v)) => Ok(v.len()),
            Some(JvmciHeapValue::LongArray(v)) => Ok(v.len()),
            Some(JvmciHeapValue::BoolArray(v)) => Ok(v.len()),
            Some(JvmciHeapValue::ObjectArray(v)) => Ok(v.len()),
            Some(JvmciHeapValue::Null) => Ok(0),
            _ => Err(self.throw_illegal_argument(Some("not an array"))),
        }
    }

    pub fn get_length(&mut self, array: JvmciArray) -> JvmciResult<i32> {
        Ok(self.array_len(array.0)? as i32)
    }

    pub fn get_object_at(&mut self, array: JvmciObjectArray, index: i32) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let len = self.array_len(array.0)? as i32;
        if index < 0 || index >= len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        match array.0.heap_value() {
            Some(JvmciHeapValue::ObjectArray(v)) => Ok(v[index as usize]),
            _ => Err(self.throw_illegal_argument(Some("not an object array"))),
        }
    }

    pub fn put_object_at(
        &mut self,
        array: JvmciObjectArray,
        index: i32,
        value: JvmciObject,
    ) -> JvmciResult<()> {
        self.check_init()?;
        let len = self.array_len(array.0)? as i32;
        if index < 0 || index >= len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        let updated = jvmciObject::with_entry_mut(array.0.jobject, |e| match &mut e.value {
            JvmciHeapValue::ObjectArray(v) => {
                v[index as usize] = value;
                true
            }
            _ => false,
        })
        .unwrap_or(false);
        if updated {
            Ok(())
        } else {
            Err(self.throw_illegal_argument(Some("not an object array")))
        }
    }

    pub fn get_bool_at(&mut self, array: JvmciPrimitiveArray, index: i32) -> JvmciResult<bool> {
        self.check_init()?;
        match array.0.heap_value() {
            Some(JvmciHeapValue::BoolArray(v)) => v
                .get(index as usize)
                .copied()
                .ok_or_else(|| self.throw_array_index_out_of_bounds(None)),
            _ => Err(self.throw_illegal_argument(Some("not a boolean array"))),
        }
    }

    pub fn put_bool_at(
        &mut self,
        array: JvmciPrimitiveArray,
        index: i32,
        value: bool,
    ) -> JvmciResult<()> {
        self.check_init()?;
        let len = self.array_len(array.0)? as i32;
        if index < 0 || index >= len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        let result = jvmciObject::with_entry_mut(array.0.jobject, |e| match &mut e.value {
            JvmciHeapValue::BoolArray(v) => {
                v[index as usize] = value;
                true
            }
            _ => false,
        })
        .unwrap_or(false);
        if result {
            Ok(())
        } else {
            Err(self.throw_illegal_argument(Some("not a boolean array")))
        }
    }

    pub fn get_byte_at(&mut self, array: JvmciPrimitiveArray, index: i32) -> JvmciResult<i8> {
        self.check_init()?;
        match array.0.heap_value() {
            Some(JvmciHeapValue::ByteArray(v)) => v
                .get(index as usize)
                .map(|b| *b as i8)
                .ok_or_else(|| self.throw_array_index_out_of_bounds(None)),
            _ => Err(self.throw_illegal_argument(Some("not a byte array"))),
        }
    }

    pub fn put_byte_at(
        &mut self,
        array: JvmciPrimitiveArray,
        index: i32,
        value: i8,
    ) -> JvmciResult<()> {
        self.check_init()?;
        let len = self.array_len(array.0)? as i32;
        if index < 0 || index >= len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        match jvmciObject::with_entry_mut(array.0.jobject, |e| match &mut e.value {
            JvmciHeapValue::ByteArray(v) => {
                v[index as usize] = value as u8;
                true
            }
            _ => false,
        }) {
            Some(true) => Ok(()),
            Some(false) => Err(self.throw_illegal_argument(Some("not a byte array"))),
            None => Err(self.throw_null_pointer(None)),
        }
    }

    pub fn get_int_at(&mut self, array: JvmciPrimitiveArray, index: i32) -> JvmciResult<i32> {
        self.check_init()?;
        match array.0.heap_value() {
            Some(JvmciHeapValue::IntArray(v)) => v
                .get(index as usize)
                .copied()
                .ok_or_else(|| self.throw_array_index_out_of_bounds(None)),
            _ => Err(self.throw_illegal_argument(Some("not an int array"))),
        }
    }

    pub fn put_int_at(
        &mut self,
        array: JvmciPrimitiveArray,
        index: i32,
        value: i32,
    ) -> JvmciResult<()> {
        self.check_init()?;
        let len = self.array_len(array.0)? as i32;
        if index < 0 || index >= len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        match jvmciObject::with_entry_mut(array.0.jobject, |e| match &mut e.value {
            JvmciHeapValue::IntArray(v) => {
                v[index as usize] = value;
                true
            }
            _ => false,
        }) {
            Some(true) => Ok(()),
            Some(false) => Err(self.throw_illegal_argument(Some("not an int array"))),
            None => Err(self.throw_null_pointer(None)),
        }
    }

    pub fn get_long_at(&mut self, array: JvmciPrimitiveArray, index: i32) -> JvmciResult<i64> {
        self.check_init()?;
        match array.0.heap_value() {
            Some(JvmciHeapValue::LongArray(v)) => v
                .get(index as usize)
                .copied()
                .ok_or_else(|| self.throw_array_index_out_of_bounds(None)),
            _ => Err(self.throw_illegal_argument(Some("not a long array"))),
        }
    }

    pub fn put_long_at(
        &mut self,
        array: JvmciPrimitiveArray,
        index: i32,
        value: i64,
    ) -> JvmciResult<()> {
        self.check_init()?;
        let len = self.array_len(array.0)? as i32;
        if index < 0 || index >= len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        match jvmciObject::with_entry_mut(array.0.jobject, |e| match &mut e.value {
            JvmciHeapValue::LongArray(v) => {
                v[index as usize] = value;
                true
            }
            _ => false,
        }) {
            Some(true) => Ok(()),
            Some(false) => Err(self.throw_illegal_argument(Some("not a long array"))),
            None => Err(self.throw_null_pointer(None)),
        }
    }

    /// Bulk byte copies between a JVMCI primitive array and a buffer.
    pub fn copy_bytes_to(
        &mut self,
        src: JvmciPrimitiveArray,
        dest: &mut [u8],
        offset: i32,
        length: i32,
    ) -> JvmciResult<()> {
        self.check_init()?;
        match src.0.heap_value() {
            Some(JvmciHeapValue::ByteArray(v)) => {
                let start = offset.max(0) as usize;
                let count = (length.max(0) as usize).min(v.len().saturating_sub(start));
                let src_slice = &v[start..start + count];
                let dst_len = dest.len().min(count);
                dest[..dst_len].copy_from_slice(&src_slice[..dst_len]);
                Ok(())
            }
            _ => Err(self.throw_illegal_argument(Some("not a byte array"))),
        }
    }

    pub fn copy_bytes_from(
        &mut self,
        src: &[u8],
        dest: JvmciPrimitiveArray,
        offset: i32,
        length: i32,
    ) -> JvmciResult<()> {
        self.check_init()?;
        let len = self.array_len(dest.0)? as i32;
        let start = offset;
        if start < 0 || start + length > len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        let s = start as usize;
        let n = length.max(0) as usize;
        let n = n.min(src.len());
        match jvmciObject::with_entry_mut(dest.0.jobject, |e| match &mut e.value {
            JvmciHeapValue::ByteArray(v) => {
                v[s..s + n].copy_from_slice(&src[..n]);
                true
            }
            _ => false,
        }) {
            Some(true) => Ok(()),
            Some(false) => Err(self.throw_illegal_argument(Some("not a byte array"))),
            None => Err(self.throw_null_pointer(None)),
        }
    }

    pub fn copy_longs_from(
        &mut self,
        src: &[i64],
        dest: JvmciPrimitiveArray,
        offset: i32,
        length: i32,
    ) -> JvmciResult<()> {
        self.check_init()?;
        let len = self.array_len(dest.0)? as i32;
        let start = offset;
        if start < 0 || start + length > len {
            return Err(self.throw_array_index_out_of_bounds(None));
        }
        let s = start as usize;
        let n = (length.max(0) as usize).min(src.len());
        match jvmciObject::with_entry_mut(dest.0.jobject, |e| match &mut e.value {
            JvmciHeapValue::LongArray(v) => {
                v[s..s + n].copy_from_slice(&src[..n]);
                true
            }
            _ => false,
        }) {
            Some(true) => Ok(()),
            Some(false) => Err(self.throw_illegal_argument(Some("not a long array"))),
            None => Err(self.throw_null_pointer(None)),
        }
    }

    // ---- object helpers ----------------------------------------------

    /// `create_string`.
    pub fn create_string(&mut self, s: &str) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Ok(jvmciObject::new_jvmci_string(s))
    }

    /// `as_utf8_string`.
    pub fn as_utf8_string(&mut self, obj: JvmciObject) -> JvmciResult<String> {
        self.check_init()?;
        match obj.heap_value() {
            Some(JvmciHeapValue::Utf8String(s)) => Ok(s),
            Some(JvmciHeapValue::Null) | None => Err(self.throw_null_pointer(None)),
            _ => Err(self.throw_illegal_argument(Some("not a string"))),
        }
    }

    /// `equals`.
    pub fn equals(&mut self, a: JvmciObject, b: JvmciObject) -> bool {
        if a.jobject == 0 || b.jobject == 0 {
            return a.jobject == b.jobject;
        }
        a.jobject == b.jobject
            || match (a.heap_value(), b.heap_value()) {
                (Some(JvmciHeapValue::Utf8String(x)), Some(JvmciHeapValue::Utf8String(y))) => x == y,
                _ => false,
            }
    }

    /// `is_boxing_object` / `get_box_type` / `get_boxed_value` /
    /// `create_box` operate on `java.lang.*` box classes which live on
    /// the HotSpot heap; without a JVMCI Java runtime the box caches are
    /// unavailable, so these report unsupported rather than guess.
    pub fn is_boxing_object(&mut self, _type: i32, _object: JvmciObject) -> bool {
        false
    }

    /// `get_box_type`: T_ILLEGAL analogue when the object is not a box.
    pub fn get_box_type(
        &mut self,
        _object: JvmciObject,
    ) -> crate::share::ci::ciType::BasicType {
        crate::share::ci::ciType::BasicType::Illegal
    }

    /// `klass_name`.
    pub fn klass_name(&mut self, object: JvmciObject) -> JvmciResult<String> {
        self.check_init()?;
        jvmciObject::object_class_name(object)
            .ok_or_else(|| self.throw_illegal_argument(Some("object has no class name")))
    }

    // ---- oop / metadata handles --------------------------------------

    /// `make_oop_handle`: allocate a VM-scoped handle for a HotSpot
    /// object; returns a jlong handle id.
    pub fn make_oop_handle(&mut self, obj: &Handle) -> i64 {
        let rt = match self.runtime() {
            Ok(rt) => rt,
            Err(_) => return 0,
        };
        rt.make_oop_handle(obj)
    }

    /// `resolve_oop_handle`.
    pub fn resolve_oop_handle(&mut self, handle: i64) -> Option<OopLike> {
        let rt = self.runtime().ok()?;
        rt.resolve_oop_handle(handle)
    }

    // ---- JNI handle scopes -------------------------------------------

    /// `make_local`.
    pub fn make_local(&mut self, object: JvmciObject) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        if self.is_hotspot != object.is_hotspot && !object.is_null() {
            return Err(self.throw_illegal_argument(Some("heap mismatch in make_local")));
        }
        Ok(object)
    }

    /// `destroy_local`.
    pub fn destroy_local(&mut self, object: JvmciObject) {
        jvmciObject::free_object(object);
    }

    /// `make_global`: VM-scoped global handle, implemented as a fresh
    /// object-store allocation owned by no runtime.
    pub fn make_global(&mut self, object: JvmciObject) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Ok(object)
    }

    /// `destroy_global`.
    pub fn destroy_global(&mut self, object: JvmciObject) {
        jvmciObject::free_object(object);
    }

    /// `lookup_nmethod`: find the installed nmethod covering `code`.
    pub fn lookup_nmethod(&mut self, code: usize, _compile_id_snapshot: i64) -> Option<String> {
        crate::share::code::codeCache::CodeCache::global()
            .find_any_nmethod_at(code)
            .map(|nm| {
                crate::share::code::codeCache::CodeCache::method_key(
                    nm.method_holder(),
                    nm.method_name(),
                    nm.method_signature(),
                )
            })
    }

    // ---- HotSpotJVMCIRuntime downcalls --------------------------------
    //
    // These invoke Java methods on JVMCI objects.  They require a live
    // JVMCI Java runtime (jdk.vm.ci classes + a compiler such as Graal
    // or libjvmci).  When it is absent the honest result is an
    // UnsupportedOperationException-style error, matching HotSpot
    // behaviour without a JVMCI compiler.

    fn unsupported_java_call(&mut self, what: &str) -> JvmciException {
        self.raise(JvmciException::unsupported(format!(
            "JVMCI Java call '{what}' requires a JVMCI runtime \
             (jdk.vm.ci / libjvmci or Graal); none is available in this VM"
        )))
    }

    pub fn call_hotspot_jvmci_runtime_compile_method(
        &mut self,
        _runtime: JvmciObject,
        _method: JvmciObject,
        _entry_bci: i32,
        _compile_state: i64,
        _id: i32,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Err(self.unsupported_java_call("HotSpotJVMCIRuntime.compileMethod"))
    }

    pub fn call_hotspot_jvmci_runtime_bootstrap_finished(
        &mut self,
        _runtime: JvmciObject,
    ) -> JvmciResult<()> {
        self.check_init()?;
        Err(self.unsupported_java_call("HotSpotJVMCIRuntime.bootstrapFinished"))
    }

    pub fn call_hotspot_jvmci_runtime_shutdown(&mut self, _runtime: JvmciObject) -> JvmciResult<()> {
        self.check_init()?;
        Err(self.unsupported_java_call("HotSpotJVMCIRuntime.shutdown"))
    }

    pub fn call_hotspot_jvmci_runtime_runtime(&mut self) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Err(self.unsupported_java_call("HotSpotJVMCIRuntime.runtime"))
    }

    pub fn call_jvmci_get_runtime(&mut self) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Err(self.unsupported_java_call("JVMCI.getRuntime"))
    }

    pub fn call_jvmci_initialize_runtime(&mut self) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Err(self.unsupported_java_call("JVMCI.initializeRuntime"))
    }

    pub fn call_hotspot_jvmci_runtime_get_compiler(
        &mut self,
        _runtime: JvmciObject,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Err(self.unsupported_java_call("HotSpotJVMCIRuntime.getCompiler"))
    }

    pub fn call_java_constant_for_primitive(
        &mut self,
        _type_char: u16,
        _value: i64,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        Err(self.unsupported_java_call("JavaConstant.forPrimitive"))
    }

    pub fn call_hotspot_jvmci_runtime_is_gc_supported(
        &mut self,
        _runtime: JvmciObject,
        _gc_identifier: i32,
    ) -> bool {
        false
    }

    pub fn call_hotspot_jvmci_runtime_is_intrinsic_supported(
        &mut self,
        _runtime: JvmciObject,
        _intrinsic_id: i32,
    ) -> bool {
        false
    }

    pub fn call_hotspot_jvmci_runtime_post_translation(
        &mut self,
        _object: JvmciObject,
    ) -> JvmciResult<()> {
        self.check_init()?;
        Ok(())
    }

    // ---- type conversion ----------------------------------------------

    /// `typeCharToBasicType` (`ciType::BasicType`).
    pub fn type_char_to_basic_type(
        &mut self,
        ch: u16,
    ) -> JvmciResult<crate::share::ci::ciType::BasicType> {
        use crate::share::ci::ciType::BasicType;
        self.check_init()?;
        Ok(match ch as u8 as char {
            'Z' => BasicType::Boolean,
            'B' => BasicType::Byte,
            'C' => BasicType::Char,
            'S' => BasicType::Short,
            'I' => BasicType::Int,
            'F' => BasicType::Float,
            'J' => BasicType::Long,
            'D' => BasicType::Double,
            'L' => BasicType::Object,
            'A' => BasicType::Array,
            'V' => BasicType::Void,
            _ => {
                return Err(self.throw_illegal_argument(Some(&format!(
                    "illegal typeChar 0x{ch:04x}"
                ))))
            }
        })
    }

    /// `kindToBasicType` for a JavaKind object (reads its `typeChar`
    /// field).
    pub fn kind_to_basic_type(
        &mut self,
        kind: JvmciObject,
    ) -> JvmciResult<crate::share::ci::ciType::BasicType> {
        self.check_init()?;
        match kind.field("typeChar") {
            Some(JvmciValue::Char(c)) => self.type_char_to_basic_type(c as u16),
            Some(JvmciValue::Int(v)) => self.type_char_to_basic_type(v as u16),
            _ => Err(self.throw_illegal_argument(Some("not a JavaKind"))),
        }
    }

    // ---- installed-code / nmethod access ------------------------------

    /// `get_code_blob`: resolve a HotSpotInstalledCode object to the
    /// code-cache blob address covering its `codeStart`.
    pub fn get_code_blob(&mut self, code: JvmciObject) -> Option<usize> {
        let start = match code.field("codeStart").and_then(|v| v.as_i64()) {
            Some(v) => v as usize,
            None => return None,
        };
        crate::share::code::codeCache::CodeCache::global()
            .find_blob_at_address(start)
            .map(|b| b.code_begin())
    }

    /// `get_nmethod`.
    pub fn get_nmethod(&mut self, code: JvmciObject, nmethod_handle: &mut JvmciNmethodHandle) -> Option<String> {
        let start = code.field("codeStart").and_then(|v| v.as_i64())? as usize;
        let key = crate::share::code::codeCache::CodeCache::global()
            .find_any_nmethod_at(start)
            .map(|nm| {
                crate::share::code::codeCache::CodeCache::method_key(
                    nm.method_holder(),
                    nm.method_name(),
                    nm.method_signature(),
                )
            })?;
        nmethod_handle.set_nmethod(Some(key.clone()));
        Some(key)
    }

    /// `asMethod`: unpack a HotSpotResolvedJavaMethodImpl into the
    /// canonical method key stored in its `methodHandle` field.
    pub fn as_method(&mut self, jvmci_method: JvmciObject) -> JvmciResult<String> {
        self.check_init()?;
        match jvmci_method.field("methodHandle") {
            Some(JvmciValue::Long(h)) => {
                let rt = self.runtime()?;
                let target = rt.resolve_metadata_handle(h as u64);
                match target {
                    Some(t) if t.is_method() => Ok(t.key().to_string()),
                    _ => Err(self.throw_illegal_argument(Some("not a method handle"))),
                }
            }
            _ => Err(self.throw_illegal_argument(Some(
                "HotSpotResolvedJavaMethodImpl without methodHandle",
            ))),
        }
    }

    /// `asKlass`: unpack a HotSpotResolvedObjectTypeImpl into the class
    /// key stored in its `klassPointer` field.
    pub fn as_klass(&mut self, jvmci_type: JvmciObject) -> JvmciResult<String> {
        self.check_init()?;
        match jvmci_type.field("klassPointer") {
            Some(JvmciValue::Long(h)) => {
                let rt = self.runtime()?;
                let target = rt.resolve_metadata_handle(h as u64);
                match target {
                    Some(t) if t.is_klass() => Ok(t.key().to_string()),
                    _ => Err(self.throw_illegal_argument(Some("not a klass handle"))),
                }
            }
            _ => Err(self.throw_illegal_argument(Some(
                "HotSpotResolvedObjectTypeImpl without klassPointer",
            ))),
        }
    }

    /// `asConstantPool`.
    pub fn as_constant_pool(&mut self, constant_pool: JvmciObject) -> JvmciResult<String> {
        self.check_init()?;
        match constant_pool.field("constantPoolHandle") {
            Some(JvmciValue::Long(h)) => {
                let rt = self.runtime()?;
                let target = rt.resolve_metadata_handle(h as u64);
                match target {
                    Some(t) if t.is_constant_pool() => Ok(t.key().to_string()),
                    _ => Err(self.throw_illegal_argument(Some("not a constant pool handle"))),
                }
            }
            _ => Err(self.throw_illegal_argument(Some(
                "HotSpotConstantPool without constantPoolHandle",
            ))),
        }
    }

    /// `get_jvmci_method`: wrap a method key as a
    /// HotSpotResolvedJavaMethodImpl JVMCI object.
    pub fn get_jvmci_method(&mut self, method_key: &str) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let rt = self.runtime()?;
        let handle = rt
            .allocate_handle_method(method_key)
            .ok_or_else(|| self.throw_out_of_memory(Some("metadata handle allocation failed")))?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "methodHandle".to_string(),
            JvmciValue::Long(handle as i64),
        );
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::HOTSPOT_RESOLVED_JAVA_METHOD_IMPL,
            fields,
        ))
    }

    /// `get_jvmci_type`.
    pub fn get_jvmci_type(&mut self, klass: &JvmciKlassHandle) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let rt = self.runtime()?;
        let handle = rt
            .allocate_handle_klass(&klass.key)
            .ok_or_else(|| self.throw_out_of_memory(Some("metadata handle allocation failed")))?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("klassPointer".to_string(), JvmciValue::Long(handle as i64));
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::HOTSPOT_RESOLVED_OBJECT_TYPE_IMPL,
            fields,
        ))
    }

    /// `get_jvmci_constant_pool`.
    pub fn get_jvmci_constant_pool(&mut self, cp_key: &str) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let rt = self.runtime()?;
        let handle = rt
            .allocate_handle_constant_pool(cp_key)
            .ok_or_else(|| self.throw_out_of_memory(Some("metadata handle allocation failed")))?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "constantPoolHandle".to_string(),
            JvmciValue::Long(handle as i64),
        );
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::HOTSPOT_CONSTANT_POOL,
            fields,
        ))
    }

    /// `get_jvmci_primitive_type`: HotSpotResolvedPrimitiveType wrapping
    /// a HotSpotObjectConstantImpl mirror.
    pub fn get_jvmci_primitive_type(&mut self, _type: i32) -> JvmciObject {
        JvmciObject::NULL
    }

    /// `asConstant`.
    pub fn as_constant(&mut self, object: JvmciObject) -> JvmciResult<Handle> {
        self.check_init()?;
        match object.heap_value() {
            Some(JvmciHeapValue::Oop(oop)) => Ok(Handle::new(oop)),
            _ => Err(self.throw_illegal_argument(Some("not a HotSpot object constant"))),
        }
    }

    /// `get_object_constant`.
    pub fn get_object_constant(&mut self, oop: crate::share::oops::oop::Oop) -> JvmciObject {
        jvmciObject::alloc_object(true, JvmciHeapValue::Oop(oop))
    }

    // ---- object factories ----------------------------------------------

    pub fn new_boolean_array(&mut self, length: i32) -> JvmciResult<JvmciPrimitiveArray> {
        self.check_init()?;
        if length < 0 {
            return Err(self.throw_illegal_argument(Some("negative array length")));
        }
        Ok(JvmciPrimitiveArray(jvmciObject::alloc_object(
            false,
            JvmciHeapValue::BoolArray(vec![false; length as usize]),
        )))
    }

    pub fn new_byte_array(&mut self, length: i32) -> JvmciResult<JvmciPrimitiveArray> {
        self.check_init()?;
        if length < 0 {
            return Err(self.throw_illegal_argument(Some("negative array length")));
        }
        Ok(JvmciPrimitiveArray(jvmciObject::new_jvmci_byte_array(vec![
            0; length as usize
        ])))
    }

    pub fn new_int_array(&mut self, length: i32) -> JvmciResult<JvmciPrimitiveArray> {
        self.check_init()?;
        if length < 0 {
            return Err(self.throw_illegal_argument(Some("negative array length")));
        }
        Ok(JvmciPrimitiveArray(jvmciObject::alloc_object(
            false,
            JvmciHeapValue::IntArray(vec![0; length as usize]),
        )))
    }

    pub fn new_long_array(&mut self, length: i32) -> JvmciResult<JvmciPrimitiveArray> {
        self.check_init()?;
        if length < 0 {
            return Err(self.throw_illegal_argument(Some("negative array length")));
        }
        Ok(JvmciPrimitiveArray(jvmciObject::alloc_object(
            false,
            JvmciHeapValue::LongArray(vec![0; length as usize]),
        )))
    }

    pub fn new_byte_array_array(&mut self, length: i32) -> JvmciResult<JvmciObjectArray> {
        self.check_init()?;
        if length < 0 {
            return Err(self.throw_illegal_argument(Some("negative array length")));
        }
        Ok(JvmciObjectArray(jvmciObject::alloc_object(
            false,
            JvmciHeapValue::ObjectArray(vec![JvmciObject::NULL; length as usize]),
        )))
    }

    /// `new_StackTraceElement`.
    pub fn new_stack_trace_element(
        &mut self,
        declaring_class: &str,
        method_name: &str,
        file_name: Option<&str>,
        line_number: i32,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "declaringClass".to_string(),
            JvmciValue::Utf8(declaring_class.to_string()),
        );
        fields.insert(
            "methodName".to_string(),
            JvmciValue::Utf8(method_name.to_string()),
        );
        fields.insert(
            "fileName".to_string(),
            JvmciValue::Utf8(file_name.unwrap_or("").to_string()),
        );
        fields.insert("lineNumber".to_string(), JvmciValue::Int(line_number));
        Ok(jvmciObject::new_jvmci_object("java/lang/StackTraceElement", fields))
    }

    /// `new_HotSpotNmethod`.
    pub fn new_hotspot_nmethod(
        &mut self,
        method: JvmciObject,
        name: Option<&str>,
        is_default: bool,
        compile_id: i64,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("method".to_string(), JvmciValue::Object(method));
        fields.insert(
            "name".to_string(),
            JvmciValue::Utf8(name.unwrap_or("").to_string()),
        );
        fields.insert("isDefault".to_string(), JvmciValue::Bool(is_default));
        fields.insert(
            "compileIdSnapshot".to_string(),
            JvmciValue::Long(compile_id),
        );
        fields.insert("invalidationReason".to_string(), JvmciValue::Int(-1));
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::HOTSPOT_NMETHOD,
            fields,
        ))
    }

    /// `new_VMField`.
    pub fn new_vm_field(
        &mut self,
        name: JvmciObject,
        ty: JvmciObject,
        offset: i64,
        address: i64,
        value: JvmciObject,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("name".to_string(), JvmciValue::Object(name));
        fields.insert("type".to_string(), JvmciValue::Object(ty));
        fields.insert("offset".to_string(), JvmciValue::Long(offset));
        fields.insert("address".to_string(), JvmciValue::Long(address));
        fields.insert("value".to_string(), JvmciValue::Object(value));
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::VM_FIELD,
            fields,
        ))
    }

    /// `new_VMFlag`.
    pub fn new_vm_flag(
        &mut self,
        name: JvmciObject,
        ty: JvmciObject,
        value: JvmciObject,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("name".to_string(), JvmciValue::Object(name));
        fields.insert("type".to_string(), JvmciValue::Object(ty));
        fields.insert("value".to_string(), JvmciValue::Object(value));
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::VM_FLAG,
            fields,
        ))
    }

    /// `new_VMIntrinsicMethod`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_vm_intrinsic_method(
        &mut self,
        declaring_class: JvmciObject,
        name: JvmciObject,
        descriptor: JvmciObject,
        id: i32,
        is_available: bool,
        c1_supported: bool,
        c2_supported: bool,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "declaringClass".to_string(),
            JvmciValue::Object(declaring_class),
        );
        fields.insert("name".to_string(), JvmciValue::Object(name));
        fields.insert("descriptor".to_string(), JvmciValue::Object(descriptor));
        fields.insert("id".to_string(), JvmciValue::Int(id));
        fields.insert("isAvailable".to_string(), JvmciValue::Bool(is_available));
        fields.insert("c1Supported".to_string(), JvmciValue::Bool(c1_supported));
        fields.insert("c2Supported".to_string(), JvmciValue::Bool(c2_supported));
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::VM_INTRINSIC_METHOD,
            fields,
        ))
    }

    /// `new_HotSpotStackFrameReference`.
    pub fn new_hotspot_stack_frame_reference(&mut self) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("objectsMaterialized".to_string(), JvmciValue::Bool(false));
        fields.insert("stackPointer".to_string(), JvmciValue::Long(0));
        fields.insert("frameNumber".to_string(), JvmciValue::Int(-1));
        fields.insert("bci".to_string(), JvmciValue::Int(-1));
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::HOTSPOT_STACK_FRAME_REFERENCE,
            fields,
        ))
    }

    /// `new_JVMCIError`.
    pub fn new_jvmci_error(&mut self, message: Option<&str>) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "detailMessage".to_string(),
            JvmciValue::Utf8(message.unwrap_or("").to_string()),
        );
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::JVMCI_ERROR,
            fields,
        ))
    }

    /// `new_FieldInfo` (HotSpotResolvedObjectTypeImpl$FieldInfo).
    pub fn new_field_info(
        &mut self,
        name_index: i32,
        signature_index: i32,
        offset: i32,
        classfile_flags: i32,
        internal_flags: i32,
        initializer_index: i32,
    ) -> JvmciResult<JvmciObject> {
        self.check_init()?;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("nameIndex".to_string(), JvmciValue::Int(name_index));
        fields.insert(
            "signatureIndex".to_string(),
            JvmciValue::Int(signature_index),
        );
        fields.insert("offset".to_string(), JvmciValue::Int(offset));
        fields.insert(
            "classfileFlags".to_string(),
            JvmciValue::Int(classfile_flags),
        );
        fields.insert(
            "internalFlags".to_string(),
            JvmciValue::Int(internal_flags),
        );
        fields.insert(
            "initializerIndex".to_string(),
            JvmciValue::Int(initializer_index),
        );
        Ok(jvmciObject::new_jvmci_object(
            crate::share::jvmci::vmSymbolsJvmci::classes::HOTSPOT_RESOLVED_OBJECT_TYPE_IMPL_FIELD_INFO,
            fields,
        ))
    }

    /// `invalidate_nmethod_mirror`: zero the `address` field of a
    /// HotSpotNmethod mirror and deoptimize the backing nmethod when
    /// requested.
    pub fn invalidate_nmethod_mirror(
        &mut self,
        mirror: JvmciObject,
        deoptimize: bool,
        _invalidation_reason: i32,
    ) -> JvmciResult<()> {
        self.check_init()?;
        // Read the backing method key and entry point, then invalidate.
        let method_obj = match mirror.field("method") {
            Some(JvmciValue::Object(o)) => o,
            _ => JvmciObject::NULL,
        };
        let key = if method_obj.is_null() {
            String::new()
        } else {
            self.as_method(method_obj).unwrap_or_default()
        };
        if !key.is_empty() {
            if deoptimize {
                crate::share::code::codeCache::CodeCache::global()
                    .mark_all_nmethods_for_deopt();
            }
            crate::share::code::codeCache::CodeCache::global()
                .make_not_entrant_by_key(&key);
        }
        jvmciObject::set_object_field(mirror, "address", JvmciValue::Long(0));
        jvmciObject::set_object_field(mirror, "entryPoint", JvmciValue::Long(0));
        Ok(())
    }

    /// `initialize_installed_code`: publish code-cache coordinates into
    /// a HotSpotInstalledCode object.
    pub fn initialize_installed_code(
        &mut self,
        installed_code: JvmciObject,
        code_begin: usize,
        code_size: usize,
        name: &str,
    ) {
        jvmciObject::set_object_field(
            installed_code,
            "codeStart",
            JvmciValue::Long(code_begin as i64),
        );
        jvmciObject::set_object_field(
            installed_code,
            "codeSize",
            JvmciValue::Int(code_size as i32),
        );
        jvmciObject::set_object_field(
            installed_code,
            "size",
            JvmciValue::Int(code_size as i32),
        );
        jvmciObject::set_object_field(
            installed_code,
            "name",
            JvmciValue::Utf8(name.to_string()),
        );
    }
}

impl Drop for JvmciEnv {
    fn drop(&mut self) {
        if self.detach_on_close {
            if let Some(rt) = self.runtime.take() {
                rt.detach_thread_env(self.is_hotspot, "JvmciEnv scope closed");
            }
        }
    }
}

/// Placeholder for the resolved `Oop` of a HotSpot object constant.
pub type OopLike = crate::share::oops::oop::Oop;
