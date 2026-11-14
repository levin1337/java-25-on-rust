use std::io::{self, Error};

pub fn method(
    descriptor: &[u8],
) -> io::Result<(impl Iterator<Item = io::Result<u8>> + '_, Option<u8>)> {
    let invalid = || Error::other("Invalid method descriptor");
    let descriptor = descriptor.strip_prefix(b"(").ok_or_else(invalid)?;
    let end = descriptor
        .iter()
        .position(|byte| *byte == b')')
        .ok_or_else(invalid)?;
    let (mut parameters, suffix) = descriptor.split_at(end);
    let mut result = &suffix[1..];
    let kind = if result == b"V" {
        None
    } else {
        let kind = field(&mut result)?;
        if !result.is_empty() {
            return Err(invalid());
        }
        Some(kind)
    };
    let parameters = std::iter::from_fn(move || {
        if parameters.is_empty() {
            return None;
        }
        let value = field(&mut parameters);
        if value.is_err() {
            parameters = &[];
        }
        Some(value)
    });
    Ok((parameters, kind))
}

pub(crate) fn fieldKind(mut descriptor: &[u8]) -> io::Result<u8> {
    let kind = field(&mut descriptor)?;
    if !descriptor.is_empty() {
        return Err(Error::other("Invalid field descriptor"));
    }
    Ok(kind)
}

fn field(bytes: &mut &[u8]) -> io::Result<u8> {
    let invalid = || Error::other("Invalid field descriptor");
    let dimensions = bytes.iter().take_while(|byte| **byte == b'[').count();
    if dimensions > 255 {
        return Err(invalid());
    }
    let (&tag, rest) = bytes[dimensions..].split_first().ok_or_else(invalid)?;
    *bytes = rest;
    let kind = match tag {
        b'B' | b'C' | b'I' | b'S' | b'Z' => 0,
        b'J' => 1,
        b'F' => 2,
        b'D' => 3,
        b'L' => {
            let end = rest
                .iter()
                .position(|byte| *byte == b';')
                .filter(|end| *end != 0)
                .ok_or_else(invalid)?;
            *bytes = &rest[end + 1..];
            4
        }
        _ => return Err(invalid()),
    };
    Ok(if dimensions == 0 { kind } else { 4 })
}
