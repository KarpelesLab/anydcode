// Diagnostic: run the PDF417 sampler stages on a PNG. Usage: pdfdebug <png>
use anyd::traits::Decode;
fn main() {
    let path = std::env::args().nth(1).expect("usage: pdfdebug <png>");
    let rgba = oxideav_png::decode_rgba8(&std::fs::read(&path).unwrap()).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();
    match anyd::codes::pdf417::sample_grid(&frame) {
        Some(m) => {
            println!("grid {}x{}", m.width(), m.height());
            let r = anyd::codes::pdf417::Pdf417Decoder::new()
                .decode(&anyd::output::Encoding::Matrix(m.clone()));
            println!("grid decode -> {:?}", r.map(|s| s.text()));
            if std::env::args().nth(2).is_some() {
                for y in 0..m.height() {
                    let row: String = (0..m.width())
                        .map(|x| if m.get(x, y) { '#' } else { '.' })
                        .collect();
                    println!("{row}");
                }
            }
        }
        None => println!("grid: guards not located"),
    }
    println!(
        "scan -> {:?}",
        anyd::codes::pdf417::scan(&frame).map(|s| (s.text(), s.meta))
    );
}
