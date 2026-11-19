//! The byte source under the decoder: the chunk stream over the input slice, and the
//! 4096-byte buffered reader that Go's `zlib.NewReader` puts around the IDAT chunks, so that
//! checksum and zlib errors come in Go's order.

use crate::crc::update as crc_update;
use crate::error::{Error, resource};
use std::cell::Cell;
use std::io::{self, BufRead, Read};

/// The size of Go's default `bufio.Reader`.
pub(crate) const BUF: usize = 4096;

/// Byte `i` of `b`, 0 past the end.
pub(crate) fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// Big-endian `u16` at `i`, 0 bytes past the end.
pub(crate) fn be16(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([at(b, i), at(b, i.wrapping_add(1))])
}

/// Big-endian `u32` at `i`, 0 bytes past the end.
pub(crate) fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([
        at(b, i),
        at(b, i.wrapping_add(1)),
        at(b, i.wrapping_add(2)),
        at(b, i.wrapping_add(3)),
    ])
}

/// An error of the byte source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Src {
    /// The input ends (`io.EOF` or `io.ErrUnexpectedEOF`).
    Eof,
    /// The CRC of a chunk is wrong.
    Checksum,
    /// An IDAT chunk ends and the next chunk is not an IDAT chunk.
    NoData,
}

impl From<Src> for Error {
    fn from(s: Src) -> Error {
        match s {
            Src::Eof => Error::UnexpectedEof,
            Src::Checksum => Error::Checksum,
            Src::NoData => Error::Format("not enough pixel data"),
        }
    }
}

/// The decoder's position in the input, the running chunk CRC and `idatLength`. Shared by the
/// chunk reader and the IDAT reader inside deflate-rs's zlib reader, through [`Stream`].
#[derive(Debug, Default)]
pub(crate) struct Pos {
    pos: Cell<usize>,
    crc: Cell<u32>,
    idat_length: Cell<u32>,
    /// The CRC of the current IDAT chunk was read and checked.
    crc_done: Cell<bool>,
}

impl Pos {
    pub(crate) fn reset(&self) {
        self.pos.set(0);
        self.crc.set(0);
        self.idat_length.set(0);
        self.crc_done.set(false);
    }
}

/// The input and its position.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stream<'a> {
    data: &'a [u8],
    p: &'a Pos,
}

impl<'a> Stream<'a> {
    pub(crate) fn new(data: &'a [u8], p: &'a Pos) -> Self {
        Stream { data, p }
    }

    pub(crate) fn data(&self) -> &'a [u8] {
        self.data
    }

    pub(crate) fn pos(&self) -> usize {
        self.p.pos.get()
    }

    pub(crate) fn set_pos(&self, pos: usize) {
        self.p.pos.set(pos);
    }

    pub(crate) fn idat_length(&self) -> u32 {
        self.p.idat_length.get()
    }

    /// Starts reading the IDAT chunks: this chunk has `length` data bytes left.
    pub(crate) fn begin_idat(&self, length: u32) {
        self.p.idat_length.set(length);
        self.p.crc_done.set(false);
    }

    /// The IDAT chunks are done and the CRC of the last one is checked or skipped.
    pub(crate) fn end_idat(&self) {
        self.p.idat_length.set(0);
        self.p.crc_done.set(true);
    }

    pub(crate) fn crc_done(&self) -> bool {
        self.p.crc_done.get()
    }

    /// `io.ReadFull(d.r, buf[:n])`.
    pub(crate) fn read_full(&self, n: usize) -> Result<&'a [u8], Src> {
        let pos = self.p.pos.get();
        let rest = self.data.get(pos..).unwrap_or(&[]);
        match rest.split_at_checked(n) {
            Some((head, _)) => {
                self.p.pos.set(pos + n);
                Ok(head)
            }
            None => {
                self.p.pos.set(self.data.len());
                Err(Src::Eof)
            }
        }
    }

    /// The length and type of the next chunk; the running CRC starts with the type.
    pub(crate) fn chunk_header(&self) -> Result<(u32, [u8; 4]), Src> {
        let h = self.read_full(8)?;
        let typ = [at(h, 4), at(h, 5), at(h, 6), at(h, 7)];
        self.p.crc.set(crc_update(0, &typ));
        Ok((be32(h, 0), typ))
    }

    /// Reads `n` bytes of chunk data and adds them to the chunk CRC.
    pub(crate) fn chunk_data(&self, n: usize) -> Result<&'a [u8], Src> {
        let b = self.read_full(n)?;
        self.p.crc.set(crc_update(self.p.crc.get(), b));
        Ok(b)
    }

    /// Reads the stored CRC and returns whether it is the running one.
    pub(crate) fn crc_matches(&self) -> Result<bool, Src> {
        let b = self.read_full(4)?;
        Ok(be32(b, 0) == self.p.crc.get())
    }

    /// `verifyChecksum`.
    pub(crate) fn verify_checksum(&self) -> Result<(), Src> {
        if self.crc_matches()? {
            Ok(())
        } else {
            Err(Src::Checksum)
        }
    }

    /// `decoder.Read`: presents the IDAT chunks as one stream.
    pub(crate) fn idat_read(&self, p: &mut [u8]) -> Result<usize, Src> {
        if p.is_empty() {
            return Ok(0);
        }
        while self.p.idat_length.get() == 0 {
            // An IDAT chunk is used up. Check its CRC, then read the next header, which must
            // be an IDAT chunk.
            self.verify_checksum()?;
            self.p.crc_done.set(true);
            let at_header = self.p.pos.get();
            let h = self.read_full(8)?;
            if h.get(4..8) != Some(b"IDAT".as_slice()) {
                // The header belongs to the next chunk of the file.
                self.p.pos.set(at_header);
                return Err(Src::NoData);
            }
            self.begin_idat(be32(h, 0));
            self.p.crc.set(crc_update(0, b"IDAT"));
        }
        let want = p
            .len()
            .min(usize::try_from(self.p.idat_length.get()).unwrap_or(usize::MAX));
        let rest = self.data.get(self.p.pos.get()..).unwrap_or(&[]);
        let n = want.min(rest.len());
        let (Some(dst), Some(src)) = (p.get_mut(..n), rest.get(..n)) else {
            return Err(Src::Eof);
        };
        if n == 0 {
            return Err(Src::Eof);
        }
        dst.copy_from_slice(src);
        self.p.crc.set(crc_update(self.p.crc.get(), src));
        self.p.pos.set(self.p.pos.get() + n);
        self.p.idat_length.set(self.p.idat_length.get() - n as u32);
        Ok(n)
    }
}

/// The `bufio.Reader` that `zlib.NewReader` wraps around the decoder. A source error is
/// carried through deflate-rs as an `io::ErrorKind` (the one place the kinds are given
/// meaning): `UnexpectedEof` is [`Src::Eof`], `InvalidInput` is [`Src::Checksum`], `NotFound` is
/// [`Src::NoData`].
#[derive(Debug)]
pub(crate) struct Idat<'a> {
    s: Stream<'a>,
    buf: [u8; BUF],
    r: usize,
    w: usize,
    err: Option<Src>,
}

fn to_io(e: Src) -> io::Error {
    io::Error::from(match e {
        Src::Eof => io::ErrorKind::UnexpectedEof,
        Src::Checksum => io::ErrorKind::InvalidInput,
        Src::NoData => io::ErrorKind::NotFound,
    })
}

impl<'a> Idat<'a> {
    pub(crate) fn new(s: Stream<'a>) -> Self {
        Idat {
            s,
            buf: [0; BUF],
            r: 0,
            w: 0,
            err: None,
        }
    }

    /// `bufio.Reader.fill`, called with the buffer empty.
    fn fill(&mut self) {
        self.r = 0;
        self.w = 0;
        match self.s.idat_read(&mut self.buf) {
            Ok(n) => self.w = n,
            Err(e) => self.err = Some(e),
        }
    }
}

impl Read for Idat<'_> {
    fn read(&mut self, p: &mut [u8]) -> io::Result<usize> {
        if p.is_empty() {
            return Ok(0);
        }
        if self.r == self.w {
            if let Some(e) = self.err {
                return Err(to_io(e));
            }
            if p.len() >= BUF {
                // A large read with an empty buffer goes straight to the source.
                return self.s.idat_read(p).map_err(to_io);
            }
            self.r = 0;
            self.w = 0;
            match self.s.idat_read(&mut self.buf) {
                Ok(n) => self.w = n,
                Err(e) => return Err(to_io(e)),
            }
        }
        let avail = self.buf.get(self.r..self.w).unwrap_or(&[]);
        let n = avail.len().min(p.len());
        if let (Some(dst), Some(src)) = (p.get_mut(..n), avail.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.r += n;
        Ok(n)
    }
}

impl BufRead for Idat<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.r == self.w {
            if self.err.is_none() {
                self.fill();
            }
            if self.r == self.w
                && let Some(e) = self.err
            {
                return Err(to_io(e));
            }
        }
        Ok(self.buf.get(self.r..self.w).unwrap_or(&[]))
    }

    fn consume(&mut self, amt: usize) {
        self.r = self.r.saturating_add(amt).min(self.w);
    }
}

/// What a failed read of the zlib stream means.
#[derive(Debug)]
pub(crate) enum ReadErr {
    /// `io.EOF` or `io.ErrUnexpectedEOF`.
    Eof,
    /// An error of the zlib or DEFLATE decoder (limits and cancellation included).
    Flate(deflate::Error),
    /// An error of the byte source.
    Src(Src),
}

fn from_kind(k: io::ErrorKind) -> ReadErr {
    match k {
        io::ErrorKind::InvalidInput => ReadErr::Src(Src::Checksum),
        io::ErrorKind::NotFound => ReadErr::Src(Src::NoData),
        _ => ReadErr::Eof,
    }
}

/// Classifies an error that deflate-rs returned (not as an `io::Error`).
pub(crate) fn classify_flate(e: deflate::Error) -> ReadErr {
    match e {
        deflate::Error::UnexpectedEof => ReadErr::Eof,
        deflate::Error::Io(k) => from_kind(k),
        e => ReadErr::Flate(e),
    }
}

/// Classifies the `io::Error` of a read of deflate-rs's reader.
pub(crate) fn classify(e: &io::Error) -> ReadErr {
    match e.get_ref().and_then(|s| s.downcast_ref::<deflate::Error>()) {
        Some(f) => classify_flate(f.clone()),
        None => from_kind(e.kind()),
    }
}

/// A deflate-rs error as a png-rs error: the limits and cancellation keep their own variants.
pub(crate) fn flate_error(e: deflate::Error) -> Error {
    match resource(&e) {
        Some(r) => r,
        None if e == deflate::Error::UnexpectedEof => Error::UnexpectedEof,
        None => Error::Zlib(e),
    }
}

/// The error of a failed read of a pixel row.
pub(crate) fn row_error(e: ReadErr) -> Error {
    match e {
        ReadErr::Eof => Error::Format("not enough pixel data"),
        ReadErr::Src(s) => Error::from(s),
        ReadErr::Flate(f) => flate_error(f),
    }
}

/// The error of a failed read of the zlib header.
pub(crate) fn init_error(e: ReadErr) -> Error {
    match e {
        ReadErr::Eof => Error::UnexpectedEof,
        ReadErr::Src(s) => Error::from(s),
        ReadErr::Flate(f) => flate_error(f),
    }
}

/// The error of a failed read after the last row: Go's `FormatError(err.Error())`.
pub(crate) fn trailing_error(e: ReadErr) -> Error {
    match e {
        ReadErr::Eof | ReadErr::Src(Src::Eof) => Error::Trailing(deflate::Error::UnexpectedEof),
        ReadErr::Src(Src::Checksum) => Error::Format("png: invalid format: invalid checksum"),
        ReadErr::Src(Src::NoData) => Error::Format("png: invalid format: not enough pixel data"),
        ReadErr::Flate(f) => resource(&f).unwrap_or(Error::Trailing(f)),
    }
}
