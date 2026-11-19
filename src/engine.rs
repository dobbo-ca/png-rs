//! The decode engine of one call: the state of the chunk loop, the row reader (the 4096-byte
//! buffered IDAT reader under deflate-rs's zlib reader, unfilter, conversion, Adam7 merge) and
//! the threaded path (inflate of the whole concatenated IDAT data).

use crate::crc::update as crc_update;
use crate::error::Error;
use crate::filter::unfilter;
use crate::info::{Format, Header, Info, Profile};
use crate::ledger::{Held, Ledger, zeroed};
use crate::limits::Limits;
use crate::meta::Pool;
use crate::pixels::{
    Conv, Pass, convert_flat, filter_bpp, filtered_len, pass, pass_count, scatter_bits,
    scatter_bytes, to_eight,
};
use crate::source::{
    Idat, Pos, ReadErr, Src, Stream, be32, classify, classify_flate, init_error, row_error,
    trailing_error,
};
use deflate::zlib::Reader;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

/// What deflate-rs's reader may use besides the output it hands out: its window and one step.
pub(crate) const ZLIB_RESERVE: usize = 256 << 10;

/// The threaded path needs at least this much compressed IDAT data.
pub(crate) const PAR_MIN: usize = 4 << 20;

const PNG_HEADER: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// `Err(Cancelled)` once the flag is set.
pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// The decoding stage (Go's `ds*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Stage {
    Start,
    SeenIhdr,
    SeenPlte,
    SeenTrns,
    SeenIdat,
    SeenIend,
}

/// What a chunk was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Next,
    /// The first IDAT chunk: its header is read and `idat_length` is set.
    Idat,
    Iend,
}

/// The zlib reader over the IDAT chunks.
pub(crate) type Zr<'d> = Reader<'d, Idat<'d>>;

/// Buffers a `Decoder` keeps from call to call.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    /// The row being read and the one before it, each with its filter byte first.
    pub(crate) cur: Vec<u8>,
    pub(crate) prev: Vec<u8>,
    /// One converted row of an interlaced pass (Eight).
    pub(crate) tmp: Vec<u8>,
    /// The window of deflate-rs's reader.
    pub(crate) zbuf: Vec<u8>,
    /// The merged image of an interlaced `Rows`.
    pub(crate) image: Vec<u8>,
    /// The threaded path: the concatenated IDAT data and its inflated bytes.
    pub(crate) cat: Vec<u8>,
    pub(crate) filtered: Vec<u8>,
    pub(crate) pool: Pool,
}

/// Where a row goes.
pub(crate) enum Dst<'a> {
    /// The output row itself.
    Flat(&'a mut [u8]),
    /// The whole output image (and its row length): the pixels of an Adam7 pass are merged in.
    Merge(&'a mut [u8], usize),
}

/// The state of one call.
pub(crate) struct Eng<'d> {
    pub(crate) s: Stream<'d>,
    pub(crate) prof: Profile,
    pub(crate) limits: Limits,
    pub(crate) cancel: &'d AtomicBool,
    pub(crate) ledger: &'d Ledger,
    pub(crate) info: &'d mut Info,
    pub(crate) sc: &'d mut Scratch,
    pub(crate) stage: Stage,
    pub(crate) have_ihdr: bool,
    pub(crate) have_plte: bool,
    pub(crate) have_idat: bool,
    /// Bytes of ancillary data kept so far.
    pub(crate) meta_used: usize,
    pub(crate) conv: Conv,
    /// Bytes per output row.
    pub(crate) out_rb: usize,
    /// Bytes per pixel for the unfilter.
    bpp: usize,
    z_budget: usize,
    z_held: Option<Held<'d>>,
    /// The threaded path's buffers (`cat`, `filtered`), booked until the call ends.
    par_held: Option<Held<'d>>,
    /// The least compressed IDAT data that takes the threaded path.
    pub(crate) par_min: usize,
    /// Where the IDAT data starts in the input and how long the first chunk is.
    idat0: (usize, usize),
}

impl<'d> Eng<'d> {
    /// Starts a call: `preload` bytes of the budget are taken by the caller already.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        data: &'d [u8],
        pos: &'d Pos,
        prof: Profile,
        limits: &Limits,
        cancel: &'d AtomicBool,
        ledger: &'d Ledger,
        info: &'d mut Info,
        sc: &'d mut Scratch,
        preload: usize,
    ) -> Result<Self, Error> {
        ledger.reset(limits.max_alloc, preload)?;
        pos.reset();
        sc.pool.recycle(info);
        Ok(Eng {
            s: Stream::new(data, pos),
            prof,
            limits: *limits,
            cancel,
            ledger,
            info,
            sc,
            stage: Stage::Start,
            have_ihdr: false,
            have_plte: false,
            have_idat: false,
            meta_used: 0,
            conv: Conv::new(&Info::default(), Format::Native, prof),
            out_rb: 0,
            bpp: 1,
            z_budget: 0,
            z_held: None,
            par_held: None,
            par_min: PAR_MIN,
            idat0: (0, 0),
        })
    }

    /// `checkHeader`, then every chunk up to the first IDAT chunk, whose header is read.
    pub(crate) fn read_to_idat(&mut self) -> Result<(), Error> {
        check_cancel(self.cancel)?;
        let sig = self.s.read_full(PNG_HEADER.len())?;
        if sig != PNG_HEADER.as_slice() {
            return Err(Error::NotPng);
        }
        loop {
            match self.chunk()? {
                Flow::Next => {}
                Flow::Idat => return Ok(()),
                Flow::Iend => return Err(Error::ChunkOrder),
            }
        }
    }

    /// Sizes the row buffers (charged) for `fmt` and takes the share of the budget that the zlib
    /// reader may use.
    pub(crate) fn prepare(&mut self, fmt: Format) -> Result<(), Error> {
        let hdr = self.info.header;
        self.conv = Conv::new(self.info, fmt, self.prof);
        self.out_rb = self.info.row_bytes(fmt).ok_or(Error::AllocLimit)?;
        self.bpp = filter_bpp(&hdr);
        // The widest row of any pass is the whole width.
        let width = usize::try_from(hdr.width).map_err(|_| Error::AllocLimit)?;
        let row = hdr
            .native_row_bytes(width)
            .and_then(|b| b.checked_add(1))
            .ok_or(Error::AllocLimit)?;
        let ledger: &'d Ledger = self.ledger;
        zeroed(&mut self.sc.cur, row, ledger)?;
        zeroed(&mut self.sc.prev, row, ledger)?;
        if hdr.interlaced && fmt == Format::Eight {
            zeroed(&mut self.sc.tmp, self.out_rb, ledger)?;
        }
        // deflate-rs charges its window and output to its own `max_alloc`; give it a share of
        // ours, so that the peak stays under `limits.max_alloc`.
        self.z_budget = ledger.left().min(ZLIB_RESERVE);
        self.z_held = Some(ledger.charge(self.z_budget)?);
        Ok(())
    }

    /// The zlib reader over the IDAT chunks; the zlib header is read.
    pub(crate) fn start_zlib(&mut self) -> Result<Zr<'d>, Error> {
        let dl = deflate::Limits {
            max_output: usize::MAX >> 1,
            max_alloc: self.z_budget,
        };
        let buf = std::mem::take(&mut self.sc.zbuf);
        self.idat0 = (
            self.s.pos(),
            usize::try_from(self.s.idat_length()).unwrap_or(0),
        );
        Reader::with_buffer(Idat::new(self.s), buf, &dl, self.cancel)
            .map_err(|e| init_error(classify_flate(e)))
    }

    /// The row before the first row of a pass is all zeros.
    pub(crate) fn start_pass(&mut self) {
        self.sc.prev.fill(0);
    }

    /// Reads, unfilters and writes row `y` of the pass `p`.
    pub(crate) fn read_row(
        &mut self,
        z: &mut Zr<'d>,
        p: &Pass,
        y: usize,
        dst: Dst<'_>,
    ) -> Result<(), Error> {
        let rs = p.rb + 1;
        let sc = &mut *self.sc;
        let (Some(cur), Some(pdat)) = (sc.cur.get_mut(..rs), sc.prev.get(1..rs)) else {
            return Err(Error::Format("row buffer"));
        };
        read_exact(z, cur)?;
        let Some((&mut filter, cdat)) = cur.split_first_mut() else {
            return Err(Error::Format("row buffer"));
        };
        if !unfilter(filter, self.bpp, cdat, pdat) {
            return Err(Error::Format("bad filter type"));
        }
        emit(&self.conv, &self.info.header, p, y, cdat, dst, &mut sc.tmp);
        // The current row for y is the previous row for y+1.
        std::mem::swap(&mut sc.cur, &mut sc.prev);
        Ok(())
    }

    /// Reads every row of the image into `out` (exactly the output size, at least).
    pub(crate) fn decode_rows(&mut self, z: &mut Zr<'d>, out: &mut [u8]) -> Result<(), Error> {
        let hdr = self.info.header;
        let rb = self.out_rb;
        if rb == 0 {
            return Err(Error::Format("empty row"));
        }
        if hdr.interlaced {
            let size = height_of(&hdr)
                .and_then(|h| h.checked_mul(rb))
                .ok_or(Error::AllocLimit)?;
            let img = out.get_mut(..size).ok_or(Error::BufferTooSmall)?;
            img.fill(0);
            for idx in 0..pass_count(&hdr) {
                let Some(p) = pass(&hdr, idx) else {
                    continue;
                };
                self.start_pass();
                for y in 0..p.h {
                    check_cancel(self.cancel)?;
                    self.read_row(z, &p, y, Dst::Merge(&mut *img, rb))?;
                }
            }
        } else {
            let Some(p) = pass(&hdr, 0) else {
                return Err(Error::Format("non-positive dimension"));
            };
            self.start_pass();
            for (y, row) in out.chunks_mut(rb).take(p.h).enumerate() {
                check_cancel(self.cancel)?;
                self.read_row(z, &p, y, Dst::Flat(row))?;
            }
        }
        Ok(())
    }

    /// After the last row: the zlib trailer and the end of the IDAT data.
    ///
    /// Go profile: an error here is Go's `FormatError`, and so is data left over ("too much
    /// pixel data"). Libpng profile: see docs/profiles.md, "The zlib tail". Extra data is
    /// ignored; damage is an error only where libpng 1.6.44 makes it one; the rest of the IDAT
    /// chunk is skipped, its CRC checked.
    pub(crate) fn finish_idat(&mut self, mut z: Zr<'d>) -> Result<(), Error> {
        // Libpng: its last row call stops at the end of the input piece holding this byte.
        let last = self.s.used().saturating_sub(1);
        // Check for EOF, to verify the zlib checksum.
        let mut one = [0u8; 1];
        let mut r = z.read(&mut one);
        // Libpng goes on reading (and discarding) until the stream ends or fails.
        let mut extra = false;
        if self.prof == Profile::Libpng {
            let mut sink = [0u8; 512];
            while matches!(r, Ok(n) if n != 0) {
                check_cancel(self.cancel)?;
                extra = true;
                r = z.read(&mut sink);
            }
        }
        self.sc.zbuf = z.into_buffer();
        match self.prof {
            Profile::Go => {
                let n = match r {
                    Ok(n) => n,
                    Err(e) => return Err(trailing_error(classify(&e))),
                };
                if n != 0 || self.s.idat_length() != 0 {
                    return Err(Error::Format("too much pixel data"));
                }
                if !self.s.crc_done() {
                    self.s.verify_checksum()?;
                }
            }
            Profile::Libpng => {
                if let Err(e) = r {
                    match classify(&e) {
                        ReadErr::Flate(f) => {
                            if let Some(r) = crate::error::resource(&f) {
                                return Err(r);
                            }
                            // Damage libpng's last row call sees is an error, later damage a
                            // warning. Data after the last row ends that call early.
                            let at = self.s.used().saturating_sub(1);
                            if !extra && self.same_piece(last, at) {
                                return Err(trailing_error(ReadErr::Flate(f)));
                            }
                        }
                        ReadErr::Src(Src::Checksum) => return Err(Error::Checksum),
                        // libpng needs more IDAT data and finds none.
                        e @ (ReadErr::Eof | ReadErr::Src(_)) => return Err(trailing_error(e)),
                    }
                }
                if !self.s.crc_done() {
                    let rest = usize::try_from(self.s.idat_length()).unwrap_or(usize::MAX);
                    self.s.chunk_data(rest)?;
                    self.s.verify_checksum()?;
                }
            }
        }
        self.s.end_idat();
        Ok(())
    }

    /// Whether bytes `a` and `b` of the IDAT data are in the same piece libpng feeds to zlib.
    fn same_piece(&self, a: usize, b: usize) -> bool {
        let (start, first) = self.idat0;
        let data = self.s.data();
        matches!(
            (piece_of(data, start, first, a), piece_of(data, start, first, b)),
            (Some(x), Some(y)) if x == y
        )
    }

    /// The chunks after the IDAT data, up to IEND.
    pub(crate) fn finish_chunks(&mut self) -> Result<(), Error> {
        while self.chunk()? != Flow::Iend {}
        Ok(())
    }

    // ---- threaded path ----------------------------------------------------------------------

    /// The threaded path: inflates the concatenated IDAT data with deflate-rs's parallel decode
    /// and unfilters it into `out`. `Ok(false)` leaves everything as it was (the position
    /// included) for the serial path, which then gives the result and the error. `Ok(true)`: the
    /// image is in `out` and the stream is at the end of the IDAT chunks.
    pub(crate) fn try_parallel(&mut self, out: &mut [u8]) -> Result<bool, Error> {
        if self.limits.max_threads < 2 {
            return Ok(false);
        }
        let (Some(need), Ok(first)) = (
            filtered_len(&self.info.header),
            usize::try_from(self.s.idat_length()),
        ) else {
            return Ok(false);
        };
        let start = self.s.pos();
        let Some((total, end)) = idat_run(self.s.data(), start, first) else {
            return Ok(false);
        };
        // A kept buffer bigger than this image needs would hold memory that is not booked.
        if self.sc.cat.capacity() > total {
            self.sc.cat = Vec::new();
        }
        if self.sc.filtered.capacity() > need {
            self.sc.filtered = Vec::new();
        }
        let Some(mem) = total.checked_add(need) else {
            return Ok(false);
        };
        if total < self.par_min || mem > self.ledger.left() {
            return Ok(false);
        }
        let ledger: &'d Ledger = self.ledger;
        let Ok(held) = ledger.charge(mem) else {
            return Ok(false);
        };
        let mut cat = std::mem::take(&mut self.sc.cat);
        let mut filtered = std::mem::take(&mut self.sc.filtered);
        let r = self.par_inner(start, first, total, need, &mut cat, &mut filtered, out);
        self.sc.cat = cat;
        self.sc.filtered = filtered;
        if matches!(r, Ok(true)) {
            self.s.set_pos(end);
            self.s.end_idat();
            // The buffers are alive until the call ends: so is their booking.
            self.par_held = Some(held);
        } else {
            // No buffer may outlive its booking: free them before it goes.
            self.sc.cat = Vec::new();
            self.sc.filtered = Vec::new();
        }
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn par_inner(
        &mut self,
        start: usize,
        first: usize,
        total: usize,
        need: usize,
        cat: &mut Vec<u8>,
        filtered: &mut Vec<u8>,
        out: &mut [u8],
    ) -> Result<bool, Error> {
        cat.clear();
        if cat.try_reserve_exact(total).is_err()
            || gather(self.s.data(), start, first, cat).is_none()
        {
            return Ok(false);
        }
        filtered.clear();
        if filtered.try_reserve_exact(need).is_err() {
            return Ok(false);
        }
        let dl = deflate::Limits {
            max_output: need,
            max_alloc: self.ledger.left(),
        };
        match deflate::zlib::decode_parallel_into(
            cat,
            filtered,
            &dl,
            self.limits.max_threads,
            self.cancel,
        ) {
            Ok(()) => {}
            Err(deflate::Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => return Ok(false),
        }
        if filtered.len() != need {
            return Ok(false);
        }
        // The zlib stream must end exactly where the IDAT data ends, as the serial path would
        // see it (deflate-rs ignores bytes after the Adler-32): the last four bytes are the
        // Adler-32 of the output, and nothing else in the data looks like it.
        let Some(trailer) = cat.last_chunk::<4>().copied() else {
            return Ok(false);
        };
        if u32::from_be_bytes(trailer) != adler32(filtered) {
            return Ok(false);
        }
        if cat.windows(4).filter(|&w| w == trailer.as_slice()).count() != 1 {
            return Ok(false);
        }
        self.unfilter_all(filtered, out)
    }

    /// Unfilters the rows of `filtered` (every row with its filter byte, pass after pass) into
    /// `out`. `Ok(false)`: a bad filter type, or the lengths do not fit.
    fn unfilter_all(&mut self, filtered: &mut [u8], out: &mut [u8]) -> Result<bool, Error> {
        let hdr = self.info.header;
        let rb = self.out_rb;
        let Some(size) = height_of(&hdr).and_then(|h| h.checked_mul(rb)) else {
            return Ok(false);
        };
        let Some(img) = out.get_mut(..size) else {
            return Ok(false);
        };
        if hdr.interlaced {
            img.fill(0);
        }
        let mut off = 0usize;
        for idx in 0..pass_count(&hdr) {
            let Some(p) = pass(&hdr, idx) else {
                continue;
            };
            let rs = p.rb + 1;
            self.start_pass();
            for y in 0..p.h {
                check_cancel(self.cancel)?;
                let Some((before, rest)) = filtered.split_at_mut_checked(off) else {
                    return Ok(false);
                };
                let Some((&mut filter, cdat)) =
                    rest.get_mut(..rs).and_then(<[u8]>::split_first_mut)
                else {
                    return Ok(false);
                };
                let pdat = if y == 0 {
                    self.sc.prev.get(1..rs)
                } else {
                    before.get(off - rs + 1..off)
                };
                let Some(pdat) = pdat else {
                    return Ok(false);
                };
                if !unfilter(filter, self.bpp, cdat, pdat) {
                    return Ok(false);
                }
                let dst = if hdr.interlaced {
                    Dst::Merge(&mut *img, rb)
                } else {
                    match row_mut(img, y, rb) {
                        Some(row) => Dst::Flat(row),
                        None => return Ok(false),
                    }
                };
                emit(&self.conv, &hdr, &p, y, cdat, dst, &mut self.sc.tmp);
                off += rs;
            }
        }
        Ok(true)
    }
}

/// libpng's `PNG_IDAT_READ_SIZE`: the most IDAT bytes it gives zlib at once (never across chunks).
const IDAT_READ: usize = 8192;

/// The piece libpng gives zlib that holds byte `off` of the IDAT data (the first chunk's data
/// starts at `start` and has `first` bytes): the chunk number and the piece number in it. `None`
/// past the data or if a chunk is cut off.
fn piece_of(data: &[u8], start: usize, first: usize, off: usize) -> Option<(usize, usize)> {
    let (mut at, mut len, mut base) = (start, first, 0usize);
    for chunk in 0usize.. {
        let end = base.checked_add(len)?;
        if off < end {
            return Some((chunk, off.saturating_sub(base) / IDAT_READ));
        }
        base = end;
        let next = at.checked_add(len)?.checked_add(4)?;
        let h = data.get(next..next.checked_add(8)?)?;
        if h.get(4..8) != Some(b"IDAT".as_slice()) {
            return None;
        }
        len = usize::try_from(be32(h, 0)).ok()?;
        at = next + 8;
    }
    None
}

fn height_of(hdr: &Header) -> Option<usize> {
    usize::try_from(hdr.height).ok()
}

/// Row `y` of an image of rows of `rb` bytes.
fn row_mut(img: &mut [u8], y: usize, rb: usize) -> Option<&mut [u8]> {
    let start = y.checked_mul(rb)?;
    img.get_mut(start..start.checked_add(rb)?)
}

/// `io.ReadFull(r, cr)`, with Go's mapping of the errors.
fn read_exact<R: Read>(z: &mut R, buf: &mut [u8]) -> Result<(), Error> {
    let mut filled = 0;
    while filled < buf.len() {
        let Some(tail) = buf.get_mut(filled..) else {
            break;
        };
        match z.read(tail) {
            Ok(0) => return Err(Error::Format("not enough pixel data")),
            Ok(n) => filled += n,
            Err(e) => return Err(row_error(classify(&e))),
        }
    }
    Ok(())
}

/// Writes the unfiltered row `raw` of the pass `p` to its place in the output.
fn emit(conv: &Conv, hdr: &Header, p: &Pass, y: usize, raw: &[u8], dst: Dst<'_>, tmp: &mut [u8]) {
    match dst {
        Dst::Flat(row) => convert_flat(conv, raw, p.w, p.rb, row),
        Dst::Merge(img, orb) => {
            let yy = y * p.scan.yf + p.scan.yo;
            let Some(row) = row_mut(img, yy, orb) else {
                return;
            };
            let (xo, xf) = (p.scan.xo, p.scan.xf);
            match conv.fmt {
                Format::Native if conv.depth < 8 => scatter_bits(raw, p.w, conv.depth, row, xo, xf),
                Format::Native => scatter_bytes(raw, conv.pixel_bytes(hdr), row, xo, xf),
                Format::Eight => {
                    if let Some(t) = tmp.get_mut(..p.w * conv.chan) {
                        to_eight(conv, raw, p.w, t);
                        scatter_bytes(t, conv.chan, row, xo, xf);
                    }
                }
            }
        }
    }
}

/// The sum of the data lengths of the consecutive IDAT chunks that start at `start` (the data of
/// the first one, `first` bytes), and the position after the last one's CRC. `None` if a chunk
/// is cut off.
fn idat_run(data: &[u8], start: usize, first: usize) -> Option<(usize, usize)> {
    let mut total = first;
    let mut end = start.checked_add(first)?.checked_add(4)?;
    if end > data.len() {
        return None;
    }
    while let Some(h) = data.get(end..end.checked_add(8)?) {
        if h.get(4..8) != Some(b"IDAT".as_slice()) {
            break;
        }
        let len = usize::try_from(be32(h, 0)).ok()?;
        total = total.checked_add(len)?;
        end = end.checked_add(12)?.checked_add(len)?;
        if end > data.len() {
            return None;
        }
    }
    Some((total, end))
}

/// Appends the data of the consecutive IDAT chunks from `start` to `cat`, checking every CRC.
/// `None`: a chunk is cut off or a CRC is wrong.
fn gather(data: &[u8], start: usize, first: usize, cat: &mut Vec<u8>) -> Option<()> {
    let (mut p, mut len) = (start, first);
    loop {
        let crc_at = p.checked_add(len)?;
        let body = data.get(p..crc_at)?;
        let stored = be32(data.get(crc_at..crc_at.checked_add(4)?)?, 0);
        if crc_update(crc_update(0, b"IDAT"), body) != stored {
            return None;
        }
        cat.extend_from_slice(body);
        p = crc_at + 4;
        let Some(h) = data.get(p..p.checked_add(8)?) else {
            return Some(());
        };
        if h.get(4..8) != Some(b"IDAT".as_slice()) {
            return Some(());
        }
        len = usize::try_from(be32(h, 0)).ok()?;
        p += 8;
    }
}

/// Adler-32 (RFC 1950).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_check_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let big = vec![0xffu8; 100_000];
        // 100000 bytes of 0xff: a = 1 + 255 * 100000 mod 65521.
        let a = (1 + 255u64 * 100_000) % 65521;
        assert_eq!(adler32(&big) & 0xffff, a as u32);
    }

    #[test]
    fn idat_run_sums_consecutive_chunks() {
        // Two IDAT chunks (3 and 2 data bytes) and an IEND header after them.
        let mut d = vec![0u8; 8];
        let mut chunk = |body: &[u8], typ: &[u8; 4]| {
            d.extend_from_slice(&(body.len() as u32).to_be_bytes());
            d.extend_from_slice(typ);
            d.extend_from_slice(body);
            d.extend_from_slice(&[0; 4]);
        };
        chunk(&[1, 2, 3], b"IDAT");
        chunk(&[4, 5], b"IDAT");
        chunk(&[], b"IEND");
        // The first chunk's header is read: start is at its data.
        let (total, end) = idat_run(&d, 16, 3).unwrap();
        assert_eq!(total, 5);
        assert_eq!(end, 8 + 12 + 3 + 12 + 2);
        // Cut off in the second chunk.
        assert!(idat_run(&d[..35], 16, 3).is_none());
        // gather checks the CRC: ours are zeros, so it fails.
        assert!(gather(&d, 16, 3, &mut Vec::new()).is_none());
    }
}
