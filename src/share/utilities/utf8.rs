pub mod utf8 {
    pub fn next(bytes: &[u8]) -> (u16, usize) {
        let ch = bytes[0];
        match ch >> 4 {
            0x0..=0x7 => (ch as u16, 1),
            0x8..=0xB | 0xF => (bytes[0] as u16, 1),
            0xC | 0xD => {
                if bytes.len() > 1 && (bytes[1] & 0xC0) == 0x80 {
                    let high_five = (ch & 0x1F) as u16;
                    let low_six = (bytes[1] & 0x3F) as u16;
                    ((high_five << 6) + low_six, 2)
                } else {
                    (bytes[0] as u16, 1)
                }
            }
            0xE => {
                if bytes.len() > 2
                    && (bytes[1] & 0xC0) == 0x80
                    && (bytes[2] & 0xC0) == 0x80
                {
                    let high_four = (ch & 0x0F) as u16;
                    let mid_six = (bytes[1] & 0x3F) as u16;
                    let low_six = (bytes[2] & 0x3F) as u16;
                    ((((high_four << 6) + mid_six) << 6) + low_six, 3)
                } else {
                    (bytes[0] as u16, 1)
                }
            }
            _ => (bytes[0] as u16, 1),
        }
    }

    pub fn next_character(bytes: &[u8]) -> (i32, usize) {
        if is_supplementary_character(bytes) {
            (get_supplementary_character(bytes), 6)
        } else {
            let (v, n) = next(bytes);
            (v as i32, n)
        }
    }

    pub fn unicode_length(bytes: &[u8]) -> (usize, bool, bool) {
        let mut num_chars = bytes.len();
        let mut has_multibyte = false;
        let mut is_latin1 = true;
        let mut prev = 0u8;
        for &c in bytes {
            if (c & 0xC0) == 0x80 {
                has_multibyte = true;
                if prev > 0xC3 {
                    is_latin1 = false;
                }
                num_chars -= 1;
            }
            prev = c;
        }
        (num_chars, is_latin1, has_multibyte)
    }

    pub fn convert_to_unicode(utf8_str: &[u8], unicode_str: &mut [u16]) {
        let mut index = 0usize;
        let mut ptr = 0usize;
        while index < unicode_str.len() && ptr < utf8_str.len() {
            let (v, n) = next(&utf8_str[ptr..]);
            unicode_str[index] = v;
            index += 1;
            ptr += n;
        }
    }

    pub fn quoted_ascii_length(utf8_str: &[u8]) -> usize {
        let mut result = 0usize;
        let mut ptr = 0usize;
        while ptr < utf8_str.len() {
            let (c, n) = next(&utf8_str[ptr..]);
            ptr += n;
            if (32..127).contains(&c) {
                result += 1;
            } else {
                result += 6;
            }
        }
        result
    }

    pub fn as_quoted_ascii(utf8_str: &[u8]) -> String {
        let mut result = String::new();
        let mut ptr = 0usize;
        while ptr < utf8_str.len() {
            let (c, n) = next(&utf8_str[ptr..]);
            ptr += n;
            if (32..127).contains(&c) {
                result.push(c as u8 as char);
            } else {
                result.push_str(&format!("\\u{:04x}", c));
            }
        }
        result
    }

    pub fn from_quoted_ascii(quoted: &str) -> Vec<u8> {
        let bytes = quoted.as_bytes();
        let mut end = 0usize;
        while end < bytes.len() && bytes[end] != b'\\' && bytes[end] >= 32 && bytes[end] < 127 {
            end += 1;
        }
        if end == bytes.len() {
            return bytes.to_vec();
        }
        let mut result = bytes[..end].to_vec();
        let mut i = end;
        while i < bytes.len() {
            if bytes[i] != b'\\' {
                result.push(bytes[i]);
                i += 1;
                continue;
            }
            if i + 1 >= bytes.len() {
                break;
            }
            match bytes[i + 1] {
                b'u' => {
                    if i + 5 >= bytes.len() {
                        break;
                    }
                    let hex = std::str::from_utf8(&bytes[i + 2..i + 6]).unwrap_or("0");
                    let v = u16::from_str_radix(hex, 16).unwrap_or(0);
                    let mut buf = Vec::new();
                    super::unicode::utf8_write_jchar(&mut buf, v);
                    result.extend_from_slice(&buf);
                    i += 6;
                }
                c => {
                    result.push(c);
                    i += 2;
                }
            }
        }
        result
    }

    pub fn equal(base1: &[u8], base2: &[u8]) -> bool {
        base1 == base2
    }

    pub fn is_supplementary_character(str_: &[u8]) -> bool {
        str_.len() >= 6
            && str_[0] == 0xED
            && (str_[1] & 0xF0) == 0xA0
            && (str_[2] & 0xC0) == 0x80
            && str_[3] == 0xED
            && (str_[4] & 0xF0) == 0xB0
            && (str_[5] & 0xC0) == 0x80
    }

    pub fn get_supplementary_character(str_: &[u8]) -> i32 {
        0x10000
            + (((str_[1] & 0x0F) as i32) << 16)
            + (((str_[2] & 0x3F) as i32) << 10)
            + (((str_[4] & 0x0F) as i32) << 6)
            + (str_[5] & 0x3F) as i32
    }

    pub fn is_legal_utf8(buffer: &[u8], version_leq_47: bool) -> bool {
        let mut i = 0usize;
        let length = buffer.len();
        let count = length >> 2;
        for _ in 0..count {
            let b0 = buffer[i];
            let b1 = buffer[i + 1];
            let b2 = buffer[i + 2];
            let b3 = buffer[i + 3];
            let res = b0 | b0.wrapping_sub(1) | b1 | b1.wrapping_sub(1) | b2 | b2.wrapping_sub(1) | b3 | b3.wrapping_sub(1);
            if res >= 128 {
                break;
            }
            i += 4;
        }
        while i < length {
            if buffer[i] == 0 {
                return false;
            }
            if buffer[i] < 128 {
                i += 1;
                continue;
            }
            if i + 5 < length && is_supplementary_character(&buffer[i..]) {
                i += 6;
                continue;
            }
            match buffer[i] >> 4 {
                0x8..=0xB | 0xF => return false,
                0xC | 0xD => {
                    let mut c = ((buffer[i] & 0x1F) as u16) << 6;
                    i += 1;
                    if i < length && (buffer[i] & 0xC0) == 0x80 {
                        c += (buffer[i] & 0x3F) as u16;
                        if version_leq_47 || c == 0 || c >= 0x80 {
                            i += 1;
                            continue;
                        }
                    }
                    return false;
                }
                0xE => {
                    let mut c = ((buffer[i] & 0x0F) as u16) << 12;
                    i += 2;
                    if i < length
                        && (buffer[i - 1] & 0xC0) == 0x80
                        && (buffer[i] & 0xC0) == 0x80
                    {
                        c += (((buffer[i - 1] & 0x3F) as u16) << 6) + (buffer[i] & 0x3F) as u16;
                        if version_leq_47 || c >= 0x800 {
                            i += 1;
                            continue;
                        }
                    }
                    return false;
                }
                _ => {
                    i += 1;
                }
            }
        }
        true
    }

    pub fn truncate_to_legal_utf8(buffer: &mut Vec<u8>) {
        assert!(buffer.len() > 5, "invalid length");
        if buffer[buffer.len() - 2] < 128 {
            return;
        }
        let mut i = buffer.len() - 1;
        loop {
            let b = buffer[i];
            if b < 0x80 {
                break;
            }
            if b >= 0xC0 {
                let need = if b < 0xE0 { 2 } else if b < 0xF0 { 3 } else { 6 };
                if i + need >= buffer.len() {
                    break;
                }
                let mut ok = true;
                for k in 1..need {
                    if buffer[i + k] & 0xC0 != 0x80 {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    break;
                }
                i += need;
                break;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        let new_len = if buffer[i] < 0x80 && i + 1 < buffer.len() { i + 1 } else { i };
        buffer.truncate(new_len);
        buffer.push(0);
    }

    pub fn strrchr(base: &[u8], c: u8) -> Option<usize> {
        base.iter().rposition(|&b| b == c)
    }
}

pub mod unicode {
    pub fn utf8_write_jchar(out: &mut Vec<u8>, ch: u16) {
        if ch != 0 && ch <= 0x7f {
            out.push(ch as u8);
        } else if ch <= 0x7FF {
            out.push(((ch >> 6) as u8) | 0xC0);
            out.push((ch as u8 & 0x3F) | 0x80);
        } else {
            out.push(((ch >> 12) as u8) | 0xE0);
            out.push((((ch >> 6) & 0x3F) as u8) | 0x80);
            out.push((ch as u8 & 0x3F) | 0x80);
        }
    }

    pub fn is_latin1(c: u16) -> bool {
        c <= 0xFF
    }

    pub fn is_latin1_str(base: &[u16]) -> bool {
        base.iter().all(|&c| c <= 0xFF)
    }

    pub fn utf8_size_jchar(c: u16) -> usize {
        if (0x0001..=0x007F).contains(&c) {
            1
        } else if c <= 0x07FF {
            2
        } else {
            3
        }
    }

    pub fn utf8_size_jbyte(c: i8) -> usize {
        if c >= 0x01 {
            1
        } else {
            2
        }
    }

    pub fn utf8_length_jchars(base: &[u16]) -> usize {
        base.iter().map(|&c| utf8_size_jchar(c)).sum()
    }

    pub fn utf8_length_jbytes(base: &[i8]) -> usize {
        base.iter().map(|&c| utf8_size_jbyte(c)).sum()
    }

    pub fn utf8_length_as_int_jchars(base: &[u16]) -> i32 {
        let mut result = 0usize;
        for &c in base {
            let sz = utf8_size_jchar(c);
            if result + sz > i32::MAX as usize - 1 {
                break;
            }
            result += sz;
        }
        result as i32
    }

    pub fn utf8_length_as_int_jbytes(base: &[i8]) -> i32 {
        let mut result = 0usize;
        for &c in base {
            let sz = utf8_size_jbyte(c);
            if result + sz > i32::MAX as usize - 1 {
                break;
            }
            result += sz;
        }
        result as i32
    }

    pub fn convert_to_utf8_jchars(base: &[u16]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(utf8_length_jchars(base) + 1);
        for &c in base {
            utf8_write_jchar(&mut buf, c);
        }
        buf.push(0);
        buf
    }

    pub fn convert_to_utf8_jbytes(base: &[i8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(utf8_length_jbytes(base) + 1);
        for &c in base {
            if utf8_size_jbyte(c) == 1 {
                buf.push(c as u8);
            } else {
                utf8_write_jchar(&mut buf, (c as u8) as u16 & 0xff);
            }
        }
        buf.push(0);
        buf
    }

    pub fn as_utf8_jchars(base: &[u16]) -> Vec<u8> {
        convert_to_utf8_jchars(base)
    }

    pub fn as_utf8_jbytes(base: &[i8]) -> Vec<u8> {
        convert_to_utf8_jbytes(base)
    }

    pub fn quoted_ascii_length_jchars(base: &[u16]) -> usize {
        base.iter()
            .map(|&c| if (32..127).contains(&c) { 1 } else { 6 })
            .sum()
    }

    pub fn quoted_ascii_length_jbytes(base: &[i8]) -> usize {
        base.iter()
            .map(|&c| if (32..127).contains(&c) { 1 } else { 6 })
            .sum()
    }

    pub fn as_quoted_ascii_jchars(base: &[u16]) -> String {
        let mut result = String::new();
        for &c in base {
            if (32..127).contains(&c) {
                result.push(c as u8 as char);
            } else {
                result.push_str(&format!("\\u{:04x}", c));
            }
        }
        result
    }

    pub fn as_quoted_ascii_jbytes(base: &[i8]) -> String {
        let mut result = String::new();
        for &c in base {
            if (32..127).contains(&c) {
                result.push(c as u8 as char);
            } else {
                result.push_str(&format!("\\u{:04x}", c as u8));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_decoding() {
        let (v, n) = utf8::next(b"A");
        assert_eq!((v, n), (65, 1));
        let (v, n) = utf8::next("é".as_bytes());
        assert_eq!((v, n), (0xE9, 2));
        let (v, n) = utf8::next("€".as_bytes());
        assert_eq!((v, n), (0x20AC, 3));
        let (v, n) = utf8::next(&[0xFF, 0x41]);
        assert_eq!((v, n), (0xFF, 1));
    }

    #[test]
    fn unicode_length_counts() {
        let (len, latin1, multi) = utf8::unicode_length("héllo".as_bytes());
        assert_eq!(len, 5);
        assert!(latin1);
        assert!(multi);
        let (len, latin1, multi) = utf8::unicode_length("€".as_bytes());
        assert_eq!(len, 1);
        assert!(!latin1);
        assert!(multi);
    }

    #[test]
    fn legal_utf8() {
        assert!(utf8::is_legal_utf8("hello".as_bytes(), false));
        assert!(utf8::is_legal_utf8("héllo€".as_bytes(), false));
        assert!(!utf8::is_legal_utf8(&[0x80, 0x80], false));
        assert!(!utf8::is_legal_utf8(b"a\0b", false));
        // modified UTF-8 encodes U+0000 as C0 80, which is always legal
        assert!(utf8::is_legal_utf8(&[0xC0, 0x80], true));
        assert!(utf8::is_legal_utf8(&[0xC0, 0x80], false));
        // overlong encoding of a non-zero char is illegal for newer versions
        assert!(!utf8::is_legal_utf8(&[0xC1, 0xBF], false));
        assert!(utf8::is_legal_utf8(&[0xC1, 0xBF], true));
    }

    #[test]
    fn modified_utf8_roundtrip() {
        let chars: Vec<u16> = vec!['h' as u16, 0, 'é' as u16, 0x20AC];
        let bytes = unicode::convert_to_utf8_jchars(&chars);
        assert_eq!(bytes[1..3], [0xC0, 0x80]);
        let back = unicode::as_utf8_jchars(&chars);
        assert_eq!(back, bytes);
        let jbytes: Vec<i8> = vec![b'h' as i8, 0, -61, -87];
        let b2 = unicode::convert_to_utf8_jbytes(&jbytes);
        assert_eq!(&b2[1..3], &[0xC0, 0x80]);
    }

    #[test]
    fn quoted_ascii() {
        let s = utf8::as_quoted_ascii("aéb".as_bytes());
        assert_eq!(s, "a\\u00e9b");
        let back = utf8::from_quoted_ascii(&s);
        assert_eq!(back, "aéb".as_bytes());
        let q = unicode::as_quoted_ascii_jchars(&[65, 233, 66]);
        assert_eq!(q, "A\\u00e9B");
    }

    #[test]
    fn supplementary() {
        let s = "\u{1F600}".as_bytes();
        let mut surrogate_utf8 = Vec::new();
        surrogate_utf8.extend_from_slice(&[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);
        assert!(utf8::is_supplementary_character(&surrogate_utf8));
        let (v, n) = utf8::next_character(&surrogate_utf8);
        assert_eq!(v, 0x1F600);
        assert_eq!(n, 6);
        let _ = s;
    }
}
