<div align="center">

# ⚡ JVM 25 on Rust

**Полный перенос Java Virtual Machine из OpenJDK 25 (HotSpot) на Rust**

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Java 25](https://img.shields.io/badge/Java-25-orange?style=for-the-badge&logo=openjdk&logoColor=white)](https://openjdk.org)
[![Status](https://img.shields.io/badge/status-work%20in%20progress-yellow?style=for-the-badge)]()
[![License](https://img.shields.io/badge/license-proprietary-red?style=for-the-badge)]()

<p>
  <img src="https://img.shields.io/badge/HotSpot-C%2B%2B%20%E2%86%92%20Rust-blueviolet?style=for-the-badge" alt="port">
  <img src="https://img.shields.io/badge/goal-Minecraft-green?style=for-the-badge" alt="goal">
</p>

> 🚧 **Проект в активной разработке.** Это амбициозный WIP-порт: структура `share/` повторяет оригинальный HotSpot 1:1, но не все подсистемы завершены.

</div>

---

## 🎯 Цель проекта

Переписать **OpenJDK HotSpot JVM (Java 25)** с C++ на **Rust**, сохранив:

- 🏗️ **Полную структуру оригинальной JVM** — каждый модуль HotSpot (`classfile`, `interpreter`, `opto`, `gc`, `runtime`…) имеет свой аналог в `src/share/`
- ⚡ **Максимальную производительность** — чистый, оптимизированный код без лишних аллокаций (arena-аллокатор `bumpalo`, zero-cost абстракции)
- ✅ **Совместимость** — запуск любых Java-приложений, включая тяжёлые, цель — **Minecraft без ошибок**
- 🧪 **Полный тестовый прогон** — все тесты JVM должны проходить на Rust-версии

---

## 🗺️ Карта переноса

Структура `src/share/` зеркалит `src/hotspot/share/` из OpenJDK:

```
src/share/
├── classfile/      📦 Парсер .class, ClassLoader, JImage, SymbolTable,
│                   SystemDictionary, верификатор байткода
├── interpreter/    🏃 Байткод-интерпретатор, шаблоны, LinkResolver
├── opto/           🔥 Оптимизирующий JIT-компилятор (C2)
├── c1/             ⚙️  Быстрый JIT-компилятор (C1)
├── compiler/       🧠 CompileBroker, CompileQueue, CompilerOracle
├── code/           💾 CodeCache, CodeBlob, nmethod, PcDesc
├── gc/             🗑️  Сборщики мусора: Serial, Parallel, G1, ZGC, Shenandoah, Epsilon
├── runtime/        🔧 Runtime, Threads, JNI, Signals, JNI-окружение
├── oops/           🧬 Object-модель: Klass, oop, метаданные
├── memory/         🧠 Universe, управление памятью, oopFactory
├── ci/             🔗 Compiler Interface (ciMethod, ciType, …)
├── cds/            📀 Class Data Sharing, metaspace shared
├── code/ + services/  JVMTI, Serviceability Agent
├── logging/        📝 HotSpot Logging (Log, LogConfiguration, Tags)
├── nmt/            📊 Native Memory Tracking
├── prims/          🔩 JVM Intrinsics / runtime-рутины
├── utilities/      🛠️  Утилиты: growableArray, stringStream, …
└── services/       🔌 JVMTI / Serviceability
```

---

## 🚀 Быстрый старт

### Требования

- [Rust](https://rustup.rs) (edition 2024, свежий stable)
- JDK / JRE с образом модулей (`jimage`) для запуска программ

### Сборка

```bash
cargo build --release
```

### Запуск Java-приложения

```bash
# Запуск main-класса
./target/release/jvm25-rust -cp path/to/classes com.example.Main arg1 arg2

# Запуск JAR-файла
./target/release/jvm25-rust -jar app.jar

# С образом модулей JDK (jimage)
./target/release/jvm25-rust --modules /path/to/lib/modules -cp classes Main

# Передача системных свойств
./target/release/jvm25-rust -Dfoo=bar -cp classes Main
```

### Дебаг одного .class файла

```bash
./target/release/jvm25-rust path/to/Foo.class
# → Foo (major.minor) | N fields | M methods
```

### Тесты

```bash
cargo test
```

---

## 🧩 Стек технологий

| Компонент | Технология |
|---|---|
| Язык | **Rust** (edition 2024) |
| Аллокатор | `bumpalo` — arena-аллокации в стиле HotSpot |
| Загрузка нативных библиотек | `libloading` |
| JAR / ZIP | `zip` (deflate через `flate2-zlib-rs`) |
| Референс | OpenJDK `jdk25u` (HotSpot, C++) |

---

## 📈 Статус разработки

Актуально на 2026-11-15. ~73k строк Rust в `src/share/` (~200k LOC C++ HotSpot как референс).

### ✅ Работает и проверено тестами

| Подсистема | Что сделано |
|---|---|
| **Classfile / ClassLoader** | Парсер class-файлов Java 25 (constant pool, атрибуты, структурная верификация имён/флагов), загрузка из директорий, JAR и **реального `lib/modules` JDK** через JImage (декомпрессия, MUTF-8, модульный индекс, ошибки формата). Верификатор байткода подключён для классов вне boot-образа (`VerifyError`, отключается `JVM25_NO_VERIFY=1`). |
| **Иерархия и линковка** | Резолюция методов/полей, itable, интерфейсные default-методы, гетерогенные ошибки связывания (`LinkageErrorKind` → реальные Java-исключения), `ClassCircularityError`, порядок инициализации `<clinit>` по JVMS 5.5. |
| **Рефлексия** | `java.lang.Class`, `java.lang.reflect`: `getMethod`/`getField`/`getDeclared*`, `invoke`, конструкторы, массивы, `newInstance`, модификаторы (включая InnerClasses). Вывод `ReflTest` **побайтово совпадает с JDK 21**. |
| **invokedynamic** | `LambdaMetafactory` (лямбды, захват, method refs), `StringConcatFactory`, **generic bootstrap** через настоящие bootstrap-методы (`genericBootstrap.rs`), `condy` (ConstantDynamic) — кэши `indy_targets`/`condy_values`. |
| **Потоки и синхронизация** | Кооперативные Java-потоки поверх OS-потоков, мониторы, `wait`/`notify`/`notifyAll`, `park`/`unpark`, `sleep`, прерывания, shutdown hooks, ожидание non-daemon потоков после `main` (семантика JVM), `JVM25_STACK_MB` для стека. |
| **Память и GC** | Mark-and-sweep с safepoint'ами, обход корней (стеки кадров, статика, `gc_extra_roots`), free-slot reuse, pending-reference list для `Reference`. |
| **Файлы / NIO** | `FileDispatcherImpl`/`WindowsFileDispatcherImpl` полностью: `pread/pwrite`, `readv/writev`, `lock/release`, `map/unmap/transferTo/setDirect` с когерентной записью маппингов, `FileChannel.map`, `FileKey` через `GetFileInformationByHandle`, стандартные дескрипторы 0/1/2, неseekable pipes, `ProcessBuilder` с pipes. |
| **Сеть** | TCP-сокеты полный цикл (socket/bind/listen/connect/accept/read/write/shutdown/poll), эхо через Java-потоки, **Selector** (WEPoll + `poll0`), non-blocking каналы, UDP send/receive, `InetAddress`, `isReachable`, multicast-заглушки, `jdk.net` опции. Совместимость сигнатур JDK 21 и JDK 25. |
| **ZIP / Deflater** | `java.util.zip` + `Inflater`: все варианты `deflateBytesBuffer`/`deflateBufferBytes`/`deflateBufferBuffer`/`inflate*`, preset dictionary, `ZipFile`/`JarFile`, чтение JAR-манифестов и class-path. |
| **Нативные методы** | ~343 дескриптора в `nativeLookup` + специализированные модули (`prims/`, 27k строк). Аудит против JDK 21 `src.zip`: **499 объявлений, ~17 остатка** — почти все осознанные стабы. |
| **Class definition** | `ClassLoader.defineClass`, `defineClass2` (из `byte[]` и `ByteBuffer` — heap и direct), регистрация в loader, парсинг и linkage. |
| **MethodHandle / VarHandle** | `MethodHandleNatives`, `VarHandleNatives` (включая view-буферы), `Unsafe` (CAS, off-heap, array base/scale). |
| **JNI** | Загрузка DLL, `JNI_OnLoad`, `RegisterNatives`, вызовы x64 Windows, JNI references. Desktop-нативы (LWJGL) не проверены запуском. |
| **Bootstrap** | `System.initPhase*`, `platformProperties`/`vmProperties` позиционные массивы, charset, сигналы, `System.out/err` через `setOut0`, `SharedSecrets`, `Reference` init. |
| **Misc** | `Process`/`ProcessImpl`, `RandomAccessFile` (режимы JDK), `Shutdown.halt0`, `StackWalker` (частично, `stackwalk.rs`), `Module`/`defineModule0`, `LambdaProxyClassArchive`/`RawNativeLibraries` стабы, `jdkIntrinsics` (быстрые JDK-хелперы), negative-result кэш и native-профайлер (`JVM25_PROFILE_NATIVE`). |

### 🟡 В работе

| Подсистема | Состояние |
|---|---|
| **Tiered compilation** | Счётчики HotSpot-style подключены: `invocation_tick` на каждый вызов, `backedge_tick` на обратных ветвях, пороги (`JVM25_C1_THRESHOLD`, по умолчанию 1000/4000), hot-set, `[compile-request:*]` лог (`JVM25_TRACE_COMPILE`). **Бэкенд JIT ещё не подключён** — скомпилированный код не вызывается. |
| **C1 / opto / compiler / code** | Структуры портированы (~12k строк): GraphBuilder, IR, Instruction, Optimizer, x64 `CodeBuffer` (RWX через VirtualProtect), node/GVN, CompileBroker/Queue/Oracle, CodeCache/nmethod/PcDesc/scopeDesc. Генерация машинного кода и связь с интерпретатором — впереди. |
| **GC-коллекторы** | Каркасы Serial/Parallel/G1/Z/Shenandoah/Epsilon + barrier sets (~2.7k строк). Рабочий путь — единый mark-sweep в `memory/heap.rs`; выбор коллектора и write-барьеры не подключены. |
| **JNI desktop** | Механизм есть; `GetModule`→null, `IsVirtualThread`→false; LWJGL/Minecraft-нативы не прогонялись. |
| **StackWalker** | `stackwalk.rs` + `StackFrameInfo` классы в tests; `StackTraceElement.initStackTraceElement` (single) не реализован — нужен MemberName plumbing. |
| **CDS / NMT / Logging** | Модули есть (cds/, nmt/, logging/), совместимость архивов CDS и JFR не подтверждена. |
| **Minecraft** | Fabric 1.21.11 (KnotClassLoader) доходит до сканирования ресурсов (`JarFile.checkForSpecialAttributes`); процесс жив, но интерпретатор слишком медленный — часы на старте без JIT. Сервер 1.16.1 ранее проходил bootstrap. |

### 🔴 Осталось перенести / сделать

1. **JIT-машинный код** — главный блокер производительности: baseline C1 → OSR → C2, переходы interpreter↔compiled, deopt, safepoint'ы в скомпилированном коде. Без этого MC непригоден по скорости.
2. **GC-коллекторы** — реализация Serial/Parallel/G1/Z/Shenandoah/Epsilon поверх единого heap, card tables / write barriers, remembered sets.
3. **Reference processing** — Cleaner'ы/Phantom/Weak семантика, обработка pending list (сейчас список есть, обработчика нет → утечки direct-памяти).
4. **Windows async I/O** — `Iocp`, `WindowsAsynchronous{File,Socket,ServerSocket}ChannelImpl` (11 нативов; Netty обычно не требует, но `AsynchronousFileChannel` API сломан).
5. **Module graph** — `Module.addExports0/addReads0/...` сейчас no-op (Java-сторона сама ведёт состояние — работает, но не HotSpot-семантика).
6. **StackWalker полный** — `StackStreamFactory.callStackWalk/fetchStackFrames`, `initStackTraceElement`, MemberName для фреймов.
7. **jvm.dll** — сборка DLL с `JNI_CreateJavaVM`/`JNI_GetCreatedJavaVMs`/`JVM_*` экспортами для запуска штатным `java.exe`.
8. **CDS-архивы, JFR, полная диагностика** — модули-заготовки без верификации.
9. **Виртуальные потоки** — `VirtualThread` механика (JVMTI-хуки заглушены).
10. **Тестовое покрытие** — расширить сравнительные прогоны с HotSpot 25: `java -jar`, загрузчики, JNI, GC-политики, ошибки class-файлов.

### Скорость (текущее состояние)

- Bootstrap до `main`: ~6–14 с (JImage-сканы ServiceLoader'ов, charset).
- Интерпретатор: ~1–2M байткодов/с на простых методах; `Hot.sum` (2M итераций) ~секунды.
- Minecraft client: прогрессирует, но старт — часы; узкое место — интерпретация `JarFile.match` и классов Knot/Fabric.

### 🧪 Тесты

`cargo test` — unit-тесты зелёные. E2E (реальный JDK 21 `lib/modules`):

| Тест | Результат |
|---|---|
| Hello / JdkHello / ExcTest / Uncaught / Repeat / SyncTest / EdgeTest / Cf / Conc / Deep / LoadNet / ProcTest | ✅ |
| LambdaTest (лямбды, captures, method refs) | ✅ |
| ReflTest vs JDK | ✅ побайтово идентичен |
| ZipTest / ZipT / ZipOne / JarTest | ✅ |
| NioTest / SelOnly / MapTest / SockTest (TCP+потоки) | ✅ |
| classlinkage (10 тестов: интерфейсы, перегрузки, malformed class) | ✅ |
| Minecraft server Fabric 1.16.1 | 🟡 bootstrap пройден |
| Minecraft client Fabric 1.21.11 | 🟡 в работе (медленно) |

Отладочные флаги: `JVM25_TRACE`, `JVM25_DUMP`, `JVM25_BLOCKTRACE`, `JVM25_TRACE_COMPILE`, `JVM25_PROFILE_NATIVE`, `JVM25_MEMSTAT`, `JVM25_STACK_MB`, `JVM25_NO_VERIFY`.

---

## 🤝 Contributing

Проект переносится поэтапно, модуль за модулем — строго по структуре HotSpot. Если хочешь помочь:

1. 🍴 Сделай fork
2. 🌿 Создай ветку `feature/<module-name>`
3. 🔀 Открой Pull Request

Ориентир для переноса — исходники OpenJDK 25 (`src/hotspot/share/`). Каждый PR должен сохранять структуру и нейминг оригинала.

---

## ⚠️ Disclaimer

Это **некоммерческий исследовательский проект**. Не аффилирован с Oracle или OpenJDK. HotSpot — товарный знак Oracle. Исходники OpenJDK распространяются под GPL-2.0 с Classpath Exception.

---

<div align="center">

**Сделано с ❤️ и большим количеством байткода**

*Rust 🦀 × Java ☕ = ⚡*

</div>
