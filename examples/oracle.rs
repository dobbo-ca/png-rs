//! Parity oracle (docs/design.md, "Oracle tools" 1).
//!
//! `oracle [--profile go|libpng] [--threads N] <in-dir> <out-dir>` decodes every `.png` file and
//! every mutation file (name contains `.mut`) under in-dir and writes `<name>.native`,
//! `<name>.eight`, `<name>.meta` or `<name>.err`. `oracle gen <in-dir> <out-dir>` writes
//! deterministic mutations `<name>.mut<k>`. `<name>` is the path relative to in-dir with `/`
//! replaced by `__`.

use png::{ColorType, Error, Format, Info, Limits, Options, Profile, Trns};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const MAX_MUTATIONS: usize = 2000;
const PER_INPUT: usize = 14;

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p.strip_prefix(root).unwrap();
            let parts: Vec<_> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push((parts.join("__"), p));
        }
    }
}

fn token(e: &Error) -> &'static str {
    match e {
        Error::NotPng => "notpng",
        Error::Checksum => "checksum",
        Error::ChunkOrder => "chunk-order",
        Error::Format(_) | Error::BadChunkLength(_) | Error::Trailing(_) => "format",
        Error::Unsupported(_) | Error::UnsupportedDepthColor { .. } => "unsupported",
        Error::UnexpectedEof => "eof",
        Error::Zlib(_) => "zlib",
        Error::Cancelled => "cancelled",
        Error::WidthLimit
        | Error::HeightLimit
        | Error::PixelLimit
        | Error::MetadataLimit
        | Error::AllocLimit
        | Error::OutOfMemory
        | Error::BufferTooSmall => "limit",
        _ => "format",
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        write!(s, "{x:02x}").unwrap();
        s
    })
}

fn meta(info: &Info) -> String {
    let h = &info.header;
    let ct = match h.color_type {
        ColorType::Gray => 0,
        ColorType::Rgb => 2,
        ColorType::Indexed => 3,
        ColorType::GrayAlpha => 4,
        ColorType::Rgba => 6,
    };
    let mut s = format!(
        "size {} {}\ndepth {}\ncolor {}\ninterlace {}\n",
        h.width, h.height, h.bit_depth, ct, h.interlaced as u8
    );
    if !info.palette.is_empty() {
        let flat: Vec<u8> = info.palette.iter().flatten().copied().collect();
        writeln!(s, "plte {} {}", info.palette.len(), hex(&flat)).unwrap();
    }
    if let Some(t) = &info.trns {
        let raw: Vec<u8> = match t {
            Trns::Gray(v) => v.to_be_bytes().to_vec(),
            Trns::Rgb(v) => v.iter().flat_map(|x| x.to_be_bytes()).collect(),
            Trns::Alpha(v) => v.clone(),
        };
        writeln!(s, "trns {}", hex(&raw)).unwrap();
    }
    s
}

fn run(args: &[String]) {
    let mut profile = Profile::Go;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut pos = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--profile" => {
                profile = match it.next().map(String::as_str) {
                    Some("go") => Profile::Go,
                    Some("libpng") => Profile::Libpng,
                    _ => usage(),
                }
            }
            "--threads" => {
                threads = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            _ => pos.push(a),
        }
    }
    let [indir, outdir] = pos[..] else { usage() };
    let (indir, outdir) = (Path::new(indir), Path::new(outdir));
    std::fs::create_dir_all(outdir).unwrap();
    let mut files = Vec::new();
    walk(indir, indir, &mut files);
    let opts = Options { profile };
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    let cancel = AtomicBool::new(false);
    for (name, path) in files {
        if !(name.ends_with(".png") || name.contains(".mut")) {
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        let out = |ext: &str| outdir.join(format!("{name}.{ext}"));
        let native = png::decode(&data, Format::Native, &opts, &limits, &cancel);
        let eight = png::decode(&data, Format::Eight, &opts, &limits, &cancel);
        match (native, eight) {
            (Ok((info, n)), Ok((_, e))) => {
                std::fs::write(out("native"), n).unwrap();
                std::fs::write(out("eight"), e).unwrap();
                std::fs::write(out("meta"), meta(&info)).unwrap();
            }
            (Err(e), _) | (_, Err(e)) => {
                std::fs::write(out("err"), format!("{}\n", token(&e))).unwrap()
            }
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: oracle [--profile go|libpng] [--threads N] <in-dir> <out-dir>\n       oracle gen <in-dir> <out-dir>"
    );
    std::process::exit(2)
}

struct Rng(u64);

impl Rng {
    fn new(name: &str) -> Self {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in name.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
        Rng(h | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// (offset of the length field, total chunk length) of each well-formed chunk header.
fn chunks(d: &[u8]) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut p = 8;
    while let Some(h) = d.get(p..p + 8) {
        let len = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
        let total = len.saturating_add(12);
        if p.saturating_add(total) > d.len() {
            break;
        }
        v.push((p, total));
        p += total;
    }
    v
}

fn generate(args: &[String]) {
    let [indir, outdir] = args else { usage() };
    let (indir, outdir) = (Path::new(indir), Path::new(outdir));
    std::fs::create_dir_all(outdir).unwrap();
    let mut files = Vec::new();
    walk(indir, indir, &mut files);
    files.retain(|(n, _)| n.ends_with(".png"));
    // At most MAX_MUTATIONS files in total: spread the chosen inputs evenly over the sorted list.
    let k = files.len().min(MAX_MUTATIONS / PER_INPUT);
    for i in 0..k {
        let (name, path) = &files[i * files.len() / k];
        let data = std::fs::read(path).unwrap();
        let mut rng = Rng::new(name);
        let cks = chunks(&data);
        let idat = cks
            .iter()
            .find(|&&(p, _)| data.get(p + 4..p + 8) == Some(b"IDAT"));
        let mut muts: Vec<Vec<u8>> = Vec::new();
        for _ in 0..4 {
            muts.push(data[..rng.below(data.len())].to_vec());
        }
        for j in 0..8 {
            let mut m = data.clone();
            let pos = match idat {
                Some(&(p, t)) if j < 2 && t > 12 => p + 8 + rng.below(t - 12),
                _ => rng.below(m.len()),
            };
            if let Some(b) = m.get_mut(pos) {
                *b ^= 1 << rng.below(8);
            }
            muts.push(m);
        }
        for _ in 0..2 {
            let mut m = data.clone();
            let p = if cks.is_empty() {
                8
            } else {
                cks[rng.below(cks.len())].0
            };
            let old = m
                .get(p..p + 4)
                .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            let new = match rng.below(3) {
                0 => old.wrapping_add(1 + rng.below(8) as u32),
                1 => old.wrapping_sub(1 + rng.below(8) as u32),
                _ => rng.next() as u32,
            };
            if let Some(b) = m.get_mut(p..p + 4) {
                b.copy_from_slice(&new.to_be_bytes());
            }
            muts.push(m);
        }
        for (j, m) in muts.iter().enumerate() {
            std::fs::write(outdir.join(format!("{name}.mut{j}")), m).unwrap();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen") => generate(&args[1..]),
        Some(_) => run(&args),
        None => usage(),
    }
}
