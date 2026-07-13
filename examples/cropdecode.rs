// Diagnostic: reproduce the live demo's crop-worker decode on a captured frame.
// Usage: cropdecode <png> [x0 y0 x1 y1] [--dump out.raw]
// Box in pixels, padded 16px like the demo; --dump writes the crop as raw luma
// (w and h printed) for feeding other harnesses, e.g. the wasm build under Node.
use std::time::Instant;

fn main() {
    let mut raw_args: Vec<String> = std::env::args().skip(1).collect();
    let dump = raw_args
        .iter()
        .position(|a| a == "--dump")
        .map(|i| raw_args.remove(i + 1))
        .inspect(|_| {
            raw_args.retain(|a| a != "--dump");
        });
    let mut args = raw_args.into_iter();
    let path = args.next().expect("usage: cropdecode <png> [x0 y0 x1 y1]");
    let rgba = oxideav_png::decode_png_to_rgba(&std::fs::read(&path).unwrap()).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();

    let boxed: Vec<usize> = args.map(|a| a.parse().unwrap()).collect();
    let (x0, y0, x1, y1) = if boxed.len() == 4 {
        (
            boxed[0].saturating_sub(16),
            boxed[1].saturating_sub(16),
            (boxed[2] + 16).min(w),
            (boxed[3] + 16).min(h),
        )
    } else {
        (0, 0, w, h)
    };
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut crop = vec![0u8; cw * ch];
    for y in 0..ch {
        crop[y * cw..(y + 1) * cw]
            .copy_from_slice(&luma[(y0 + y) * w + x0..(y0 + y) * w + x1]);
    }
    let frame = anyd::GrayFrame::new(&crop, cw, ch).unwrap();
    if let Some(out) = dump {
        std::fs::write(&out, &crop).unwrap();
        println!("dumped {cw}x{ch} raw luma to {out}");
    }

    let t = Instant::now();
    let found = anyd::pipeline::scan_all(&frame);
    println!(
        "scan_all on {cw}x{ch} crop ({x0},{y0})-({x1},{y1}): {} results in {:.1}ms",
        found.len(),
        t.elapsed().as_secs_f64() * 1000.0
    );
    for s in &found {
        println!("  {}: {:?}", s.symbology, s.text());
    }

    // Break the 1D path down so a miss can be attributed.
    let t = Instant::now();
    let ean = anyd::codes::ean::scan(&frame, &anyd::scan1d::ScanOptions::default());
    println!(
        "ean::scan (width-ratio edge path): {:?} in {:.1}ms",
        ean.and_then(|s| s.text()),
        t.elapsed().as_secs_f64() * 1000.0
    );
    let t = Instant::now();
    let lines = anyd::scan1d::scan_lines(&frame, &anyd::scan1d::ScanOptions::default());
    println!(
        "scan1d::scan_lines: {} candidates in {:.1}ms (conf: {:?})",
        lines.len(),
        t.elapsed().as_secs_f64() * 1000.0,
        lines.iter().map(|c| c.confidence).collect::<Vec<_>>()
    );
}
