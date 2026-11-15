

use crate::share::{
    jfr::jni::jni_method,
    services::{
        diagnosticArgument::DcmdResult,
        diagnosticFramework::{CmdLine, DCmdSource},
    },
};
use std::{
    io::Write,
    path::PathBuf,
};

pub struct JfrPrintDcmd {
    filename: Option<String>,
}

impl JfrPrintDcmd {
    pub fn new() -> Self {
        Self { filename: None }
    }
}

impl Default for JfrPrintDcmd {
    fn default() -> Self {
        Self::new()
    }
}

fn repository_dir() -> PathBuf {
    std::env::var("JFR_REPOSITORY")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("jfr"))
}

fn chunk_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|ext| ext == "jfr")
                        || p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("chunk-"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn print_chunk(path: &PathBuf, out: &mut dyn Write) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    let size = bytes.len();
    writeln!(out, "File: {}", path.display())?;
    writeln!(out, "Size: {size} bytes")?;
    if bytes.len() >= 4 && &bytes[..4] == b"FLR\0" {
        writeln!(out, "Magic: FLR (JFR chunk)")?;
        if bytes.len() >= 20 {
            let start = i64::from_le_bytes(bytes[4..12].try_into().unwrap());
            let end = i64::from_le_bytes(bytes[12..20].try_into().unwrap());
            writeln!(out, "Chunk start (ns): {start}")?;
            writeln!(out, "Chunk end (ns):   {end}")?;
            writeln!(out, "Duration (ms):    {}", (end - start) / 1_000_000)?;
        }
    } else {
        writeln!(out, "Warning: file does not start with the JFR magic 'FLR\\0'")?;
        let preview: String = bytes
            .iter()
            .take(32)
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        writeln!(out, "Header: {preview}")?;
    }
    writeln!(out)?;
    Ok(())
}

impl crate::share::services::diagnosticFramework::DCmdInstance for JfrPrintDcmd {
    fn parse(&mut self, line: &CmdLine, _delim: char) -> DcmdResult<()> {
        let args = line.args().trim();
        if !args.is_empty() {
            self.filename = Some(args.to_string());
        }
        Ok(())
    }

    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let _ = jni_method::dispatch_global(b"isRecording", b"()Z", &[], out);
        if let Some(name) = &self.filename {
            let path = PathBuf::from(name);
            if !path.exists() {
                return Err(format!("File not found: {name}"));
            }
            print_chunk(&path, out).map_err(|e| e.to_string())?;
            return Ok(());
        }
        let dir = repository_dir();
        let files = chunk_files(&dir);
        if files.is_empty() {
            writeln!(
                out,
                "No JFR chunks found in repository {}",
                dir.display()
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
        writeln!(out, "Repository: {}", dir.display()).map_err(|e| e.to_string())?;
        writeln!(out, "Chunks: {}", files.len()).map_err(|e| e.to_string())?;
        for file in &files {
            print_chunk(file, out).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "{name} [filename]")?;
        writeln!(out, "  Prints JFR recording chunk contents.")?;
        writeln!(out, "  Without arguments prints all chunks in the repository.")?;
        Ok(())
    }

    fn reset(&mut self) -> DcmdResult<()> {
        self.filename = None;
        Ok(())
    }

    fn cleanup(&mut self) {}

    fn argument_name_array(&self) -> Vec<&'static str> {
        vec!["filename"]
    }

    fn argument_info_array(&self) -> Vec<crate::share::services::diagnosticFramework::DCmdArgumentInfo> {
        vec![crate::share::services::diagnosticFramework::DCmdArgumentInfo {
            name: "filename",
            description: "JFR chunk file to print (default: all chunks in repository)",
            type_name: "FILE",
            default_string: None,
            is_mandatory: false,
            is_option: false,
            allow_multiple: false,
            position: 0,
        }]
    }
}
