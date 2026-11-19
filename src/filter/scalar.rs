//! The oracle: Sub, Up, Average and Paeth as Go's `image/png` computes them. Each fn handles the
//! bytes from `start` on, so a SIMD kernel can hand over its tail. Arguments are validated by
//! the caller (`bpp >= 1`, equal lengths).

pub(super) fn run(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8], start: usize) {
    match filter {
        1 => sub(cur, bpp, start),
        2 => up(cur, prev, start),
        3 => avg(cur, prev, bpp, start),
        4 => paeth(cur, prev, bpp, start),
        _ => {}
    }
}

/// The byte `bpp` before `j` (0 before the row start).
fn left(row: &[u8], j: usize, bpp: usize) -> u8 {
    j.checked_sub(bpp)
        .and_then(|k| row.get(k))
        .copied()
        .unwrap_or(0)
}

pub(super) fn sub(cur: &mut [u8], bpp: usize, start: usize) {
    for j in start..cur.len() {
        let a = left(cur, j, bpp);
        if let Some(c) = cur.get_mut(j) {
            *c = c.wrapping_add(a);
        }
    }
}

pub(super) fn up(cur: &mut [u8], prev: &[u8], start: usize) {
    for (c, &p) in cur.iter_mut().zip(prev).skip(start) {
        *c = c.wrapping_add(p);
    }
}

pub(super) fn avg(cur: &mut [u8], prev: &[u8], bpp: usize, start: usize) {
    for j in start..cur.len() {
        let a = u16::from(left(cur, j, bpp));
        let b = u16::from(prev.get(j).copied().unwrap_or(0));
        if let Some(c) = cur.get_mut(j) {
            *c = c.wrapping_add(((a + b) / 2) as u8);
        }
    }
}

pub(super) fn paeth(cur: &mut [u8], prev: &[u8], bpp: usize, start: usize) {
    for j in start..cur.len() {
        let a = i32::from(left(cur, j, bpp));
        let b = i32::from(prev.get(j).copied().unwrap_or(0));
        let c = i32::from(left(prev, j, bpp));
        let pa = (b - c).abs();
        let pb = (a - c).abs();
        let pc = (a + b - 2 * c).abs();
        let p = if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        };
        if let Some(x) = cur.get_mut(j) {
            *x = x.wrapping_add(p as u8);
        }
    }
}
