//! The chunk loop: Go's `parseChunk` for [`Profile::Go`] (chunk order, CRC order, error order as
//! byblos-rs's port of `image/png/reader.go`) and libpng 1.6's `png_read_info` / `png_read_end`
//! rules for [`Profile::Libpng`]; both fill the same [`Info`].

use crate::engine::{Eng, Flow, Stage, check_cancel};
use crate::error::Error;
use crate::info::{ColorType, Header, Profile, Text, TextKind, Trns};
use crate::meta::{self, Inf};
use crate::source::{at, be16, be32};

/// libpng: the widest row it accepts (`PNG_UINT_32_MAX >> 3` less its row buffer slack).
const LP_MAX_WIDTH: u32 = 0x1fff_ff8e;

impl Eng<'_> {
    /// Reads and handles the next chunk.
    pub(crate) fn chunk(&mut self) -> Result<Flow, Error> {
        check_cancel(self.cancel)?;
        match self.prof {
            Profile::Go => self.chunk_go(),
            Profile::Libpng => self.chunk_lp(),
        }
    }

    /// Fails with the limit that the header is over.
    fn check_dims(&self) -> Result<(), Error> {
        let h = &self.info.header;
        if h.width > self.limits.max_width {
            return Err(Error::WidthLimit);
        }
        if h.height > self.limits.max_height {
            return Err(Error::HeightLimit);
        }
        if u64::from(h.width) * u64::from(h.height) > self.limits.max_pixels {
            return Err(Error::PixelLimit);
        }
        Ok(())
    }

    /// Keeps the PLTE entries `t`.
    fn set_palette(&mut self, t: &[u8]) -> Result<(), Error> {
        let n = t.len() / 3;
        self.info.palette.clear();
        if n > self.info.palette.capacity() {
            self.ledger.reserve(n * 3)?;
            self.info
                .palette
                .try_reserve_exact(n)
                .map_err(|_| Error::OutOfMemory)?;
        }
        self.info
            .palette
            .extend(t.as_chunks::<3>().0.iter().copied());
        Ok(())
    }

    /// Keeps the tRNS alpha values `t`.
    fn set_alpha(&mut self, t: &[u8]) -> Result<(), Error> {
        self.ledger.reserve(t.len())?;
        let mut v = self.sc.pool.take(t.len());
        v.try_reserve_exact(t.len())
            .map_err(|_| Error::OutOfMemory)?;
        v.extend_from_slice(t);
        self.info.trns = Some(Trns::Alpha(v));
        Ok(())
    }

    // ---- Go profile -------------------------------------------------------------------------

    fn chunk_go(&mut self) -> Result<Flow, Error> {
        let (length, typ) = self.s.chunk_header()?;
        let ct = self.info.header.color_type;
        let paletted = ct == ColorType::Indexed;
        match &typ {
            b"IHDR" => {
                if self.stage != Stage::Start {
                    return Err(Error::ChunkOrder);
                }
                self.stage = Stage::SeenIhdr;
                self.go_ihdr(length)?;
                return Ok(Flow::Next);
            }
            b"PLTE" => {
                if self.stage != Stage::SeenIhdr {
                    return Err(Error::ChunkOrder);
                }
                self.stage = Stage::SeenPlte;
                self.have_plte = true;
                self.go_plte(length)?;
                return Ok(Flow::Next);
            }
            b"tRNS" => {
                let ok = if paletted {
                    self.stage == Stage::SeenPlte
                } else if ct == ColorType::Rgb {
                    self.stage == Stage::SeenIhdr || self.stage == Stage::SeenPlte
                } else {
                    self.stage == Stage::SeenIhdr
                };
                if !ok {
                    return Err(Error::ChunkOrder);
                }
                self.stage = Stage::SeenTrns;
                self.go_trns(length)?;
                return Ok(Flow::Next);
            }
            b"IDAT" => {
                if self.stage < Stage::SeenIhdr
                    || self.stage > Stage::SeenIdat
                    || (self.stage == Stage::SeenIhdr && paletted)
                {
                    return Err(Error::ChunkOrder);
                }
                // Trailing zero-length or garbage IDAT chunks are ignored: the first one
                // consumes every consecutive IDAT chunk that the image needs.
                if self.stage != Stage::SeenIdat {
                    self.stage = Stage::SeenIdat;
                    self.have_idat = true;
                    self.s.begin_idat(length);
                    return Ok(Flow::Idat);
                }
            }
            b"IEND" => {
                if self.stage != Stage::SeenIdat {
                    return Err(Error::ChunkOrder);
                }
                self.stage = Stage::SeenIend;
                if length != 0 {
                    return Err(Error::Format("bad IEND length"));
                }
                self.s.verify_checksum()?;
                return Ok(Flow::Iend);
            }
            _ => {}
        }
        if length > 0x7fff_ffff {
            return Err(Error::Format("Bad chunk length"));
        }
        // Ignore this chunk (of a known length), keep what the metadata of Info wants.
        let data = self.s.chunk_data(length as usize)?;
        self.s.verify_checksum()?;
        self.ancillary(typ, data)?;
        Ok(Flow::Next)
    }

    fn go_ihdr(&mut self, length: u32) -> Result<(), Error> {
        if length != 13 {
            return Err(Error::Format("bad IHDR length"));
        }
        let t = self.s.chunk_data(13)?;
        if at(t, 10) != 0 {
            return Err(Error::Unsupported("compression method"));
        }
        if at(t, 11) != 0 {
            return Err(Error::Unsupported("filter method"));
        }
        if at(t, 12) > 1 {
            return Err(Error::Format("invalid interlace method"));
        }
        // Go reads the dimensions as int32.
        let w = be32(t, 0) as i32;
        let h = be32(t, 4) as i32;
        if w <= 0 || h <= 0 {
            return Err(Error::Format("non-positive dimension"));
        }
        let n_pixels = i64::from(w) * i64::from(h);
        // There can be up to 8 bytes per pixel, for 16 bits per channel RGBA. Go's int
        // multiplication wraps.
        if n_pixels != n_pixels.wrapping_mul(8) / 8 {
            return Err(Error::Unsupported("dimension overflow"));
        }
        let depth = at(t, 8);
        let Some(color_type) = ColorType::from_u8(at(t, 9)).filter(|c| c.allows_depth(depth))
        else {
            return Err(Error::Unsupported("bit depth and color type"));
        };
        self.info.header = Header {
            width: w as u32,
            height: h as u32,
            bit_depth: depth,
            color_type,
            interlaced: at(t, 12) == 1,
        };
        self.s.verify_checksum()?;
        self.have_ihdr = true;
        self.check_dims()
    }

    fn go_plte(&mut self, length: u32) -> Result<(), Error> {
        let np = (length / 3) as usize; // The number of palette entries.
        if !length.is_multiple_of(3)
            || np == 0
            || np > 256
            || np > 1usize << self.info.header.bit_depth
        {
            return Err(Error::Format("bad PLTE length"));
        }
        let t = self.s.chunk_data(3 * np)?;
        match self.info.header.color_type {
            // The spec says that any out-of-range pixel value is an error, but some real-world
            // files have them; Go falls back to opaque black, as libpng 1.5.13 does. A PLTE
            // chunk is optional, and ignorable, for the true colour types.
            ColorType::Indexed | ColorType::Rgb | ColorType::Rgba => self.set_palette(t)?,
            _ => return Err(Error::Format("PLTE, color type mismatch")),
        }
        self.s.verify_checksum()?;
        Ok(())
    }

    fn go_trns(&mut self, length: u32) -> Result<(), Error> {
        match self.info.header.color_type {
            ColorType::Gray => {
                if length != 2 {
                    return Err(Error::Format("bad tRNS length"));
                }
                let t = self.s.chunk_data(2)?;
                self.info.trns = Some(Trns::Gray(be16(t, 0)));
            }
            ColorType::Rgb => {
                if length != 6 {
                    return Err(Error::Format("bad tRNS length"));
                }
                let t = self.s.chunk_data(6)?;
                self.info.trns = Some(Trns::Rgb([be16(t, 0), be16(t, 2), be16(t, 4)]));
            }
            ColorType::Indexed => {
                if length > 256 {
                    return Err(Error::Format("bad tRNS length"));
                }
                let t = self.s.chunk_data(length as usize)?;
                self.set_alpha(t)?;
            }
            _ => return Err(Error::Format("tRNS, color type mismatch")),
        }
        self.s.verify_checksum()?;
        Ok(())
    }

    // ---- Libpng profile ---------------------------------------------------------------------

    fn chunk_lp(&mut self) -> Result<Flow, Error> {
        let (length, typ) = self.s.chunk_header()?;
        if length > 0x7fff_ffff {
            return Err(Error::Format("Bad chunk length"));
        }
        if !typ.iter().all(u8::is_ascii_alphabetic) {
            return Err(Error::Format("invalid chunk type"));
        }
        if !self.have_ihdr && &typ != b"IHDR" {
            return Err(Error::Format("missing IHDR"));
        }
        match &typ {
            b"IHDR" => self.lp_ihdr(length).map(|()| Flow::Next),
            b"PLTE" => self.lp_plte(length).map(|()| Flow::Next),
            b"tRNS" => self.lp_trns(length).map(|()| Flow::Next),
            b"IDAT" => self.lp_idat(length),
            b"IEND" => self.lp_iend(length),
            _ => self.lp_other(length, typ).map(|()| Flow::Next),
        }
    }

    fn lp_ihdr(&mut self, length: u32) -> Result<(), Error> {
        if self.have_ihdr {
            return Err(Error::ChunkOrder);
        }
        if length != 13 {
            return Err(Error::Format("bad IHDR length"));
        }
        let t = self.s.chunk_data(13)?;
        if !self.s.crc_matches()? {
            return Err(Error::Checksum);
        }
        let (width, height) = (be32(t, 0), be32(t, 4));
        if width == 0 || height == 0 {
            return Err(Error::Format("zero dimension"));
        }
        if width > 0x7fff_ffff || height > 0x7fff_ffff {
            return Err(Error::Format("invalid image size"));
        }
        if width > LP_MAX_WIDTH {
            return Err(Error::Format("image width too large"));
        }
        let depth = at(t, 8);
        let Some(color_type) = ColorType::from_u8(at(t, 9)).filter(|c| c.allows_depth(depth))
        else {
            return Err(Error::Unsupported("bit depth and color type"));
        };
        if at(t, 10) != 0 {
            return Err(Error::Unsupported("compression method"));
        }
        if at(t, 11) != 0 {
            return Err(Error::Unsupported("filter method"));
        }
        if at(t, 12) > 1 {
            return Err(Error::Format("invalid interlace method"));
        }
        self.info.header = Header {
            width,
            height,
            bit_depth: depth,
            color_type,
            interlaced: at(t, 12) == 1,
        };
        self.have_ihdr = true;
        self.check_dims()
    }

    /// PLTE is critical: a bad CRC is an error, even in a true colour image.
    fn lp_plte(&mut self, length: u32) -> Result<(), Error> {
        let data = self.s.chunk_data(length as usize)?;
        if !self.s.crc_matches()? {
            return Err(Error::Checksum);
        }
        if self.have_idat {
            return Ok(()); // out of place: ignored
        }
        if self.have_plte {
            return Err(Error::ChunkOrder); // duplicate
        }
        self.have_plte = true;
        let hdr = self.info.header;
        if matches!(hdr.color_type, ColorType::Gray | ColorType::GrayAlpha) {
            return Ok(());
        }
        let indexed = hdr.color_type == ColorType::Indexed;
        if length > 768 || !length.is_multiple_of(3) {
            return if indexed {
                Err(Error::Format("bad PLTE length"))
            } else {
                Ok(())
            };
        }
        // Entries past what the bit depth can index are cut off silently.
        let max = if indexed {
            1usize << hdr.bit_depth
        } else {
            256
        };
        let n = (data.len() / 3).min(max);
        self.set_palette(data.get(..n * 3).unwrap_or(&[]))
    }

    /// tRNS is ancillary: a bad CRC, a wrong place or a wrong length drops the chunk.
    fn lp_trns(&mut self, length: u32) -> Result<(), Error> {
        let data = self.s.chunk_data(length as usize)?;
        if !self.s.crc_matches()? || self.have_idat || self.info.trns.is_some() {
            return Ok(());
        }
        match self.info.header.color_type {
            ColorType::Gray if data.len() == 2 => self.info.trns = Some(Trns::Gray(be16(data, 0))),
            ColorType::Rgb if data.len() == 6 => {
                self.info.trns = Some(Trns::Rgb([be16(data, 0), be16(data, 2), be16(data, 4)]));
            }
            ColorType::Indexed
                if self.have_plte && !data.is_empty() && data.len() <= self.info.palette.len() =>
            {
                self.set_alpha(data)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn lp_idat(&mut self, length: u32) -> Result<Flow, Error> {
        if self.have_idat {
            // IDAT chunks after the image data: libpng reads them and ignores them.
            self.s.chunk_data(length as usize)?;
            if !self.s.crc_matches()? {
                return Err(Error::Checksum);
            }
            return Ok(Flow::Next);
        }
        if self.info.header.color_type == ColorType::Indexed && !self.have_plte {
            return Err(Error::Format("missing PLTE"));
        }
        self.have_idat = true;
        self.s.begin_idat(length);
        Ok(Flow::Idat)
    }

    fn lp_iend(&mut self, length: u32) -> Result<Flow, Error> {
        if !self.have_idat {
            return Err(Error::ChunkOrder);
        }
        self.s.chunk_data(length as usize)?;
        if !self.s.crc_matches()? {
            return Err(Error::Checksum);
        }
        Ok(Flow::Iend)
    }

    fn lp_other(&mut self, length: u32, typ: [u8; 4]) -> Result<(), Error> {
        if typ[0] & 0x20 == 0 {
            return Err(Error::Format("unknown critical chunk"));
        }
        let data = self.s.chunk_data(length as usize)?;
        if !self.s.crc_matches()? {
            return Ok(()); // an ancillary chunk with a bad CRC is dropped
        }
        self.ancillary(typ, data)
    }

    // ---- Metadata, both profiles ------------------------------------------------------------

    /// Books `n` bytes of kept metadata. `Ok(false)`: over `max_metadata` and the chunk is to be
    /// dropped (Libpng profile); the Go profile fails instead.
    fn meta_charge(&mut self, n: usize) -> Result<bool, Error> {
        match self.meta_used.checked_add(n) {
            Some(t) if t <= self.limits.max_metadata => {
                self.ledger.reserve(n)?;
                self.meta_used = t;
                Ok(true)
            }
            _ => match self.prof {
                Profile::Go => Err(Error::MetadataLimit),
                Profile::Libpng => Ok(false),
            },
        }
    }

    /// The chunk `typ` with checked data: keeps what `Info` holds. An invalid chunk, one in the
    /// wrong place and a duplicate are dropped, as libpng does; every other chunk is skipped.
    fn ancillary(&mut self, typ: [u8; 4], data: &[u8]) -> Result<(), Error> {
        if !self.have_ihdr {
            return Ok(());
        }
        // gAMA, cHRM, sRGB, iCCP and cICP come before PLTE and IDAT.
        let early = !self.have_plte && !self.have_idat;
        match &typ {
            b"gAMA" if early && self.info.gamma.is_none() => self.info.gamma = meta::gama(data),
            b"cHRM" if early && self.info.chrm.is_none() => self.info.chrm = meta::chrm(data),
            b"sRGB" if early && self.info.srgb.is_none() => self.info.srgb = meta::srgb(data),
            b"cICP" if early && self.info.cicp.is_none() => self.info.cicp = meta::cicp(data),
            b"acTL" if !self.have_idat && self.info.apng.is_none() => {
                self.info.apng = meta::actl(data);
            }
            b"iCCP" if early && self.info.iccp.is_none() => self.iccp(data)?,
            b"eXIf" if self.info.exif.is_none() && data.len() >= 2 => {
                if self.meta_charge(data.len())? {
                    let v = self.keep(data)?;
                    self.info.exif = Some(v);
                }
            }
            b"tEXt" => self.text(TextKind::Text, data)?,
            b"zTXt" => self.text(TextKind::Ztxt, data)?,
            b"iTXt" => self.text(TextKind::Itxt, data)?,
            _ => {}
        }
        Ok(())
    }

    /// A pooled copy of `data` (its size is booked already).
    fn keep(&mut self, data: &[u8]) -> Result<Vec<u8>, Error> {
        let mut v = self.sc.pool.take(data.len());
        v.try_reserve_exact(data.len())
            .map_err(|_| Error::OutOfMemory)?;
        v.extend_from_slice(data);
        Ok(v)
    }

    /// A metadata size that is over `max_metadata`: an error or a dropped chunk.
    fn meta_limit(&self) -> Result<(), Error> {
        match self.prof {
            Profile::Go => Err(Error::MetadataLimit),
            Profile::Libpng => Ok(()),
        }
    }

    fn iccp(&mut self, data: &[u8]) -> Result<(), Error> {
        let Some((name, z)) = meta::iccp_parts(data) else {
            return Ok(());
        };
        let room = self.limits.max_metadata.saturating_sub(self.meta_used);
        let Some(room) = room.checked_sub(name.len()) else {
            return self.meta_limit();
        };
        let mut profile = self.sc.pool.take(0);
        let r = meta::inflate(
            z,
            &mut self.sc.zbuf,
            Some(&mut profile),
            room,
            self.ledger.left(),
            self.cancel,
        );
        match r {
            Ok(()) => {}
            Err(Inf::Bad) => {
                self.sc.pool.give(profile);
                return Ok(());
            }
            Err(Inf::Limit) => {
                self.sc.pool.give(profile);
                return self.meta_limit();
            }
            Err(Inf::Fatal(e)) => return Err(e),
        }
        if self.prof == Profile::Libpng && !meta::icc_plausible(&profile) {
            self.sc.pool.give(profile);
            return Ok(());
        }
        let total = name.len() + profile.capacity();
        if let Err(e) = self.ledger.reserve(total) {
            self.sc.pool.give(profile);
            return Err(e);
        }
        self.meta_used += name.len() + profile.len();
        let name = self.keep(name)?;
        self.info.iccp = Some(crate::info::Iccp { name, profile });
        Ok(())
    }

    fn text(&mut self, kind: TextKind, data: &[u8]) -> Result<(), Error> {
        let Some(z) = meta::text_parts(kind, data) else {
            return Ok(());
        };
        // Over the budget: no need to look inside.
        if self
            .meta_used
            .checked_add(data.len())
            .is_none_or(|t| t > self.limits.max_metadata)
        {
            return self.meta_limit();
        }
        if let Some(z) = z {
            // A compressed text is dropped if its stream is bad; if it is merely big, it counts
            // as good (only the raw chunk is kept).
            let room = self.limits.max_metadata.saturating_sub(self.meta_used);
            let r = meta::inflate(
                z,
                &mut self.sc.zbuf,
                None,
                room,
                self.ledger.left(),
                self.cancel,
            );
            match r {
                Ok(()) | Err(Inf::Limit) => {}
                Err(Inf::Bad) => return Ok(()),
                Err(Inf::Fatal(e)) => return Err(e),
            }
        }
        if !self.meta_charge(data.len())? {
            return Ok(());
        }
        let v = self.keep(data)?;
        let t = &mut self.info.text;
        if t.len() == t.capacity() {
            self.ledger.reserve(std::mem::size_of::<Text>())?;
            t.try_reserve_exact(1).map_err(|_| Error::OutOfMemory)?;
        }
        t.push(Text { kind, data: v });
        Ok(())
    }
}
