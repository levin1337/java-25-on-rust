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

| Подсистема | Статус |
|---|---|
| Classfile parser / ClassLoader / JImage | 🟡 В работе |
| Байткод-интерпретатор | 🟡 В работе |
| Runtime / Threads / JNI | 🟡 В работе |
| GC (Serial, Parallel, G1, ZGC, Shenandoah, Epsilon) | 🔴 Каркас |
| C1 / C2 JIT-компиляторы | 🔴 Каркас |
| CDS / NMT / Logging | 🟡 В работе |
| Запуск Java-приложений | 🟡 Частично |
| Minecraft | 🔴 Цель |

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
