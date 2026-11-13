// Run the still-image pipeline over a corpus of photographs with known payloads.
//
// Usage: corpus [--live] [-v] [--scale=<n>] <dir>...
// Each <dir> holds `N.png` beside `N.txt` (the expected text), `N.bin` (the expected
// bytes) or `N.sjis` (the text in Shift-JIS); a PNG with neither must not read (a false-positive set). `ci/fetch-corpus.sh`
// fetches the ZXing blackbox sets in this layout.
//
// Default: `pipeline::scan_all` on the whole image, as the CLI does. `--live`: the
// demo's path — `detect::locate`, a decode of each padded crop, then `scan_2d` on the
// frame. `-v` lists every image that did not read; `--scale=<n>` enlarges each image
// by pixel replication first (to tell a resolution limit from anything else).
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyd::GrayFrame;
use anyd::symbol::Symbol;

fn load(path: &Path) -> Option<(Vec<u8>, usize, usize)> {
    let rgba = oxideav_png::decode_rgba8(&std::fs::read(path).ok()?).ok()?;
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma = rgba
        .data
        .as_chunks::<4>()
        .0
        .iter()
        // Transparent pixels are paper.
        .map(|p| {
            let y = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
            ((y * p[3] as u32 + 255 * (255 - p[3] as u32)) / 255) as u8
        })
        .collect();
    Some((luma, w, h))
}

fn live(luma: &[u8], w: usize, h: usize) -> Vec<Symbol> {
    let frame = GrayFrame::new(luma, w, h).unwrap();
    let opts = anyd::detect::LocateOptions {
        downscale: if w.min(h) >= 800 { 2 } else { 1 },
        ..Default::default()
    };
    let mut found = Vec::new();
    for c in anyd::detect::locate(&frame, &opts) {
        let cs = c.location.outline.corners;
        let span = |f: fn(&anyd::geometry::Point) -> f32, limit: usize| {
            let lo = cs.iter().map(f).fold(f32::MAX, f32::min) - 16.0;
            let hi = cs.iter().map(f).fold(f32::MIN, f32::max) + 16.0;
            (
                lo.clamp(0.0, (limit - 1) as f32) as usize,
                hi.clamp(0.0, (limit - 1) as f32) as usize,
            )
        };
        let ((x0, x1), (y0, y1)) = (span(|p| p.x, w), span(|p| p.y, h));
        let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
        if cw < 8 || ch < 8 {
            continue;
        }
        let mut crop = vec![0u8; cw * ch];
        for y in 0..ch {
            crop[y * cw..(y + 1) * cw]
                .copy_from_slice(&luma[(y0 + y) * w + x0..(y0 + y) * w + x0 + cw]);
        }
        let crop = GrayFrame::new(&crop, cw, ch).unwrap();
        let linear = c.symbology.map(|s| s.dimension()) != Some(anyd::Dimension::Matrix);
        found.extend(if linear {
            anyd::pipeline::scan_linear_at(&crop, c.location.rotation.unwrap_or(0.0))
        } else {
            anyd::pipeline::scan_all(&crop)
        });
    }
    found.extend(anyd::pipeline::scan_2d(&frame));
    found
}

/// Whether `symbol` carries `want`: the same bytes, or the same text once the
/// payload is taken as Latin-1 (what the corpus' UTF-8 expectations transcribe).
fn matches(symbol: &Symbol, want: &[u8]) -> bool {
    let got = symbol.payload_bytes();
    let trim = |b: &[u8]| -> Vec<u8> {
        let mut b = b.to_vec();
        while b.last().is_some_and(|c| matches!(c, b'\n' | b'\r')) {
            b.pop();
        }
        b
    };
    let latin1: String = got.iter().map(|&b| b as char).collect();
    got == want || trim(&got) == trim(want) || latin1.as_bytes() == want
}

fn main() {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let (mut use_live, mut verbose, mut scale) = (false, false, 1usize);
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--live" => use_live = true,
            "-v" => verbose = true,
            _ if a.starts_with("--scale=") => scale = a[8..].parse().expect("--scale=<n>"),
            _ => dirs.push(a.into()),
        }
    }
    let (mut total, mut total_ok, mut total_wrong) = (0, 0, 0);
    for dir in &dirs {
        let mut images: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .collect();
        images.sort_by_key(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            (stem.parse::<u64>().unwrap_or(u64::MAX), stem)
        });
        let (mut ok, mut wrong, mut n, mut ms, mut worst) = (0, 0, 0, 0.0f64, 0.0f64);
        let mut missed = Vec::new();
        for path in &images {
            // Any of the transcriptions of the expected payload.
            let wants: Vec<Vec<u8>> = ["txt", "bin", "sjis"]
                .iter()
                .filter_map(|ext| std::fs::read(path.with_extension(ext)).ok())
                .collect();
            let want = (!wants.is_empty()).then_some(wants);
            let Some((luma, w, h)) = load(path) else {
                eprintln!("unreadable: {}", path.display());
                continue;
            };
            // Diagnostic: enlarge by pixel replication before scanning.
            let (luma, w, h) = if scale > 1 {
                let mut big = vec![0u8; w * h * scale * scale];
                for y in 0..h * scale {
                    for x in 0..w * scale {
                        big[y * w * scale + x] = luma[(y / scale) * w + x / scale];
                    }
                }
                (big, w * scale, h * scale)
            } else {
                (luma, w, h)
            };
            n += 1;
            let t = Instant::now();
            let found = if use_live {
                live(&luma, w, h)
            } else {
                anyd::pipeline::scan_all(&GrayFrame::new(&luma, w, h).unwrap())
            };
            let dt = t.elapsed().as_secs_f64() * 1000.0;
            ms += dt;
            worst = worst.max(dt);
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            match &want {
                Some(wants) if found.iter().any(|s| wants.iter().any(|w| matches(s, w))) => ok += 1,
                Some(_) if found.is_empty() => missed.push(format!("{name} ({w}x{h})")),
                // Something read, but not the expected payload — or, in a
                // false-positive set, anything read at all.
                _ if !found.is_empty() => {
                    wrong += 1;
                    let got = &found[0];
                    missed.push(format!(
                        "{name} ({w}x{h}) WRONG {:?} {:?}",
                        got.symbology,
                        String::from_utf8_lossy(&got.payload_bytes())
                            .chars()
                            .take(40)
                            .collect::<String>()
                    ));
                }
                _ => ok += 1,
            }
        }
        println!(
            "{:<24} {ok:>3}/{n:<3} {:>5.1}%  wrong={wrong:<2} {:>6.1} ms avg {:>7.1} ms worst",
            dir.file_name().unwrap().to_string_lossy(),
            100.0 * ok as f64 / n.max(1) as f64,
            ms / n.max(1) as f64,
            worst
        );
        if verbose {
            for m in &missed {
                println!("    {m}");
            }
        }
        total += n;
        total_ok += ok;
        total_wrong += wrong;
    }
    println!(
        "{:<24} {total_ok:>3}/{total:<3} {:>5.1}%  wrong={total_wrong}",
        "TOTAL",
        100.0 * total_ok as f64 / total.max(1) as f64
    );
}
