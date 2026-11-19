//! `bench <dir> <reps> [max_threads]`: decode_into with one reused Decoder (Format::Eight, Go
//! profile, as `pngoracle bench` decodes with libpng) for every `.png` under dir. `max_threads`
//! defaults to available_parallelism. TSV on stdout: `name<TAB>bytes<TAB>ns` where bytes is the
//! output size and ns the mean time of one decode over `reps` timed runs after one untimed
//! warm-up. Files that fail to decode are skipped.

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
    let (dir, reps, threads) = match &args[..] {
        [d, r] => (d, r, None),
        [d, r, t] => (d, r, t.parse::<usize>().ok()),
        _ => {
            eprintln!("usage: bench <dir> <reps> [max_threads]");
            std::process::exit(2);
        }
    };
    let reps: u32 = reps.parse().unwrap_or(1).max(1);
    let dir = Path::new(dir);
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    let threads =
        threads.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    let (opts, cancel) = (Options::default(), AtomicBool::new(false));
    let mut dec = Decoder::new();
    for (name, path) in files {
        let data = std::fs::read(&path).unwrap();
        {
            let limits = Limits {
                max_threads: threads,
                ..Limits::default()
            };
            let Some(size) = png::read_info(&data, &opts, &limits)
                .ok()
                .and_then(|i| i.output_size(Format::Eight))
            else {
                continue;
            };
            let mut out = vec![0u8; size];
            if dec
                .decode_into(&data, Format::Eight, &mut out, &opts, &limits, &cancel)
                .is_err()
            {
                continue;
            }
            let t = Instant::now();
            for _ in 0..reps {
                std::hint::black_box(
                    dec.decode_into(&data, Format::Eight, &mut out, &opts, &limits, &cancel)
                        .is_ok(),
                );
            }
            let ns = t.elapsed().as_nanos() / reps as u128;
            println!("{name}\t{size}\t{ns}");
        }
    }
}
