//! The public decoder: [`Decoder`], [`Rows`] and the one-call functions.

use crate::engine::{Dst, Eng, PAR_MIN, Scratch, Zr, check_cancel};
use crate::error::Error;
use crate::info::{Format, Info, Options};
use crate::ledger::{Ledger, zeroed};
use crate::limits::Limits;
use crate::pixels::pass;
use crate::source::Pos;
use std::sync::atomic::AtomicBool;

/// Reusable decoder. A second decode of an image of the same or smaller size, into the same
/// output buffer, allocates nothing: the row buffers, the inflate window (deflate-rs's
/// `zlib::Reader::with_buffer` / `into_buffer`) and the vectors of [`Info`] keep their capacity.
///
/// The kept buffers are not part of any call's `max_alloc`: each call charges the memory it
/// uses, kept or new, and fails with [`Error::AllocLimit`] if it does not fit.
#[derive(Debug, Default)]
pub struct Decoder {
    pub(crate) info: Info,
    ledger: Ledger,
    pos: Pos,
    sc: Scratch,
}

impl Decoder {
    /// A decoder with no buffers yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the chunks up to the first IDAT chunk (IHDR, PLTE, tRNS and the ancillary
    /// chunks before the image data) and returns what they say. The pixels are not read.
    ///
    /// Enforces `max_width`, `max_height`, `max_pixels` (as soon as IHDR is read), `max_metadata`
    /// and `max_alloc` (palette and metadata vectors). The contents of the ancillary chunks do
    /// not fail the call in either profile: a bad or over-limit one is dropped (the Go profile
    /// ignores them, as Go does).
    ///
    /// # Errors
    /// [`Error::NotPng`], [`Error::Checksum`], [`Error::ChunkOrder`], [`Error::Format`],
    /// [`Error::Unsupported`], [`Error::UnexpectedEof`] for a bad or cut-off file;
    /// [`Error::WidthLimit`], [`Error::HeightLimit`], [`Error::PixelLimit`],
    /// [`Error::AllocLimit`] and [`Error::OutOfMemory`] for the limits.
    pub fn read_info(
        &mut self,
        data: &[u8],
        opts: &Options,
        limits: &Limits,
    ) -> Result<&Info, Error> {
        self.read_info_with(data, opts, limits, &AtomicBool::new(false))
    }

    /// [`Decoder::read_info`] with the caller's cancel flag.
    pub(crate) fn read_info_with(
        &mut self,
        data: &[u8],
        opts: &Options,
        limits: &Limits,
        cancel: &AtomicBool,
    ) -> Result<&Info, Error> {
        let Decoder {
            info,
            ledger,
            pos,
            sc,
        } = self;
        Eng::new(data, pos, opts.profile, limits, cancel, ledger, info, sc, 0)?.read_to_idat()?;
        Ok(&self.info)
    }

    /// Decodes the whole image into `out`, rows top to bottom, in the layout of `fmt`. The
    /// chunks after the IDAT data up to IEND are read too, so the returned [`Info`] also holds
    /// text and eXIf chunks that follow the image.
    ///
    /// With `max_threads > 1` and at least 4 MiB of compressed data the inflate runs on up to
    /// `max_threads` threads (see `docs/threading.md`); the result is the serial one.
    ///
    /// Enforces every [`Limits`] field. The cancel flag is read per chunk and per row.
    ///
    /// # Errors
    /// As [`Decoder::read_info`], and [`Error::BufferTooSmall`] if `out` is shorter than
    /// `info.output_size(fmt)` (checked after the chunks before the image data),
    /// [`Error::Zlib`] for a bad zlib stream in the pixel rows, [`Error::Trailing`] for a bad one
    /// after the last row (Go profile), [`Error::Format`] for "not enough pixel data",
    /// "too much pixel data" and "bad filter type", and [`Error::Cancelled`].
    pub fn decode_into(
        &mut self,
        data: &[u8],
        fmt: Format,
        out: &mut [u8],
        opts: &Options,
        limits: &Limits,
        cancel: &AtomicBool,
    ) -> Result<&Info, Error> {
        self.decode_into_with(data, fmt, out, opts, limits, cancel, 0, PAR_MIN)?;
        Ok(&self.info)
    }

    /// [`Decoder::decode_into`] with `preload` bytes of the memory budget taken by the caller
    /// (its output buffer) and the threaded path from `par_min` bytes of IDAT data.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_into_with(
        &mut self,
        data: &[u8],
        fmt: Format,
        out: &mut [u8],
        opts: &Options,
        limits: &Limits,
        cancel: &AtomicBool,
        preload: usize,
        par_min: usize,
    ) -> Result<(), Error> {
        let Decoder {
            info,
            ledger,
            pos,
            sc,
        } = self;
        let mut eng = Eng::new(
            data,
            pos,
            opts.profile,
            limits,
            cancel,
            ledger,
            info,
            sc,
            preload,
        )?;
        eng.par_min = par_min;
        eng.read_to_idat()?;
        let size = eng.info.output_size(fmt).ok_or(Error::AllocLimit)?;
        if out.len() < size {
            return Err(Error::BufferTooSmall);
        }
        eng.prepare(fmt)?;
        if eng.try_parallel(out)? {
            return eng.finish_chunks();
        }
        let mut z = eng.start_zlib()?;
        eng.decode_rows(&mut z, out)?;
        eng.finish_idat(z)?;
        eng.finish_chunks()
    }

    /// Row streaming: rows top to bottom, one [`Rows::next_row`] call each. A non-interlaced
    /// image holds two rows plus the inflate window. An interlaced image holds the whole merged
    /// image (charged to `max_alloc`) from the first `next_row` call on, because Adam7 passes
    /// fill rows out of order.
    ///
    /// Reads the chunks up to the first IDAT chunk and the zlib header here, so it fails with
    /// the errors of [`Decoder::read_info`] and the zlib header's. Enforces every [`Limits`]
    /// field; the cancel flag is read per chunk and per row.
    ///
    /// # Errors
    /// As [`Decoder::read_info`], plus [`Error::Cancelled`] and [`Error::UnexpectedEof`] /
    /// [`Error::Zlib`] for the zlib header.
    pub fn rows<'d>(
        &'d mut self,
        data: &'d [u8],
        fmt: Format,
        opts: &Options,
        limits: &Limits,
        cancel: &'d AtomicBool,
    ) -> Result<Rows<'d>, Error> {
        let Decoder {
            info,
            ledger,
            pos,
            sc,
        } = self;
        let mut eng = Eng::new(data, pos, opts.profile, limits, cancel, ledger, info, sc, 0)?;
        eng.read_to_idat()?;
        eng.prepare(fmt)?;
        let z = eng.start_zlib()?;
        let rb = eng.out_rb;
        Ok(Rows {
            eng,
            z: Some(z),
            rb,
            y: 0,
            merged: false,
            done: false,
            failed: None,
        })
    }
}

/// The rows of an image, one at a time. Made by [`Decoder::rows`].
pub struct Rows<'d> {
    eng: Eng<'d>,
    z: Option<Zr<'d>>,
    rb: usize,
    y: u32,
    /// Interlaced: the passes are decoded into the image kept in the decoder.
    merged: bool,
    done: bool,
    failed: Option<Error>,
}

impl std::fmt::Debug for Rows<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rows")
            .field("row_bytes", &self.rb)
            .field("next_row", &self.y)
            .finish_non_exhaustive()
    }
}

impl Drop for Rows<'_> {
    fn drop(&mut self) {
        // Give the inflate window back to the decoder for the next call.
        if let Some(z) = self.z.take() {
            self.eng.sc.zbuf = z.into_buffer();
        }
    }
}

impl Rows<'_> {
    /// The header and metadata read before the image data (the chunks after it are read when
    /// the last row is done).
    #[must_use]
    pub fn info(&self) -> &Info {
        self.eng.info
    }

    /// Bytes of one output row: the length `out` of [`Rows::next_row`] needs.
    #[must_use]
    pub fn row_bytes(&self) -> usize {
        self.rb
    }

    /// Writes the next row into `out[..row_bytes]` and returns its index; `Ok(None)` after the
    /// last row, once the chunks up to IEND are read and checked. An error is final: later
    /// calls return it again.
    ///
    /// Enforces the limits of [`Decoder::rows`]; reads the cancel flag per row.
    ///
    /// # Errors
    /// [`Error::BufferTooSmall`] if `out` is shorter than `row_bytes`; for the rest as
    /// [`Decoder::decode_into`].
    pub fn next_row(&mut self, out: &mut [u8]) -> Result<Option<u32>, Error> {
        if out.len() < self.rb {
            return Err(Error::BufferTooSmall);
        }
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let r = self.step(out);
        if let Err(e) = &r {
            self.failed = Some(e.clone());
        }
        r
    }

    fn step(&mut self, out: &mut [u8]) -> Result<Option<u32>, Error> {
        if self.done {
            return Ok(None);
        }
        check_cancel(self.eng.cancel)?;
        let hdr = self.eng.info.header;
        let y = self.y;
        if hdr.interlaced {
            if !self.merged {
                self.merge_passes()?;
            }
            if y >= hdr.height {
                self.done = true;
                return Ok(None);
            }
            let start = usize::try_from(y)
                .ok()
                .and_then(|y| y.checked_mul(self.rb))
                .ok_or(Error::AllocLimit)?;
            let src = self
                .eng
                .sc
                .image
                .get(start..start + self.rb)
                .ok_or(Error::Format("merged image"))?;
            let dst = out.get_mut(..self.rb).ok_or(Error::BufferTooSmall)?;
            dst.copy_from_slice(src);
        } else {
            if y >= hdr.height {
                let z = self.z.take().ok_or(Error::Format("rows"))?;
                self.eng.finish_idat(z)?;
                self.eng.finish_chunks()?;
                self.done = true;
                return Ok(None);
            }
            let p = pass(&hdr, 0).ok_or(Error::Format("non-positive dimension"))?;
            if y == 0 {
                self.eng.start_pass();
            }
            let dst = out.get_mut(..self.rb).ok_or(Error::BufferTooSmall)?;
            let z = self.z.as_mut().ok_or(Error::Format("rows"))?;
            self.eng.read_row(
                z,
                &p,
                usize::try_from(y).unwrap_or(usize::MAX),
                Dst::Flat(dst),
            )?;
        }
        self.y += 1;
        Ok(Some(y))
    }

    /// Interlaced: decodes every pass into the merged image and reads the chunks after it.
    fn merge_passes(&mut self) -> Result<(), Error> {
        let mut z = self.z.take().ok_or(Error::Format("rows"))?;
        let height = usize::try_from(self.eng.info.header.height).map_err(|_| Error::AllocLimit)?;
        let size = self.rb.checked_mul(height).ok_or(Error::AllocLimit)?;
        let mut img = std::mem::take(&mut self.eng.sc.image);
        let mut r = zeroed(&mut img, size, self.eng.ledger);
        if r.is_ok() {
            r = self.eng.decode_rows(&mut z, &mut img);
        }
        self.eng.sc.image = img;
        r?;
        self.eng.finish_idat(z)?;
        self.eng.finish_chunks()?;
        self.merged = true;
        Ok(())
    }
}

/// Decodes `data` in one call and returns the [`Info`] and the pixels in the layout of `fmt`.
///
/// The output buffer is charged to `limits.max_alloc` before it is allocated. Enforces every
/// [`Limits`] field; the cancel flag is read per chunk and per row.
///
/// # Errors
/// As [`Decoder::decode_into`], except that [`Error::BufferTooSmall`] cannot happen;
/// [`Error::AllocLimit`] if the output alone does not fit `max_alloc`.
pub fn decode(
    data: &[u8],
    fmt: Format,
    opts: &Options,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<(Info, Vec<u8>), Error> {
    decode_with(data, fmt, opts, limits, cancel, PAR_MIN)
}

/// [`decode`] with the threaded path from `par_min` bytes of IDAT data.
pub(crate) fn decode_with(
    data: &[u8],
    fmt: Format,
    opts: &Options,
    limits: &Limits,
    cancel: &AtomicBool,
    par_min: usize,
) -> Result<(Info, Vec<u8>), Error> {
    // The first pass (header and metadata, with the caller's cancel flag) runs in a decoder of
    // its own that is dropped before the output is allocated, so no metadata is held then.
    let size = Decoder::new()
        .read_info_with(data, opts, limits, cancel)?
        .output_size(fmt)
        .ok_or(Error::AllocLimit)?;
    if size > limits.max_alloc {
        return Err(Error::AllocLimit);
    }
    let mut dec = Decoder::new();
    let mut out = Vec::new();
    out.try_reserve_exact(size)
        .map_err(|_| Error::OutOfMemory)?;
    out.resize(size, 0);
    dec.decode_into_with(data, fmt, &mut out, opts, limits, cancel, size, par_min)?;
    Ok((std::mem::take(&mut dec.info), out))
}

/// Reads the header and the metadata before the image data of `data`, as
/// [`Decoder::read_info`].
///
/// # Errors
/// As [`Decoder::read_info`].
pub fn read_info(data: &[u8], opts: &Options, limits: &Limits) -> Result<Info, Error> {
    let mut dec = Decoder::new();
    dec.read_info(data, opts, limits)?;
    // Moved out, not cloned: a clone would sit next to the booked original.
    Ok(std::mem::take(&mut dec.info))
}
