/*
 * Port of OpenJDK 25 HotSpot Arguments (share/runtime/arguments.hpp / arguments.cpp)
 *
 * Implements JVM command-line argument processing, memory sizing (-Xms, -Xmx, -Xss),
 * garbage collector selection (-XX:+UseG1GC, -XX:+UseParallelGC, etc.),
 * execution mode selection (-Xint, -Xcomp, -Xmixed), and ergonomics validation.
 */

use std::collections::HashMap;
use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GCSelected {
    G1,
    Parallel,
    Serial,
    Epsilon,
    Z,
    Shenandoah,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Mixed,
    Interpreter,
    Compiler,
}

#[derive(Debug, Clone)]
pub struct Arguments {
    pub jar: Option<String>,
    pub classpath: Option<String>,
    pub properties: HashMap<String, String>,
    pub main_class: Option<String>,
    pub app_arguments: Vec<String>,

    // Memory sizing
    pub initial_heap_size: usize,
    pub max_heap_size: usize,
    pub thread_stack_size: usize,
    pub max_metaspace_size: usize,

    // GC & JIT selections
    pub gc: GCSelected,
    pub execution_mode: ExecutionMode,
    pub tiered_compilation: bool,
    pub tiered_stop_at_level: u8,
}

impl Default for Arguments {
    fn default() -> Self {
        Self {
            jar: None,
            classpath: None,
            properties: HashMap::new(),
            main_class: None,
            app_arguments: Vec::new(),

            initial_heap_size: 128 * 1024 * 1024,  // 128MB
            max_heap_size: 512 * 1024 * 1024,      // 512MB
            thread_stack_size: 1024 * 1024,        // 1MB
            max_metaspace_size: 256 * 1024 * 1024, // 256MB

            gc: GCSelected::G1,
            execution_mode: ExecutionMode::Mixed,
            tiered_compilation: true,
            tiered_stop_at_level: 4,
        }
    }
}

impl Arguments {
    pub fn parse<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut parsed = Self::default();
        let raw_args: Vec<String> = args
            .into_iter()
            .map(|a| a.into().to_string_lossy().into_owned())
            .collect();

        let mut iter = raw_args.into_iter();
        while let Some(arg) = iter.next() {
            if arg == "-jar" {
                if let Some(jar_path) = iter.next() {
                    parsed.jar = Some(jar_path);
                    parsed.app_arguments.extend(iter);
                    break;
                }
            } else if arg == "-cp" || arg == "-classpath" {
                if let Some(cp) = iter.next() {
                    parsed.classpath = Some(cp);
                }
            } else if let Some(ms_str) = arg.strip_prefix("-Xms") {
                if let Some(sz) = Self::parse_memory_size(ms_str) {
                    parsed.initial_heap_size = sz;
                }
            } else if let Some(mx_str) = arg.strip_prefix("-Xmx") {
                if let Some(sz) = Self::parse_memory_size(mx_str) {
                    parsed.max_heap_size = sz;
                }
            } else if let Some(ss_str) = arg.strip_prefix("-Xss") {
                if let Some(sz) = Self::parse_memory_size(ss_str) {
                    parsed.thread_stack_size = sz;
                }
            } else if arg == "-Xint" {
                parsed.execution_mode = ExecutionMode::Interpreter;
            } else if arg == "-Xcomp" {
                parsed.execution_mode = ExecutionMode::Compiler;
            } else if arg == "-Xmixed" {
                parsed.execution_mode = ExecutionMode::Mixed;
            } else if arg == "-XX:+UseG1GC" {
                parsed.gc = GCSelected::G1;
            } else if arg == "-XX:+UseParallelGC" {
                parsed.gc = GCSelected::Parallel;
            } else if arg == "-XX:+UseSerialGC" {
                parsed.gc = GCSelected::Serial;
            } else if arg == "-XX:+UseEpsilonGC" {
                parsed.gc = GCSelected::Epsilon;
            } else if arg == "-XX:+UseZGC" {
                parsed.gc = GCSelected::Z;
            } else if arg == "-XX:+UseShenandoahGC" {
                parsed.gc = GCSelected::Shenandoah;
            } else if arg == "-XX:+TieredCompilation" {
                parsed.tiered_compilation = true;
            } else if arg == "-XX:-TieredCompilation" {
                parsed.tiered_compilation = false;
            } else if let Some(lvl_str) = arg.strip_prefix("-XX:TieredStopAtLevel=") {
                if let Ok(lvl) = lvl_str.parse::<u8>() {
                    parsed.tiered_stop_at_level = lvl.clamp(1, 4);
                }
            } else if let Some(prop) = arg.strip_prefix("-D") {
                if let Some((k, v)) = prop.split_once('=') {
                    parsed.properties.insert(k.to_string(), v.to_string());
                } else {
                    parsed
                        .properties
                        .insert(prop.to_string(), "true".to_string());
                }
            } else if !arg.starts_with('-') && parsed.main_class.is_none() {
                parsed.main_class = Some(arg);
                parsed.app_arguments.extend(iter);
                break;
            }
        }

        parsed.apply_ergo();
        parsed
    }

    /// Parses human-readable memory size string (e.g. "512m", "2g", "128k")
    pub fn parse_memory_size(s: &str) -> Option<usize> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let (digits, multiplier) = if s.ends_with(|c| matches!(c, 'k' | 'K')) {
            (&s[..s.len() - 1], 1024usize)
        } else if s.ends_with(|c| matches!(c, 'm' | 'M')) {
            (&s[..s.len() - 1], 1024 * 1024usize)
        } else if s.ends_with(|c| matches!(c, 'g' | 'G')) {
            (&s[..s.len() - 1], 1024 * 1024 * 1024usize)
        } else if s.ends_with(|c| matches!(c, 't' | 'T')) {
            (&s[..s.len() - 1], 1024 * 1024 * 1024 * 1024usize)
        } else {
            (s, 1usize)
        };

        let val: usize = digits.parse().ok()?;
        val.checked_mul(multiplier)
    }

    /// HotSpot ergonomics: adjusts memory bounds based on constraints
    pub fn apply_ergo(&mut self) {
        if self.initial_heap_size > self.max_heap_size {
            self.max_heap_size = self.initial_heap_size;
        }
        if self.execution_mode == ExecutionMode::Interpreter {
            self.tiered_compilation = false;
        }
    }

    /// Validates HotSpot argument limits and range constraints
    pub fn check_constraints(&self) -> Result<(), &'static str> {
        if self.initial_heap_size > self.max_heap_size {
            return Err("Initial heap size set to a larger value than the maximum heap size");
        }
        if self.thread_stack_size < 32 * 1024 {
            return Err("The stack size specified is too small, Specify at least 32k");
        }
        Ok(())
    }
}
