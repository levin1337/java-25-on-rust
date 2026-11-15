

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::jfr::leakprofiler::utilities::root_type::UnifiedOopRef;

pub fn outgoing_edges<'a>(
    loader: &ClassLoader<'a>,
    object: UnifiedOopRef,
) -> Vec<UnifiedOopRef> {
    let mut targets = Vec::new();
    match object {
        UnifiedOopRef::Object(handle) => {
            let objects = loader.borrow_objects();
            if let Some(instance) = objects.get(handle) {
                for value in &instance.fields {
                    let reference = UnifiedOopRef::from(*value);
                    if !reference.is_null() {
                        targets.push(reference);
                    }
                }
            }
        }
        UnifiedOopRef::ObjectArray(handle) => {
            let arrays = loader.obj_arrays.borrow();
            if let Some(array) = arrays.get(handle) {
                for value in &array.elements {
                    let reference = UnifiedOopRef::from(*value);
                    if !reference.is_null() {
                        targets.push(reference);
                    }
                }
            }
        }
        _ => {}
    }
    targets
}

pub fn object_size(loader: &ClassLoader<'_>, object: UnifiedOopRef) -> usize {
    match object {
        UnifiedOopRef::Object(handle) => loader
            .borrow_objects()
            .get(handle)
            .map(|o| o.fields.len() * 8)
            .unwrap_or(0),
        UnifiedOopRef::ObjectArray(handle) => loader
            .obj_arrays
            .borrow()
            .get(handle)
            .map(|a| a.elements.len() * 8)
            .unwrap_or(0),
        UnifiedOopRef::TypeArray(handle) => loader
            .arrays
            .borrow()
            .get(handle)
            .map(|a| match a {
                crate::share::oops::typeArrayOop::TypeArrayOop::Byte(v) => v.len(),
                crate::share::oops::typeArrayOop::TypeArrayOop::Short(v) => v.len() * 2,
                crate::share::oops::typeArrayOop::TypeArrayOop::Char(v) => v.len() * 2,
                crate::share::oops::typeArrayOop::TypeArrayOop::Int(v) => v.len() * 4,
                crate::share::oops::typeArrayOop::TypeArrayOop::Long(v) => v.len() * 8,
                crate::share::oops::typeArrayOop::TypeArrayOop::Float(v) => v.len() * 4,
                crate::share::oops::typeArrayOop::TypeArrayOop::Double(v) => v.len() * 8,
                crate::share::oops::typeArrayOop::TypeArrayOop::Boolean(v) => v.len(),
            })
            .unwrap_or(0),
        UnifiedOopRef::Null => 0,
    }
}
