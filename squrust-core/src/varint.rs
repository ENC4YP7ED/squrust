//! SQLite-style big-endian base-128 varints (1–9 bytes).
//!
//! Bytes 1–8 contribute 7 bits each, high bit set means "more follows". A 9th
//! byte, if present, contributes all 8 bits, giving the full 64-bit range.
//! See <https://www.sqlite.org/fileformat.html#varint>.

/// Number of bytes [`write`] would emit for `v`.
pub fn len(v: u64) -> usize {
    match v {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0xfff_ffff => 4,
        0x1000_0000..=0x7_ffff_ffff => 5,
        0x8_0000_0000..=0x3ff_ffff_ffff => 6,
        0x400_0000_0000..=0x1_ffff_ffff_ffff => 7,
        0x2_0000_0000_0000..=0xff_ffff_ffff_ffff => 8,
        _ => 9,
    }
}

/// Append the varint encoding of `v` to `out`. Returns the number of bytes written.
pub fn write(out: &mut Vec<u8>, v: u64) -> usize {
    if v > 0x00ff_ffff_ffff_ffff {
        // 9-byte form: 8 groups of 7 bits then a final full byte.
        let mut buf = [0u8; 9];
        buf[8] = v as u8;
        let mut x = v >> 8;
        for i in (0..8).rev() {
            buf[i] = (x as u8 & 0x7f) | 0x80;
            x >>= 7;
        }
        out.extend_from_slice(&buf);
        return 9;
    }
    let n = len(v);
    let mut buf = [0u8; 9];
    let mut x = v;
    for i in (0..n).rev() {
        buf[i] = (x as u8 & 0x7f) | 0x80;
        x >>= 7;
    }
    buf[n - 1] &= 0x7f; // clear continuation bit on the last byte
    out.extend_from_slice(&buf[..n]);
    n
}

/// Read a varint from the front of `data`. Returns `(value, bytes_consumed)`.
pub fn read(data: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    for i in 0..8 {
        let byte = *data.get(i)?;
        if i == 8 {
            break;
        }
        if byte & 0x80 == 0 {
            result = (result << 7) | (byte as u64);
            return Some((result, i + 1));
        }
        result = (result << 7) | ((byte & 0x7f) as u64);
    }
    // 9th byte contributes all 8 bits.
    let byte = *data.get(8)?;
    result = (result << 8) | (byte as u64);
    Some((result, 9))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(v: u64) {
        let mut buf = Vec::new();
        let n = write(&mut buf, v);
        assert_eq!(n, buf.len());
        assert_eq!(n, len(v), "len() disagrees for {v}");
        let (back, consumed) = read(&buf).unwrap();
        assert_eq!(back, v, "value mismatch");
        assert_eq!(consumed, n, "consumed mismatch for {v}");
    }

    #[test]
    fn roundtrips() {
        for v in [
            0u64,
            1,
            127,
            128,
            16383,
            16384,
            0xff,
            0xffff,
            0x10_0000,
            1 << 35,
            1 << 49,
            1 << 56,
            u64::MAX,
            u64::MAX - 1,
            i64::MAX as u64,
        ] {
            roundtrip(v);
        }
    }

    #[test]
    fn known_encodings() {
        let mut b = Vec::new();
        write(&mut b, 1);
        assert_eq!(b, vec![0x01]);
        b.clear();
        write(&mut b, 128);
        assert_eq!(b, vec![0x81, 0x00]);
        b.clear();
        write(&mut b, 300);
        assert_eq!(b, vec![0x82, 0x2c]);
    }
}
