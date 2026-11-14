use super::bytecodeInterpreter::Value;
use std::io::{self, Error};

pub fn binary<'a>(opcode: u8, left: Value<'a>, right: Value<'a>) -> io::Result<Value<'a>> {
    let invalid = || Error::other("Invalid numeric operands");
    let operation = (opcode - 0x60) / 4;
    match (opcode % 4, left, right) {
        (1, Value::Long(left), Value::Long(right)) => {
            if matches!(operation, 3 | 4) && right == 0 {
                return Err(Error::other("Long division by zero"));
            }
            Ok(Value::Long(match operation {
                0 => left.wrapping_add(right),
                1 => left.wrapping_sub(right),
                2 => left.wrapping_mul(right),
                3 => left.wrapping_div(right),
                4 => left.wrapping_rem(right),
                _ => return Err(invalid()),
            }))
        }
        (2, Value::Float(left), Value::Float(right)) => Ok(Value::Float(match operation {
            0 => left + right,
            1 => left - right,
            2 => left * right,
            3 => left / right,
            4 => left % right,
            _ => return Err(invalid()),
        })),
        (3, Value::Double(left), Value::Double(right)) => Ok(Value::Double(match operation {
            0 => left + right,
            1 => left - right,
            2 => left * right,
            3 => left / right,
            4 => left % right,
            _ => return Err(invalid()),
        })),
        _ => Err(invalid()),
    }
}

pub fn convert(opcode: u8, value: Value<'_>) -> io::Result<Value<'_>> {
    use Value::{Double, Float, Int, Long};
    Ok(match (opcode, value) {
        (0x85, Int(x)) => Long(i64::from(x)),
        (0x86, Int(x)) => Float(x as f32),
        (0x87, Int(x)) => Double(f64::from(x)),
        (0x88, Long(x)) => Int(x as i32),
        (0x89, Long(x)) => Float(x as f32),
        (0x8a, Long(x)) => Double(x as f64),
        (0x8b, Float(x)) => Int(x as i32),
        (0x8c, Float(x)) => Long(x as i64),
        (0x8d, Float(x)) => Double(f64::from(x)),
        (0x8e, Double(x)) => Int(x as i32),
        (0x8f, Double(x)) => Long(x as i64),
        (0x90, Double(x)) => Float(x as f32),
        (0x91, Int(x)) => Int(i32::from(x as i8)),
        (0x92, Int(x)) => Int(i32::from(x as u16)),
        (0x93, Int(x)) => Int(i32::from(x as i16)),
        _ => return Err(Error::other("Invalid conversion operand")),
    })
}

pub fn compare(opcode: u8, left: Value<'_>, right: Value<'_>) -> io::Result<i32> {
    use Value::{Double, Float, Long};
    let order = match (opcode, left, right) {
        (0x94, Long(a), Long(b)) => Some(a.cmp(&b)),
        (0x95 | 0x96, Float(a), Float(b)) => a.partial_cmp(&b),
        (0x97 | 0x98, Double(a), Double(b)) => a.partial_cmp(&b),
        _ => return Err(Error::other("Invalid comparison operands")),
    };
    Ok(order.map_or(
        if matches!(opcode, 0x95 | 0x97) { -1 } else { 1 },
        |order| order as i32,
    ))
}
