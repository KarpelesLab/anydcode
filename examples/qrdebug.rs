fn main() {
    let path = std::env::args().nth(1).expect("usage: qrdebug <png>");
    let bytes = std::fs::read(&path).unwrap();
    let rgba = oxideav_png::decode_png_to_rgba(&bytes).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();
    if let Ok(m) = anyd::codes::qr::sample_grid(&frame) {
        for y in 0..m.height() {
            let row: String = (0..m.width())
                .map(|x| if m.get(x, y) { '#' } else { '.' })
                .collect();
            println!("{row}");
        }
    }
    println!(
        "scan -> {:?}",
        anyd::codes::qr::scan(&frame).map(|s| s.text())
    );
}
