// Diagnostic: run the Data Matrix sampler on a PNG. Usage: dmdebug <png> [scale]
// With a scale, the image is first enlarged by pixel replication.
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: dmdebug <png> [scale]");
    let scale: usize = args.next().map_or(1, |s| s.parse().unwrap());
    let rgba = oxideav_png::decode_rgba8(&std::fs::read(&path).unwrap()).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let mut big = vec![0u8; w * h * scale * scale];
    for y in 0..h * scale {
        for x in 0..w * scale {
            big[y * w * scale + x] = luma[(y / scale) * w + x / scale];
        }
    }
    let frame = anyd::GrayFrame::new(&big, w * scale, h * scale).unwrap();
    match anyd::codes::datamatrix::sample_grid(&frame) {
        Ok(m) => println!("grid {}x{}", m.width(), m.height()),
        Err(e) => println!("grid: {e}"),
    }
    println!(
        "scan -> {:?}",
        anyd::codes::datamatrix::scan(&frame).map(|s| (s.text(), s.meta))
    );
}
