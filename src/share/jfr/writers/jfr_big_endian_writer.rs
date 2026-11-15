/*
 * OpenJDK 25 HotSpot jfr/writers/jfrBigEndianWriter.hpp port
 *
 * JfrBigEndianWriter — writes big-endian values into a fixed memory range
 * (used for the chunk header checkpoint content).
 */

pub struct JfrBigEndianWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
    valid: bool,
}

impl<'a> JfrBigEndianWriter<'a> {
    pub fn new(buf: &'a mut [u8]) -> JfrBigEndianWriter<'a> {
        JfrBigEndianWriter {
            buf,
            pos: 0,
            valid: true,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    fn ensure(&mut self, n: usize) -> bool {
        if self.pos + n > self.buf.len() {
            self.valid = false;
            return false;
        }
        true
    }

    pub fn write_u1(&mut self, v: u8) {
        if self.ensure(1) {
            self.buf[self.pos] = v;
            self.pos += 1;
        }
    }

    pub fn write_u2(&mut self, v: u16) {
        if self.ensure(2) {
            self.buf[self.pos..self.pos + 2].copy_from_slice(&v.to_be_bytes());
            self.pos += 2;
        }
    }

    pub fn write_u4(&mut self, v: u32) {
        if self.ensure(4) {
            self.buf[self.pos..self.pos + 4].copy_from_slice(&v.to_be_bytes());
            self.pos += 4;
        }
    }

    pub fn write_u8(&mut self, v: u64) {
        if self.ensure(8) {
            self.buf[self.pos..self.pos + 8].copy_from_slice(&v.to_be_bytes());
            self.pos += 8;
        }
    }

    pub fn write_i8(&mut self, v: i64) {
        self.write_u8(v as u64);
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        if self.ensure(data.len()) {
            self.buf[self.pos..self.pos + data.len()].copy_from_slice(data);
            self.pos += data.len();
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }
}
