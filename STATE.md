# JVM25-Rust Minecraft bootstrap state
Goal: full Minecraft 26.3 + Fabric on Rust JVM, JIT enabled, no crashes.

## JIT progress (opto/C2)
- C2 enabled in production tiered pipeline; invocation-entry compiles work
  for all supported signatures (int/long/float/double/ref/array params).
- OSR works end to end: backedge tick -> OsrFrame capture -> C2
  compile_osr -> OSR nmethod installed at osr_bci -> interpreter enters it
  via jitruntime::invoke_osr (marshal buffer in rcx). Verified by
  OsrSmoke.java (50M-iter single-call loop, correct result).
- C1 rejects is_osr tasks (no OSR entry support); interpreter requests
  max_osr_level=FullOptimization.
- Typed catches work: one IfException token per covering handler
  (catch site + table priority); codegen emits an ordered
  ExcCheck->InstanceOf dispatch chain with rethrow-on-miss.
  Verified by e2e_typed_catch (match / non-match propagate /
  typed-fail -> catch-all).
- Known C2 gaps -> interpreter fallback: invokedynamic, precise deopt,
  GC barriers in JIT code (conservative frame scan only).

## Build/run
- `cargo build --release` -> `target/release/jvm25-rust.exe`; MUST `cp` to `release/jvm25-rust.exe` (ps1 uses release/).
- Run: `powershell -File tests/run-mc263.ps1` (writes tests/mc-run.out). Env flags prefix it.
- Kill stale: `taskkill //F //IM jvm25-rust.exe` before rebuild (exe lock).

## FIXED this session
- C1 `const_arith` lz/rz swap (x+0/x-0 ->0) -> JIT NPE in voxel-shapes GONE.
- `gc_extra_roots` TOCTOU + reentrant borrow_mut -> `drop(roots)` after remove (6 sites).
- `ScopedMemoryAccess.closeScope0` -> no-op.
- VarHandle `set*` void phantom-Null push -> interpreter+nativeLookup guard on `)V`.
- record equals/hashCode, Object.hashCode invokespecial, hidden-class dedup (earlier).

## CURRENT BLOCKER: Invalid return operand (areturn+Int in JDK proxy)
- `[ret-operand] owner=jdk/proxy2/$Proxy23 method=IsProcessorFeaturePresent desc=(I)Z opcode=0xb0(areturn) pc=31 value=Int`
- `[ret-operand] owner=jdk/proxy2/$Proxy24 method=RegOpenKeyEx desc=(...)I opcode=0xb0 pc=52 value=Int`
- JNA binds kernel32/advapi32 via java.lang.reflect.Proxy -> ProxyGenerator emits areturn instead of ireturn.

## KEY FINDINGS (verified by isolated tests on OUR jvm)
- Minimal PX proxy `(I)Z`/`(...)I` -> WORKS (call1=true, call2=42).
- Real JNA `Native.load("kernel32",Kernel32.class)` + `IsProcessorFeaturePresent` -> WORKS (=false).
- `Kernel32.getDeclaredMethod("IsProcessorFeaturePresent").getReturnType()` -> `:Z`, isPrimitive=true, getName="boolean", ==boolean.class=true, equals=true. paramTypes `:I`==int.class=true. ALL CORRECT.
- => getReturnType/isPrimitive/==/getName ALL correct. Proxy generation produces ireturn in isolation.
- => MC areturn is CONTEXT-DEPENDENT (Knot/Fabric classloader + Mixin) OR stack/pc corruption, NOT a general proxy bug.

## Diagnostics added
- `[ret-operand]` in bytecodeInterpreter xreturn prints owner/method/desc/opcode/pc/value-variant/byte-tail.
- `JVM25_TRACE_ISPRIM=1` logs isPrimitive on `:`-classes.
- `JVM25_DUMP_PROXY=1` dumps `jdk/proxy*` class bytes -> `D:/jvm25-rust/proxy_*.class` (added in defineClassNatives::define_from_window; earlier define_class_bytes attempt missed - proxies use define_class0/2 path).

## NEXT
- Disassemble dumped proxy_*.class with `javap -c` (JDK21: /c/Program Files/Java/jdk-21.0.12/bin/javap.exe) to see if areturn is real bytecode or interpreter misdecode.
- If real areturn: returnType.isPrimitive() was false at gen time in MC context -> find why (Mixin-transformed interface? different Method resolution? classloader?).
- If ireturn but interpreter reads 0xb0: pc/stack corruption bug.
- Pending other bugs: coerce_value->Empty, Log4j newInstance, JNA field-order, OSHI, javac arg0 field.

## C2 PRODUCTION MILESTONE (commit e2ca60a)
- C2 now reachable in tiered pipeline: flag on, C2Compiler registered, policy falls back to raw counters w/o MDO, hotness ticks re-fire every threshold.
- Fixed: phi width classification (i64/f64 phis were 32-bit -> loop corruption); ldc/ldc2_w constants now resolved from CP (were ConI(0) -> i*3=0 miscompile); builder-unsupported bytecodes now reject to interpreter; permanent per-method compile failures recorded (no retry storms).
- Verified on real JVM: C2Smoke burn(J)J + sumLoop(J)J C2-compiled, correct output.
- C2 limits: no OSR, no typed catches, array/ref signatures unsupported, fields/methods ctx maps still empty in production (field/invoke methods -> interpreter fallback).
