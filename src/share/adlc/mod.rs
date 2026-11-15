/*
 * OpenJDK 25 HotSpot adlc/ port
 *
 * Rust port of the ADL compiler front-end: parses .ad architecture
 * description files into an owned data model (ArchDesc) and emits a
 * machine-readable matcher specification as JSON for the C2 opto
 * instruction-selection port.
 */

pub mod filebuff;
pub mod inspect;
pub mod model;
pub mod opcodes;
pub mod parser;
pub mod spec;

pub use filebuff::FileBuff;
pub use model::*;
pub use parser::AdlParser;
pub use spec::*;

/// Parse an ADL source string into an ArchDesc.
/// Returns the ArchDesc; check `syntax_errs`/`semantic_errs` for diagnostics
/// (diagnostics are also printed to stderr like the C++ adlc).
pub fn parse_adl(source: &str, file_name: &str) -> ArchDesc {
    let mut ad = ArchDesc::new();
    ad.adl_file_name = file_name.to_string();
    let buf = FileBuff::new(source.as_bytes(), file_name);
    {
        let mut parser = AdlParser::new(buf, &mut ad);
        parser.parse();
    }
    ad
}

/// Parse an ADL file into an ArchDesc.
pub fn parse_adl_file(path: &str) -> Result<ArchDesc, String> {
    let buf = FileBuff::from_file(path)?;
    let mut ad = ArchDesc::new();
    ad.adl_file_name = path.to_string();
    {
        let mut parser = AdlParser::new(buf, &mut ad);
        parser.parse();
    }
    Ok(ad)
}
