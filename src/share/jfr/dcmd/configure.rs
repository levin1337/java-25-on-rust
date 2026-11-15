

use crate::share::{
    jfr::jni::java_support,
    runtime::{javaCalls::JavaValue, threading},
    services::{
        diagnosticArgument::{DcmdResult, DCmdArgument},
        diagnosticFramework::{CmdLine, DCmdParser, DCmdSource},
        diagnosticFramework::{DCmdInstance, DCmdArgumentInfo},
    },
};
use std::io::Write;

pub struct JfrConfigureDcmd {
    parser: DCmdParser,
    verbose: bool,
}

impl JfrConfigureDcmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_option(DCmdArgument::<String>::with_default(
            "repositorypath",
            "Path to repository",
            "STRING",
            false,
            "",
        ));
        parser.add_dcmd_option(DCmdArgument::<String>::with_default(
            "dumppath",
            "Path to dump",
            "STRING",
            false,
            "",
        ));
        parser.add_dcmd_option(DCmdArgument::<i64>::with_default(
            "stackdepth",
            "Stack depth",
            "JULONG",
            false,
            "64",
        ));
        parser.add_dcmd_option(DCmdArgument::<i64>::with_default(
            "globalbuffercount",
            "Number of global buffers",
            "JULONG",
            false,
            "20",
        ));
        parser.add_dcmd_option(DCmdArgument::<crate::share::services::diagnosticArgument::MemorySizeArgument>::with_default(
            "globalbuffersize",
            "Size of a global buffer",
            "MEMORY SIZE",
            false,
            "512k",
        ));
        parser.add_dcmd_option(DCmdArgument::<crate::share::services::diagnosticArgument::MemorySizeArgument>::with_default(
            "threadbuffersize",
            "Size of a thread buffer",
            "MEMORY SIZE",
            false,
            "8k",
        ));
        parser.add_dcmd_option(DCmdArgument::<crate::share::services::diagnosticArgument::MemorySizeArgument>::with_default(
            "memorysize",
            "Overall memory size",
            "MEMORY SIZE",
            false,
            "10m",
        ));
        parser.add_dcmd_option(DCmdArgument::<crate::share::services::diagnosticArgument::MemorySizeArgument>::with_default(
            "maxchunksize",
            "Size of an individual disk chunk",
            "MEMORY SIZE",
            false,
            "12m",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "samplethreads",
            "Activate thread sampling",
            "BOOLEAN",
            false,
            "true",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "preserve-repository",
            "Preserve the disk repository after JVM exit",
            "BOOLEAN",
            false,
            "false",
        ));
        Self {
            parser,
            verbose: false,
        }
    }
}

impl Default for JfrConfigureDcmd {
    fn default() -> Self {
        Self::new()
    }
}

impl DCmdInstance for JfrConfigureDcmd {
    fn parse(&mut self, line: &CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }

    fn execute(&mut self, source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        use crate::share::interpreter::bytecodeInterpreter::Value;
        let Some(loader) = threading::jvm_loader() else {
            return Err("JVM not initialized".to_string());
        };
        if !java_support::is_jdk_jfr_module_available_out(&loader, out) {
            return Ok(());
        }
        if self.parser.option_is_set("samplethreads") {
            let startup = source == DCmdSource::Internal;
            if startup {
                eprintln!("Option samplethreads is deprecated. Use -XX:StartFlightRecording:method-profiling=<off|normal|high|max>");
            } else {
                writeln!(
                    out,
                    "Option samplethreads is deprecated. Use JFR.start method-profiling=<off|normal|high|max>"
                )
                .map_err(|e| e.to_string())?;
                writeln!(out).map_err(|e| e.to_string())?;
            }
        }

        let java_class = b"jdk/jfr/internal/dcmd/DCmdConfigure";
        let instance = java_support::new_object(&loader, java_class, out)
            .map_err(|e| e.to_string())?;
        let klass = loader.load(java_class).map_err(|e| e.to_string())?;

        let opt_string = |out: &mut dyn Write, name: &str| -> DcmdResult<Option<Value<'_>>> {
            if !self.parser.option_is_set(name) {
                return Ok(None);
            }
            let text = self.parser.option::<String>(name).unwrap_or_default();
            if text.is_empty() {
                return Ok(None);
            }
            java_support::new_string(&loader, &text, out)
                .map(Some)
                .map_err(|e| e.to_string())
        };
        let opt_long = |out: &mut dyn Write, name: &str| -> DcmdResult<Option<Value<'_>>> {
            if !self.parser.option_is_set(name) {
                return Ok(None);
            }
            let value = self.parser.option::<i64>(name).unwrap_or(0);
            Ok(Some(java_support::new_java_lang_long(&loader, value, out).map_err(|e| e.to_string())?))
        };
        let opt_int = |out: &mut dyn Write, name: &str| -> DcmdResult<Option<Value<'_>>> {
            if !self.parser.option_is_set(name) {
                return Ok(None);
            }
            let value = self.parser.option::<i64>(name).unwrap_or(0) as i32;
            Ok(Some(java_support::new_java_lang_integer(&loader, value, out).map_err(|e| e.to_string())?))
        };
        let opt_bool = |out: &mut dyn Write, name: &str| -> DcmdResult<Option<Value<'_>>> {
            if !self.parser.option_is_set(name) {
                return Ok(None);
            }
            let value = self.parser.option::<bool>(name).unwrap_or(false);
            Ok(Some(java_support::new_java_lang_boolean(&loader, value, out).map_err(|e| e.to_string())?))
        };
        let mem_size = |out: &mut dyn Write, name: &str| -> DcmdResult<Option<Value<'_>>> {
            if !self.parser.option_is_set(name) {
                return Ok(None);
            }
            let arg = self
                .parser
                .option::<crate::share::services::diagnosticArgument::MemorySizeArgument>(name);
            let size = arg.map(|a| a.size).unwrap_or(0) as i64;
            Ok(Some(java_support::new_java_lang_long(&loader, size, out).map_err(|e| e.to_string())?))
        };

        let repository_path = opt_string(out, "repositorypath")?;
        let dump_path = opt_string(out, "dumppath")?;
        let stack_depth = opt_int(out, "stackdepth")?;
        let global_buffer_count = opt_long(out, "globalbuffercount")?;
        let global_buffer_size = mem_size(out, "globalbuffersize")?;
        let thread_buffer_size = mem_size(out, "threadbuffersize")?;
        let memory_size = mem_size(out, "memorysize")?;
        let max_chunk_size = mem_size(out, "maxchunksize")?;
        let preserve_repository = opt_bool(out, "preserve-repository")?;

        let _ = (stack_depth, global_buffer_count, global_buffer_size, thread_buffer_size, memory_size, max_chunk_size);

        let mut args = crate::share::jfr::jni::java_call::JfrJavaArguments::with_klass(
            JavaValue::Value(Value::Null),
            klass,
            b"execute",
            b"(ZLjava/lang/String;Ljava/lang/String;Ljava/lang/Integer;Ljava/lang/Long;Ljava/lang/Long;Ljava/lang/Long;Ljava/lang/Long;Ljava/lang/Long;Ljava/lang/Boolean;)[Ljava/lang/String;",
        );
        args.set_receiver(instance);
        args.push_int(i32::from(self.verbose));
        for value in [
            repository_path,
            dump_path,
            stack_depth,
            global_buffer_count,
            global_buffer_size,
            thread_buffer_size,
            memory_size,
            max_chunk_size,
            preserve_repository,
        ] {
            args.push_oop(value.unwrap_or(Value::Null));
        }
        args.call_virtual(&loader, out).map_err(|e| e.to_string())?;

        if let JavaValue::Value(result) = args.result().clone() {
            print_result(&loader, result, out)?;
        }
        Ok(())
    }

    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }

    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }

    fn cleanup(&mut self) {
        self.parser.cleanup();
    }

    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }

    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

fn print_result(
    loader: &crate::share::classfile::classLoader::ClassLoader<'_>,
    result: crate::share::interpreter::bytecodeInterpreter::Value<'_>,
    out: &mut dyn Write,
) -> DcmdResult<()> {
    use crate::share::interpreter::bytecodeInterpreter::Value;
    if let Value::ObjectArray(handle) = result {
        let arrays = loader.obj_arrays.borrow();
        if let Some(array) = arrays.get(handle) {
            for element in array.elements.clone() {
                if let Some(text) = java_support::c_str(loader, element) {
                    writeln!(out, "{text}").map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}
