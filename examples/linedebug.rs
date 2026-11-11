//! Diagnostic: what does the 1D front-end recover from a frame, line by line, compared
//! with the module pattern that is actually there?
//!
//! `cargo run --release --example linedebug -- <scene.pgm> <code128|code39|itf|ean13> <payload> [axis-deg]`
//!
//! Prints every span candidate of every scan line with its length, quiet zone and its
//! best-aligned Hamming distance to the reference pattern (also tried mirrored), then
//! what [`anyd::pipeline::scan_1d_at`] reads. Scenes come from `LIVEEVAL_DUMP=<dir>`.

use anyd::codes::code39::Code39Encoder;
use anyd::codes::code128::Code128Encoder;
use anyd::codes::ean::EanEncoder;
use anyd::codes::itf::ItfEncoder;
use anyd::output::Encoding;
use anyd::scan1d::{ScanOptions, scan_spans};
use anyd::traits::Encode;

fn read_pgm(path: &str) -> (usize, usize, Vec<u8>) {
    let bytes = std::fs::read(path).expect("read pgm");
    // "P5\n<w> <h>\n255\n" then raw bytes.
    let mut fields = Vec::new();
    let mut pos = 0;
    while fields.len() < 4 {
        while bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let start = pos;
        while !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..pos]).into_owned());
    }
    assert_eq!(fields[0], "P5", "binary PGM expected");
    let (w, h) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    (w, h, bytes[pos + 1..].to_vec())
}

fn reference(kind: &str, payload: &str) -> Vec<bool> {
    let enc = match kind {
        "code128" => {
            let e = Code128Encoder::new();
            e.encode(&e.build_text(payload).unwrap()).unwrap()
        }
        "code39" => {
            let e = Code39Encoder::new();
            e.encode(&e.build(payload.as_bytes(), false, false).unwrap())
                .unwrap()
        }
        "itf" => {
            let e = ItfEncoder::new();
            e.encode(&e.build(payload.as_bytes(), false).unwrap())
                .unwrap()
        }
        "ean13" => {
            let e = EanEncoder::new();
            e.encode(&e.build_ean13(payload).unwrap()).unwrap()
        }
        other => panic!("unknown symbology {other}"),
    };
    match enc {
        Encoding::Linear(p) => p.modules,
        _ => unreachable!("linear symbology"),
    }
}

/// Least Hamming distance of `a` against `b` over every alignment of the shorter in the
/// longer, plus the length difference.
fn distance(a: &[bool], b: &[bool]) -> usize {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    (0..=long.len() - short.len())
        .map(|off| {
            short
                .iter()
                .zip(&long[off..])
                .filter(|(x, y)| x != y)
                .count()
        })
        .min()
        .unwrap_or(short.len())
        + (long.len() - short.len())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (w, h, luma) = read_pgm(&args[1]);
    let want = reference(&args[2], &args[3]);
    let axis: f32 = args.get(4).map_or(0.0, |a| a.parse().unwrap());
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();
    let mirrored: Vec<bool> = want.iter().rev().copied().collect();
    let show = |m: &[bool]| -> String { m.iter().map(|&b| if b { '#' } else { '.' }).collect() };

    println!("reference: {} modules\n{}", want.len(), show(&want));
    let mut best = usize::MAX;
    for c in scan_spans(&frame, &ScanOptions::around(axis)) {
        let m = &c.pattern.modules;
        if m.len() * 2 < want.len() {
            continue;
        }
        let d = distance(m, &want).min(distance(m, &mirrored));
        best = best.min(d);
        println!(
            "len={:<4} quiet={:<3} module={:.2}px conf={:.2} dist={d}",
            m.len(),
            c.pattern.quiet_zone,
            c.location.module_size.unwrap_or(0.0),
            c.confidence
        );
        if d > 0 && d <= 12 {
            println!("{}", show(m));
        }
    }
    println!("best distance: {best}");
    for s in anyd::pipeline::scan_1d_at(&frame, axis.to_radians()) {
        println!("READ {}: {:?}", s.symbology, s.text());
    }
}
