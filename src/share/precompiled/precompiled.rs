pub const DISABLE_PRECOMPILED_HEADERS: &str = "--disable-precompiled-headers";

pub const PRECOMPILED_HEADERS: &[&str] = &[
    "classfile/classLoaderData.hpp",
    "classfile/javaClasses.hpp",
    "classfile/systemDictionary.hpp",
    "gc/shared/collectedHeap.hpp",
    "gc/shared/gcCause.hpp",
    "logging/log.hpp",
    "memory/allocation.hpp",
    "memory/iterator.hpp",
    "memory/memRegion.hpp",
    "memory/resourceArea.hpp",
    "memory/universe.hpp",
    "nmt/memTracker.hpp",
    "oops/instanceKlass.hpp",
    "oops/klass.hpp",
    "oops/method.hpp",
    "oops/objArrayKlass.hpp",
    "oops/objArrayOop.hpp",
    "oops/oop.hpp",
    "oops/oopsHierarchy.hpp",
    "runtime/atomic.hpp",
    "runtime/globals.hpp",
    "runtime/handles.hpp",
    "runtime/javaThread.hpp",
    "runtime/mutex.hpp",
    "runtime/orderAccess.hpp",
    "runtime/os.hpp",
    "runtime/timer.hpp",
    "utilities/align.hpp",
    "utilities/bitMap.hpp",
    "utilities/copy.hpp",
    "utilities/debug.hpp",
    "utilities/exceptions.hpp",
    "utilities/globalDefinitions.hpp",
    "utilities/growableArray.hpp",
    "utilities/macros.hpp",
    "utilities/ostream.hpp",
    "utilities/ticks.hpp",
];

pub const PRECOMPILED_INLINE_HEADERS: &[&str] = &[
    "memory/allocation.inline.hpp",
    "oops/access.inline.hpp",
    "oops/oop.inline.hpp",
    "runtime/handles.inline.hpp",
];

pub fn is_precompiled_header(path: &str) -> bool {
    PRECOMPILED_HEADERS.contains(&path)
}
