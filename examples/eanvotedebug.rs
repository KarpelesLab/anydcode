// Diagnostic: per-scanline EAN/UPC edge-decode vote tally (to tune the consensus floor).
use anyd::codes::ean::decode_edges;
use anyd::scan1d::{ScanOptions, scan_edges};

fn main() {
    let path = std::env::args().nth(1).expect("usage: eanvotedebug <png>");
    let rgba = oxideav_png::decode_png_to_rgba(&std::fs::read(&path).unwrap()).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();

    let opts = ScanOptions::default();
    let lines = scan_edges(&frame, &opts);
    let mut tally: Vec<(String, usize)> = Vec::new();
    let mut decoded = 0usize;
    for c in &lines {
        if let Some(sym) = decode_edges(&c.edges) {
            decoded += 1;
            let key = format!("{:?}|{}", sym.symbology, sym.text().unwrap_or_default());
            if let Some(e) = tally.iter_mut().find(|(k, _)| *k == key) {
                e.1 += 1;
            } else {
                tally.push((key, 1));
            }
        }
    }
    tally.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!(
        "frame {w}x{h}: {} scanlines, {decoded} decoded, {} distinct values",
        lines.len(),
        tally.len()
    );
    for (k, n) in &tally {
        println!("  {n:4}  {k}");
    }
}
