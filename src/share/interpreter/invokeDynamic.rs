//! Shared invokedynamic dispatch used by both the bytecode interpreter
//! (`0xba`) and the C2 `InvokeDynamic` runtime stub.  Resolves the
//! bootstrap call site once (`LinkResolver::resolve_invokedynamic` is
//! the caller's job) and then services it through the same chain the
//! interpreter uses: StringConcatFactory fast path, ObjectMethods,
//! LambdaMetafactory, generic JVMS linkage.

use std::io::{self, Write};

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bootstrapInfo::{BootstrapArg, BootstrapInfo};
use crate::share::interpreter::bytecodeInterpreter::{Value, describeValueKind};
use crate::share::interpreter::{genericBootstrap, lambdaMetafactory, objectMethods};
use crate::share::oops::instanceKlass::InstanceKlass;

/// Services an already-resolved invokedynamic call site.
/// `captured` is the full dynamic argument slice (captures + call
/// arguments, exactly what the call-site descriptor covers).
/// Returns `Ok(Some(value))` with the call result, `Ok(None)` for a
/// void call site.
pub fn dispatch<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    bs_info: &BootstrapInfo,
    name: &[u8],
    descriptor: &[u8],
    captured: &[Value<'a>],
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Option<Value<'a>>> {
    let mut is_string_concat = name == b"makeConcat" || name == b"makeConcatWithConstants";
    if !is_string_concat {
        if let Some(target) = bs_info.resolved_target() {
            if target.contains("StringConcatFactory") {
                is_string_concat = true;
            }
        }
    }

    if is_string_concat {
        let mut formatted = String::new();
        let recipe = bs_info.arguments().first().and_then(|arg| {
            if let BootstrapArg::String(s) = arg {
                Some(s.as_str())
            } else {
                None
            }
        });

        let mut boolean_params = Vec::new();
        {
            let mut rest = descriptor.strip_prefix(b"(").unwrap_or(descriptor);
            while !rest.is_empty() && rest[0] != b')' {
                let dims = rest.iter().take_while(|byte| **byte == b'[').count();
                let tag = rest.get(dims).copied().unwrap_or(0);
                boolean_params.push(tag == b'Z');
                let len = if tag == b'L' {
                    dims + 1
                        + rest[dims + 1..]
                            .iter()
                            .position(|byte| *byte == b';')
                            .map(|pos| pos + 1)
                            .unwrap_or(rest.len().saturating_sub(dims + 1))
                } else {
                    dims + 1
                };
                rest = rest.get(len..).unwrap_or(&[]);
            }
        }
        let mut dynamic_args = Vec::with_capacity(captured.len());
        for (arg_index, value) in captured.iter().enumerate() {
            dynamic_args.push(describeValueKind(
                loader,
                *value,
                boolean_params.get(arg_index).copied().unwrap_or(false),
                output,
                depth + 1,
            )?);
        }

        if let Some(recipe_str) = recipe {
            let mut arg_iter = dynamic_args.into_iter();
            let mut const_iter = bs_info.arguments().iter().skip(1);
            for ch in recipe_str.chars() {
                if ch == '\u{0001}' {
                    if let Some(arg_val) = arg_iter.next() {
                        formatted.push_str(&arg_val);
                    }
                } else if ch == '\u{0002}' {
                    if let Some(c) = const_iter.next() {
                        match c {
                            BootstrapArg::String(s) => formatted.push_str(s),
                            BootstrapArg::Int(i) => formatted.push_str(&i.to_string()),
                            BootstrapArg::Long(l) => formatted.push_str(&l.to_string()),
                            _ => {}
                        }
                    }
                } else {
                    formatted.push(ch);
                }
            }
        } else {
            for arg_val in dynamic_args {
                formatted.push_str(&arg_val);
            }
        }

        let allocated_str = loader.storage.alloc_str(&formatted);
        return Ok(Some(Value::HostString(allocated_str)));
    }

    if let Some(value) =
        objectMethods::invoke(loader, klass, bs_info, captured, output, depth)?
    {
        return Ok(Some(value));
    }
    if let Some(value) =
        lambdaMetafactory::invoke(loader, klass, bs_info, name, descriptor, captured)?
    {
        return Ok(Some(value));
    }
    genericBootstrap::invoke(
        loader,
        klass,
        bs_info,
        name,
        descriptor,
        captured,
        output,
        depth,
    )
}
