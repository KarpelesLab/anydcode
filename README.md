# AnyDCode

[![CI](https://github.com/KarpelesLab/anydcode/actions/workflows/ci.yml/badge.svg)](https://github.com/KarpelesLab/anydcode/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/anyd.svg)](https://crates.io/crates/anyd)
[![docs.rs](https://img.shields.io/docsrs/anyd)](https://docs.rs/anyd)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A from-scratch Rust library for **decoding and encoding 1D and 2D barcodes**, aiming
for the widest possible symbology coverage while preserving every variation of a code.

**▶ [Live WASM demo](https://karpeleslab.github.io/anydcode/)** — generate any
symbology (Apple App Clip Codes included), decode an image, and run live-camera
scanning with a tracked target-lock overlay, all in the browser (the library compiled
to WebAssembly, no wasm-bindgen).

## Goals

- **Lossless round-trip.** A decoded [`Symbol`] carries not just its payload but the
  exact encoding decisions — segmentation, version/size, error-correction level, mask.
  Feeding a decoded symbol straight back into the encoder reproduces the original
  symbol byte-for-byte.
- **Live-video ready.** A cheap per-frame *detection* pass (locate + classify) is
  separated from a heavier *analysis/decode* pass, and previous-frame hints let the
  pipeline skip re-analyzing codes it already knows.
- **Portable input.** The video boundary is a borrowed luminance buffer
  (`GrayFrame`); the caller owns capture and color conversion, so the core has no
  media dependencies.

## Status

The full symbology catalog is **modeled** in the `Symbology` enum, but
implementations land incrementally. Query `Symbology::is_implemented()` at runtime.
Legend: ✅ done · 🚧 in progress · ⬜ planned.

### 2D — matrix

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| QR Code (v1–40, all EC levels & masks) | ✅ | ✅ | ✅¹ |
| Data Matrix (ECC 200, square sizes, ASCII+Base256) | ✅ | ✅ | ✅³ |
| Aztec (compact + full range, all modes) · Aztec Runes | ✅ | ✅ | ✅⁶ |
| Micro QR Code (M1–M4) | ✅ | ✅ | ✅⁶ |
| Rectangular Micro QR (rMQR, all 32 sizes) | ✅ | ✅ | ✅⁶ |
| MaxiCode (modes 2–6) | ✅ | ✅ | ⬜ |
| Han Xin (versions 1–3, Numeric/Text/Byte) | ✅ | ✅ | ⬜ |
| DotCode (GF(113) RS) | ✅ | ✅ | ⬜ |
| Grid Matrix (all 13 versions) | ✅ | ✅ | ⬜ |
| Apple App Clip Code (URL codec + SVG, camera & NFC variants)⁵ | ✅ | ✅ | ✅ |

### 2D — stacked

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| PDF417 (Text/Byte/Numeric, EC 0–8) | ✅ | ✅ | ✅⁴ |
| MicroPDF417 (all 34 size variants) | ✅ | ✅ | ⬜ |
| Code 16K · Code 49 · Codablock F | ✅ | ✅ | ⬜ |
| GS1 DataBar Stacked / Stacked Omni / Expanded Stacked | ✅ | ✅ | ⬜ |

### 1D — linear

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| EAN-13 · EAN-8 · UPC-A · UPC-E | ✅ | ✅ | ✅² |
| UPC/EAN add-ons: EAN-2 · EAN-5 | ✅ | ✅ | ✅² |
| Code 128 · GS1-128 | ✅ | ✅ | ✅² |
| Code 39 · Code 93 · Code 11 | ✅ | ✅ | ✅² |
| ITF · Standard / IATA / Matrix 2 of 5 | ✅ | ✅ | ✅² |
| Codabar · MSI Plessey · Plessey | ✅ | ✅ | ✅² |
| Telepen | ✅ | ✅ | ✅² |
| Pharmacode (1- & 2-track) | ✅ | ✅ | ⬜ |
| GS1 DataBar Omni (RSS-14) · Limited · Expanded | ✅ | ✅ | ⬜ |
| DX Film Edge | ✅ | ✅ | ⬜ |

### Postal (height-modulated)

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| POSTNET · PLANET · Intelligent Mail (IMb) | ✅ | ✅ | ⬜ |
| Royal Mail (RM4SCC) · Dutch KIX | ✅ | ✅ | ⬜ |
| Royal Mail Mailmark (Barcode C & L) | ✅ | ✅ | ⬜ |
| Australia Post 4-State · Japan Post 4-State | ✅ | ✅ | ⬜ |

¹ QR ships a hardened image sampler: a global-Otsu → adaptive (Bradley/Sauvola)
binarization ladder, dual-axis finder detection with a synthesized-third-corner
fallback, perspective recovery via the alignment pattern, and — for curved or
degraded captures — a thin-plate-spline dewarp anchored on every finder, alignment
pattern and timing-pattern crossing, biased module classification, and a
Reed–Solomon-arbitrated brute force over all 32 format configurations when blur
wrecks the format fields. Robust to any-angle rotation, tilt, blur, noise, uneven
lighting and cylinder curvature; clean-render envelope in `tests/harness_image.rs`,
real-capture envelope (bottle and can labels down to ≈4.6 px/module) in
`tests/real_world.rs`.

² 1D detection is the shared **`scan1d`** luminance front-end (edge detection →
run-length → normalized `LinearPattern`), feeding each symbology's decoder. The
end-to-end pixel→symbol path is verified for Code 128, Code 39 and EAN-13 in
`tests/scan1d_pipeline.rs`; it applies to any standard bar/space linear code.
Through `pipeline::scan_1d` the read is rotation-complete: mirrored patterns cover
180°, and any other angle is handled by estimating the crop's dominant texture
orientation and derotating before the rescan (`tests/rotated1d.rs`). DataBar
(finder-pattern based) and Pharmacode need dedicated samplers.

³ Data Matrix ships an image sampler: Otsu binarization → largest-component isolation
→ solid-L finder + timing-edge line fitting → perspective corners → `imgproc` grid
sampling, with symbol size and grid chirality confirmed by Reed–Solomon. Robust to
any-angle rotation, scale, blur/noise and (size-graded) tilt; envelope in
`tests/datamatrix_image.rs`.

⁴ PDF417 (finder-less) is located by its start/stop guard columns → RANSAC edge fit →
affine grid sampling → RS-corrected decode. Handles upright, rotation ≤±10°, scale,
blur and noise; perspective keystoning is out of scope (no interior anchor). Envelope
in `tests/pdf417_image.rs`.

⁶ Aztec, Micro QR and rMQR ship image samplers built on their native fiducials:
Aztec's bullseye (equal-run cross-section scan → the enclosed light annulus floods
cleanly and its corners anchor a core homography → the mode-message ring, read in
image space, gives the exact symbol size); Micro QR's single finder (QR-style ratio
scan → the separator-isolated outer ring's corners anchor the grid → four rotations
× four sizes arbitrated by format/ECC); and rMQR's finder plus its finder-side
format copy, which is read in image space to learn the exact rectangle before
sampling — with the finder-sub centre dot pinned as a far anchor so wide symbols
(up to 139 modules) stay registered. All handle any-angle rotation, scale, blur and
noise; envelopes in `tests/aztec_image.rs` / `tests/microqr_image.rs` /
`tests/rmqr_image.rs`.

⁵ Apple has never published the App Clip Code format; this is an independent,
from-scratch implementation following the public reverse engineering of Apple's
generator ([rs/appclipcode], MIT), byte-verified against `URLCompression.framework`
output. Encode produces the circular five-ring SVG (all 18 Apple color templates,
camera and NFC center glyphs); decode covers both the structural path (ring bits →
URL) and camera detection — a grayscale ring detector (gradient-normal Hough over the
five-ring geometry, affine/tilt refinement, rotation search, RS-arbitrated bit
recovery) wired into `pipeline::scan_2d`. The trained
Huffman tables embed ~1.7 MB, so the module sits behind the default-on `appclip`
cargo feature (`default-features = false` for a lean build). Apple, App Clips and
related marks are trademarks of Apple Inc.

[rs/appclipcode]: https://github.com/rs/appclipcode

> **Naming note.** GS1 DataBar was formerly "RSS": DataBar Omnidirectional = RSS-14,
> DataBar Limited = RSS Limited, DataBar Expanded = RSS Expanded. Codabar is sometimes
> called "Coda"/NW-7. These are aliases for the same `Symbology` variants above.

## Example: QR round-trip

```rust
use anyd::codes::qr::{EcLevel, QrDecoder, QrEncoder};
use anyd::traits::{Decode, Encode};

let encoder = QrEncoder::new();
let symbol = encoder.build_text("HELLO WORLD", EcLevel::Q).unwrap();
let encoding = encoder.encode(&symbol).unwrap();

let decoded = QrDecoder::new().decode(&encoding).unwrap();
assert_eq!(decoded.text().as_deref(), Some("HELLO WORLD"));
// Re-encoding the decoded symbol yields the identical matrix.
assert_eq!(encoder.encode(&decoded).unwrap(), encoding);
```

## Live-video detection & performance

For a camera feed you rarely want to fully decode every frame. The `detect` module
provides a fast, symbology-agnostic **locator** that returns the *position* of every
code (a bounding quad + coarse family guess) in a single downscaled pass, **without
decoding** — the cheap stage of the two-stage `Detect` → `Analyze` pipeline. It is
recall-biased but structurally picky: concentric-finder detection backs matrix
guesses, scene-sized merged blobs, text rows/columns masquerading as barcodes and
non-code ink densities are gated out, overlapping duplicates of one physical code are
suppressed, and a close-up code that defeats the texture pass is still reported from
its finder hits alone:

```rust
use anyd::detect::{locate, LocateOptions};

let candidates = locate(&frame, &LocateOptions::default());
for c in &candidates {
    // c.location — where the code is; c.symbology — family guess.
    // Hand off to the matching sampler/decoder only for new regions.
}
```

Benchmark (`cargo bench --bench detect`, dependency-free, `std::time::Instant`),
locating **all** codes of all families per frame vs. a full QR decode:

| resolution | locate median | locate FPS | full QR decode | detect speedup |
|-----------:|--------------:|-----------:|---------------:|---------------:|
| 640×480    | 0.29 ms | ~3400 | 12.1 ms | ~41× |
| 1280×720   | 0.70 ms | ~1430 | 14.1 ms | ~20× |
| 1920×1080  | 1.53 ms | ~650  | 17.4 ms | ~11× |

Recall 100% and false-positive rate 0% on the benchmark's planted-code frames.
Locating is comfortably real-time at every resolution; decoding then runs only on the
regions the locator hands off (and `Hints` lets already-decoded codes be skipped
across frames).

The demo page shows the intended live wiring: the main thread runs `locate` on a
half-resolution grab ~30×/s, a crop worker decodes located regions off-thread —
routed by family, so linear crops take the fast 1D path (`scan_1d` is
rotation-complete via mirrored patterns and gradient-orientation derotation) — a
second worker sweeps the full frame for self-localizing 2D codes, and decoded locks
are position-tracked across frames so the overlay follows the code between reads.

## Command-line tool (`anyd`)

An optional binary is bundled behind the **`cli`** feature, so the library itself
stays dependency-free. It reads/writes PNG via `oxideav-png` (with default features
off — no codec-registry dependency) and can also emit Unicode (for the terminal) or
SVG (raw XML).

```console
$ cargo build --release --features cli      # builds target/release/anyd

# Encode → PNG / SVG / terminal
$ anyd encode qr "https://example.com" --format png --out qr.png --scale 8
$ anyd encode ean13 5901234123457 --format svg --out barcode.svg
$ anyd encode qr "HELLO" --format unicode          # half-block art in the terminal

# Decode a PNG (tries QR, Micro QR, rMQR, Data Matrix, Aztec, PDF417,
# App Clip Code, and the rotation-complete 1D front-end)
$ anyd decode qr.png
QR Code: https://example.com

$ anyd list                                        # encodable symbology names
```

Options: `--format png|unicode|svg`, `--out FILE`, `--scale N` (px/module),
`--ec L|M|Q|H` (QR/Micro QR) or `--ec 0-8` (PDF417), `--invert` (terminal colours).

## Architecture

```
Symbol  ─── the lossless currency: segments + symbology-specific meta
  │
  ├─ Encode ──▶ Encoding (BitMatrix for 2D, LinearPattern for 1D)
  └─ Decode ◀── Encoding

Image pipeline (pixels → symbol):
  GrayFrame ─▶ [ imgproc: binarize · homography · grid-sample ]  ─▶ BitMatrix ─▶ Decode   (2D)
  GrayFrame ─▶ [ scan1d: edge-detect · run-length · quantize   ]  ─▶ LinearPattern ─▶ Decode (1D)

Live video:  GrayFrame ──▶ Detect ──▶ Candidate ──▶ Analyze ──▶ Symbol
                              ▲                          │
                              └────── Hints ◀────────────┘  (skip known codes)
```

Supporting modules: **`imgproc`** (Otsu/adaptive binarization, connected components,
4-point homography, thin-plate-spline warps, dominant-orientation estimation,
sub-pixel grid sampling — the 2D sampler toolkit), **`scan1d`** (generic 1D luminance
front-end), **`render`** (Encoding → grayscale) and **`transform`**
(rotation/perspective/blur/noise) for the test harness.

Correctness is anchored to independent reference vectors, not self-consistency: the
ISO/IEC 18004 QR example, the ISO/IEC 16022 Data Matrix example, ISO/IEC 15438 +
ZXing vectors for PDF417, BWIPP-rendered patterns for DataBar, and documented
check-digit/pattern references for the 1D codes — plus exhaustive encode→decode→
re-encode identity tests, and full image-pipeline tests (encode → render → transform
→ sample → decode) for QR and the 1D front-end.

## License

MIT © Karpelès Lab Inc.
