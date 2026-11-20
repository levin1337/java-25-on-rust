# JVM25-Rust Minecraft bootstrap state
Goal: full Minecraft 26.3 CLIENT + Fabric on Rust JVM, JIT enabled, no crashes.
(Leaf/Paper server runs are used as a bootstrap stress-test, not the goal.)

## JIT progress (opto/C2) — production enabled
- C2 enabled in tiered pipeline; invocation-entry compiles work for all
  supported signatures (int/long/float/double/ref/array params).
- OSR works end to end: backedge tick -> OsrFrame capture -> C2
  compile_osr -> OSR nmethod installed at osr_bci -> interpreter enters it
  via jitruntime::invoke_osr (marshal buffer in rcx). Verified by
  OsrSmoke.java (50M-iter single-call loop, correct result) and C2Catch
  OSR nmethods in production.
- C1 rejects is_osr tasks (no OSR entry support); interpreter requests
  max_osr_level=FullOptimization.
- Typed catches work: one IfException token per covering handler
  (catch site + table priority); codegen emits an ordered
  ExcCheck->InstanceOf dispatch chain with rethrow-on-miss.
  Verified by e2e_typed_catch and C2Catch.java in production.
- OSR supports exception handlers: restrict_to_osr keeps handler
  blocks reachable via exception edges and remaps exc preds.
  Verified by e2e_osr_try_catch + C2Catch OSR nmethods in production.
- invokedynamic works: interpreter dispatch extracted to
  interpreter/invokeDynamic.rs (shared); C2 resolves InvokeDynamic
  CP entries to callsite sites and routes Call nodes to
  RuntimeStub::InvokeDynamic (CP index via stub opcode slot).
  Verified by C2Indy.java in production — concat (string concat
  indy), apply (lambda metafactory indy), main all C2-installed
  with correct results.
- FIXED (f85c2e6): OSR installs no longer publish their entry to
  methodCounters::mark_compiled — regular invocations were entering
  OSR code with the wrong ABI (rcx=marshal buf vs Java args) and
  segfaulting on the first OsrArg load. Repro: hot
  String.toLowerCase -> C2 OSR of StringLatin1.toLowerCase crashed
  at [encoded-ref+8]; NettyProbe (PlatformDependent clinit) crashed
  the same way. VEH crash handler now prints fault RIP -> nmethod.
- Known C2 gaps -> interpreter fallback: precise deopt.
  GC barriers are N/A in this design (stores go through Rust stubs;
  safety via gc_extra_roots/frame_roots conservative scan).
- Tests: 104/104 opto, 35/35 jitruntime, 25/25 compiler green.

## Build/run
- `cargo build --release` -> `target/release/jvm25-rust.exe`.
- Smoke tests: `target/release/jvm25-rust.exe --modules "<jdk>/lib/modules" -cp . <Test>` (JDK: C:\Program Files\Eclipse Adoptium\jdk-25.0.4.101-hotspot).
- MC/Leaf run: leaf-runner crate in the Leaf workspace
  (Desktop\Leaf-ver-1.21.11\crates\leaf-runner, target/debug/leaf-runner.exe)
  with `--jvm <path-to-jvm25-rust.exe> --main org.bukkit.craftbukkit.Main
  --cp @leaf-server/build/runtime-classpath.txt -- nogui`, cwd=run/.
  Or `-jar leaf-server/build/libs/leaf-*.jar`.
- Kill stale: `taskkill //F //IM jvm25-rust.exe` before rebuild (exe lock).

## FIXED this session (cont.)
- Hidden-class nest hosts: `defineClass0` now reads the lookup `Class`
  mirror + flags; `NESTMATE_CLASS` (0x1) records a `nest_host_overrides`
  entry (host = lookup class's nest host). `getNestHost0`,
  `Reflection.areNestMates` and linkResolver's `nestmates` all consult
  the override. Verified: direct `LambdaMetafactory.metafactory` call
  spins `MiniMeta$$Lambda`, private ctor access passes, `META=ok`
  (was LambdaConversionException "member is private"). BmhProbe
  (bindTo/insertArguments/lambda indy/method ref) all pass.

## Diagnostics
- VEH crash handler (always on, Windows): on AV prints
  `[crash] AV rip=.. rw=.. -> nmethod holder.name(sig)` and writes
  D:\jvm25-rust\crash.log.
- `JVM25_NO_C2=1` disables C2 registration (C1-only diagnosis).
- `JVM25_DUMP_C2=<methodName>` hex-dumps emitted C2 code bytes.
- `JVM25_TRACE` unsafe-null reads print native name + `[unsafe-spin]`
  stack dump after 30 repeats of one offset.
- `JVM25_TRACE_WIN=1` logs every GetFileAttributesEx0 path (+MISSING).
- `JVM25_TRACE_COMPILE`, `JVM25_JIT_DEBUG`, `JVM25_JIT_ARGS`,
  `JVM25_DUMP_JIT`/`JVM25_DUMP_LIR` (C1), `JVM25_NO_JIT`,
  `JVM25_C1_THRESHOLD`, `JVM25_JIT_SKIP`, `JVM25_STACK_MB`,
  `JVM25_TRACE_EXC`, `JVM25_DBG_EXC`, `JVM25_DBG_INDY`,
  `JVM25_DBG_PHASES`, `JVM25_TRACE_DEFINE` (defineClass0 flags/lookup).

## MC bootstrap status (Leaf 1.21.11 server jar — stress test only;
real goal = Minecraft 26.3 client + Fabric)
- Previous blocker `areturn`+`Int` in JDK proxies did NOT reproduce in
  isolated tests (ProxyStress: generics/default/void/arrays all pass;
  real JNA Native.load works). Context-dependent (Knot/Fabric+Mixin)
  or was actually the OSR-dispatch segfault below.
- Latest full run (jvm-trace5.log, older binary) died in Netty
  PlatformDependent.<clinit> -> log4j -> OsgiServiceLocator class
  loading with `Missing instance field memberLjava/lang/invoke/
  MemberName;` — that run also had JIT on; the actual crash on the
  current build was the OSR-dispatch SIGSEGV (f85c2e6), now fixed.
- NettyProbe (Class.forName PlatformDependent) on current build:
  JVM25_NO_JIT=1 completes `loaded=io.netty.util.internal.
  PlatformDependent` in ~18s. With JIT: no crash in 500s but clinit
  still running at timeout — interpreted clinit is just very slow
  (~1.5s per jar open x ~160 jars + heavy MH/LambdaForm work).
- Pending other bugs (from earlier runs): coerce_value->Empty,
  Log4j newInstance, JNA field-order, OSHI, javac arg0 field.

## NEXT
- Leaf server run = stress test only. For the real client goal the
  heavy remaining subsystems are FFM/libffi (LWJGL needs it), OpenAL,
  GLFW native loading — audit which of the ~77 missing natives block
  the client first.
- Re-run full Leaf bootstrap (leaf-runner) on the fixed binary when
  convenient; watch for the next blocker past PlatformDependent clinit.
- If clinit speed blocks iteration, consider -XX:TieredStopAtLevel
  equivalent or raising JIT thresholds for bootstrap.
- Remaining C2 gap: precise deopt. Then jvm.dll exports for java.exe.
