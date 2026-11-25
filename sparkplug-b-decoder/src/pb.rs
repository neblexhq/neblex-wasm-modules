//! Minimal protobuf wire format reader and writer (proto2, no code generation).

pub enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut v: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos).ok_or("payload ends inside a number")?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("number longer than 10 bytes".into())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.buf.len()).ok_or("payload ends inside a field")?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// The next (field number, value), or None at the end.
    pub fn next(&mut self) -> Result<Option<(u32, Wire<'a>)>, String> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = (key >> 3) as u32;
        if field == 0 {
            return Err("field number 0: this is not a Sparkplug B payload".into());
        }
        let v = match key & 7 {
            0 => Wire::Varint(self.varint()?),
            1 => Wire::Fixed64(u64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            2 => {
                let n = self.varint()? as usize;
                Wire::Bytes(self.take(n)?)
            }
            5 => Wire::Fixed32(u32::from_le_bytes(self.take(4)?.try_into().unwrap())),
            t => return Err(format!("unsupported protobuf wire type {t}: this is not a Sparkplug B payload")),
        };
        Ok(Some((field, v)))
    }
}

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    fn raw_varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.buf.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.buf.push(v as u8);
    }
    fn key(&mut self, field: u32, wire: u64) {
        self.raw_varint((u64::from(field) << 3) | wire);
    }
    pub fn varint(&mut self, field: u32, v: u64) {
        self.key(field, 0);
        self.raw_varint(v);
    }
    pub fn fixed32(&mut self, field: u32, v: u32) {
        self.key(field, 5);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn fixed64(&mut self, field: u32, v: u64) {
        self.key(field, 1);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, field: u32, b: &[u8]) {
        self.key(field, 2);
        self.raw_varint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }
}
