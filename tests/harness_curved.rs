//! Tier B: end-to-end *image* harness for **non-planar (curved-surface)** distortions.
//!
//! Real symbols are printed on bottles, cans, crumpled/rippled paper and bulging
//! labels — surfaces the planar warps (rotation/scale/perspective) cannot express. This
//! harness renders representative symbols, subjects them to the four curved transforms
//! added to [`anyd::transform`] — [`cylinder`], [`wave`], [`fold`] and [`bulge`] —
//! and feeds the result back through the **current, flat-plane** image samplers.
//!
//! [`cylinder`]: anyd::transform::cylinder
//! [`wave`]: anyd::transform::wave
//! [`fold`]: anyd::transform::fold
//! [`bulge`]: anyd::transform::bulge
//!
//! ## Purpose
//!
//! The samplers are **not** modified here (that is a separate follow-up). This file
//! (a) exercises the reusable curved transforms, (b) locks in — with *positive*
//! assertions only — the curvature the flat samplers already tolerate so a regression
//! is caught, and (c) documents the empirical point at which each distortion currently
//! breaks each symbology, to scope the robustness work.
//!
//! Only mild, confirmed-passing levels are asserted. Where the flat-plane sampler fails,
//! there is **no** "must fail" assertion (that would break once the samplers improve);
//! instead the breakpoint is recorded in a `BREAKPOINT:` comment next to the case.
//!
//! ## Measured current-sampler curvature envelope
//!
//! Every symbol is rendered at 6 px/module with its standard quiet zone. The values
//! below are the empirical *breakpoints* (largest strength that still decodes → first
//! that fails), measured by the (deleted) sweep that produced this table. Asserted
//! levels sit at or just inside the "last-ok" column.
//!
//! | Symbol (dim)     | cylinder rad (V / H) | wave px (amp)  | fold deg (V) | bulge (+ / −) |
//! |------------------|----------------------|----------------|--------------|---------------|
//! | QR v1 (21)       | 1.5+ / 0.90          | 1.5            | 32.5         | 0.10 / −0.10  |
//! | QR v3 (29)       | 1.10 / 1.00          | 2.5            | 27.5         | 0.05 / −0.05  |
//! | QR v7 (45)       | 0.55 / 0.55          | 1.5            | 22.5         | 0.05 / −0.05  |
//! | Data Matrix (10) | 1.40 / 1.30          | 3.0            | 35.0         | 0.25 / −0.15  |
//! | Data Matrix (16) | 1.00 / 1.00          | 2.5            | 30.0         | 0.15 / −0.10  |
//! | Data Matrix (22) | 0.80 / —             | 2.5            | 25.0         | 0.10 / —      |
//! | Code 128 (1D)    | 0.50 / 1.5+          | 20+            | 5.0 (V)      | 0.05 / −0.05  |
//!
//! Reading of the map:
//!
//! * **Bulge is by far the harshest** — a radial magnification desynchronizes the whole
//!   module grid at once, so even the perspective-corrected 2D samplers give out around
//!   `|strength|` 0.05–0.10 (small Data Matrix tolerates ~0.25). This is the clearest
//!   target for the follow-up.
//! * **Cylinder is the most forgiving** for 2D: gentle, monotone foreshortening leaves
//!   finder/alignment patterns detectable, so QR/DM survive to ~0.8–1.4 rad; tolerance
//!   shrinks as module count grows (QR v7, DM 22).
//! * **Fold** tolerance also shrinks with module count (denser grids amplify the crease
//!   discontinuity): ~22–35° across the 2D symbols.
//! * **1D (Code 128)** is anisotropic: distortions that bend *across* the bars
//!   (cylinder V, fold V) break early (~0.5 rad, ~5°) because bar widths are corrupted,
//!   while distortions *along* the bars (cylinder H, wave, fold H) are shrugged off — no
//!   breakpoint was found within the swept range (cylinder H ≥1.5 rad, wave ≥20 px,
//!   fold H ≥85°).

use anyd::GrayImage;
use anyd::codes::code128::{Code128Decoder, Code128Encoder};
use anyd::codes::datamatrix::{DataMatrixEncoder, scan as dm_scan};
use anyd::codes::qr::{EcLevel, QrEncoder, scan as qr_scan};
use anyd::output::Encoding;
use anyd::render::{render, render_matrix};
use anyd::scan1d::{ScanOptions, scan_lines, try_decode};
use anyd::segment::Segment;
use anyd::traits::Encode;
use anyd::transform::{self, Axis};

/// Pixels per module for every rendered symbol in this harness.
const SCALE: usize = 6;

// ===================================================================================
// Encode / render / decode helpers (one per symbology family)
// ===================================================================================

fn qr_base(segments: Vec<Segment>, level: EcLevel) -> (GrayImage, Vec<u8>, usize) {
    let enc = QrEncoder::new();
    let sym = enc.build(segments, level).expect("qr build");
    let payload = sym.payload_bytes();
    match enc.encode(&sym).expect("qr encode") {
        Encoding::Matrix(m) => (render_matrix(&m, SCALE), payload, m.width()),
        Encoding::Linear(_) => unreachable!("QR encodes to a matrix"),
    }
}

fn dm_base(payload: &[u8]) -> (GrayImage, Vec<u8>, usize) {
    let enc = DataMatrixEncoder::new();
    let sym = enc.build(payload).expect("dm build");
    let bytes = sym.payload_bytes();
    match enc.encode(&sym).expect("dm encode") {
        Encoding::Matrix(m) => (render_matrix(&m, SCALE), bytes, m.width()),
        Encoding::Linear(_) => unreachable!("Data Matrix encodes to a matrix"),
    }
}

fn code128_base(text: &str) -> (GrayImage, String) {
    let enc = Code128Encoder::new();
    let sym = enc.build_text(text).expect("code128 build");
    let encoding = enc.encode(&sym).expect("code128 encode");
    (render(&encoding, SCALE), text.to_string())
}

/// Assert the QR sampler recovers `expected` from `img`.
fn qr_decodes(img: &GrayImage, expected: &[u8], label: &str) {
    match qr_scan(&img.as_frame()) {
        Ok(sym) => assert_eq!(
            sym.payload_bytes(),
            expected,
            "{label}: QR payload mismatch"
        ),
        Err(e) => panic!("{label}: QR sampler failed to decode ({e})"),
    }
}

/// Assert the Data Matrix sampler recovers `expected` from `img`.
fn dm_decodes(img: &GrayImage, expected: &[u8], label: &str) {
    match dm_scan(&img.as_frame()) {
        Ok(sym) => assert_eq!(
            sym.payload_bytes(),
            expected,
            "{label}: DM payload mismatch"
        ),
        Err(e) => panic!("{label}: DM sampler failed to decode ({e})"),
    }
}

/// Assert the 1D pipeline recovers `expected` text from `img` via Code 128.
fn code128_decodes(img: &GrayImage, expected: &str, label: &str) {
    let frame = img.as_frame();
    let candidates = scan_lines(&frame, &ScanOptions::default());
    let dec = Code128Decoder::new();
    for cand in &candidates {
        if let Some(sym) = try_decode(cand, &dec)
            && sym.text().unwrap_or_default() == expected
        {
            return;
        }
    }
    panic!("{label}: Code 128 sampler failed to decode");
}

/// The QR symbols exercised: a low version (v1), a mid version (v3) and a HIGH version
/// (v7, dim 45) that carries several alignment patterns. Returns (image, payload, dim).
fn qr_symbols() -> Vec<(GrayImage, Vec<u8>, usize)> {
    vec![
        qr_base(vec![Segment::byte(b"HI".to_vec())], EcLevel::M), // v1 (21)
        qr_base(
            vec![Segment::byte(b"https://example.com/x?q=hello".to_vec())],
            EcLevel::Q,
        ), // v3 (29)
        qr_base(vec![Segment::byte(vec![b'B'; 120])], EcLevel::M), // v7 (45)
    ]
}

/// The Data Matrix symbols exercised: a small (dim 10), mid (16) and larger (22) square.
fn dm_symbols() -> Vec<(GrayImage, Vec<u8>, usize)> {
    vec![
        dm_base(b"AB"),                            // dim 10
        dm_base(b"Data Matrix"),                   // dim 16
        dm_base(b"Data Matrix 2026 test payload"), // dim 22
    ]
}

// ===================================================================================
// Cylinder (bottle / can): foreshortening toward both edges.
// ===================================================================================

#[test]
fn cylinder_qr() {
    for (base, expected, dim) in qr_symbols() {
        // Gentle curve, both cylinder-axis orientations, every version.
        for &axis in &[Axis::Vertical, Axis::Horizontal] {
            let img = transform::cylinder(&base, 0.4, axis);
            qr_decodes(&img, &expected, &format!("qr dim{dim} cyl0.4 {axis:?}"));
        }
        // Stronger curve holds on the lower versions (finder/alignment stay resolvable).
        if dim <= 29 {
            let c = if dim <= 21 { 0.8 } else { 1.0 };
            let img = transform::cylinder(&base, c, Axis::Vertical);
            qr_decodes(&img, &expected, &format!("qr dim{dim} cyl{c} V"));
        }
        // BREAKPOINT: QR v1 ~1.5 (V)/0.90 (H) rad, v3 ~1.10/1.00, v7 ~0.55/0.55.
    }
}

#[test]
fn cylinder_datamatrix() {
    for (base, expected, dim) in dm_symbols() {
        for &axis in &[Axis::Vertical, Axis::Horizontal] {
            let img = transform::cylinder(&base, 0.5, axis);
            dm_decodes(&img, &expected, &format!("dm dim{dim} cyl0.5 {axis:?}"));
        }
        if dim <= 16 {
            let img = transform::cylinder(&base, 1.0, Axis::Vertical);
            dm_decodes(&img, &expected, &format!("dm dim{dim} cyl1.0 V"));
        }
        // BREAKPOINT: DM 10 ~1.40/1.30 rad, DM 16 ~1.00/1.00, DM 22 ~0.80.
    }
}

#[test]
fn cylinder_code128() {
    let (base, expected) = code128_base("ABC-1234");
    // Bending ACROSS the bars (vertical cylinder axis) corrupts bar widths early.
    let across = transform::cylinder(&base, 0.4, Axis::Vertical);
    code128_decodes(&across, &expected, "c128 cyl0.4 V");
    // BREAKPOINT: cylinder V ~0.50 rad.
    // Bending ALONG the bars (horizontal axis) is shrugged off; no failure was found.
    let along = transform::cylinder(&base, 1.0, Axis::Horizontal);
    code128_decodes(&along, &expected, "c128 cyl1.0 H");
    // BREAKPOINT: cylinder H — none within sweep (≥1.5 rad still decodes).
}

// ===================================================================================
// Wave (rippled paper): sinusoidal displacement.
// ===================================================================================

#[test]
fn wave_qr() {
    // Wavelength is scaled per version so roughly one ripple spans the symbol.
    for ((base, expected, dim), wl) in qr_symbols().into_iter().zip([60.0f32, 90.0, 140.0]) {
        for &axis in &[Axis::Horizontal, Axis::Vertical] {
            let img = transform::wave(&base, 1.0, wl, axis);
            qr_decodes(&img, &expected, &format!("qr dim{dim} wave1.0 {axis:?}"));
        }
        // A touch more amplitude still holds on the mid version.
        if dim == 29 {
            let img = transform::wave(&base, 2.0, wl, Axis::Horizontal);
            qr_decodes(&img, &expected, &format!("qr dim{dim} wave2.0 H"));
        }
        // BREAKPOINT (peak amplitude px): QR v1 ~1.5, v3 ~2.5, v7 ~1.5.
    }
}

#[test]
fn wave_datamatrix() {
    for ((base, expected, dim), wl) in dm_symbols().into_iter().zip([40.0f32, 60.0, 80.0]) {
        for &axis in &[Axis::Horizontal, Axis::Vertical] {
            let img = transform::wave(&base, 1.5, wl, axis);
            dm_decodes(&img, &expected, &format!("dm dim{dim} wave1.5 {axis:?}"));
        }
        if dim <= 16 {
            let img = transform::wave(&base, 2.0, wl, Axis::Horizontal);
            dm_decodes(&img, &expected, &format!("dm dim{dim} wave2.0 H"));
        }
        // BREAKPOINT (peak amplitude px): DM 10 ~3.0, DM 16 ~2.5, DM 22 ~2.5.
    }
}

#[test]
fn wave_code128() {
    let (base, expected) = code128_base("ABC-1234");
    // 1D reading tolerates large ripples on either axis (bar edges stay separable).
    for &axis in &[Axis::Horizontal, Axis::Vertical] {
        let img = transform::wave(&base, 5.0, 60.0, axis);
        code128_decodes(&img, &expected, &format!("c128 wave5.0 {axis:?}"));
    }
    // BREAKPOINT: none within sweep (≥20 px amplitude still decodes on both axes).
}

// ===================================================================================
// Fold (folded paper): a crease with one half foreshortened in depth.
// ===================================================================================

#[test]
fn fold_qr() {
    for (base, expected, dim) in qr_symbols() {
        // Mild crease through the middle, both crease orientations.
        for &axis in &[Axis::Vertical, Axis::Horizontal] {
            let img = transform::fold(&base, 0.5, 15.0, axis);
            qr_decodes(&img, &expected, &format!("qr dim{dim} fold15 {axis:?}"));
        }
        // Steeper crease still holds, tolerance shrinking with module count.
        let deg = if dim <= 21 {
            30.0
        } else if dim <= 29 {
            25.0
        } else {
            20.0
        };
        let img = transform::fold(&base, 0.5, deg, Axis::Vertical);
        qr_decodes(&img, &expected, &format!("qr dim{dim} fold{deg} V"));
        // BREAKPOINT (dihedral deg): QR v1 ~32.5, v3 ~27.5, v7 ~22.5.
    }
}

#[test]
fn fold_datamatrix() {
    for (base, expected, dim) in dm_symbols() {
        for &axis in &[Axis::Vertical, Axis::Horizontal] {
            let img = transform::fold(&base, 0.5, 20.0, axis);
            dm_decodes(&img, &expected, &format!("dm dim{dim} fold20 {axis:?}"));
        }
        if dim <= 16 {
            let deg = if dim <= 10 { 30.0 } else { 28.0 };
            let img = transform::fold(&base, 0.5, deg, Axis::Vertical);
            dm_decodes(&img, &expected, &format!("dm dim{dim} fold{deg} V"));
        }
        // BREAKPOINT (dihedral deg): DM 10 ~35.0, DM 16 ~30.0, DM 22 ~25.0.
    }
}

#[test]
fn fold_code128() {
    let (base, expected) = code128_base("ABC-1234");
    // Crease ACROSS the bars (vertical crease) compresses bar widths → breaks early.
    let across = transform::fold(&base, 0.5, 5.0, Axis::Vertical);
    code128_decodes(&across, &expected, "c128 fold5 V");
    // BREAKPOINT: fold V ~5.0 deg.
    // Crease ALONG the bars (horizontal crease) leaves widths intact.
    let along = transform::fold(&base, 0.5, 45.0, Axis::Horizontal);
    code128_decodes(&along, &expected, "c128 fold45 H");
    // BREAKPOINT: fold H — none within sweep (≥85 deg still decodes).
}

// ===================================================================================
// Bulge / pinch (rounded bump / dimple): radial magnification.
// ===================================================================================

#[test]
fn bulge_qr() {
    for (base, expected, dim) in qr_symbols() {
        // Bulge is the harshest distortion for the flat samplers; only a slight radial
        // magnification/pinch is tolerated. v1 holds a touch more than the denser grids.
        let s = if dim <= 21 { 0.1 } else { 0.05 };
        let bulged = transform::bulge(&base, s, (0.5, 0.5));
        qr_decodes(&bulged, &expected, &format!("qr dim{dim} bulge+{s}"));
        let pinched = transform::bulge(&base, -s, (0.5, 0.5));
        qr_decodes(&pinched, &expected, &format!("qr dim{dim} bulge-{s}"));
        // BREAKPOINT (|strength|): QR v1 ~0.10, v3 ~0.05, v7 ~0.05.
    }
}

#[test]
fn bulge_datamatrix() {
    for (base, expected, dim) in dm_symbols() {
        // Small Data Matrix tolerates a surprisingly strong bulge; it falls off with size.
        let (pos, neg) = if dim <= 10 {
            (0.2, -0.15)
        } else if dim <= 16 {
            (0.15, -0.1)
        } else {
            (0.1, -0.05)
        };
        let bulged = transform::bulge(&base, pos, (0.5, 0.5));
        dm_decodes(&bulged, &expected, &format!("dm dim{dim} bulge+{pos}"));
        let pinched = transform::bulge(&base, neg, (0.5, 0.5));
        dm_decodes(&pinched, &expected, &format!("dm dim{dim} bulge{neg}"));
        // BREAKPOINT (+ / −): DM 10 ~0.25/−0.15, DM 16 ~0.15/−0.10, DM 22 ~0.10/−0.05.
    }
}

#[test]
fn bulge_code128() {
    let (base, expected) = code128_base("ABC-1234");
    let bulged = transform::bulge(&base, 0.05, (0.5, 0.5));
    code128_decodes(&bulged, &expected, "c128 bulge+0.05");
    let pinched = transform::bulge(&base, -0.05, (0.5, 0.5));
    code128_decodes(&pinched, &expected, "c128 bulge-0.05");
    // BREAKPOINT (|strength|): Code 128 ~0.05.
}
