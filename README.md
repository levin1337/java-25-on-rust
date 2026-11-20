<div align="center">

# ⚡ JVM 25 on Rust

**Полный перенос Java Virtual Machine из OpenJDK 25 (HotSpot) на Rust**

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Java 25](https://img.shields.io/badge/Java-25-orange?style=for-the-badge&logo=openjdk&logoColor=white)](https://openjdk.org)
[![Status](https://img.shields.io/badge/status-work%20in%20progress-yellow?style=for-the-badge)]()

<p>
  <img src="https://img.shields.io/badge/HotSpot-C%2B%2B%20%E2%86%92%20Rust-blueviolet?style=for-the-badge" alt="port">
  <img src="https://img.shields.io/badge/goal-Minecraft-green?style=for-the-badge" alt="goal">
</p>

> 🚧 **Проект в активной разработке.** Структура `src/share/` повторяет HotSpot 25 **1:1** — все 24 директории `share/` присутствуют и наполняются параллельно командой сабагентов.

</div>

---

## 🎯 Цель проекта

Переписать **OpenJDK HotSpot JVM (Java 25)** с C++ на **Rust**, сохранив:

- 🏗️ **Полную структуру оригинальной JVM** — каждый модуль `share/` из `srcJVMc++` (C++-референс OpenJDK 25 в репозитории) имеет свой аналог в `src/share/`
- ⚡ **Максимальную производительность** — чистый оптимизированный код, arena-аллокации, JIT C1+C2
- ✅ **Совместимость** — запуск любых Java-приложений, включая тяжёлые, цель — **Minecraft без ошибок**
- 🧪 **Полный тестовый прогон** — все тесты JVM должны проходить на Rust-версии

---

## 🗺️ Структура `src/share/` — 1:1 с HotSpot

```
src/share/
├── adlc/           🔧 Architecture Description Language Compiler
├── asm/            🧩 Assembler: register/reloc/codeBuffer/x86_64/MacroAssembler
├── c1/             ⚙️  Быстрый JIT-компилятор (C1): HIR/LIR, x64 backend, runtime-стабы
├── cds/            📀 Class Data Sharing: archive/filemap/metaspaceShared (реальный формат)
├── ci/             🔗 Compiler Interface: ciMethod/ciType/ciField/ciEnv/ciLiveness
├── classfile/      📦 Парсер .class, ClassLoader, JImage, верификатор, attributes
├── code/           💾 CodeCache, nmethod, PcDesc, scopeDesc, vtable stubs, адаптеры
├── compiler/       🧠 CompileBroker, CompileQueue, CompilationPolicy
├── gc/             🗑️  Serial, Parallel, G1 (~6900 строк), ZGC (18 модулей),
│                   Shenandoah, Epsilon — полные структурные порты
├── include/        🔌 JNI/JVM FFI-типы, JVM_* экспорты, JMM, CDS-структуры
├── interpreter/    🏃 Байткод-интерпретатор, шаблоны, rewriter, zero/
├── jfr/            📹 Java Flight Recorder: writers/recorder/metadata/periodic
├── jitruntime/     🌉 Мост интерпретатор↔JIT, executable stubs, frame metadata
├── jvmci/          ☕ JVMCI интерфейс (Graal)
├── libadt/         🧰 Arena/Dict/VectorSet — resource management HotSpot
├── logging/        📝 HotSpot Logging (-Xlog): конфигурация, стримы, dcmd
├── memory/         🧠 Universe, heap, arena, управление памятью
├── metaprogramming/ ⚙️  Compile-time трейты (EnableIf, PrimitiveCast)
├── nmt/            📊 Native Memory Tracking
├── oops/           🧬 Object-модель: Klass, markWord, compressedOops, metaspace
├── opto/           🔥 Оптимизирующий JIT (C2): типы, ноды, GVN, liveness,
│                   scheduler, x64 codegen
├── platform/       🖥️  Windows: cpu_features (cpuid/AVX-512), file_io, threads
├── precompiled/    📦 Precompiled headers аналоги
├── prims/          🔩 JVM Intrinsics / runtime-рутины (~700 нативов java.base)
├── runtime/        🔧 Runtime, Threads, deopt, frame, JNI API
├── sanitizers/     🧹 ASan/UBSan-аннотации
├── services/       🔌 Attach Listener (V1/V2), dcmd framework, writeableFlags, HPROF
└── utilities/      🛠️  growableArray, hashtable, utf8, decoder, zip, события
```

Референс C++-исходников: `srcJVMc++/share/` (полное дерево OpenJDK 25 HotSpot).

---

## 🚀 Быстрый старт

### Требования

- Rust (edition 2024, свежий stable)
- JDK с образом модулей (`jimage`) для запуска программ

### Сборка и запуск

```bash
cargo build --release

./target/release/jvm25-rust --modules /path/to/lib/modules -cp classes Main
./target/release/jvm25-rust -jar app.jar
JVM25_NO_JIT=1 ./target/release/jvm25-rust --modules ... -cp classes Main  # без JIT
```

### Тесты

```bash
cargo test                 # unit + integration
cargo test --lib opto      # C2-модуль
cargo test --test asm_x86  # ассемблерный слой
# JIT e2e: tests/jit/run-jit-tests.ps1 (JitBench: interpreter vs JIT diff)
```

Отладочные флаги: `JVM25_TRACE`, `JVM25_DUMP`, `JVM25_BLOCKTRACE`, `JVM25_TRACE_COMPILE`, `JVM25_C1_THRESHOLD`, `JVM25_NO_JIT`, `JVM25_NO_C2`, `JVM25_PROFILE_NATIVE`, `JVM25_MEMSTAT`, `JVM25_STACK_MB`, `JVM25_NO_VERIFY`, `JVM25_DUMP_JIT`, `JVM25_DUMP_LIR`, `JVM25_DUMP_C2`, `JVM25_TRACE_WIN`, `JVM25_TRACE_EXC`, `JVM25_DBG_INDY`, `JVM25_DBG_PHASES`. На Windows при SIGSEGV печатается `[crash]` с RIP→nmethod-маппингом.

---

## 📈 Статус разработки

Актуально на 2026-11-20. **~202k строк Rust** в `src/share/` (было ~73k до волны портирования GC/JIT/инфраструктуры).

### ✅ Работает и проверено тестами

| Подсистема | Что сделано |
|---|---|
| **Classfile / ClassLoader** | Полный парсер class-файлов Java 25: все JVMS-атрибуты структурно (`attributes.rs` ~1250 строк — BootstrapMethods/indy/condy, Signature grammar, Annotations, Module, Record, StackMapTable-декодер), верификатор с JVMS-проверками, загрузка из директорий/JAR/**JImage `lib/modules`**. |
| **Иерархия и линковка** | Резолюция методов/полей, itable/vtable, default-методы, `<clinit>` порядок JVMS 5.5, реальные Java-исключения связывания. **Hidden classes**: `defineClass0` обрабатывает `NESTMATE_CLASS`/lookup-миррор — nest host для классов, которые спинит `LambdaMetafactory`/`LambdaForm`; `getNestHost0`/`areNestMates`/`getNestMembers0` учитывают VM-назначенный хост. |
| **Рефлексия / indy** | `java.lang.reflect`, `LambdaMetafactory` (включая прямые вызовы `metafactory` — спиннинг hidden-классов и приватный доступ), `StringConcatFactory`, generic bootstrap, condy, `BoundMethodHandle` species через `lambda_form_invoke`. |
| **Потоки / синхронизация** | Java-потоки на OS-потоках, мониторы, wait/notify, park/unpark, shutdown hooks. |
| **Память и GC** | Рабочий mark-sweep с safepoints и `gc_extra_roots`; reference processing починен (pending-list — GC-корень). |
| **Файлы / NIO / Сеть / ZIP** | FileDispatcher (pread/pwrite/mmap/locks), Selector/WEPoll, TCP/UDP, `java.util.zip` Inflater — всё проверено e2e. |
| **Нативные методы** | **~700/777 нативов java.base** (аудит против src.zip JDK 25). Остаток ~77: FFM/libffi, Windows ACL, host-locale — capability-gated. |
| **C1 JIT** | **Работает e2e**: heap-стабы (getfield/putfield/array/new/cast/instanceof/athrow с семантикой интерпретатора), JIT→JIT прямые вызовы, JIT→интерпретатор через stub, non-leaf unwind, рекурсия, проброс исключений, deopt/OSR-fallback. GC-безопасность JIT-фреймов через `FrameRoots`+`gc_extra_roots`. |
| **C2 JIT (opto)** | **В проде tiered-пайплайне**: типы/ноды/GVN/scheduler/liveness/x64-эмиттер; все сигнатуры (примитивы, float/double, ref/array), поля, вызовы, свитчи, массивы; **typed catches** (упорядоченная цепочка ExcCheck→InstanceOf→rethrow), **OSR** (marshal-буфер в rcx, `invoke_osr`), **`invokedynamic`** (общий с интерпретатором callsite-resolution стаб). 104/104 тестов. |
| **asm/ (MacroAssembler)** | Полный порт: register/reloc/codeBuffer/x86_64 кодировки/MacroAssembler с oopmaps, safepoint_poll, G1 write barriers. 12/12 тестов. |
| **code/ + compiler/** | CompileBroker с реальными C1/C2 очередями и worker-потоками, policy, nmethod с PcDesc/DeoptLiveMap, CodeCache с адаптерами и vtable-стабами (реальный x64-код). |
| **ci/** | Compiler Interface: CiEnv/CiInstanceKlass/CiMethod/CiField/CIType/ciLiveness (полный MethodLiveness). |
| **GC-коллекторы** | **Полные структурные порты**: G1 (~6900 строк: card table/set, remsets, SATB, par-scan, policy, young gen sizer — 39/39 тестов), ZGC (18 модулей: цветные указатели, forwarding, relocation, barriers — 22/22), Shenandoah (SATB, mark bitmap, pacer — 59/59), Epsilon (эластичная TLAB). |
| **cds/** | Реальный формат CDS-архива: FileMapInfo/mmap, CRC, VirtualSpaceList, dump/load, `-Xshare:*`. 25/25 тестов. |
| **logging/** | HotSpot `-Xlog` 1:1: конфигурация, tag sets, stream, decorations, `jcmd VM.log`. 46 тестов. |
| **services/ (без JVMTI)** | Attach Listener V1/V2 протокол (pipe), dcmd framework, реальные команды (VM.*, GC.run, Thread.print), writeableFlags, HPROF-дамп. |
| **platform/** | Windows: cpuid-детекция ISA (SSE→AVX-512, XCR0), cache line sizes, file_io, thread sync, Park-Miller random. |
| **include/libadt/metaprogramming** | JNI/JVM FFI-типы (~150 `JVM_*` экспортов), Arena/Dict/VectorSet, compile-time трейты. |
| **jfr/** | Ядро JFR: writers (varint/strings/chunk FLR), checkpoint manager, recorder service, metadata — каркас + ключевые события. |
| **jvmci/** | JVMCI интерфейс для Graal-интеграции (структурный порт). |
| **adlc/ + utilities/** | ADLC parser/генератор, утилиты HotSpot (hashtable, utf8, decoder, zip, events). |

### 🟡 В работе

| Подсистема | Состояние |
|---|---|
| **C2 покрытие** | Основное покрыто и в проде (см. выше). Осталось: точная деоптимизация (сейчас fallback = переисполнение метода), интеграция с `asm/` MacroAssembler для output-слоя. |
| **Деоптимизация** | Fallback = переисполнение метода; нужен точный frame-state restore с BCI. |
| **GC-интеграция** | Коллекторы портированы структурно; write-барьеры в этом дизайне не нужны (сторы идут через Rust-стабы, безопасность — `gc_extra_roots`/консервативный скан фреймов). Рабочий путь — mark-sweep. |
| **jvm.dll** | Экспорты `JNI_CreateJavaVM`/`JVM_*` для запуска штатным `java.exe` — в работе. |
| **interpreter/** | Доводка `templateInterpreter*`, `zero/`, `bytecodeUtils`, `rewriter` до полной структуры. |
| **JFR события** | Каркас готов; генерация всех ~171 событий из events.xml и emit-точки — впереди. |
| **Minecraft** | Leaf/Paper server 1.21.11 (цель 26.3): bootstrap доходит до Netty `PlatformDependent.<clinit>`; критические баги исправлены — OSR-dispatch segfault (`f85c2e6`), hidden-class nest hosts (`32e8c95`). Клиент — упирается в скорость интерпретатора до завершения JIT. |

### 🔴 Осталось

1. **Стабилизация сборки** — параллельная работа агентов оставляет временные ошибки компиляции в WIP-областях.
2. **C2 codegen** — output-слой через новый `asm/` MacroAssembler; точная деоптимизация.
3. **Производительность JIT** — FrameRoots (консервативный скан) заменить точными oop-maps; интерпретатор всё ещё быстрее JIT на части нагрузок.
4. **jvm.dll + CDS-загрузка JDK-архивов**.
5. **FFM (libffi)**, Windows ACL, host-locale — остаток ~77 нативов.
6. **Minecraft client/server до конца** — bootstrap Leaf-сервера, затем Fabric/Knot и клиент.

### 📊 JIT-бенчмарк (JitBench scale=1, release)

| Режим | Время | Результат |
|---|---|---|
| Интерпретатор | ~25s | CHECKSUM `-6033222509371927279` |
| C1 JIT | ~64s (вкл. компиляцию ~150 методов) | ✅ **побайтово идентичен** |

Корректность JIT полная; скорость пока ниже интерпретатора — FrameRoots-скан на каждый stub-вызов доминирует. Ускорение ожидается после oop-maps и C2.

### 🎯 Текущая точка (2026-11-20)

- **C2 в проде**: invocation + OSR-компиляция, typed catches, `invokedynamic`, все сигнатуры — верифицировано smoke-тестами (`OsrSmoke`, `C2Catch`, `C2Indy`, `C2Fields`, `ProxyStress`, `ToLowerCrash`).
- **Критические баги MC-path закрыты**: OSR nmethod публиковался в обычный dispatch → segfault на первом `OsrArg`; hidden classes не имели nest host → `LambdaMetafactory` падал.
- **Диагности**: Windows VEH crash handler (RIP→nmethod), `JVM25_NO_C2`, `JVM25_DUMP_C2`, `JVM25_TRACE_DEFINE`, `JVM25_TRACE_WIN`, `JVM25_TRACE` unsafe-spin stack dumps.
- **Следующий шаг**: полный прогон Leaf bootstrap на текущем бинаре — смотреть, что идёт после `PlatformDependent.<clinit>`; затем либо `jvm.dll`-экспорты (`JNI_CreateJavaVM` уже есть, нужны `JVM_*`), либо точная деоптимизация в C2.

---

## 🤝 Contributing

Проект переносится поэтапно, модуль за модулем — строго по структуре HotSpot. Если хочешь помочь:

1. 🍴 Сделай fork
2. 🌿 Создай ветку `feature/<module-name>`
3. 🔀 Открой Pull Request
