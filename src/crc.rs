//! CRC-32 (IEEE, the PNG chunk CRC): slicing-by-8, safe code.

// Const-evaluated: an out-of-range index would fail the build, not run.
#[allow(clippy::indexing_slicing)]
const TABLES: [[u32; 256]; 8] = {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut j = 1;
    while j < 8 {
        let mut i = 0;
        while i < 256 {
            let p = t[j - 1][i];
            t[j][i] = t[0][(p & 0xFF) as usize] ^ (p >> 8);
            i += 1;
        }
        j += 1;
    }
    t
};

/// One table entry. `i` is a byte, so the index is always inside the 256 entries.
#[allow(clippy::indexing_slicing)]
#[inline(always)]
fn tab(k: usize, i: u32) -> u32 {
    TABLES[k & 7][(i & 0xFF) as usize]
}

/// The CRC of `p` continued from `crc` (0 to start): Go's `crc32.Update(crc, IEEETable, p)`.
#[must_use]
pub(crate) fn update(crc: u32, p: &[u8]) -> u32 {
    let mut c = !crc;
    let (blocks, rest) = p.as_chunks::<8>();
    for &[b0, b1, b2, b3, b4, b5, b6, b7] in blocks {
        let lo = c ^ u32::from_le_bytes([b0, b1, b2, b3]);
        c = tab(7, lo)
            ^ tab(6, lo >> 8)
            ^ tab(5, lo >> 16)
            ^ tab(4, lo >> 24)
            ^ tab(3, u32::from(b4))
            ^ tab(2, u32::from(b5))
            ^ tab(1, u32::from(b6))
            ^ tab(0, u32::from(b7));
    }
    for &b in rest {
        c = tab(0, c ^ u32::from(b)) ^ (c >> 8);
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytewise(p: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in p {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }

    #[test]
    fn crc_matches_the_ieee_check_value() {
        assert_eq!(update(0, b"123456789"), 0xcbf4_3926);
        assert_eq!(update(update(0, b"1234"), b"56789"), 0xcbf4_3926);
        assert_eq!(update(0, b""), 0);
    }

    #[test]
    fn slicing_equals_bytewise() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 31 + 7) as u8).collect();
        for n in [0, 1, 7, 8, 9, 63, 64, 65, 1000] {
            assert_eq!(update(0, &data[..n]), bytewise(&data[..n]), "n = {n}");
        }
    }
}
