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

Отладочные флаги: `JVM25_TRACE`, `JVM25_DUMP`, `JVM25_BLOCKTRACE`, `JVM25_TRACE_COMPILE`, `JVM25_C1_THRESHOLD`, `JVM25_NO_JIT`, `JVM25_PROFILE_NATIVE`, `JVM25_MEMSTAT`, `JVM25_STACK_MB`, `JVM25_NO_VERIFY`, `JVM25_DUMP_JIT`, `JVM25_DUMP_LIR`.

---

## 📈 Статус разработки

Актуально на 2026-11-15. **~103k+ строк Rust** в `src/share/` (было ~73k до волны портирования GC/JIT/инфраструктуры).

### ✅ Работает и проверено тестами

| Подсистема | Что сделано |
|---|---|
| **Classfile / ClassLoader** | Полный парсер class-файлов Java 25: все JVMS-атрибуты структурно (`attributes.rs` ~1250 строк — BootstrapMethods/indy/condy, Signature grammar, Annotations, Module, Record, StackMapTable-декодер), верификатор с JVMS-проверками, загрузка из директорий/JAR/**JImage `lib/modules`**. |
| **Иерархия и линковка** | Резолюция методов/полей, itable/vtable, default-методы, `<clinit>` порядок JVMS 5.5, реальные Java-исключения связывания. |
| **Рефлексия / indy** | `java.lang.reflect`, `LambdaMetafactory`, `StringConcatFactory`, generic bootstrap, condy. |
| **Потоки / синхронизация** | Java-потоки на OS-потоках, мониторы, wait/notify, park/unpark, shutdown hooks. |
| **Память и GC** | Рабочий mark-sweep с safepoints и `gc_extra_roots`; reference processing починен (pending-list — GC-корень). |
| **Файлы / NIO / Сеть / ZIP** | FileDispatcher (pread/pwrite/mmap/locks), Selector/WEPoll, TCP/UDP, `java.util.zip` Inflater — всё проверено e2e. |
| **Нативные методы** | **~700/777 нативов java.base** (аудит против src.zip JDK 25). Остаток ~77: FFM/libffi, Windows ACL, host-locale — capability-gated. |
| **C1 JIT** | **Работает e2e**: heap-стабы (getfield/putfield/array/new/cast/instanceof/athrow с семантикой интерпретатора), JIT→JIT прямые вызовы, JIT→интерпретатор через stub, non-leaf unwind, рекурсия, проброс исключений, deopt/OSR-fallback. GC-безопасность JIT-фреймов через `FrameRoots`+`gc_extra_roots`. |
| **C2 JIT (opto)** | Типы/ноды/GVN/scheduler/liveness+interference/x64-эмиттер; рабочее подмножество: примитивная арифметика, idiv с исключениями, массивы через стабы, float/double SSE. 45/45 тестов. |
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
| **C2 покрытие** | Расширение опкодов: fields/calls/ref-arrays в работе; полный regalloc (Chaitin), интеграция с `asm/` MacroAssembler. |
| **Настоящий OSR** | Сейчас best-effort whole-method вызов; нужен перенос locals/stack в OSR-entry. |
| **Деоптимизация** | Fallback = переисполнение метода; нужен точный frame-state restore с BCI. |
| **GC-интеграция** | Коллекторы портированы структурно, но выбор коллектора/барьеры в JIT-коде не подключены; рабочий путь — mark-sweep. |
| **jvm.dll** | Экспорты `JNI_CreateJavaVM`/`JVM_*` для запуска штатным `java.exe` — в работе. |
| **interpreter/** | Доводка `templateInterpreter*`, `zero/`, `bytecodeUtils`, `rewriter` до полной структуры. |
| **JFR события** | Каркас готов; генерация всех ~171 событий из events.xml и emit-точки — впереди. |
| **Minecraft** | Fabric-сервер 1.16.1 проходит bootstrap; client — упирается в скорость интерпретатора до завершения JIT. |

### 🔴 Осталось

1. **Стабилизация сборки** — параллельная работа агентов оставляет временные ошибки компиляции в WIP-областях.
2. **C2 codegen** — matcher/regalloc/output через новый `asm/` слой.
3. **Производительность JIT** — FrameRoots (консервативный скан) заменить точными oop-maps; сейчас JIT scale=1 медленнее интерпретатора (~63s vs ~25s на JitBench).
4. **jvm.dll + CDS-загрузка JDK-архивов**.
5. **FFM (libffi)**, Windows ACL, host-locale — остаток ~77 нативов.
6. **Minecraft client** — финальная цель.

### 📊 JIT-бенчмарк (JitBench scale=1, release)

| Режим | Время | Результат |
|---|---|---|
| Интерпретатор | ~25s | CHECKSUM `-6033222509371927279` |
| C1 JIT | ~64s (вкл. компиляцию ~150 методов) | ✅ **побайтово идентичен** |

Корректность JIT полная; скорость пока ниже интерпретатора — FrameRoots-скан на каждый stub-вызов доминирует. Ускорение ожидается после oop-maps и C2.

---

## 🤝 Contributing

Проект переносится поэтапно, модуль за модулем — строго по структуре HotSpot. Если хочешь помочь:

1. 🍴 Сделай fork
2. 🌿 Создай ветку `feature/<module-name>`
3. 🔀 Открой Pull Request
