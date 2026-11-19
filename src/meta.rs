//! Ancillary chunks: validation of the chunk data (as libpng 1.6 validates it), the zlib helper
//! for iCCP / zTXt / iTXt, and the pool that keeps the `Info` vectors between calls.

use crate::engine::ZLIB_RESERVE;
use crate::error::{Error, resource};
use crate::info::{Apng, Cicp, Header, Info, TextKind, Trns};
use crate::ledger::Ledger;
use crate::source::be32;
use std::io::Read;
use std::sync::atomic::AtomicBool;

/// gAMA: the gamma times 100000. 0 and values over `i32::MAX` are invalid.
pub(crate) fn gama(d: &[u8]) -> Option<u32> {
    let v = be32(d, 0);
    (d.len() == 4 && v != 0 && v <= 0x7fff_ffff).then_some(v)
}

/// cHRM: white point, red, green, blue as x, y pairs times 100000. As libpng 1.5's check: the
/// white y is positive and no x + y is over 1.
pub(crate) fn chrm(d: &[u8]) -> Option<[u32; 8]> {
    if d.len() != 32 {
        return None;
    }
    let mut v = [0u32; 8];
    for (slot, i) in v.iter_mut().zip(0..) {
        *slot = be32(d, i * 4);
    }
    let ok = v.iter().all(|&x| x <= 0x7fff_ffff)
        && v.get(1).is_some_and(|&wy| wy > 0)
        && v.as_chunks::<2>()
            .0
            .iter()
            .all(|&[x, y]| u64::from(x) + u64::from(y) <= 100_000);
    ok.then_some(v)
}

/// sRGB: the rendering intent, 0 to 3.
pub(crate) fn srgb(d: &[u8]) -> Option<u8> {
    match d {
        &[v] if v <= 3 => Some(v),
        _ => None,
    }
}

/// cICP.
pub(crate) fn cicp(d: &[u8]) -> Option<Cicp> {
    match d {
        &[primaries, transfer, matrix, full_range] if full_range <= 1 => Some(Cicp {
            primaries,
            transfer,
            matrix,
            full_range,
        }),
        _ => None,
    }
}

/// acTL: at least one frame, both numbers under 2^31.
pub(crate) fn actl(d: &[u8]) -> Option<Apng> {
    if d.len() != 8 {
        return None;
    }
    let (num_frames, num_plays) = (be32(d, 0), be32(d, 4));
    (num_frames != 0 && num_frames <= 0x7fff_ffff && num_plays <= 0x7fff_ffff).then_some(Apng {
        num_frames,
        num_plays,
    })
}

/// iCCP: the profile name (1 to 79 bytes) and the zlib stream of the profile. The compression
/// method byte must be 0.
pub(crate) fn iccp_parts(d: &[u8]) -> Option<(&[u8], &[u8])> {
    let nul = d.iter().take(80).position(|&b| b == 0)?;
    if nul == 0 {
        return None;
    }
    let (name, rest) = d.split_at(nul);
    match rest {
        [_, 0, z @ ..] => Some((name, z)),
        _ => None,
    }
}

/// libpng's check of a decompressed profile: a header of 132 bytes whose first four bytes are the
/// length of the whole profile.
pub(crate) fn icc_plausible(profile: &[u8]) -> bool {
    profile.len() >= 132 && usize::try_from(be32(profile, 0)).is_ok_and(|n| n == profile.len())
}

/// What a text chunk needs checked: `None` is an invalid chunk, `Some(None)` a valid one, and
/// `Some(Some(z))` one that is valid if the zlib stream `z` inflates.
pub(crate) fn text_parts(kind: TextKind, d: &[u8]) -> Option<Option<&[u8]>> {
    match kind {
        TextKind::Text => Some(None),
        TextKind::Ztxt => {
            let nul = d.iter().position(|&b| b == 0)?;
            match d.get(nul..)? {
                [_, 0, z @ ..] => Some(Some(z)),
                _ => None,
            }
        }
        TextKind::Itxt => {
            let nul = d.iter().position(|&b| b == 0)?;
            let [_, flag, method, rest @ ..] = d.get(nul..)? else {
                return None;
            };
            if *flag > 1 || (*flag == 1 && *method != 0) {
                return None;
            }
            // The language tag and the translated keyword, each ended by a NUL.
            let lang = rest.iter().position(|&b| b == 0)?;
            let rest = rest.get(lang + 1..)?;
            let tkey = rest.iter().position(|&b| b == 0)?;
            let text = rest.get(tkey + 1..)?;
            Some((*flag == 1).then_some(text))
        }
    }
}

/// Why an inflate of chunk data failed.
#[derive(Debug)]
pub(crate) enum Inf {
    /// The stream is invalid.
    Bad,
    /// The output would pass the cap.
    Limit,
    /// Memory or cancellation: the call fails with this.
    Fatal(Error),
}

fn inf(e: &deflate::Error) -> Inf {
    match e {
        deflate::Error::OutputLimit => Inf::Limit,
        e => resource(e).map_or(Inf::Bad, Inf::Fatal),
    }
}

/// Bytes of the output `Vec` that one `read` call may fill (zeroed first).
const READ_STEP: usize = 64 << 10;

fn io_inf(e: &std::io::Error) -> Inf {
    match crate::source::classify(e) {
        crate::source::ReadErr::Flate(f) => inf(&f),
        _ => Inf::Bad,
    }
}

/// Reads `r` to its end into `out`. The `Vec` grows in steps, each booked in the ledger before
/// it is allocated; the capacity it holds on entry is booked by the caller.
fn fill<R: Read>(
    r: &mut R,
    out: &mut Vec<u8>,
    max_output: usize,
    ledger: &Ledger,
) -> Result<(), Inf> {
    loop {
        let n = out.len();
        if n == out.capacity() {
            // One byte more than the cap, so that a stream that passes it is seen.
            let step = n
                .clamp(4096, 1 << 20)
                .min(max_output.saturating_sub(n).saturating_add(1));
            ledger.reserve(step).map_err(Inf::Fatal)?;
            out.try_reserve_exact(step)
                .map_err(|_| Inf::Fatal(Error::OutOfMemory))?;
        }
        let end = out.capacity().min(n.saturating_add(READ_STEP));
        out.resize(end, 0);
        let got = match out.get_mut(n..) {
            Some(buf) => r.read(buf),
            None => Ok(0),
        };
        match got {
            Ok(g) => {
                out.truncate(n.saturating_add(g).min(end));
                if g == 0 {
                    return Ok(());
                }
            }
            Err(e) => {
                out.truncate(n);
                return Err(io_inf(&e));
            }
        }
    }
}

/// Inflates the whole zlib stream `z`: into `out` (grown in booked steps), or just checks it
/// when `out` is `None`. The reader's window is booked while it runs and freed when it ends;
/// `zbuf` is left empty.
pub(crate) fn inflate(
    z: &[u8],
    zbuf: &mut Vec<u8>,
    out: Option<&mut Vec<u8>>,
    max_output: usize,
    ledger: &Ledger,
    cancel: &AtomicBool,
) -> Result<(), Inf> {
    // With an output vector the window shares what is left with it; a check-only inflate
    // can use it all.
    let left = if out.is_some() {
        ledger.left() / 2
    } else {
        ledger.left()
    };
    let budget = left.min(ZLIB_RESERVE);
    let _held = ledger.charge(budget).map_err(Inf::Fatal)?;
    let dl = deflate::Limits {
        max_output,
        max_alloc: budget,
    };
    let mut r = deflate::zlib::Reader::with_buffer(z, std::mem::take(zbuf), &dl, cancel)
        .map_err(|e| inf(&e))?;
    let res = match out {
        Some(o) => fill(&mut r, o, max_output, ledger),
        None => {
            let mut tmp = [0u8; 512];
            loop {
                match r.read(&mut tmp) {
                    Ok(0) => break Ok(()),
                    Ok(_) => {}
                    Err(e) => break Err(io_inf(&e)),
                }
            }
        }
    };
    // The window is freed here, not kept in `zbuf`: its booking ends with this call, and a
    // kept buffer would be held without a charge until the IDAT reader books it.
    drop(r);
    res
}

/// Eight vectors that the `Info` of an earlier call held, so that the next call fills the
/// same memory (a `Decoder` reused on files with metadata allocates nothing new).
#[derive(Debug, Default)]
pub(crate) struct Pool {
    v: [Vec<u8>; 8],
}

impl Pool {
    pub(crate) fn give(&mut self, mut x: Vec<u8>) {
        x.clear();
        if x.capacity() == 0 {
            return;
        }
        if let Some(slot) = self.v.iter_mut().find(|s| s.capacity() == 0) {
            *slot = x;
        }
    }

    /// An empty vector: the smallest kept one that holds `n` bytes, else the biggest one (also
    /// for `n == 0`, the size is not known yet).
    pub(crate) fn take(&mut self, n: usize) -> Vec<u8> {
        let fit = (n > 0)
            .then(|| {
                self.v
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.capacity() >= n)
                    .min_by_key(|(_, v)| v.capacity())
                    .map(|(i, _)| i)
            })
            .flatten();
        let pick = fit.or_else(|| {
            self.v
                .iter()
                .enumerate()
                .max_by_key(|(_, v)| v.capacity())
                .map(|(i, _)| i)
        });
        pick.and_then(|i| self.v.get_mut(i))
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Clears `info` for the next call and keeps its vectors.
    pub(crate) fn recycle(&mut self, info: &mut Info) {
        for t in info.text.drain(..) {
            self.give(t.data);
        }
        if let Some(e) = info.exif.take() {
            self.give(e);
        }
        if let Some(i) = info.iccp.take() {
            self.give(i.name);
            self.give(i.profile);
        }
        if let Some(Trns::Alpha(v)) = info.trns.take() {
            self.give(v);
        }
        info.palette.clear();
        info.header = Header::default();
        info.trns_cancelled = false;
        info.gamma = None;
        info.chrm = None;
        info.srgb = None;
        info.cicp = None;
        info.apng = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_chunks() {
        assert_eq!(gama(&[0, 0, 0x88, 0xb8]), Some(35000));
        assert_eq!(gama(&[0, 0, 0, 0]), None);
        assert_eq!(gama(&[0, 0, 0]), None);
        assert_eq!(srgb(&[3]), Some(3));
        assert_eq!(srgb(&[4]), None);
        assert_eq!(srgb(&[]), None);
        assert_eq!(cicp(&[1, 13, 0, 1]).map(|c| c.transfer), Some(13));
        assert_eq!(cicp(&[1, 13, 0, 2]), None);
        assert_eq!(
            actl(&[0, 0, 0, 2, 0, 0, 0, 0]),
            Some(Apng {
                num_frames: 2,
                num_plays: 0
            })
        );
        assert_eq!(actl(&[0; 8]), None);
        let mut c = Vec::new();
        for v in [31270u32, 32900, 64000, 33000, 30000, 60000, 15000, 6000] {
            c.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(chrm(&c).map(|v| v[0]), Some(31270));
        // White point y = 0.
        c.get_mut(4..8)
            .unwrap()
            .copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(chrm(&c), None);
        assert_eq!(chrm(&c[..31]), None);
    }

    #[test]
    fn text_layouts() {
        assert_eq!(text_parts(TextKind::Text, b"k\0v"), Some(None));
        assert_eq!(
            text_parts(TextKind::Ztxt, b"k\0\0zz"),
            Some(Some(&b"zz"[..]))
        );
        assert_eq!(text_parts(TextKind::Ztxt, b"k\0\x01zz"), None);
        assert_eq!(text_parts(TextKind::Ztxt, b"k"), None);
        // iTXt: keyword, flag, method, language, translated keyword, text.
        assert_eq!(
            text_parts(TextKind::Itxt, b"k\0\0\0en\0kw\0text"),
            Some(None)
        );
        assert_eq!(
            text_parts(TextKind::Itxt, b"k\0\x01\0\0\0zz"),
            Some(Some(&b"zz"[..]))
        );
        assert_eq!(text_parts(TextKind::Itxt, b"k\0\x02\0\0\0t"), None);
        assert_eq!(text_parts(TextKind::Itxt, b"k\0\0\0en"), None);
    }

    #[test]
    fn iccp_layout() {
        assert_eq!(iccp_parts(b"sRGB\0\0zz"), Some((&b"sRGB"[..], &b"zz"[..])));
        assert_eq!(iccp_parts(b"\0\0zz"), None);
        assert_eq!(iccp_parts(b"sRGB\0\x01zz"), None);
        assert_eq!(iccp_parts(&[b'a'; 90]), None);
        let mut p = vec![0u8; 132];
        p.get_mut(..4)
            .unwrap()
            .copy_from_slice(&132u32.to_be_bytes());
        assert!(icc_plausible(&p));
        p.push(0);
        assert!(!icc_plausible(&p));
    }

    #[test]
    fn pool_reuses_vectors() {
        let mut pool = Pool::default();
        let mut a = Vec::with_capacity(100);
        a.push(1);
        pool.give(a);
        pool.give(Vec::with_capacity(10));
        pool.give(Vec::new());
        let small = pool.take(5);
        assert!(small.is_empty() && small.capacity() == 10);
        let big = pool.take(50);
        assert_eq!(big.capacity(), 100);
        assert_eq!(pool.take(1).capacity(), 0);
    }
}
