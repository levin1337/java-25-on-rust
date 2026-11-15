#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoderStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug)]
pub struct DecodedInstruction {
    pub offset: usize,
    pub length: usize,
    pub bytes: Vec<u8>,
}

pub trait DecoderImpl {
    fn decode(&mut self, code: &[u8]) -> Option<DecodedInstruction>;
    fn has_error(&self) -> bool;
    fn status(&self) -> DecoderStatus {
        if self.has_error() {
            DecoderStatus::Error
        } else {
            DecoderStatus::Ok
        }
    }
}

#[derive(Default)]
pub struct NullDecoder {
    error: bool,
}

impl NullDecoder {
    pub fn new() -> Self {
        Self { error: false }
    }
}

impl DecoderImpl for NullDecoder {
    fn decode(&mut self, _code: &[u8]) -> Option<DecodedInstruction> {
        None
    }

    fn has_error(&self) -> bool {
        self.error
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnyDecoder {
    Null,
    Amd64,
}

pub struct Decoder {
    kind: AnyDecoder,
    amd64: decoder_amd64::Amd64Decoder,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            kind: AnyDecoder::Amd64,
            amd64: decoder_amd64::Amd64Decoder::new(),
        }
    }

    pub fn null() -> Self {
        Self {
            kind: AnyDecoder::Null,
            amd64: decoder_amd64::Amd64Decoder::new(),
        }
    }

    pub fn decode(&mut self, code: &[u8]) -> Option<DecodedInstruction> {
        match self.kind {
            AnyDecoder::Null => None,
            AnyDecoder::Amd64 => self.amd64.decode(code),
        }
    }

    pub fn decode_at(&mut self, base: usize, code: &[u8]) -> Option<DecodedInstruction> {
        let mut insn = self.decode(code)?;
        insn.offset = base;
        Some(insn)
    }

    pub fn has_error(&self) -> bool {
        match self.kind {
            AnyDecoder::Null => false,
            AnyDecoder::Amd64 => self.amd64.has_error(),
        }
    }

    pub fn instruction_length(code: &[u8]) -> Option<usize> {
        decoder_amd64::instruction_length(code)
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn decode_next(decoder: &mut Decoder, code: &[u8]) -> Option<DecodedInstruction> {
    decoder.decode(code)
}

pub fn disassemble_hex(code: &[u8], max_insns: usize) -> Vec<DecodedInstruction> {
    let mut decoder = Decoder::new();
    let mut result = Vec::new();
    let mut offset = 0usize;
    while offset < code.len() && result.len() < max_insns {
        match decoder.decode_at(offset, &code[offset..]) {
            Some(insn) => {
                offset += insn.length;
                result.push(insn);
            }
            None => break,
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_simple() {
        let mut d = Decoder::new();
        let insn = d.decode(&[0x90]).unwrap();
        assert_eq!(insn.length, 1);
        let insn = d.decode(&[0x48, 0x89, 0xD8]).unwrap();
        assert_eq!(insn.length, 3);
        let insn = d.decode(&[0xE8, 0x00, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(insn.length, 5);
    }
}
