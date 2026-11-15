use super::decoder::{DecodedInstruction, DecoderImpl, DecoderStatus};

const M: u8 = 0x01;
const I8: u8 = 0x02;
const I16: u8 = 0x04;
const I32: u8 = 0x08;
const IOS: u8 = 0x10;
const IOS32: u8 = 0x20;
const MOFFS: u8 = 0x40;
const FARPTR: u8 = 0x80;

const PRIMARY: [u8; 256] = [
    // 0x00
    M, M, M, M, I8, IOS32, 0, 0, M, M, M, M, I8, IOS32, 0, 0,
    // 0x10
    M, M, M, M, I8, IOS32, 0, 0, M, M, M, M, I8, IOS32, 0, 0,
    // 0x20
    M, M, M, M, I8, IOS32, 0, 0, M, M, M, M, I8, IOS32, 0, 0,
    // 0x30
    M, M, M, M, I8, IOS32, 0, 0, M, M, M, M, I8, IOS32, 0, 0,
    // 0x40 REX prefixes
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0x50 push/pop regs
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0x60
    0, 0, 0, 0, 0, 0, 0, 0, IOS32, M | IOS, I8, M | I8, 0, 0, 0, 0,
    // 0x70 jcc rel8
    I8, I8, I8, I8, I8, I8, I8, I8, I8, I8, I8, I8, I8, I8, I8, I8,
    // 0x80
    M | I8, M | IOS, M | I8, M | I8, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0x90
    0, 0, 0, FARPTR, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0xA0
    MOFFS, MOFFS, MOFFS, MOFFS, 0, 0, 0, 0, I8, IOS32, 0, 0, 0, 0, 0, 0,
    // 0xB0
    I8, I8, I8, I8, I8, I8, I8, I8, IOS, IOS, IOS, IOS, IOS, IOS, IOS, IOS,
    // 0xC0
    M | I8, M | I8, I16, 0, 0, 0, M | I8, M | IOS, I8, 0, I16, 0, 0, I8, 0, 0,
    // 0xD0
    M, M, M, M, I8, I8, 0, 0, M, M, M, M, M, M, M, M,
    // 0xE0
    I8, I8, I8, I8, I8, I8, I8, I8, I32, I32, FARPTR, I8, 0, 0, 0, 0,
    // 0xF0
    0, 0, 0, 0, 0, 0, M, M, 0, 0, 0, 0, 0, 0, 0, M,
];

const OF_MAP: [u8; 256] = [
    // 0x00
    M, M, M, M, M, M, 0, 0, 0, 0, 0, M, 0, 0, M, M | I8,
    // 0x10
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0x20
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0x30
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0x40 cmov
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0x50
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0x60
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0x70
    M | I8, M | I8, M | I8, M | I8, M, M, M, 0, M, M, M, M, M, M, M, M,
    // 0x80 jcc rel32
    I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32,
    // 0x90 setcc
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0xA0
    0, 0, 0, M, M | I8, M, M, M, 0, 0, 0, M, M | I8, M, M, M,
    // 0xB0
    M, M, M, M, M | I8, M, M, M, M, M, M | I8, M, M, M, M, M,
    // 0xC0
    M, M, M | I8, M, M | I8, M | I8, M, M, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0xD0
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0xE0
    M, M, M, M, M, M, M, M, M, M, M, M, M, M, M, M,
    // 0xF0
    M, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, M,
];

const OF38_MAP: [u8; 256] = [M; 256];

const OF3A_MAP: [u8; 256] = [M | I8; 256];

#[derive(Default)]
pub struct Amd64Decoder {
    error: bool,
}

impl Amd64Decoder {
    pub fn new() -> Self {
        Self { error: false }
    }
}

impl DecoderImpl for Amd64Decoder {
    fn decode(&mut self, code: &[u8]) -> Option<DecodedInstruction> {
        let len = instruction_length(code)?;
        Some(DecodedInstruction {
            offset: 0,
            length: len,
            bytes: code[..len].to_vec(),
        })
    }

    fn has_error(&self) -> bool {
        self.error
    }

    fn status(&self) -> DecoderStatus {
        if self.error {
            DecoderStatus::Error
        } else {
            DecoderStatus::Ok
        }
    }
}

fn imm_size(kind: u8, op16: bool, rex_w: bool, addr16: bool) -> usize {
    match kind {
        I8 => 1,
        I16 => 2,
        I32 => 4,
        IOS => {
            if rex_w {
                8
            } else if op16 {
                2
            } else {
                4
            }
        }
        IOS32 => {
            if op16 {
                2
            } else {
                4
            }
        }
        MOFFS => {
            if addr16 {
                4
            } else {
                8
            }
        }
        FARPTR => 6,
        _ => 0,
    }
}

fn modrm_disp_size(code: &[u8], pos: usize, addr16: bool) -> usize {
    if pos >= code.len() {
        return 0;
    }
    let m = code[pos];
    let modu = m >> 6;
    let rm = m & 7;
    if modu == 3 {
        return 0;
    }
    if addr16 {
        return match modu {
            0 => {
                if rm == 6 {
                    2
                } else {
                    0
                }
            }
            1 => 1,
            2 => 2,
            _ => 0,
        };
    }
    let mut p = pos + 1;
    if rm == 4 {
        if p >= code.len() {
            return 0;
        }
        let sib = code[p];
        p += 1;
        if modu == 0 && (sib & 7) == 5 {
            return 4 + (p - pos - 1);
        }
    }
    match modu {
        0 => {
            if rm == 5 {
                4 + (p - pos - 1)
            } else {
                p - pos - 1
            }
        }
        1 => 1 + (p - pos - 1),
        2 => 4 + (p - pos - 1),
        _ => 0,
    }
}

fn fpu_no_modrm(opcode: u8, second: u8) -> bool {
    match opcode {
        0xD9 => {
            second == 0xD0
                || (0xC0..=0xCF).contains(&second)
                || (0xE0..=0xFF).contains(&second)
        }
        0xDB => (0xE0..=0xF7).contains(&second),
        0xDA => second == 0xE9,
        0xDE => second == 0xD9,
        _ => false,
    }
}

pub fn instruction_length(code: &[u8]) -> Option<usize> {
    if code.is_empty() {
        return None;
    }
    let mut i = 0usize;
    let mut op16 = false;
    let mut addr16 = false;
    while i < code.len() {
        match code[i] {
            0x66 => {
                op16 = true;
                i += 1;
            }
            0x67 => {
                addr16 = true;
                i += 1;
            }
            0xF0 | 0xF2 | 0xF3 | 0x2E | 0x36 | 0x3E | 0x26 | 0x64 | 0x65 => {
                i += 1;
            }
            _ => break,
        }
    }
    if i >= code.len() {
        return None;
    }
    let mut rex_w = false;
    if (0x40..=0x4F).contains(&code[i]) {
        rex_w = (code[i] & 0x08) != 0;
        i += 1;
        if i >= code.len() {
            return None;
        }
    }
    let b = code[i];
    i += 1;

    let attr: u8;

    if b == 0x0F {
        if i >= code.len() {
            return None;
        }
        let b2 = code[i];
        i += 1;
        if b2 == 0x38 {
            if i >= code.len() {
                return None;
            }
            let b3 = code[i];
            i += 1;
            attr = OF38_MAP[b3 as usize];
        } else if b2 == 0x3A {
            if i >= code.len() {
                return None;
            }
            let b3 = code[i];
            i += 1;
            attr = OF3A_MAP[b3 as usize];
        } else {
            attr = OF_MAP[b2 as usize];
        }
    } else if b == 0xC4 {
        if i + 1 >= code.len() {
            return None;
        }
        let mmmmm = code[i] & 0x1F;
        i += 2;
        match mmmmm {
            1 => {
                if i >= code.len() {
                    return None;
                }
                attr = OF_MAP[code[i] as usize];
                i += 1;
            }
            2 => {
                if i >= code.len() {
                    return None;
                }
                attr = OF38_MAP[code[i] as usize];
                i += 1;
            }
            3 => {
                if i >= code.len() {
                    return None;
                }
                attr = OF3A_MAP[code[i] as usize];
                i += 1;
            }
            _ => {
                attr = 0;
            }
        }
    } else if b == 0xC5 {
        if i >= code.len() {
            return None;
        }
        i += 1;
        if i >= code.len() {
            return None;
        }
        attr = OF_MAP[code[i] as usize];
        i += 1;
    } else if b == 0x62 {
        if i + 2 >= code.len() {
            return None;
        }
        let mm = code[i] & 0x03;
        i += 3;
        match mm {
            1 => {
                if i >= code.len() {
                    return None;
                }
                attr = OF_MAP[code[i] as usize];
                i += 1;
            }
            2 => {
                if i >= code.len() {
                    return None;
                }
                attr = OF38_MAP[code[i] as usize];
                i += 1;
            }
            _ => {
                if i >= code.len() {
                    return None;
                }
                attr = OF3A_MAP[code[i] as usize];
                i += 1;
            }
        }
    } else if b == 0x8F {
        if i + 1 >= code.len() {
            return None;
        }
        i += 2;
        if i >= code.len() {
            return None;
        }
        attr = OF_MAP[code[i] as usize];
        i += 1;
    } else {
        attr = PRIMARY[b as usize];
    }
    if attr == 0 && !matches!(b, 0x0F | 0xC4 | 0xC5 | 0x62 | 0x8F) {
        return Some(i);
    }

    let mut extra = 0usize;
    let mut has_modrm = (attr & M) != 0;

    if !matches!(b, 0xC4 | 0xC5 | 0x62 | 0x8F) && (0xD8..=0xDF).contains(&b)
    {
        if i < code.len() && fpu_no_modrm(b, code[i]) {
            // two-byte x87 opcode (e.g. D9 E8 = fld1): consume the second byte
            i += 1;
            has_modrm = false;
        }
    }

    if has_modrm {
        if i >= code.len() {
            return None;
        }
        let modrm_pos = i;
        let disp = modrm_disp_size(code, i, addr16);
        i += 1 + disp;
        if b == 0xF6 && i - 1 - disp < code.len() {
            let reg = (code[modrm_pos] >> 3) & 7;
            if reg <= 1 {
                extra += 1;
            }
        } else if b == 0xF7 && i - 1 - disp < code.len() {
            let reg = (code[modrm_pos] >> 3) & 7;
            if reg <= 1 {
                extra += imm_size(IOS, op16, rex_w, addr16);
            }
        }
    }

    let imm_kind = attr & !(M | FARPTR);
    let imm = if b == 0xC8 {
        3
    } else if (attr & FARPTR) != 0 {
        6
    } else {
        imm_size(imm_kind, op16, rex_w, addr16) + extra
    };

    Some(i + imm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(instruction_length(&[0x90]), Some(1));
        assert_eq!(instruction_length(&[0x48, 0x89, 0xD8]), Some(3));
        assert_eq!(instruction_length(&[0xE8, 1, 2, 3, 4]), Some(5));
        assert_eq!(instruction_length(&[0xEB, 0xFE]), Some(2));
        assert_eq!(instruction_length(&[0x74, 0x05]), Some(2));
        assert_eq!(instruction_length(&[0x0F, 0x84, 1, 2, 3, 4]), Some(6));
        assert_eq!(instruction_length(&[0xB8, 1, 2, 3, 4]), Some(5));
        assert_eq!(instruction_length(&[0x48, 0xB8, 1, 2, 3, 4, 5, 6, 7, 8]), Some(10));
        assert_eq!(instruction_length(&[0x66, 0xB8, 1, 2]), Some(4));
        assert_eq!(instruction_length(&[0x6A, 0x7F]), Some(2));
        assert_eq!(instruction_length(&[0x68, 1, 2, 3, 4]), Some(5));
        assert_eq!(instruction_length(&[0xC3]), Some(1));
        assert_eq!(instruction_length(&[0xC2, 0x10, 0x00]), Some(3));
        assert_eq!(instruction_length(&[0xC8, 0x10, 0x00, 0x05]), Some(4));
        assert_eq!(instruction_length(&[0xCD, 0x80]), Some(2));
        assert_eq!(instruction_length(&[0xF4]), Some(1));
    }

    #[test]
    fn modrm_disp() {
        assert_eq!(instruction_length(&[0x8B, 0x00]), Some(2));
        assert_eq!(instruction_length(&[0x8B, 0x40, 0x10]), Some(3));
        assert_eq!(instruction_length(&[0x8B, 0x80, 1, 2, 3, 4]), Some(6));
        assert_eq!(instruction_length(&[0x8B, 0x05, 1, 2, 3, 4]), Some(6));
        assert_eq!(instruction_length(&[0x8B, 0x04, 0x25, 1, 2, 3, 4]), Some(7));
        assert_eq!(instruction_length(&[0x8B, 0x44, 0x24, 0x08]), Some(4));
        assert_eq!(instruction_length(&[0x8B, 0x84, 0x24, 1, 2, 3, 4]), Some(7));
        assert_eq!(instruction_length(&[0x8B, 0x04, 0x48]), Some(3));
    }

    #[test]
    fn sse() {
        assert_eq!(instruction_length(&[0x66, 0x0F, 0x6F, 0xC0]), Some(4));
        assert_eq!(instruction_length(&[0xF3, 0x0F, 0x1E, 0xFA]), Some(4));
        assert_eq!(instruction_length(&[0x66, 0x0F, 0x70, 0xC0, 0x01]), Some(5));
        assert_eq!(instruction_length(&[0x0F, 0xBA, 0xE0, 0x08]), Some(4));
        assert_eq!(instruction_length(&[0x66, 0x0F, 0x3A, 0x0F, 0xC1, 0x04]), Some(6));
        assert_eq!(instruction_length(&[0xF2, 0x0F, 0x38, 0xF0, 0xC1]), Some(5));
    }

    #[test]
    fn vex_evex() {
        assert_eq!(instruction_length(&[0xC5, 0xF8, 0x77]), Some(3));
        assert_eq!(instruction_length(&[0xC4, 0xE2, 0x7D, 0x18, 0xC1]), Some(5));
        assert_eq!(instruction_length(&[0x62, 0xF1, 0x7C, 0x48, 0x58, 0xC1]), Some(6));
    }

    #[test]
    fn fpu() {
        assert_eq!(instruction_length(&[0xD9, 0xE8]), Some(2));
        assert_eq!(instruction_length(&[0xD9, 0xC0]), Some(2));
        assert_eq!(instruction_length(&[0xDB, 0xE0]), Some(2));
        assert_eq!(instruction_length(&[0xDE, 0xD9]), Some(2));
    }
}
