//! Parity oracle: decodes PNGs with byblos and writes `.eight` or `.err` files.

use byblos::error::Error;
use byblos::image::png_read;
use byblos::image::{Color, Image, Rect};
use byblos::limits::Limits;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: byblos-oracle <in-dir> <out-dir>");
        std::process::exit(2);
    }
    let (inp, out) = (Path::new(&args[1]), Path::new(&args[2]));
    if let Err(e) = fs::create_dir_all(out) {
        eprintln!("{}: {e}", out.display());
        std::process::exit(1);
    }
    let mut files = Vec::new();
    walk(inp, &mut files);
    files.sort();
    let limits = Limits::default();
    let cancel = AtomicBool::new(false);
    let mut failed = false;
    for f in files {
        let rel = f.strip_prefix(inp).unwrap_or(&f);
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("__");
        let Ok(data) = fs::read(&f) else {
            eprintln!("cannot read {}", f.display());
            failed = true;
            continue;
        };
        let res = png_read::decode(&data, &limits, &cancel)
            .map_err(|e| token(&e))
            .and_then(|img| eight(&img, &data));
        let r = match res {
            Ok(b) => fs::write(out.join(format!("{name}.eight")), b),
            Err(t) => fs::write(out.join(format!("{name}.err")), format!("{t}\n")),
        };
        if let Err(e) = r {
            eprintln!("write {name}: {e}");
            failed = true;
        }
    }
    if failed {
        std::process::exit(1);
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".png") || n.contains(".mut") {
                out.push(p);
            }
        }
    }
}

/// Maps a byblos error to a spec token by Go text.
fn token(e: &Error) -> String {
    match e {
        Error::Image(s) => {
            if s == "png: invalid format: not a PNG file" {
                "notpng".into()
            } else if s.contains("invalid checksum") {
                "checksum".into()
            } else if s.contains("chunk out of order") {
                "chunk-order".into()
            } else if s.starts_with("png: invalid format") {
                "format".into()
            } else if s.starts_with("png: unsupported feature") {
                "unsupported".into()
            } else {
                format!("other:Image({s})")
            }
        }
        Error::Limit { .. } | Error::Alloc { .. } | Error::OutOfMemory => "limit".into(),
        Error::Cancelled => "cancelled".into(),
        Error::Pdf(p) => {
            let s = format!("{p:?}");
            if s.contains("UnexpectedEof") {
                "eof".into()
            } else if s.contains("Limit") || s.contains("Alloc") || s.contains("OutOfMemory") {
                "limit".into()
            } else if s.contains("Cancel") {
                "cancelled".into()
            } else if s.contains("Flate") || s.contains("flate") || s.contains("zlib") {
                "zlib".into()
            } else {
                format!("other:Pdf({s})")
            }
        }
        e => format!(
            "other:{}",
            format!("{e:?}").split(['(', ' ', '{']).next().unwrap_or("")
        ),
    }
}

/// (colour type, has tRNS before IDAT) from a bounds-checked chunk walk.
fn scan(d: &[u8]) -> Option<(u8, bool)> {
    if d.len() < 8 + 8 + 13 {
        return None;
    }
    let mut ct = None;
    let mut trns = false;
    let mut p = 8usize;
    while let Some(h) = d.get(p..p.checked_add(8)?) {
        let len = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
        let ty = &h[4..8];
        let body = p + 8;
        match ty {
            b"IHDR" => ct = d.get(body + 9).copied(),
            b"tRNS" => trns = true,
            b"IDAT" => break,
            _ => {}
        }
        p = body.checked_add(len)?.checked_add(4)?;
    }
    Some((ct?, trns))
}

fn rows(
    rect: Rect,
    stride: usize,
    bpp: usize,
    pix: &[u8],
    mut f: impl FnMut(&[u8]),
) -> Result<(), String> {
    let (dx, dy) = (rect.dx().max(0) as usize, rect.dy().max(0) as usize);
    for y in 0..dy {
        for x in 0..dx {
            let o = y
                .checked_mul(stride)
                .and_then(|v| v.checked_add(x * bpp))
                .ok_or("other:stride")?;
            f(pix.get(o..o + bpp).ok_or("other:short-pix")?);
        }
    }
    Ok(())
}

fn eight(img: &Image, data: &[u8]) -> Result<Vec<u8>, String> {
    let (ct, trns) = scan(data).ok_or("other:scan")?;
    let alpha = ct == 0 || ct == 4; // gray + alpha (tRNS key or alpha channel)
    let mut o = Vec::new();
    match img {
        Image::Gray(i) => rows(i.rect, i.stride, 1, &i.pix, |p| o.push(p[0]))?,
        Image::Gray16(i) => rows(i.rect, i.stride, 2, &i.pix, |p| o.push(p[0]))?,
        Image::Rgba(i) => rows(i.rect, i.stride, 4, &i.pix, |p| {
            o.extend_from_slice(&p[..3])
        })?,
        Image::Rgba64(i) => rows(i.rect, i.stride, 8, &i.pix, |p| {
            o.extend([p[0], p[2], p[4]])
        })?,
        Image::Nrgba(i) => rows(i.rect, i.stride, 4, &i.pix, |p| {
            if alpha {
                o.extend([p[0], p[3]]);
            } else {
                o.extend_from_slice(p);
            }
        })?,
        Image::Nrgba64(i) => rows(i.rect, i.stride, 8, &i.pix, |p| {
            if alpha {
                o.extend([p[0], p[6]]);
            } else {
                o.extend([p[0], p[2], p[4], p[6]]);
            }
        })?,
        Image::Paletted(i) => {
            let pal: Vec<[u8; 4]> = i
                .palette
                .iter()
                .map(|c| match *c {
                    Color::Nrgba { r, g, b, a } | Color::Rgba { r, g, b, a } => [r, g, b, a],
                    c => {
                        let (r, g, b, a) = c.rgba();
                        [
                            (r >> 8) as u8,
                            (g >> 8) as u8,
                            (b >> 8) as u8,
                            (a >> 8) as u8,
                        ]
                    }
                })
                .collect();
            rows(i.rect, i.stride, 1, &i.pix, |p| {
                let e = pal.get(p[0] as usize).copied().unwrap_or([0, 0, 0, 255]);
                o.extend_from_slice(&e[..if trns { 4 } else { 3 }]);
            })?;
        }
        _ => return Err("other:image-kind".into()),
    }
    Ok(o)
}
