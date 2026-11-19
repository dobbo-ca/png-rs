# Profiles

`Options { profile }` picks how strict the decoder is. The default is `Profile::Go`.

| Case | `Go` (Go 1.27 `image/png`, byblos-rs) | `Libpng` (libpng 1.6 defaults) |
|---|---|---|
| Bad CRC in an ancillary chunk | `Error::Checksum` | chunk dropped |
| Bad CRC in a critical chunk | `Error::Checksum` | error |
| Unknown chunk, critical or not | skipped | critical: error; ancillary: skipped |
| Invalid ancillary chunk (iCCP, zTXt, iTXt, tEXt, eXIf, cICP, gAMA, cHRM, sRGB, acTL) | dropped (Go ignores the contents) | dropped, as libpng does |
| Extra pixel data after the last row | error | ignored |
| Bad Adler-32 or truncated zlib stream after the last row | error (`Trailing`) | error or ignored, see "The zlib tail" |
| Ancillary data over `max_metadata` | chunk dropped | chunk dropped |

Ancillary chunks never fail a decode in the Go profile, as Go ignores their contents: a malformed,
over-limit or over-budget one is dropped (CRC and order checks stay as they are). zTXt and iTXt are
not inflated (the raw chunk is kept); iCCP is inflated for `Info`, and a profile that fails or does
not fit is dropped. `read_info` follows the same rule. Every kept text chunk counts at least 16
bytes against `max_metadata`.

In the Go profile whether an over-budget chunk is kept depends on `max_alloc`, and on the thread
count: a tEXt, zTXt, iTXt or eXIf chunk (before or after the image data) that does not fit in what
is left of `max_alloc` is dropped without an error, so `Info::text` or `Info::exif` can be shorter
or empty. After a threaded inflate (`max_threads > 1`, at least 4 MiB of IDAT data) the compressed
data and the filtered image stay booked until the call ends, so a chunk after the image data can
be dropped there and kept with `max_threads = 1`. This is a documented limit-dependent drop, the
same as before the IDAT chunk. It is the only way the Go profile's result depends on `max_threads`
besides `Error::AllocLimit`. The Libpng profile returns `Error::AllocLimit` in these cases.

The Go profile keeps Go's chunk order checks, the 4096-byte buffered IDAT reader (so checksum and
zlib errors come in Go's order), and the "too much pixel data" and "not enough pixel data" checks.

Both profiles produce the same pixels for a valid file. They are compared separately in the parity
job: Go against byblos-rs, Libpng against libpng (see [parity.md](parity.md)). `Display` of an
`Error` gives Go's text where Go has one, so a caller can map errors one to one.

## The zlib tail (`Libpng`)

libpng 1.6.44 as `pngoracle` calls it (`png_read_image`, `png_read_end`; benign errors are warnings,
which is the default) turns a problem after the last row into an error only when:

- zlib reports it while libpng inflates the last row (`png_read_IDAT_data` with an output buffer:
  `png_chunk_error`), or
- libpng needs more IDAT data after the last row and the next chunk is not an IDAT chunk or the file
  ends (`"Not enough image data"`, or a read error), or
- the CRC of an IDAT chunk it reads is wrong (a critical chunk, `png_crc_finish`).

Everything else is a warning (`png_chunk_benign_error`): zlib damage found in the check pass after
the last row (`png_read_finish_IDAT`), `"Extra compressed data"`, `"Too much image data"`, extra
IDAT chunks.

What decides is how much input the last-row call has. libpng gives zlib at most
`PNG_IDAT_READ_SIZE` = 8192 bytes at a time, never across an IDAT chunk (pieces start at the chunk
start), and loads the next piece only when zlib has used all of the last one. zlib's `inflate` goes
on after the last output byte as long as it needs no output space and has input: it reads the end
of block code, the next block header and the Adler-32 in the same call (the `CHECK` state needs no
output). So:

| Case | libpng | png-rs `Libpng` |
|---|---|---|
| Bad Adler-32, the end of the data and the 4 trailer bytes in one piece | `Z_DATA_ERROR` in the last-row call: error | `Error::Trailing(Checksum)` |
| Bad Adler-32, trailer in a later piece (next chunk, or past a multiple of 8192 in the chunk) | check pass: warning | ignored |
| Corrupt deflate data after the last row, seen in the piece of the last row | error | `Error::Trailing` |
| The same, seen in a later piece | warning | ignored |
| Trailer missing or short, data ends, or the next chunk is not IDAT | error | `Error::Trailing` |
| Pixel data past the last row (stream ends well) | warning | ignored |
| Pixel data past the last row, then a bad Adler-32 or corrupt data | warning (the last-row call stops at the first extra byte, so all of it is the check pass) | ignored |
| Data after the Adler-32 in the chunk, extra IDAT chunks | warning | ignored |

`byblos` `tests/data/png/invalid-zlib.png` (a bad Adler-32 in the one IDAT chunk) and libpng's
`contrib/testpngs/crashers/badadler.png` are the first row.

Approximation: png-rs does not know which byte of the input produced libpng's last output byte. It
uses the last byte its own zlib reader had taken after the last row (`Pos::used`). That reader may
decode a little further than zlib needs, so a damaged stream whose last symbols straddle two pieces
can be judged one piece too late (an error where libpng warns). Valid streams are not affected.
`Profile::Go` is unchanged.

## Libpng chunk rules

As libpng 1.6.44's `png_read_info` / `png_read_end` (`pngrutil.c`, `pngset.c`):

- An unknown ancillary chunk before IHDR is skipped (`png_handle_unknown` does not look for
  IHDR; its CRC is checked). A chunk that libpng has a handler for (PLTE, IDAT, IEND, tRNS, tEXt,
  gAMA, ... ) fails with "missing IHDR"; an unknown critical chunk fails.
- A second PLTE is an error ("duplicate"), also after IDAT; this is checked before the "after
  IDAT" rule that ignores a first PLTE that comes late.
- A PLTE with no entry is "Invalid palette", an error for RGB, RGBA and indexed images (a gray
  image ignores its PLTE).
- tRNS: dropped when it is late (after IDAT), a duplicate, of the wrong length, on an image with
  alpha, or (indexed) before PLTE or longer than the palette. For an RGB image a PLTE that comes
  after the tRNS cancels the colour key (`num_trans = 0`): `Info::trns` still holds the chunk, but
  `Info::has_trns` is false and the Eight output has no alpha channel.
