

use crate::share::{
    jfr::jni::java_support,
    runtime::{javaCalls::JavaValue, threading},
    services::{
        diagnosticArgument::DcmdResult,
        diagnosticFramework::{CmdLine, DCmdArgumentInfo, DCmdInstance, DCmdSource},
    },
};
use std::io::Write;

fn source_name(source: DCmdSource) -> &'static str {
    match source {
        DCmdSource::Internal => "internal",
        DCmdSource::MBean => "mbean",
        DCmdSource::AttachApi => "attach",
    }
}

pub struct JfrDcmd {
    java_class: &'static [u8],
    num_arguments: usize,
    args: String,
    delimiter: char,
}

impl JfrDcmd {
    pub fn new(java_class: &'static [u8], num_arguments: usize) -> Self {
        Self {
            java_class,
            num_arguments,
            args: String::new(),
            delimiter: '\0',
        }
    }

    fn loader(
    ) -> std::io::Result<std::sync::Arc<crate::share::classfile::classLoader::ClassLoader<'static>>>
    {
        threading::jvm_loader()
            .ok_or_else(|| std::io::Error::other("JVM not initialized"))
    }

    fn invoke_virtual<'a>(
        loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
        java_class: &[u8],
        instance: crate::share::interpreter::bytecodeInterpreter::Value<'a>,
        name: &[u8],
        signature: &[u8],
        push_args: impl FnOnce(&mut crate::share::jfr::jni::java_call::JfrJavaArguments<'a>),
        output: &mut dyn Write,
    ) -> DcmdResult<crate::share::interpreter::bytecodeInterpreter::Value<'a>> {
        use crate::share::jfr::jni::java_call::JfrJavaArguments;
        let klass = loader
            .load(java_class)
            .map_err(|e| e.to_string())?;
        let mut args = JfrJavaArguments::with_klass(
            JavaValue::Value(crate::share::interpreter::bytecodeInterpreter::Value::Null),
            klass,
            name,
            signature,
        );
        args.set_receiver(instance);
        push_args(&mut args);
        args.call_virtual(loader, output).map_err(|e| e.to_string())?;
        let JavaValue::Value(value) = args.result().clone() else {
            return Ok(crate::share::interpreter::bytecodeInterpreter::Value::Null);
        };
        Ok(value)
    }

    fn print_help_lines(
        loader: &crate::share::classfile::classLoader::ClassLoader<'_>,
        java_class: &[u8],
        method: &[u8],
        output: &mut dyn Write,
    ) -> DcmdResult<()> {
        let instance = java_support::new_object(loader, java_class, output)
            .map_err(|e| e.to_string())?;
        let result = Self::invoke_virtual(
            loader,
            java_class,
            instance,
            method,
            b"()[Ljava/lang/String;",
            |_| {},
            output,
        )?;
        Self::print_string_array(loader, result, output)
    }

    fn print_string_array<'a>(
        loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
        result: crate::share::interpreter::bytecodeInterpreter::Value<'a>,
        output: &mut dyn Write,
    ) -> DcmdResult<()> {
        use crate::share::interpreter::bytecodeInterpreter::Value;
        match result {
            Value::ObjectArray(handle) => {
                let arrays = loader.obj_arrays.borrow();
                if let Some(array) = arrays.get(handle) {
                    for element in array.elements.clone() {
                        if let Some(text) = java_support::c_str(loader, element) {
                            writeln!(output, "{text}").map_err(|e| e.to_string())?;
                        }
                    }
                }
                Ok(())
            }
            other => {
                if let Some(text) = java_support::c_str(loader, other) {
                    writeln!(output, "{text}").map_err(|e| e.to_string())?;
                }
                Ok(())
            }
        }
    }

    fn new_instance<'a>(
        loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
        java_class: &[u8],
        output: &mut dyn Write,
    ) -> DcmdResult<crate::share::interpreter::bytecodeInterpreter::Value<'a>> {
        java_support::new_object(loader, java_class, output).map_err(|e| e.to_string())
    }
}

impl DCmdInstance for JfrDcmd {
    fn parse(&mut self, line: &CmdLine, delim: char) -> DcmdResult<()> {
        self.args = line.args().to_string();
        self.delimiter = delim;
        Ok(())
    }

    fn execute(&mut self, source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let loader = Self::loader().map_err(|e| e.to_string())?;
        if !java_support::is_jdk_jfr_module_available_out(&loader, out) {
            return Ok(());
        }
        if source == DCmdSource::Internal && self.args == "help" {
            return Self::print_help_lines(
                &loader,
                self.java_class,
                b"getStartupHelp",
                out,
            );
        }

        let instance = Self::new_instance(&loader, self.java_class, out)?;
        let args_string = java_support::new_string(&loader, &self.args, out)
            .map_err(|e| e.to_string())?;
        let source_string =
            java_support::new_string(&loader, source_name(source), out)
                .map_err(|e| e.to_string())?;
        let delimiter = self.delimiter as i32;
        let java_class = self.java_class;
        let result = Self::invoke_virtual(
            &loader,
            java_class,
            instance,
            b"execute",
            b"(Ljava/lang/String;Ljava/lang/String;C)[Ljava/lang/String;",
            |args| {
                args.push_oop(source_string);
                args.push_oop(args_string);
                args.push_int(delimiter);
            },
            out,
        )?;
        Self::print_string_array(&loader, result, out)
    }

    fn print_help(&self, _name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        let loader = match Self::loader() {
            Ok(loader) => loader,
            Err(_) => return Ok(()),
        };
        let _ = Self::print_help_lines(&loader, self.java_class, b"getHelp", out);
        Ok(())
    }

    fn reset(&mut self) -> DcmdResult<()> {
        self.args.clear();
        self.delimiter = '\0';
        Ok(())
    }

    fn cleanup(&mut self) {}

    fn argument_name_array(&self) -> Vec<&'static str> {
        vec!["arguments"]
    }

    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        vec![DCmdArgumentInfo {
            name: "arguments",
            description: "Command arguments passed to the JFR dcmd",
            type_name: "STRING",
            default_string: None,
            is_mandatory: false,
            is_option: false,
            allow_multiple: true,
            position: 0,
        }]
    }
}

pub fn start_factory() -> Box<dyn crate::share::services::diagnosticFramework::DCmdInstance> {
    Box::new(JfrDcmd::new(b"jdk/jfr/internal/dcmd/DCmdStart", 11))
}

pub fn stop_factory() -> Box<dyn crate::share::services::diagnosticFramework::DCmdInstance> {
    Box::new(JfrDcmd::new(b"jdk/jfr/internal/dcmd/DCmdStop", 2))
}

pub fn dump_factory() -> Box<dyn crate::share::services::diagnosticFramework::DCmdInstance> {
    Box::new(JfrDcmd::new(b"jdk/jfr/internal/dcmd/DCmdDump", 7))
}

pub fn check_factory() -> Box<dyn crate::share::services::diagnosticFramework::DCmdInstance> {
    Box::new(JfrDcmd::new(b"jdk/jfr/internal/dcmd/DCmdCheck", 2))
}

pub fn view_factory() -> Box<dyn crate::share::services::diagnosticFramework::DCmdInstance> {
    Box::new(JfrDcmd::new(b"jdk/jfr/internal/dcmd/DCmdView", 7))
}

pub fn query_factory() -> Box<dyn crate::share::services::diagnosticFramework::DCmdInstance> {
    Box::new(JfrDcmd::new(b"jdk/jfr/internal/dcmd/DCmdQuery", 5))
}
