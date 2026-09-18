//! Diagnostic: per-sampler wall time of the 2D scan pass on a PNG (or on a synthetic
//! text-like frame when no path is given).
use std::time::Instant;
fn main() {
    let (luma, w, h) = match std::env::args().nth(1) {
        Some(path) => {
            let rgba = oxideav_png::decode_png_to_rgba(&std::fs::read(&path).unwrap()).unwrap();
            let (w, h) = (rgba.width as usize, rgba.height as usize);
            let luma: Vec<u8> = rgba
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
                .collect();
            (luma, w, h)
        }
        None => {
            let (w, h) = (640usize, 360usize);
            let mut rng = anyd::transform::Rng::new(7);
            let mut luma = vec![230u8; w * h];
            for y in (10..h - 16).step_by(20) {
                let mut x = 8;
                while x + 10 < w {
                    let gw = 2 + (rng.next_f32() * 5.0) as usize;
                    for yy in y..y + 10 {
                        for xx in x..x + gw {
                            luma[yy * w + xx] = 40;
                        }
                    }
                    x += gw + 2 + (rng.next_f32() * 4.0) as usize;
                }
            }
            (luma, w, h)
        }
    };
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();
    println!("frame {w}x{h}");
    macro_rules! t {
        ($name:expr, $e:expr) => {{
            let t0 = Instant::now();
            let r = $e;
            println!(
                "{:<12} {:8.1} ms  found={}",
                $name,
                t0.elapsed().as_secs_f64() * 1e3,
                r
            );
        }};
    }
    t!("qr", anyd::codes::qr::scan(&frame).is_ok());
    t!("datamatrix", anyd::codes::datamatrix::scan(&frame).is_ok());
    t!("aztec", anyd::codes::aztec::scan(&frame).is_ok());
    t!("microqr", anyd::codes::microqr::scan(&frame).is_ok());
    t!("rmqr", anyd::codes::rmqr::scan(&frame).is_ok());
    t!("pdf417", anyd::codes::pdf417::scan(&frame).is_some());
    t!("micropdf", anyd::codes::pdf417::scan_micro(&frame).is_ok());
    t!("appclip", anyd::codes::appclip::scan(&frame).is_ok());
    t!("scan_1d", {
        let v = anyd::pipeline::scan_1d(&frame);
        for s in &v {
            println!(
                "   1d read: {} {:?}",
                s.symbology,
                s.text().unwrap_or_default()
            );
        }
        !v.is_empty()
    });
    t!(
        "locate",
        !anyd::detect::locate(&frame, &Default::default()).is_empty()
    );
}
