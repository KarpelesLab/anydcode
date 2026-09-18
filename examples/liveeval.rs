//! End-to-end evaluation of the **live** pipeline on synthetic camera-like scenes.
//!
//! The per-symbology image tests exercise each sampler on a pristine render; this tool
//! instead replays what a live front-end (the browser demo) actually does on every
//! frame — locate on a half-resolution grab, crop each candidate out of the full frame,
//! decode the crops, plus a whole-frame 2D pass — across a grid of scene conditions
//! (background, rotation, module size, lighting), and reports *where* reads are lost:
//!
//! * `located`  — the locator returned a usable box over the code;
//! * `crop`     — decoding the located crops read it (the 1D + located-2D path);
//! * `frame2d`  — the whole-frame half-resolution 2D pass read it;
//! * `live`     — either live path read it (what the user sees);
//! * `ideal`    — decoding a ground-truth crop read it (decoder ceiling, no locator);
//! * `false`    — reads whose text is not the planted payload.
//!
//! Run with `cargo run --release --example liveeval` (add `-- -v` to list every miss).

use anyd::codes::aztec::AztecEncoder;
use anyd::codes::code39::Code39Encoder;
use anyd::codes::code128::Code128Encoder;
use anyd::codes::datamatrix::DataMatrixEncoder;
use anyd::codes::ean::EanEncoder;
use anyd::codes::itf::ItfEncoder;
use anyd::codes::pdf417::{self, Pdf417Encoder};
use anyd::codes::qr::{EcLevel, QrEncoder};
use anyd::detect::{LocateOptions, locate};
use anyd::render::render;
use anyd::traits::Encode;
use anyd::transform::{Rng, add_noise, gaussian_blur};
use anyd::{GrayFrame, GrayImage, Symbol};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const W: usize = 1280;
const H: usize = 720;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Bg {
    /// Code printed on a plain light page.
    Page,
    /// White label on a dark surface.
    Label,
    /// Light page covered in text-like print right up to the quiet zone.
    Clutter,
}

struct Code {
    name: &'static str,
    text: &'static str,
    linear: bool,
    sprite: fn(usize) -> GrayImage,
    /// Module sizes (full-resolution pixels) to evaluate.
    scales: [usize; 2],
}

fn codes() -> Vec<Code> {
    vec![
        Code {
            name: "qr",
            text: "https://anyd.dev/live",
            linear: false,
            sprite: |s| {
                let e = QrEncoder::new();
                render(
                    &e.encode(&e.build_text("https://anyd.dev/live", EcLevel::M).unwrap())
                        .unwrap(),
                    s,
                )
            },
            scales: [4, 7],
        },
        Code {
            name: "datamatrix",
            text: "DM-LIVE-2026",
            linear: false,
            sprite: |s| {
                let e = DataMatrixEncoder::new();
                render(
                    &e.encode(&e.build_text("DM-LIVE-2026").unwrap()).unwrap(),
                    s,
                )
            },
            scales: [5, 8],
        },
        Code {
            name: "aztec",
            text: "AZTEC LIVE",
            linear: false,
            sprite: |s| {
                let e = AztecEncoder::new();
                render(&e.encode(&e.build_text("AZTEC LIVE").unwrap()).unwrap(), s)
            },
            scales: [5, 8],
        },
        Code {
            name: "pdf417",
            text: "PDF417 LIVE",
            linear: false,
            sprite: |s| {
                let e = Pdf417Encoder::new();
                let sym = e.build_text("PDF417 LIVE", pdf417::EcLevel::new(2).unwrap());
                let img = render(&e.encode(&sym.unwrap()).unwrap(), s);
                // Rows are rendered one module tall; real symbols use ~3 modules per row.
                stretch_y(&img, 3)
            },
            scales: [2, 3],
        },
        Code {
            name: "ean13",
            text: "4901085663356",
            linear: true,
            sprite: |s| {
                let e = EanEncoder::new();
                render(
                    &e.encode(&e.build_ean13("4901085663356").unwrap()).unwrap(),
                    s,
                )
            },
            scales: [2, 4],
        },
        Code {
            name: "code128",
            text: "ANYD-128-LIVE",
            linear: true,
            sprite: |s| {
                let e = Code128Encoder::new();
                render(
                    &e.encode(&e.build_text("ANYD-128-LIVE").unwrap()).unwrap(),
                    s,
                )
            },
            scales: [2, 4],
        },
        Code {
            name: "code39",
            text: "ANYD39",
            linear: true,
            sprite: |s| {
                let e = Code39Encoder::new();
                render(
                    &e.encode(&e.build(b"ANYD39", false, false).unwrap())
                        .unwrap(),
                    s,
                )
            },
            scales: [2, 4],
        },
        Code {
            name: "itf",
            text: "12345678901231",
            linear: true,
            sprite: |s| {
                let e = ItfEncoder::new();
                render(
                    &e.encode(&e.build(b"1234567890123", true).unwrap()).unwrap(),
                    s,
                )
            },
            scales: [2, 4],
        },
    ]
}

fn stretch_y(img: &GrayImage, k: usize) -> GrayImage {
    let mut out = GrayImage::filled(img.width(), img.height() * k, 255);
    for y in 0..out.height() {
        for x in 0..out.width() {
            out.set(x, y, img.get(x, y / k));
        }
    }
    out
}

/// Paste `sprite` rotated by `deg` about the canvas centre `(cx, cy)`; returns the
/// axis-aligned bounds of the pasted sprite.
fn paste(canvas: &mut GrayImage, sprite: &GrayImage, cx: f32, cy: f32, deg: f32) -> [f32; 4] {
    let (s, c) = deg.to_radians().sin_cos();
    let (sw, sh) = (sprite.width() as f32, sprite.height() as f32);
    let (hx, hy) = (sw / 2.0, sh / 2.0);
    let ex = (c * hx).abs() + (s * hy).abs();
    let ey = (s * hx).abs() + (c * hy).abs();
    let b = [
        (cx - ex).max(0.0),
        (cy - ey).max(0.0),
        (cx + ex).min(canvas.width() as f32),
        (cy + ey).min(canvas.height() as f32),
    ];
    for y in b[1] as usize..b[3] as usize {
        for x in b[0] as usize..b[2] as usize {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let sx = c * dx + s * dy + hx - 0.5;
            let sy = -s * dx + c * dy + hy - 0.5;
            if sx < -0.5 || sy < -0.5 || sx > sw - 0.5 || sy > sh - 0.5 {
                continue;
            }
            let v = sprite
                .sample_bilinear(sx.clamp(0.0, sw - 1.0), sy.clamp(0.0, sh - 1.0))
                .unwrap_or(255.0);
            canvas.set(x, y, v.round() as u8);
        }
    }
    b
}

/// Rows of glyph-sized dark boxes: printed text as the locator sees it.
fn clutter(canvas: &mut GrayImage, rng: &mut Rng) {
    let mut y = 14;
    while y + 16 < canvas.height() {
        let mut x = 12;
        while x + 12 < canvas.width() {
            if rng.next_f32() < 0.16 {
                x += 14; // word gap
                continue;
            }
            let gw = 3 + (rng.next_f32() * 6.0) as usize;
            let gh = 8 + (rng.next_f32() * 6.0) as usize;
            for yy in y + (14 - gh)..y + 14 {
                for xx in x..x + gw {
                    // Hollow-ish glyphs: strokes, not solid blocks.
                    if (xx - x) % 3 != 1 || (yy - y) % 5 == 0 {
                        canvas.set(xx, yy, 40);
                    }
                }
            }
            x += gw + 3;
        }
        y += 24;
    }
}

struct Scene {
    image: GrayImage,
    truth: [f32; 4],
}

fn scene(code: &Code, scale: usize, bg: Bg, deg: f32, gradient: bool, seed: u64) -> Scene {
    let mut rng = Rng::new(seed);
    let base = if bg == Bg::Label { 55 } else { 232 };
    let mut canvas = GrayImage::filled(W, H, base);
    if bg == Bg::Clutter {
        clutter(&mut canvas, &mut rng);
    }
    // The sprite carries its own quiet zone; a label adds a little extra white.
    let raw = (code.sprite)(scale);
    let margin = if bg == Bg::Label { 10 } else { 0 };
    let mut sprite = GrayImage::filled(raw.width() + 2 * margin, raw.height() + 2 * margin, 255);
    for y in 0..raw.height() {
        for x in 0..raw.width() {
            sprite.set(x + margin, y + margin, raw.get(x, y));
        }
    }
    // Off-centre, jittered placement.
    let cx = W as f32 * (0.40 + 0.2 * rng.next_f32());
    let cy = H as f32 * (0.42 + 0.16 * rng.next_f32());
    let truth = paste(&mut canvas, &sprite, cx, cy, deg);

    if gradient {
        // A 2.2:1 illumination falloff across the frame.
        for y in 0..H {
            for x in 0..W {
                let f = 0.45 + 0.55 * (x as f32 / W as f32);
                canvas.set(x, y, (canvas.get(x, y) as f32 * f) as u8);
            }
        }
    }
    let image = add_noise(&gaussian_blur(&canvas, 0.8), 4.0, &mut rng);
    Scene { image, truth }
}

fn half(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width() / 2, img.height() / 2);
    let mut out = GrayImage::filled(w, h, 0);
    for y in 0..h {
        for x in 0..w {
            let s = img.get(2 * x, 2 * y) as u32
                + img.get(2 * x + 1, 2 * y) as u32
                + img.get(2 * x, 2 * y + 1) as u32
                + img.get(2 * x + 1, 2 * y + 1) as u32;
            out.set(x, y, (s / 4) as u8);
        }
    }
    out
}

fn crop(img: &GrayImage, b: [f32; 4]) -> Option<GrayImage> {
    let x0 = b[0].max(0.0) as usize;
    let y0 = b[1].max(0.0) as usize;
    let x1 = (b[2].ceil() as usize).min(img.width());
    let y1 = (b[3].ceil() as usize).min(img.height());
    if x1 < x0 + 12 || y1 < y0 + 12 {
        return None;
    }
    let mut out = GrayImage::filled(x1 - x0, y1 - y0, 0);
    for y in y0..y1 {
        for x in x0..x1 {
            out.set(x - x0, y - y0, img.get(x, y));
        }
    }
    Some(out)
}

fn frame(img: &GrayImage) -> GrayFrame<'_> {
    img.as_frame()
}

#[derive(Default, Clone, Copy)]
struct Tally {
    n: u32,
    located: u32,
    crop: u32,
    frame2d: u32,
    live: u32,
    ideal: u32,
    false_reads: u32,
}

/// Maximum crops the live front-end decodes per dispatch.
const MAX_CROPS: usize = 6;

fn reads(syms: &[Symbol]) -> Vec<String> {
    syms.iter()
        .map(|s| format!("{}:{}", s.symbology, s.text().unwrap_or_default()))
        .collect()
}

#[derive(Clone, Copy)]
struct Trial {
    code: usize,
    scale: usize,
    bg: Bg,
    deg: f32,
    gradient: bool,
    seed: u64,
}

struct Outcome {
    tally: Tally,
    /// Seconds spent in: locate, crop batch, whole-frame 2D, ideal crop.
    secs: [f64; 4],
    cands: usize,
    falses: Vec<String>,
}

fn run(code: &Code, t: &Trial) -> Outcome {
    let sc = scene(code, t.scale, t.bg, t.deg, t.gradient, t.seed);
    let small = half(&sc.image);
    let mut secs = [0.0f64; 4];
    // LIVEEVAL_DUMP=<dir> writes every scene as a PGM for the other debug tools.
    if let Ok(dir) = std::env::var("LIVEEVAL_DUMP") {
        let name = format!(
            "{dir}/{}-s{}-{:?}-r{}-g{}.pgm",
            code.name, t.scale, t.bg, t.deg, t.gradient as u8
        );
        let mut bytes =
            format!("P5\n{} {}\n255\n", sc.image.width(), sc.image.height()).into_bytes();
        bytes.extend_from_slice(sc.image.pixels());
        std::fs::write(name, bytes).expect("write scene dump");
    }

    // --- locate on the half-res grab (as the demo does) ---
    let opts = LocateOptions {
        downscale: if small.width().min(small.height()) >= 800 {
            2
        } else {
            1
        },
        ..Default::default()
    };
    let t0 = Instant::now();
    let cands = locate(&frame(&small), &opts);
    secs[0] = t0.elapsed().as_secs_f64();

    let boxes: Vec<([f32; 4], Option<f32>)> = cands
        .iter()
        .map(|c| {
            let cs = c.location.outline.corners;
            let xs = cs.iter().map(|p| p.x * 2.0);
            let ys = cs.iter().map(|p| p.y * 2.0);
            let lin = c.symbology.map(|s| s.dimension()) != Some(anyd::Dimension::Matrix);
            // A linear candidate carries its reading axis; matrix ones none.
            let lin = lin.then(|| c.location.rotation.unwrap_or(0.0));
            (
                [
                    xs.clone().fold(f32::MAX, f32::min),
                    ys.clone().fold(f32::MAX, f32::min),
                    xs.fold(f32::MIN, f32::max),
                    ys.fold(f32::MIN, f32::max),
                ],
                lin,
            )
        })
        .collect();

    let tr = sc.truth;
    let t_area = (tr[2] - tr[0]) * (tr[3] - tr[1]);
    let covers = |b: &[f32; 4]| {
        let ix = (b[2].min(tr[2]) - b[0].max(tr[0])).max(0.0);
        let iy = (b[3].min(tr[3]) - b[1].max(tr[1])).max(0.0);
        let area = (b[2] - b[0]) * (b[3] - b[1]);
        ix * iy >= 0.7 * t_area && area <= 8.0 * t_area
    };
    let located = boxes.iter().any(|(b, _)| covers(b));

    // --- crop path ---
    let mut crop_reads: Vec<String> = Vec::new();
    let t0 = Instant::now();
    for (b, lin) in live_batch(&boxes) {
        let padded = [b[0] - 16.0, b[1] - 16.0, b[2] + 16.0, b[3] + 16.0];
        let Some(c) = crop(&sc.image, padded) else {
            continue;
        };
        let syms = if let Some(axis) = lin {
            anyd::pipeline::scan_1d_at(&frame(&c), axis)
        } else {
            anyd::pipeline::scan_all(&frame(&c))
        };
        crop_reads.extend(reads(&syms));
    }
    secs[1] = t0.elapsed().as_secs_f64();
    // --- whole-frame 2D path (half-res) ---
    let t0 = Instant::now();
    let f2d = reads(&anyd::pipeline::scan_2d(&frame(&small)));
    secs[2] = t0.elapsed().as_secs_f64();
    // --- decoder ceiling: ground-truth crop ---
    let t0 = Instant::now();
    let ideal = crop(
        &sc.image,
        [tr[0] - 16.0, tr[1] - 16.0, tr[2] + 16.0, tr[3] + 16.0],
    )
    .map(|c| reads(&anyd::pipeline::scan_all(&frame(&c))))
    .unwrap_or_default();
    secs[3] = t0.elapsed().as_secs_f64();

    let want = format!(":{}", code.text);
    let hit = |v: &[String]| v.iter().any(|s| s.ends_with(&want));
    let mut falses: Vec<String> = crop_reads
        .iter()
        .chain(f2d.iter())
        .filter(|s| !s.ends_with(&want))
        .cloned()
        .collect();
    falses.sort();
    falses.dedup();
    Outcome {
        tally: Tally {
            n: 1,
            located: located as u32,
            crop: hit(&crop_reads) as u32,
            frame2d: hit(&f2d) as u32,
            live: (hit(&crop_reads) || hit(&f2d)) as u32,
            ideal: hit(&ideal) as u32,
            false_reads: falses.len() as u32,
        },
        secs,
        cands: boxes.len(),
        falses,
    }
}

fn main() {
    let verbose = std::env::args().any(|a| a == "-v");
    let only: Option<String> = std::env::args().skip(1).find(|a| !a.starts_with('-'));
    let codes = codes();

    let mut trials: Vec<Trial> = Vec::new();
    let mut seed = 1u64;
    for (ci, code) in codes.iter().enumerate() {
        for &scale in &code.scales {
            for bg in [Bg::Page, Bg::Label, Bg::Clutter] {
                for deg in [0.0f32, 4.0, 17.0, 45.0, 90.0] {
                    for gradient in [false, true] {
                        seed += 1;
                        if only.as_deref().is_none_or(|o| o == code.name) {
                            trials.push(Trial {
                                code: ci,
                                scale,
                                bg,
                                deg,
                                gradient,
                                seed,
                            });
                        }
                    }
                }
            }
        }
    }

    // Trials are independent: fan them out over the available cores.
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(t) = trials.get(i) else { break };
                    let out = run(&codes[t.code], t);
                    results.lock().unwrap().push((i, out));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);

    let mut per_code: Vec<Tally> = vec![Tally::default(); codes.len()];
    let mut per_cond: BTreeMap<String, Tally> = BTreeMap::new();
    let mut secs = [(0.0f64, 0.0f64); 4];
    for (i, out) in &results {
        let t = &trials[*i];
        let code = &codes[t.code];
        add(&mut per_code[t.code], out.tally);
        for key in [
            format!("bg={:?}", t.bg),
            format!("rot={}", t.deg),
            format!("light={}", if t.gradient { "gradient" } else { "flat" }),
            format!("kind={}", if code.linear { "1d" } else { "2d" }),
        ] {
            add(per_cond.entry(key).or_default(), out.tally);
        }
        for (acc, s) in secs.iter_mut().zip(out.secs) {
            acc.0 += s;
            acc.1 = acc.1.max(s);
        }
        if verbose && (out.tally.live == 0 || !out.falses.is_empty()) {
            println!(
                "{} {:<10} scale={} bg={:?} rot={} grad={} located={} ideal={} cands={} falses={:?}",
                if out.tally.live == 0 {
                    "MISS "
                } else {
                    "FALSE"
                },
                code.name,
                t.scale,
                t.bg,
                t.deg,
                t.gradient,
                out.tally.located,
                out.tally.ideal,
                out.cands,
                out.falses
            );
        }
    }

    let line = |name: &str, t: &Tally| {
        let p = |v: u32| 100.0 * v as f32 / t.n.max(1) as f32;
        println!(
            "{name:<16} n={:<4} located={:5.1}%  crop={:5.1}%  frame2d={:5.1}%  live={:5.1}%  ideal={:5.1}%  false={}",
            t.n,
            p(t.located),
            p(t.crop),
            p(t.frame2d),
            p(t.live),
            p(t.ideal),
            t.false_reads
        );
    };
    println!("--- by symbology ---");
    let mut total = Tally::default();
    for (code, t) in codes.iter().zip(&per_code) {
        if t.n > 0 {
            line(code.name, t);
            add(&mut total, *t);
        }
    }
    println!("--- by condition ---");
    for (k, t) in &per_cond {
        line(k, t);
    }
    println!("--- overall ---");
    line("all", &total);
    let n = results.len().max(1) as f64;
    for (name, (sum, worst)) in ["locate", "crop batch", "frame2d", "ideal crop"]
        .iter()
        .zip(secs)
    {
        println!(
            "{name:<11}: {:7.1} ms/frame avg, {:7.1} ms worst",
            1000.0 * sum / n,
            1000.0 * worst
        );
    }
}

/// The crops the live front-end actually decodes from a candidate list.
fn live_batch(boxes: &[([f32; 4], Option<f32>)]) -> Vec<([f32; 4], Option<f32>)> {
    let mut v: Vec<_> = boxes.to_vec();
    if std::env::var("LIVEEVAL_LEGACY_BATCH").is_ok() {
        v.truncate(MAX_CROPS);
        return v;
    }
    // Linear crops first (cheap), then the rest, capped.
    v.sort_by_key(|(_, lin)| lin.is_none());
    v.truncate(MAX_CROPS);
    v
}

fn add(a: &mut Tally, b: Tally) {
    a.n += b.n;
    a.located += b.located;
    a.crop += b.crop;
    a.frame2d += b.frame2d;
    a.live += b.live;
    a.ideal += b.ideal;
    a.false_reads += b.false_reads;
}
