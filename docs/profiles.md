# Profiles

`Options { profile }` picks how strict the decoder is. The default is `Profile::Go`.

| Case | `Go` (Go 1.27 `image/png`, byblos-rs) | `Libpng` (libpng 1.6 defaults) |
|---|---|---|
| Bad CRC in an ancillary chunk | `Error::Checksum` | chunk dropped |
| Bad CRC in a critical chunk | `Error::Checksum` | error |
| Unknown chunk, critical or not | skipped | critical: error; ancillary: skipped |
| Invalid ancillary chunk | per Go's checks | dropped, as libpng does |
| Extra pixel data after the last row | error | ignored |
| Bad Adler-32 or truncated zlib stream after the last row | error (`Trailing`) | ignored |
| Ancillary data over `max_metadata` | `Error::MetadataLimit` | chunk dropped |

The Go profile keeps Go's chunk order checks, the 4096-byte buffered IDAT reader (so checksum and
zlib errors come in Go's order), and the "too much pixel data" and "not enough pixel data" checks.

Both profiles produce the same pixels for a valid file. They are compared separately in the parity
job: Go against byblos-rs, Libpng against libpng (see [parity.md](parity.md)). `Display` of an
`Error` gives Go's text where Go has one, so a caller can map errors one to one.
