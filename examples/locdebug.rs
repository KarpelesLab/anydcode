// Diagnostic: what does detect::locate find on a real photo?
use anyd::detect::{LocateOptions, locate};
fn main() {
    let path = std::env::args().nth(1).expect("usage: locdebug <png>");
    let rgba = oxideav_png::decode_png_to_rgba(&std::fs::read(&path).unwrap()).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();
    let cands = locate(&frame, &LocateOptions::default());
    println!("frame {w}x{h}: {} candidates", cands.len());
    for c in &cands {
        let cr = c.location.outline.corners;
        let (xs, ys): (Vec<f32>, Vec<f32>) = (
            cr.iter().map(|p| p.x).collect(),
            cr.iter().map(|p| p.y).collect(),
        );
        let (x0, y0) = (
            xs.iter().cloned().fold(f32::MAX, f32::min),
            ys.iter().cloned().fold(f32::MAX, f32::min),
        );
        let (x1, y1) = (
            xs.iter().cloned().fold(0.0, f32::max),
            ys.iter().cloned().fold(0.0, f32::max),
        );
        let fam = c
            .symbology
            .map(|s| format!("{:?}", s.dimension()))
            .unwrap_or("?".into());
        println!(
            "  {fam:8} ({:.0},{:.0})-({:.0},{:.0})  {:.0}x{:.0}",
            x0,
            y0,
            x1,
            y1,
            x1 - x0,
            y1 - y0
        );
    }
}
