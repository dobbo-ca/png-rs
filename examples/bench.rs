//! `bench <dir> <reps>`: decode_into with one reused Decoder (Format::Native, Go profile) for
//! every `.png` under dir, with max_threads 1 and with the default. TSV on stdout:
//! `name<TAB>bytes<TAB>ns<TAB>threads` where bytes is the file size and ns the mean time of one
//! decode over `reps` timed runs after one untimed warm-up. Files that fail to decode are skipped.

use png::{Decoder, Format, Limits, Options};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(root, &p, out);
        } else if p.extension().is_some_and(|e| e == "png") {
            let rel = p.strip_prefix(root).unwrap();
            let parts: Vec<_> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push((parts.join("__"), p));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, reps] = &args[..] else {
        eprintln!("usage: bench <dir> <reps>");
        std::process::exit(2);
    };
    let reps: u32 = reps.parse().unwrap_or(1).max(1);
    let dir = Path::new(dir);
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    let many = std::thread::available_parallelism().map_or(1, |n| n.get());
    let (opts, cancel) = (Options::default(), AtomicBool::new(false));
    let mut decoders = [(1, Decoder::new()), (many, Decoder::new())];
    for (name, path) in files {
        let data = std::fs::read(&path).unwrap();
        for (threads, dec) in decoders.iter_mut() {
            let limits = Limits {
                max_threads: *threads,
                ..Limits::default()
            };
            let Some(size) = png::read_info(&data, &opts, &limits)
                .ok()
                .and_then(|i| i.output_size(Format::Native))
            else {
                continue;
            };
            let mut out = vec![0u8; size];
            if dec
                .decode_into(&data, Format::Native, &mut out, &opts, &limits, &cancel)
                .is_err()
            {
                continue;
            }
            let t = Instant::now();
            for _ in 0..reps {
                std::hint::black_box(
                    dec.decode_into(&data, Format::Native, &mut out, &opts, &limits, &cancel)
                        .is_ok(),
                );
            }
            let ns = t.elapsed().as_nanos() / reps as u128;
            println!("{name}\t{}\t{ns}\t{threads}", data.len());
        }
    }
}
