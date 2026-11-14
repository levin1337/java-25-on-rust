/*
 * Port of OpenJDK 25 HotSpot heap reachability analysis
 * (share/memory + share/gc/shared)
 *
 * Mark-sweep collector over the ClassLoader object arenas
 * (objects / primitive arrays / object arrays). Handles are slot
 * indices, so collection is non-moving: dead slots go on a free
 * list and are reused by alloc_* helpers.
 *
 * Roots: interpreter frames of all active executions, static fields
 * of loaded classes, the system class loader and current thread
 * objects, extra GC roots registered by the VM, and JNI references.
 */

use crate::share::classfile::classLoader::{ClassLoader, FreeSlot};
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::oop::Oop;
use std::collections::HashSet;
use std::io;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Handle {
    Object(usize),
    TypeArray(usize),
    ObjArray(usize),
}

impl Handle {
    fn of(value: &Value<'_>) -> Option<Self> {
        match *value {
            Value::Object(handle) => Some(Self::Object(handle)),
            Value::PrimitiveArray(handle) => Some(Self::TypeArray(handle)),
            Value::ObjectArray(handle) => Some(Self::ObjArray(handle)),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Marks {
    objects: HashSet<usize>,
    type_arrays: HashSet<usize>,
    obj_arrays: HashSet<usize>,
}

impl Marks {
    fn insert(&mut self, handle: Handle) -> bool {
        match handle {
            Handle::Object(index) => self.objects.insert(index),
            Handle::TypeArray(index) => self.type_arrays.insert(index),
            Handle::ObjArray(index) => self.obj_arrays.insert(index),
        }
    }

    #[allow(dead_code)]
    fn contains(&self, handle: Handle) -> bool {
        match handle {
            Handle::Object(index) => self.objects.contains(&index),
            Handle::TypeArray(index) => self.type_arrays.contains(&index),
            Handle::ObjArray(index) => self.obj_arrays.contains(&index),
        }
    }
}

fn mark_value(loader: &ClassLoader<'_>, value: &Value<'_>, worklist: &mut Vec<Handle>) {
    if let Some(handle) = Handle::of(value) {
        worklist.push(handle);
    }
    let _ = loader;
}

fn drain(loader: &ClassLoader<'_>, marks: &mut Marks, worklist: &mut Vec<Handle>) {
    while let Some(handle) = worklist.pop() {
        if !marks.insert(handle) {
            continue;
        }
        match handle {
            Handle::Object(index) => {
                let fields = {
                    let objects = loader.borrow_objects();
                    objects.get(index).map(|object| object.fields.clone())
                };
                if let Some(fields) = fields {
                    for value in &fields {
                        mark_value(loader, value, worklist);
                    }
                }
            }
            Handle::ObjArray(index) => {
                let elements = {
                    let arrays = loader.obj_arrays.borrow();
                    arrays.get(index).map(|array| array.elements.clone())
                };
                if let Some(elements) = elements {
                    for value in &elements {
                        mark_value(loader, value, worklist);
                    }
                }
            }
            Handle::TypeArray(_) => {}
        }
    }
}

fn jni_roots() -> Vec<Oop> {
    let Some(vm) = crate::share::prims::jniInvocation::JavaVm::global_if_exists() else {
        return Vec::new();
    };
    unsafe { &*vm }.references().referenced_oops()
}

/// Runs a full mark-sweep collection over the loader's object arenas.
pub fn collect<'a>(loader: &ClassLoader<'a>) -> io::Result<()> {
    let mut marks = Marks::default();
    let mut worklist = Vec::new();

    crate::share::interpreter::bytecodeInterpreter::for_each_active_frame(&mut |frame| {
        for value in frame.locals().iter().chain(frame.stack()) {
            mark_value(loader, value, &mut worklist);
        }
    });

    for klass in loader.classes.borrow().values() {
        for cell in &klass.static_fields {
            mark_value(loader, &cell.get(), &mut worklist);
        }
    }

    for handle in [
        *loader.class_loader_object_handle().borrow(),
        *loader.current_thread_object_handle().borrow(),
        *loader.unnamed_module_object_handle().borrow(),
    ]
    .into_iter()
    .flatten()
    {
        worklist.push(Handle::Object(handle));
    }

    for value in loader.gc_extra_roots.borrow().iter() {
        mark_value(loader, value, &mut worklist);
    }

    for oop in jni_roots() {
        match oop {
            Oop::Instance(handle) => worklist.push(Handle::Object(handle)),
            Oop::TypeArray(handle) => worklist.push(Handle::TypeArray(handle)),
            Oop::ObjArray(handle) => worklist.push(Handle::ObjArray(handle)),
            Oop::Null => {}
        }
    }

    drain(loader, &mut marks, &mut worklist);

    {
        let mut objects = loader.borrow_objects_mut();
        let mut live = loader.objects_live.borrow_mut();
        live.resize(objects.len(), true);
        for index in 0..objects.len() {
            if live[index] && !marks.objects.contains(&index) {
                live[index] = false;
                objects[index].fields = Vec::new();
                loader.free_slot(FreeSlot::Object(index));
            }
        }
    }
    {
        let mut arrays = loader.arrays.borrow_mut();
        let mut live = loader.arrays_live.borrow_mut();
        live.resize(arrays.len(), true);
        for index in 0..arrays.len() {
            if live[index] && !marks.type_arrays.contains(&index) {
                live[index] = false;
                arrays[index] = crate::share::oops::typeArrayOop::TypeArrayOop::Byte(Vec::new());
                loader.free_slot(FreeSlot::TypeArray(index));
            }
        }
    }
    {
        let mut arrays = loader.obj_arrays.borrow_mut();
        let mut live = loader.obj_arrays_live.borrow_mut();
        live.resize(arrays.len(), true);
        for index in 0..arrays.len() {
            if live[index] && !marks.obj_arrays.contains(&index) {
                live[index] = false;
                arrays[index].elements = Vec::new();
                loader.free_slot(FreeSlot::ObjArray(index));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::oops::typeArrayOop::TypeArrayOop;

    #[test]
    fn releasesUnreachableArrayStorageAndReusesSlot() {
        let arena = bumpalo::Bump::new();
        let loader = ClassLoader::open(std::path::Path::new("."), &arena).unwrap();
        let handle = loader.alloc_type_array(TypeArrayOop::Byte(vec![1; 1_000_000]));
        collect(&loader).unwrap();
        assert_eq!(loader.arrays.borrow()[handle].len(), 0);
        let reused = loader.alloc_type_array(TypeArrayOop::Int(vec![42]));
        assert_eq!(reused, handle);
        assert_eq!(loader.arrays.borrow()[reused].len(), 1);
    }
}
