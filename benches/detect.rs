//! Dependency-free benchmark for the fast frame locator (`anyd::detect`).
//!
//! No criterion: timing uses `std::time::Instant` with an explicit warmup and many
//! iterations, reporting median, p95 and FPS. It measures [`locate`] across three
//! resolutions and three scenarios (empty / one QR / a few mixed codes), and — for
//! contrast — a full QR decode (`anyd::codes::qr::scan`) on the one-QR frames, so the
//! table shows the detect-vs-decode speedup that motivates the two-stage pipeline.
//!
//! The heavy timing lives entirely in `main`, which `cargo test` does not run for a
//! `harness = false` bench. Run it with `cargo bench --bench detect`.

use std::hint::black_box;
use std::time::Instant;

use anyd::GrayImage;
use anyd::codes::code128::Code128Encoder;
use anyd::codes::datamatrix::DataMatrixEncoder;
use anyd::codes::qr::{self, EcLevel, QrEncoder};
use anyd::detect::{LocateOptions, locate};
use anyd::geometry::Point;
use anyd::render::render;
use anyd::traits::Encode;

// ----------------------------------------------------------------------------------
// Frame synthesis
// ----------------------------------------------------------------------------------

fn qr_image(text: &str, scale: usize) -> GrayImage {
    let enc = QrEncoder::new();
    let sym = enc.build_text(text, EcLevel::M).unwrap();
    render(&enc.encode(&sym).unwrap(), scale)
}

fn datamatrix_image(text: &str, scale: usize) -> GrayImage {
    let enc = DataMatrixEncoder::new();
    let sym = enc.build_text(text).unwrap();
    render(&enc.encode(&sym).unwrap(), scale)
}

fn code128_image(text: &str, scale: usize) -> GrayImage {
    let enc = Code128Encoder::new();
    let sym = enc.build_text(text).unwrap();
    render(&enc.encode(&sym).unwrap(), scale)
}

fn place(canvas: &mut GrayImage, sprite: &GrayImage, ox: usize, oy: usize) -> Point {
    for y in 0..sprite.height() {
        for x in 0..sprite.width() {
            let cx = ox + x;
            let cy = oy + y;
            if cx < canvas.width() && cy < canvas.height() {
                canvas.set(cx, cy, sprite.get(x, y));
            }
        }
    }
    Point::new(
        (ox + sprite.width() / 2) as f32,
        (oy + sprite.height() / 2) as f32,
    )
}

/// A synthetic frame together with the true centres of the codes planted in it.
struct Frame {
    scenario: &'static str,
    image: GrayImage,
    planted: Vec<Point>,
}

fn empty_frame(w: usize, h: usize) -> Frame {
    Frame {
        scenario: "empty",
        image: GrayImage::filled(w, h, 255),
        planted: Vec::new(),
    }
}

fn one_qr_frame(w: usize, h: usize) -> Frame {
    let mut image = GrayImage::filled(w, h, 255);
    let qr = qr_image("ANYD DETECT BENCH", 6);
    let ox = (w / 2).saturating_sub(qr.width() / 2);
    let oy = (h / 2).saturating_sub(qr.height() / 2);
    let c = place(&mut image, &qr, ox, oy);
    Frame {
        scenario: "one-qr",
        image,
        planted: vec![c],
    }
}

fn mixed_frame(w: usize, h: usize) -> Frame {
    let mut image = GrayImage::filled(w, h, 255);
    let qr = qr_image("MIXED-QR", 6);
    let dm = datamatrix_image("MIXED-DM", 6);
    let c128 = code128_image("MIXED128", 2);
    let planted = vec![
        place(&mut image, &qr, w / 12, h / 12),
        place(&mut image, &dm, w * 7 / 12, h / 10),
        place(&mut image, &c128, w / 10, h * 3 / 5),
    ];
    Frame {
        scenario: "mixed",
        image,
        planted,
    }
}

// ----------------------------------------------------------------------------------
// Timing
// ----------------------------------------------------------------------------------

struct Stat {
    median_ms: f64,
    p95_ms: f64,
    fps: f64,
}

/// Time `f` over `warmup + iters` runs, returning median/p95/FPS of the timed runs.
fn measure<F: FnMut() -> usize>(mut f: F, warmup: usize, iters: usize) -> Stat {
    for _ in 0..warmup {
        black_box(f());
    }
    let mut times = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        let r = f();
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        black_box(r);
        times.push(dt);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    let p95_idx = (((times.len() as f64) * 0.95) as usize).min(times.len() - 1);
    let p95 = times[p95_idx];
    Stat {
        median_ms: median,
        p95_ms: p95,
        fps: if median > 0.0 {
            1000.0 / median
        } else {
            f64::INFINITY
        },
    }
}

// ----------------------------------------------------------------------------------
// Accuracy (recall / false-positive rate) over the populated frames
// ----------------------------------------------------------------------------------

#[derive(Default)]
struct Accuracy {
    planted_total: usize,
    planted_hit: usize,
    candidates_total: usize,
    candidates_matched: usize,
}

impl Accuracy {
    fn tally(&mut self, frame: &Frame, opts: &LocateOptions) {
        if frame.planted.is_empty() {
            // Blank / empty frames: every returned candidate is a false positive.
            let cands = locate(&frame.image.as_frame(), opts);
            self.candidates_total += cands.len();
            return;
        }
        let tol = 0.15 * frame.image.width().min(frame.image.height()) as f32;
        let cands = locate(&frame.image.as_frame(), opts);
        for &p in &frame.planted {
            self.planted_total += 1;
            if cands
                .iter()
                .any(|c| c.location.outline.center().distance(p) <= tol)
            {
                self.planted_hit += 1;
            }
        }
        for c in &cands {
            self.candidates_total += 1;
            if frame
                .planted
                .iter()
                .any(|&p| c.location.outline.center().distance(p) <= tol)
            {
                self.candidates_matched += 1;
            }
        }
    }

    fn recall(&self) -> f64 {
        if self.planted_total == 0 {
            return 0.0;
        }
        self.planted_hit as f64 / self.planted_total as f64
    }

    fn false_positive_rate(&self) -> f64 {
        if self.candidates_total == 0 {
            return 0.0;
        }
        (self.candidates_total - self.candidates_matched) as f64 / self.candidates_total as f64
    }
}

// ----------------------------------------------------------------------------------
// Driver
// ----------------------------------------------------------------------------------

fn main() {
    let opts = LocateOptions::default();
    let resolutions = [(640usize, 480usize), (1280, 720), (1920, 1080)];

    println!("anyd frame-detector benchmark (locate vs full decode)");
    println!(
        "downscale={}  tile={}  edge_density={}",
        opts.downscale, opts.tile, opts.edge_density
    );
    println!();
    println!(
        "{:<12} {:<8} {:>11} {:>10} {:>9}",
        "resolution", "scenario", "median ms", "p95 ms", "FPS"
    );
    println!("{}", "-".repeat(54));

    let mut acc = Accuracy::default();
    // Remember one-QR detect medians per resolution for the decode-ratio table.
    let mut qr_detect_median: Vec<(String, f64)> = Vec::new();

    for &(w, h) in &resolutions {
        let res = format!("{w}x{h}");
        let frames = [empty_frame(w, h), one_qr_frame(w, h), mixed_frame(w, h)];
        for frame in &frames {
            let img = frame.image.clone();
            let stat = measure(|| locate(&img.as_frame(), &opts).len(), 12, 120);
            println!(
                "{:<12} {:<8} {:>11.3} {:>10.3} {:>9.1}",
                res, frame.scenario, stat.median_ms, stat.p95_ms, stat.fps
            );
            if frame.scenario == "one-qr" {
                qr_detect_median.push((res.clone(), stat.median_ms));
            }
            acc.tally(frame, &opts);
        }
    }

    // Full-decode comparison on the one-QR frames.
    println!();
    println!("Full QR decode (anyd::codes::qr::scan) on the one-QR frames:");
    println!(
        "{:<12} {:>11} {:>10} {:>9} {:>18}",
        "resolution", "median ms", "p95 ms", "FPS", "detect speedup"
    );
    println!("{}", "-".repeat(64));
    for (i, &(w, h)) in resolutions.iter().enumerate() {
        let frame = one_qr_frame(w, h);
        let img = frame.image.clone();
        let stat = measure(
            || match qr::scan(&img.as_frame()) {
                Ok(_) => 1,
                Err(_) => 0,
            },
            4,
            40,
        );
        let detect_median = qr_detect_median[i].1;
        let speedup = if detect_median > 0.0 {
            stat.median_ms / detect_median
        } else {
            f64::INFINITY
        };
        println!(
            "{:<12} {:>11.3} {:>10.3} {:>9.1} {:>16.1}x",
            format!("{w}x{h}"),
            stat.median_ms,
            stat.p95_ms,
            stat.fps,
            speedup
        );
    }

    println!();
    println!("Accuracy over populated frames (recall) and all frames (false positives):");
    println!(
        "  planted codes: {}   located: {}   recall: {:.1}%",
        acc.planted_total,
        acc.planted_hit,
        acc.recall() * 100.0
    );
    println!(
        "  candidates: {}   matched a planted code: {}   false-positive rate: {:.1}%",
        acc.candidates_total,
        acc.candidates_matched,
        acc.false_positive_rate() * 100.0
    );
}
