/*
 * Port of OpenJDK 25 HotSpot StackWalk
 * (share/prims/stackwalk.hpp / stackwalk.cpp)
 *
 * Implements JEP 259 Stack-Walking API runtime support (java.lang.StackWalker):
 * StackWalker options, stack frame batch fetching, and caller-sensitive method inspection.
 */

use super::methodHandleNatives::{method_type_descriptor, object_field};
use super::nativeLookup::set_instance_field;
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::method::Code;
use crate::share::oops::symbol::Symbol;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use std::cell::RefCell;
use std::io::{self, Write};
use std::sync::atomic::{AtomicI64, Ordering};

const walkerClass: &[u8] = b"java/lang/StackStreamFactory$AbstractStackWalker";

struct FrameWalk {
    anchor: i64,
    receiver: usize,
    classOnly: bool,
    continuation: Option<usize>,
    frames: std::vec::IntoIter<StackFrameInfo>,
}

thread_local! {
    static frameWalks: RefCell<Vec<FrameWalk>> = const { RefCell::new(Vec::new()) };
}

struct WalkScope;

impl Drop for WalkScope {
    fn drop(&mut self) {
        frameWalks.with_borrow_mut(|walks| {
            walks.pop();
        });
    }
}

fn fillFrames<'a>(
    loader: &ClassLoader<'a>,
    walk: &mut FrameWalk,
    array: usize,
    limit: i32,
    start: i32,
) -> io::Result<i32> {
    let startIndex = usize::try_from(start).map_err(io::Error::other)?;
    let limitIndex = usize::try_from(limit).map_err(io::Error::other)?;
    let mut count = 0;
    for index in startIndex..limitIndex {
        let Some(frame) = walk.frames.next() else {
            break;
        };
        let handle = match loader
            .obj_arrays
            .borrow()
            .get(array)
            .and_then(|a| a.elements.get(index))
        {
            Some(Value::Object(handle)) => *handle,
            _ => return Err(io::Error::other("Invalid StackWalker frame buffer")),
        };
        let class = loader.load(frame.class_name.as_bytes())?;
        let method = class
            .class
            .methods
            .iter()
            .find(|method| {
                method.name == frame.method_name.as_bytes()
                    && method.descriptor == frame.signature.as_bytes()
            })
            .ok_or_else(|| io::Error::other("StackWalker frame method is missing"))?;
        let kind = if method.name == b"<init>" {
            0x20000
        } else {
            0x10000
        };
        let mut flags = i32::from(method.accessFlags) | kind;
        if frame.is_hidden_frame() {
            flags |= 0x400000;
        }
        let mirror = Value::ClassMirror(class.class.name);
        let classOrMethod = if walk.classOnly {
            mirror
        } else {
            set_instance_field(
                loader,
                handle,
                b"name",
                b"Ljava/lang/String;",
                Value::String(method.name),
            )?;
            set_instance_field(
                loader,
                handle,
                b"type",
                b"Ljava/lang/Object;",
                Value::String(method.descriptor),
            )?;
            set_instance_field(loader, handle, b"bci", b"I", Value::Int(frame.bci))?;
            set_instance_field(
                loader,
                handle,
                b"ste",
                b"Ljava/lang/StackTraceElement;",
                Value::Null,
            )?;
            let resolved = loader.load(b"java/lang/invoke/ResolvedMethodName")?;
            let holderSlot = resolved.instanceField(b"vmholder", b"Ljava/lang/Class;")?;
            let mut token = InstanceOop::new(resolved)?;
            token.fields[holderSlot] = mirror;
            let token = loader.alloc_object(token);
            Value::Object(token)
        };
        let mut objects = loader.borrow_objects_mut();
        let object = objects
            .get_mut(handle)
            .ok_or_else(|| io::Error::other("Invalid StackWalker frame"))?;
        let classSlot = object
            .klass
            .instanceField(b"classOrMemberName", b"Ljava/lang/Object;")?;
        let flagsSlot = object.klass.instanceField(b"flags", b"I")?;
        let Value::Int(previous) = object.fields[flagsSlot] else {
            return Err(io::Error::other("Invalid StackWalker flags"));
        };
        object.fields[classSlot] = classOrMethod;
        object.fields[flagsSlot] = Value::Int((previous & 0x08000000) | flags);
        count += 1;
    }
    Ok(count)
}

pub(super) fn initTraceElement<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<()> {
    let [Value::Object(element), Value::Object(frame)] = arguments else {
        return Err(io::Error::other("Invalid StackWalker trace element"));
    };
    let Value::Object(token) =
        object_field(loader, *frame, b"classOrMemberName", b"Ljava/lang/Object;")?
    else {
        return Err(io::Error::other("Missing StackWalker method"));
    };
    let Value::ClassMirror(name) = object_field(loader, token, b"vmholder", b"Ljava/lang/Class;")?
    else {
        return Err(io::Error::other("Missing StackWalker declaring class"));
    };
    let methodName = object_field(loader, *frame, b"name", b"Ljava/lang/String;")?;
    let descriptor = match object_field(loader, *frame, b"type", b"Ljava/lang/Object;")? {
        Value::String(bytes) => std::borrow::Cow::Borrowed(bytes),
        methodType => std::borrow::Cow::Owned(method_type_descriptor(loader, methodType)?),
    };
    let class = loader.load(name)?;
    let method = class
        .class
        .methods
        .iter()
        .find(|method| {
            matches!(methodName, Value::String(bytes) if bytes == method.name)
                && descriptor.as_ref() == method.descriptor
        })
        .ok_or_else(|| io::Error::other("StackWalker frame method is missing"))?;
    let line = match object_field(loader, *frame, b"bci", b"I")? {
        _ if method.accessFlags & 0x0100 != 0 => -2,
        Value::Int(bci) => u16::try_from(bci)
            .ok()
            .and_then(|bci| {
                Code::parse(method)
                    .ok()?
                    .lineNumber(&class.class.constants, bci)
            })
            .map_or(-1, i32::from),
        _ => -1,
    };
    let dotted = crate::classfile::decodeString(name)?.replace('/', ".");
    for (field, value) in [
        (
            b"declaringClass".as_slice(),
            Value::HostString(loader.storage.alloc_str(&dotted)),
        ),
        (b"methodName", methodName),
        (
            b"fileName",
            class.class.source_file().map_or(Value::Null, Value::String),
        ),
    ] {
        set_instance_field(loader, *element, field, b"Ljava/lang/String;", value)?;
    }
    set_instance_field(loader, *element, b"lineNumber", b"I", Value::Int(line))?;
    set_instance_field(
        loader,
        *element,
        b"declaringClassObject",
        b"Ljava/lang/Class;",
        Value::ClassMirror(name),
    )
}

#[allow(clippy::too_many_arguments)]
fn call_stack_walk<'a>(
    loader: &ClassLoader<'a>,
    receiver: usize,
    mode: i64,
    skip: i32,
    buffer_size: i32,
    start_index: i32,
    array: usize,
    output: &mut dyn Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    if mode & i64::from(JVM_STACKWALK_FILL_LIVE_STACK_FRAMES) != 0 {
        return Ok(None);
    }
    static nextAnchor: AtomicI64 = AtomicI64::new(1);
    let anchor = nextAnchor.fetch_add(1, Ordering::Relaxed);
    let frames = StackWalk::currentFrames(loader)
        .into_iter()
        .skip_while(|frame| {
            frame.class_name.as_bytes() == b"java/lang/StackWalker"
                || frame
                    .class_name
                    .as_bytes()
                    .starts_with(b"java/lang/StackStreamFactory$")
        })
        .skip(usize::try_from(skip).map_err(io::Error::other)?)
        .filter(|frame| {
            mode & i64::from(JVM_STACKWALK_SHOW_HIDDEN_FRAMES) != 0
                || !frame.is_hidden_frame()
        })
        .collect::<Vec<_>>();
    let mut walk = FrameWalk {
        anchor,
        receiver,
        classOnly: mode & i64::from(JVM_STACKWALK_FILL_CLASS_REFS_ONLY) != 0,
        continuation: None,
        frames: frames.into_iter(),
    };
    let count = fillFrames(loader, &mut walk, array, buffer_size, start_index)?;
    frameWalks.with_borrow_mut(|walks| walks.push(walk));
    let _scope = WalkScope;
    let mut call = JavaCallArguments::with_receiver(Value::Object(receiver));
    call.push_long(anchor);
    for value in [skip, count, start_index, start_index + count] {
        call.push_int(value);
    }
    let klass = loader.load(walkerClass)?;
    JavaCalls::call_special(
        loader,
        &klass,
        b"doStackWalk",
        b"(JIIII)Ljava/lang/Object;",
        &call,
        output,
    )
    .map(Some)
}

pub(super) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
    output: &mut dyn Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    match (name, descriptor, args) {
        // JDK 21: callStackWalk(long mode, int skipframes, ...)
        (
            b"callStackWalk",
            b"(JILjdk/internal/vm/ContinuationScope;Ljdk/internal/vm/Continuation;II[Ljava/lang/Object;)Ljava/lang/Object;",
            [Value::Object(receiver), Value::Long(mode), Value::Int(skip), Value::Null, Value::Null, Value::Int(buffer), Value::Int(start), Value::ObjectArray(array)],
        ) => call_stack_walk(
            loader, *receiver, *mode, *skip, *buffer, *start, *array, output,
        ),
        // JDK 25: callStackWalk(int mode, int skipframes, ...)
        (
            b"callStackWalk",
            b"(IILjdk/internal/vm/ContinuationScope;Ljdk/internal/vm/Continuation;II[Ljava/lang/Object;)Ljava/lang/Object;",
            [Value::Object(receiver), Value::Int(mode), Value::Int(skip), Value::Null, Value::Null, Value::Int(buffer), Value::Int(start), Value::ObjectArray(array)],
        ) => call_stack_walk(
            loader,
            *receiver,
            i64::from(*mode),
            *skip,
            *buffer,
            *start,
            *array,
            output,
        ),
        // JDK 25: fetchStackFrames(int mode, long anchor, int lastBatchFrameCount, int bufferSize, int startIndex, T[] frames)
        (
            b"fetchStackFrames",
            b"(IJIII[Ljava/lang/Object;)I",
            [Value::Object(receiver), Value::Int(_), Value::Long(anchor), Value::Int(_), Value::Int(limit), Value::Int(start), Value::ObjectArray(array)],
        ) => frameWalks.with_borrow_mut(|walks| {
            let walk = walks.last_mut().filter(|walk| walk.anchor == *anchor && walk.receiver == *receiver)
                .ok_or_else(|| io::Error::other("Invalid StackWalker anchor"))?;
            fillFrames(loader, walk, *array, *limit, *start).map(|count| Some(Some(Value::Int(count))))
        }),
        // JDK 21: fetchStackFrames(long mode, long anchor, int batchSize, int startIndex, T[] frames)
        (
            b"fetchStackFrames",
            b"(JJII[Ljava/lang/Object;)I",
            [Value::Object(receiver), Value::Long(_), Value::Long(anchor), Value::Int(batch), Value::Int(start), Value::ObjectArray(array)],
        ) => frameWalks.with_borrow_mut(|walks| {
            let walk = walks.last_mut().filter(|walk| walk.anchor == *anchor && walk.receiver == *receiver)
                .ok_or_else(|| io::Error::other("Invalid StackWalker anchor"))?;
            let end = start.saturating_add(*batch);
            fillFrames(loader, walk, *array, end, *start).map(|count| Some(Some(Value::Int(count))))
        }),
        (
            b"setContinuation",
            b"(J[Ljava/lang/Object;Ljdk/internal/vm/Continuation;)V",
            [Value::Object(_receiver), Value::Long(anchor), Value::ObjectArray(_), continuation],
        ) => {
            let continuation = match continuation {
                Value::Null => None,
                Value::Object(handle) => Some(*handle),
                _ => return Err(io::Error::other("Invalid continuation")),
            };
            frameWalks.with_borrow_mut(|walks| {
                if let Some(walk) = walks.iter_mut().rev().find(|walk| walk.anchor == *anchor) {
                    walk.continuation = continuation;
                }
            });
            Ok(Some(None))
        }
        (
            b"expandStackFrameInfo",
            b"()V",
            [Value::Object(receiver)],
        ) => {
            expand_frame_info(loader, *receiver)?;
            Ok(Some(None))
        }
        _ => {
            if name == b"callStackWalk" || name == b"fetchStackFrames" {
                eprintln!(
                    "[stackwalk-miss] {}{} args={:?}",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor),
                    args.iter()
                        .map(|v| match v {
                            Value::Null => "null".to_string(),
                            Value::Int(i) => format!("int({i})"),
                            Value::Long(l) => format!("long({l})"),
                            Value::Object(_) => "obj".to_string(),
                            Value::ObjectArray(_) => "objarr".to_string(),
                            _ => format!("{v:?}"),
                        })
                        .collect::<Vec<_>>()
                );
            }
            Ok(None)
        }
    }
}

pub const JVM_STACKWALK_FILL_CLASS_REFS_ONLY: i32 = 0x2;
pub const JVM_STACKWALK_GET_CALLER_CLASS: i32 = 0x4;
pub const JVM_STACKWALK_SHOW_HIDDEN_FRAMES: i32 = 0x20;
pub const JVM_STACKWALK_FILL_LIVE_STACK_FRAMES: i32 = 0x100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackWalkOptions {
    pub retain_class_reference: bool,
    pub show_reflect_frames: bool,
    pub show_hidden_frames: bool,
}

impl Default for StackWalkOptions {
    fn default() -> Self {
        Self {
            retain_class_reference: false,
            show_reflect_frames: false,
            show_hidden_frames: false,
        }
    }
}

impl StackWalkOptions {
    pub fn from_flags(flags: i32) -> Self {
        Self {
            retain_class_reference: (flags & JVM_STACKWALK_FILL_CLASS_REFS_ONLY) != 0,
            show_reflect_frames: false,
            show_hidden_frames: (flags & JVM_STACKWALK_SHOW_HIDDEN_FRAMES) != 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrameInfo {
    pub class_name: Symbol,
    pub method_name: Symbol,
    pub signature: Symbol,
    pub bci: i32,
    pub line_number: i32,
    pub is_native: bool,
}

impl StackFrameInfo {
    pub fn new(
        class_name: Symbol,
        method_name: Symbol,
        signature: Symbol,
        bci: i32,
        line_number: i32,
        is_native: bool,
    ) -> Self {
        Self {
            class_name,
            method_name,
            signature,
            bci,
            line_number,
            is_native,
        }
    }

    #[inline]
    pub fn is_reflection_frame(&self) -> bool {
        self.class_name
            .as_str()
            .starts_with("jdk/internal/reflect/")
            || self.class_name.as_str().starts_with("java/lang/reflect/")
    }

    #[inline]
    pub fn is_hidden_frame(&self) -> bool {
        self.class_name
            .as_str()
            .starts_with("java/lang/invoke/LambdaForm")
    }
}

pub struct StackWalk;

impl StackWalk {
    pub fn currentFrames(
        loader: &crate::share::classfile::classLoader::ClassLoader<'_>,
    ) -> Vec<StackFrameInfo> {
        let mut frames = Vec::new();
        let currentThread = crate::share::runtime::threading::current_tid(loader);
        crate::share::interpreter::bytecodeInterpreter::for_each_active_frame(&mut |frame| {
            if frame.threadId() == currentThread {
                frames.push(frame.stackWalkInfo());
            }
        });
        frames.reverse();
        frames
    }

    /// Filters and collects stack frames according to the requested StackWalker options
    pub fn filter_frames(
        frames: &[StackFrameInfo],
        options: &StackWalkOptions,
        skip_count: usize,
        max_batch_size: usize,
    ) -> Vec<StackFrameInfo> {
        if max_batch_size == 0 {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut skipped = 0;

        for frame in frames {
            if !options.show_reflect_frames && frame.is_reflection_frame() {
                continue;
            }
            if !options.show_hidden_frames && frame.is_hidden_frame() {
                continue;
            }

            if skipped < skip_count {
                skipped += 1;
                continue;
            }

            result.push(frame.clone());
            if result.len() >= max_batch_size {
                break;
            }
        }

        result
    }

    /// Finds the immediate caller class, skipping reflection and StackWalker infrastructure
    pub fn get_caller_class(frames: &[StackFrameInfo]) -> Option<Symbol> {
        let mut passed_walker = false;
        for frame in frames {
            let cls = frame.class_name.as_str();
            if cls.starts_with("java/lang/StackWalker") {
                passed_walker = true;
                continue;
            }
            if passed_walker {
                if frame.is_reflection_frame() || frame.is_hidden_frame() {
                    continue;
                }
                return Some(frame.class_name.clone());
            }
        }
        None
    }
}

fn expand_frame_info<'a>(loader: &ClassLoader<'a>, receiver: usize) -> io::Result<()> {
    let name = object_field(loader, receiver, b"name", b"Ljava/lang/String;")?;
    if !matches!(name, Value::Null | Value::Empty) {
        return Ok(());
    }
    let member = object_field(loader, receiver, b"classOrMemberName", b"Ljava/lang/Object;")?;
    let Value::Object(token) = member else {
        return Ok(());
    };
    let member_name = object_field(loader, token, b"name", b"Ljava/lang/String;")
        .unwrap_or(Value::Null);
    let member_type = object_field(loader, token, b"type", b"Ljava/lang/Object;")
        .unwrap_or(Value::Null);
    if !matches!(member_name, Value::Null | Value::Empty) {
        set_instance_field(loader, receiver, b"name", b"Ljava/lang/String;", member_name)?;
    }
    if !matches!(member_type, Value::Null | Value::Empty) {
        set_instance_field(loader, receiver, b"type", b"Ljava/lang/Object;", member_type)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emptyBatchDoesNotReturnFrame() {
        let frame = StackFrameInfo::new(
            Symbol::new(b"example/Main"),
            Symbol::new(b"main"),
            Symbol::new(b"()V"),
            3,
            -1,
            false,
        );
        assert!(StackWalk::filter_frames(&[frame], &StackWalkOptions::default(), 0, 0).is_empty());
    }

    #[test]
    fn missingWalkerHasNoCaller() {
        let frame = StackFrameInfo::new(
            Symbol::new(b"example/Main"),
            Symbol::new(b"main"),
            Symbol::new(b"()V"),
            3,
            -1,
            false,
        );
        assert_eq!(StackWalk::get_caller_class(&[frame]), None);
    }
}
