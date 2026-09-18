//! Deterministic no-panic fuzzing of the self-localizing image samplers (Aztec,
//! Micro QR, rMQR, PDF417, MicroPDF417).
//!
//! `pipeline::scan_2d` runs every one of them on every camera frame and on arbitrary
//! crops of it, so on *any* pixels — degenerate sizes, flat or saturated frames, noise,
//! finder-provoking textures, symbols clipped by the crop edge — each must return
//! (`Ok` or `Err`/`None`), never panic, stay within a bounded amount of work, and never
//! report a payload other than the one that was planted.
#![cfg(all(
    feature = "encode",
    feature = "scan",
    feature = "aztec",
    feature = "microqr",
    feature = "rmqr",
    feature = "pdf417",
    feature = "qr",
    feature = "datamatrix"
))]

use anyd::codes::aztec::AztecEncoder;
use anyd::codes::datamatrix::DataMatrixEncoder;
use anyd::codes::microqr::{MicroEcLevel, MicroQrEncoder};
use anyd::codes::pdf417::{EcLevel, MicroPdf417Encoder, Pdf417Encoder};
use anyd::codes::qr::{EcLevel as QrEcLevel, QrEncoder};
use anyd::codes::rmqr::{RmqrEcLevel, RmqrEncoder, SizeStrategy};
use anyd::render::render;
use anyd::segment::Segment;
use anyd::symbol::Symbol;
use anyd::symbology::Symbology;
use anyd::traits::Encode;
use anyd::{GrayFrame, GrayImage};

/// xorshift64: a fixed-seed generator keeps every run identical.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 16) % n as u64) as usize
    }
}

/// What a frame is known to contain: nothing readable, or one symbol.
#[derive(Clone)]
struct Planted {
    symbology: Symbology,
    payload: Vec<u8>,
}

/// Run all five samplers on `frame`. Anything they report must be the planted symbol.
fn probe(frame: &GrayFrame<'_>, planted: Option<&Planted>, label: &str) {
    let results: [(&str, Option<Symbol>); 5] = [
        ("aztec", anyd::codes::aztec::scan(frame).ok()),
        ("microqr", anyd::codes::microqr::scan(frame).ok()),
        ("rmqr", anyd::codes::rmqr::scan(frame).ok()),
        ("pdf417", anyd::codes::pdf417::scan(frame)),
        ("micropdf417", anyd::codes::pdf417::scan_micro(frame).ok()),
    ];
    for (name, result) in results {
        let Some(sym) = result else { continue };
        let ok = planted
            .is_some_and(|p| p.symbology == sym.symbology && p.payload == sym.payload_bytes());
        assert!(
            ok,
            "{label}: {name} sampler reported {} {:?} from a {}x{} frame planted with {:?}",
            sym.symbology,
            sym.text(),
            frame.width(),
            frame.height(),
            planted.map(|p| (
                p.symbology,
                String::from_utf8_lossy(&p.payload).into_owned()
            )),
        );
    }
}

fn probe_raw(data: &[u8], w: usize, h: usize, label: &str) {
    let frame = GrayFrame::new(data, w, h).expect("valid frame");
    probe(&frame, None, label);
}

#[test]
fn tiny_frames_never_panic() {
    let mut rng = Rng(0x5EED_0001);
    let mut sizes: Vec<(usize, usize)> = Vec::new();
    for h in 1..=9 {
        for w in 1..=9 {
            sizes.push((w, h));
        }
    }
    for n in [10, 16, 17, 39, 40, 41, 64, 100, 257] {
        sizes.push((1, n));
        sizes.push((n, 1));
        sizes.push((2, n));
        sizes.push((n, 2));
        sizes.push((n, 8));
        sizes.push((n, 9));
        sizes.push((n, 10));
    }
    for (w, h) in sizes {
        for fill in [0u8, 128, 255] {
            probe_raw(&vec![fill; w * h], w, h, "tiny flat");
        }
        let noise: Vec<u8> = (0..w * h).map(|_| rng.next() as u8).collect();
        probe_raw(&noise, w, h, "tiny noise");
        let bw: Vec<u8> = (0..w * h)
            .map(|_| if rng.next() & 0x100 == 0 { 0 } else { 255 })
            .collect();
        probe_raw(&bw, w, h, "tiny bw noise");
        let checker: Vec<u8> = (0..w * h)
            .map(|i| {
                if (i % w + i / w).is_multiple_of(2) {
                    0
                } else {
                    255
                }
            })
            .collect();
        probe_raw(&checker, w, h, "tiny checker");
    }
}

#[test]
fn strided_frames_never_panic() {
    // A view into a wider buffer whose padding bytes are garbage, ending exactly at the
    // last row's last pixel.
    let (w, h, stride) = (37usize, 23usize, 50usize);
    let mut rng = Rng(0x5EED_0002);
    let data: Vec<u8> = (0..stride * (h - 1) + w)
        .map(|_| rng.next() as u8)
        .collect();
    let frame = GrayFrame::with_stride(&data, w, h, stride).expect("valid strided frame");
    probe(&frame, None, "strided noise");
}

#[test]
fn flat_and_noise_frames() {
    let mut rng = Rng(0x5EED_0003);
    for (w, h) in [(64usize, 48usize), (160, 120), (241, 97), (97, 241)] {
        for fill in [0u8, 1, 127, 254, 255] {
            probe_raw(&vec![fill; w * h], w, h, "flat");
        }
        // One dark / one light pixel on a saturated field: a degenerate histogram.
        let mut one = vec![255u8; w * h];
        one[w * h / 2] = 0;
        probe_raw(&one, w, h, "single dark pixel");
        let mut one = vec![0u8; w * h];
        one[w * h / 2] = 255;
        probe_raw(&one, w, h, "single light pixel");

        let gray: Vec<u8> = (0..w * h).map(|_| rng.next() as u8).collect();
        probe_raw(&gray, w, h, "gray noise");
        let bw: Vec<u8> = (0..w * h)
            .map(|_| if rng.next() & 0x100 == 0 { 0 } else { 255 })
            .collect();
        probe_raw(&bw, w, h, "bw noise");
        // Blocky noise: random cells a few pixels wide look like modules.
        for cell in [2usize, 3, 5] {
            let cw = w / cell + 1;
            let cells: Vec<bool> = (0..cw * (h / cell + 1))
                .map(|_| rng.next() & 0x100 == 0)
                .collect();
            let blocky: Vec<u8> = (0..w * h)
                .map(|i| {
                    if cells[(i / w / cell) * cw + (i % w) / cell] {
                        0
                    } else {
                        255
                    }
                })
                .collect();
            probe_raw(&blocky, w, h, "blocky noise");
        }
        let gradient: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8).collect();
        probe_raw(&gradient, w, h, "gradient");
    }
}

/// Periodic textures: every row is wall-to-wall false finder / bullseye / guard hits.
fn textures(w: usize, h: usize, period: usize) -> Vec<(&'static str, Vec<u8>)> {
    let ink = |dark: bool| if dark { 0u8 } else { 255 };
    let p = period;
    vec![
        (
            "vertical stripes",
            (0..w * h)
                .map(|i| ink(((i % w) / p).is_multiple_of(2)))
                .collect(),
        ),
        (
            "horizontal stripes",
            (0..w * h)
                .map(|i| ink(((i / w) / p).is_multiple_of(2)))
                .collect(),
        ),
        (
            "checkerboard",
            (0..w * h)
                .map(|i| ink(((i % w) / p + (i / w) / p).is_multiple_of(2)))
                .collect(),
        ),
        (
            "concentric squares",
            (0..w * h)
                .map(|i| {
                    let dx = (i % w).abs_diff(w / 2);
                    let dy = (i / w).abs_diff(h / 2);
                    ink((dx.max(dy) / p).is_multiple_of(2))
                })
                .collect(),
        ),
        (
            // 1:1:3:1:1 in both axes, tiled: a QR-family finder at every tile.
            "finder tiles",
            (0..w * h)
                .map(|i| {
                    let on = |v: usize| matches!(v / p % 9, 0 | 2 | 3 | 4 | 6);
                    ink(on(i % w) && on(i / w))
                })
                .collect(),
        ),
    ]
}

#[test]
fn periodic_textures() {
    for (w, h) in [(96usize, 72usize), (150, 110)] {
        for period in [1usize, 2, 3, 5, 8] {
            for (name, data) in textures(w, h, period) {
                probe_raw(&data, w, h, name);
            }
        }
    }
}

/// A texture that is one false hit per few pixels must not cost quadratic time: these
/// frames took seconds per sampler (minutes at camera resolution) when every hit was
/// matched against every cluster found so far.
#[test]
fn dense_false_hits_stay_bounded() {
    let (w, h) = (400usize, 300usize);
    for period in [1usize, 2, 3] {
        for (name, data) in textures(w, h, period) {
            let frame = GrayFrame::new(&data, w, h).expect("valid frame");
            let t0 = std::time::Instant::now();
            probe(&frame, None, name);
            let took = t0.elapsed();
            assert!(
                took.as_secs_f64() < 4.0,
                "{name} period {period}: samplers took {took:?} on a {w}x{h} frame"
            );
        }
    }
}

/// One rendered symbol of each kind the samplers read, plus QR and Data Matrix — which
/// none of them may report as anything.
fn symbols() -> Vec<(GrayImage, Option<Planted>)> {
    let mut out = Vec::new();
    let mut plant = |enc: &dyn Encode, sym: Symbol, scale: usize, readable: bool| {
        // A light margin around the render: Aztec carries no quiet zone of its own.
        let bare = render(&enc.encode(&sym).expect("encode"), scale);
        let margin = 4 * scale;
        let mut img = GrayImage::filled(bare.width() + 2 * margin, bare.height() + 2 * margin, 255);
        for y in 0..bare.height() {
            for x in 0..bare.width() {
                img.set(x + margin, y + margin, bare.get(x, y));
            }
        }
        let planted = readable.then(|| Planted {
            symbology: sym.symbology,
            payload: sym.payload_bytes(),
        });
        out.push((img, planted));
    };

    let aztec = AztecEncoder::new();
    plant(&aztec, aztec.build_text("AZTEC FUZZ 123").unwrap(), 4, true);
    plant(
        &aztec,
        aztec
            .build_text("A full-range Aztec symbol needs a rather longer payload than this one? No: this one.")
            .unwrap(),
        3,
        true,
    );
    plant(&aztec, aztec.build_rune(200), 4, true);
    let micro = MicroQrEncoder::new();
    plant(
        &micro,
        micro
            .build(vec![Segment::byte(b"MQR".to_vec())], MicroEcLevel::M)
            .unwrap(),
        4,
        true,
    );
    plant(
        &micro,
        micro
            .build(vec![Segment::byte(b"12345".to_vec())], MicroEcLevel::L)
            .unwrap(),
        4,
        true,
    );
    let rmqr = RmqrEncoder::new();
    plant(
        &rmqr,
        rmqr.build_text_with("RMQR FUZZ", RmqrEcLevel::M, SizeStrategy::Balanced)
            .unwrap(),
        4,
        true,
    );
    plant(
        &rmqr,
        rmqr.build_text_with(
            "WIDE AND FLAT RMQR SYMBOL",
            RmqrEcLevel::M,
            SizeStrategy::MinHeight,
        )
        .unwrap(),
        3,
        true,
    );
    let pdf = Pdf417Encoder::new();
    plant(
        &pdf,
        pdf.build_text("PDF417 FUZZ", EcLevel::new(2).unwrap())
            .unwrap(),
        4,
        true,
    );
    let mpdf = MicroPdf417Encoder::new();
    plant(&mpdf, mpdf.build_text("MICROPDF").unwrap(), 3, true);

    let qr = QrEncoder::new();
    plant(
        &qr,
        qr.build_text("QR IS NOT YOURS", QrEcLevel::M).unwrap(),
        3,
        false,
    );
    let dm = DataMatrixEncoder::new();
    plant(&dm, dm.build_text("NOR IS DATA MATRIX").unwrap(), 4, false);
    out
}

/// Copy the `w`×`h` window of `img` at `(x0, y0)` (may hang over any edge; outside is
/// light) into a fresh buffer.
fn crop(img: &GrayImage, x0: i64, y0: i64, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![255u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x0 + x as i64, y0 + y as i64);
            if sx >= 0 && sy >= 0 && (sx as usize) < img.width() && (sy as usize) < img.height() {
                out[y * w + x] = img.get(sx as usize, sy as usize);
            }
        }
    }
    out
}

#[test]
fn whole_symbols_read_back() {
    // The harness itself must be sound: every planted symbol is readable uncropped, by
    // its own sampler, as exactly what was planted.
    for (img, planted) in symbols() {
        let Some(p) = planted else { continue };
        let frame = img.as_frame();
        let got = match p.symbology {
            Symbology::Aztec | Symbology::AztecRunes => anyd::codes::aztec::scan(&frame).ok(),
            Symbology::MicroQrCode => anyd::codes::microqr::scan(&frame).ok(),
            Symbology::RectMicroQrCode => anyd::codes::rmqr::scan(&frame).ok(),
            Symbology::Pdf417 => anyd::codes::pdf417::scan(&frame),
            Symbology::MicroPdf417 => anyd::codes::pdf417::scan_micro(&frame).ok(),
            other => panic!("unexpected planted symbology {other}"),
        };
        let got = got.unwrap_or_else(|| panic!("planted {} did not scan", p.symbology));
        assert_eq!(got.symbology, p.symbology);
        assert_eq!(got.payload_bytes(), p.payload);
    }
}

/// Slide crop windows over every `n`-th planted symbol starting at `first` (the work is
/// split over several tests so they run in parallel).
fn clip_fuzz(first: usize, n: usize) {
    let mut rng = Rng(0x5EED_0004 + first as u64);
    for (img, planted) in symbols().into_iter().skip(first).step_by(n) {
        let (iw, ih) = (img.width() as i64, img.height() as i64);
        // Windows of the symbol's own size slid so that every edge (and corner) clips
        // it by a little, by half and almost entirely.
        let steps = [-0.9f64, -0.5, -0.12, 0.0, 0.12, 0.5, 0.9];
        for &fy in &steps {
            for &fx in &steps {
                let (x0, y0) = ((fx * iw as f64) as i64, (fy * ih as f64) as i64);
                let mut data = crop(&img, x0, y0, iw as usize, ih as usize);
                let frame = GrayFrame::new(&data, iw as usize, ih as usize).unwrap();
                probe(&frame, planted.as_ref(), "clipped");
                // The same view with a few pixels of random corruption.
                for _ in 0..1 + rng.below(12) {
                    let i = rng.below(data.len());
                    data[i] = rng.next() as u8;
                }
                let frame = GrayFrame::new(&data, iw as usize, ih as usize).unwrap();
                probe(&frame, planted.as_ref(), "clipped + corrupted");
            }
        }
        // Narrow windows: strips and small squares cut from inside the symbol.
        for _ in 0..12 {
            let w = 1 + rng.below(iw as usize);
            let h = 1 + rng.below(ih as usize);
            let x0 = rng.below(iw as usize) as i64 - (w / 2) as i64;
            let y0 = rng.below(ih as usize) as i64 - (h / 2) as i64;
            let data = crop(&img, x0, y0, w, h);
            let frame = GrayFrame::new(&data, w, h).unwrap();
            probe(&frame, planted.as_ref(), "window");
        }
    }
}

#[test]
fn clipped_symbols_a() {
    clip_fuzz(0, 4);
}

#[test]
fn clipped_symbols_b() {
    clip_fuzz(1, 4);
}

#[test]
fn clipped_symbols_c() {
    clip_fuzz(2, 4);
}

#[test]
fn clipped_symbols_d() {
    clip_fuzz(3, 4);
}

/// An Aztec Rune's ring code accepts about one random ring in 200, so the rest of the
/// 11×11 grid has to vouch for it: a 10×9 one-pixel checkerboard — smaller than a rune —
/// used to read as rune 46.
#[test]
fn checkerboard_is_not_an_aztec_rune() {
    for (w, h) in [(10usize, 9usize), (9, 10), (12, 12), (30, 30)] {
        let data: Vec<u8> = (0..w * h)
            .map(|i| {
                if (i % w + i / w).is_multiple_of(2) {
                    0
                } else {
                    255
                }
            })
            .collect();
        let frame = GrayFrame::new(&data, w, h).unwrap();
        assert!(
            anyd::codes::aztec::scan(&frame).is_err(),
            "{w}x{h} checkerboard read as an Aztec symbol"
        );
    }
}
