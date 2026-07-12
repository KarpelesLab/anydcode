// Realistic-degradation harness: render a QR, warp/blur/noise it, write PNG.
use anyd::codes::qr::{EcLevel, QrEncoder};
use anyd::render::render;
use anyd::traits::Encode;
use anyd::transform::{self, Axis, Rng};

fn main() {
    let mut a = std::env::args().skip(1);
    let out = a.next().expect("out.png");
    let cyl: f32 = a.next().unwrap().parse().unwrap();
    let rot: f32 = a.next().unwrap().parse().unwrap();
    let blur: f32 = a.next().unwrap().parse().unwrap();
    let noise: f32 = a.next().unwrap().parse().unwrap();
    let scale: usize = a.next().map(|s| s.parse().unwrap()).unwrap_or(6);

    let e = QrEncoder::new();
    let sym = e.build_text("https://anyd.dev/x", EcLevel::M).unwrap();
    let enc = e.encode(&sym).unwrap();
    let mut img = render(&enc, scale);
    if cyl != 0.0 {
        img = transform::cylinder(&img, cyl, Axis::Vertical);
    }
    if rot != 0.0 {
        img = transform::rotate(&img, rot);
    }
    if blur != 0.0 {
        img = transform::gaussian_blur(&img, blur);
    }
    if noise != 0.0 {
        let mut rng = Rng::new(42);
        img = transform::add_noise(&img, noise, &mut rng);
    }
    let png = oxideav_png::PngImage {
        width: img.width() as u32,
        height: img.height() as u32,
        pixel_format: oxideav_png::PngPixelFormat::Gray8,
        stride: img.width(),
        data: img.pixels().to_vec(),
        palette: Vec::new(),
    };
    std::fs::write(&out, oxideav_png::encode_png_image(&png).unwrap()).unwrap();
    println!("wrote {out} {}x{}", img.width(), img.height());
}
