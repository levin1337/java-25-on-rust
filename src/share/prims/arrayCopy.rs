use crate::share::{
    classfile::classLoader::ClassLoader, interpreter::bytecodeInterpreter::Value,
    oops::typeArrayOop::TypeArrayOop,
};
use std::{
    io::{self, Error},
    ops::Range,
};

pub(crate) fn copy<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>; 5]) -> io::Result<()> {
    use Value::{Int, Null, ObjectArray, PrimitiveArray};
    let [source, Int(sourcePos), target, Int(targetPos), Int(length)] = *args else {
        return Err(Error::other("Invalid arraycopy arguments"));
    };
    match (source, target) {
        (Null, _) | (_, Null) => Err(Error::other("NullPointerException")),
        (PrimitiveArray(source), PrimitiveArray(target)) => {
            let mut arrays = loader.arrays.borrow_mut();
            let available = arrays.len();
            if source >= available || target >= available {
                return Err(Error::other("Invalid array reference"));
            }
            if arrays[source].descriptor() != arrays[target].descriptor() {
                return Err(Error::other("ArrayStoreException"));
            }
            let from = range(sourcePos, length, arrays[source].len())?;
            let to = range(targetPos, length, arrays[target].len())?;
            macro_rules! copyElements {
                ($array:expr; $($kind:ident),+) => {
                    match $array {
                        $(TypeArrayOop::$kind(values) => values.copy_within(from, to.start),)+
                    }
                };
                ($source:expr, $target:expr; $($kind:ident),+) => {
                    match ($source, $target) {
                        $((TypeArrayOop::$kind(src), TypeArrayOop::$kind(dst)) => dst[to].copy_from_slice(&src[from]),)+
                        _ => return Err(Error::other("ArrayStoreException")),
                    }
                };
            }
            if source == target {
                copyElements!(
                    &mut arrays[source];
                    Boolean,
                    Char,
                    Float,
                    Double,
                    Byte,
                    Short,
                    Int,
                    Long
                );
            } else {
                let (prefix, suffix) = arrays.split_at_mut(source.max(target));
                let (src, dst) = if source < target {
                    (&prefix[source], &mut suffix[0])
                } else {
                    (&suffix[0], &mut prefix[target])
                };
                copyElements!(
                    src, dst; Boolean, Char, Float, Double, Byte, Short, Int, Long
                );
            }
            Ok(())
        }
        (ObjectArray(source), ObjectArray(target)) => {
            let (from, to, component) = {
                let arrays = loader.obj_arrays.borrow();
                let src = arrays
                    .get(source)
                    .ok_or_else(|| Error::other("Invalid array reference"))?;
                let dst = arrays
                    .get(target)
                    .ok_or_else(|| Error::other("Invalid array reference"))?;
                (
                    range(sourcePos, length, src.len())?,
                    range(targetPos, length, dst.len())?,
                    dst.component,
                )
            };
            if source == target {
                loader.obj_arrays.borrow_mut()[source]
                    .elements
                    .copy_within(from, to.start);
                return Ok(());
            }
            for (src, dst) in from.zip(to) {
                let value = loader.obj_arrays.borrow()[source].get(src)?;
                if !loader.referenceAssignable(value, component)? {
                    return Err(Error::other("ArrayStoreException"));
                }
                loader.obj_arrays.borrow_mut()[target].set(dst, value)?;
            }
            Ok(())
        }
        _ => Err(Error::other("ArrayStoreException")),
    }
}

fn range(start: i32, length: i32, available: usize) -> io::Result<Range<usize>> {
    let start = usize::try_from(start).ok();
    let length = usize::try_from(length).ok();
    match start.zip(length) {
        Some((start, length)) if start <= available && length <= available - start => {
            Ok(start..start + length)
        }
        _ => Err(Error::other("ArrayIndexOutOfBoundsException")),
    }
}
