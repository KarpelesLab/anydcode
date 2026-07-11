# AnyDCode

[![CI](https://github.com/KarpelesLab/anydcode/actions/workflows/ci.yml/badge.svg)](https://github.com/KarpelesLab/anydcode/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/anyd.svg)](https://crates.io/crates/anyd)
[![docs.rs](https://img.shields.io/docsrs/anyd)](https://docs.rs/anyd)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A from-scratch Rust library for **decoding and encoding 1D and 2D barcodes**, aiming
for the widest possible symbology coverage while preserving every variation of a code.

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
| Aztec (compact + full range, all modes) · Aztec Runes | ✅ | ✅ | ⬜ |
| Micro QR Code (M1–M4) | ✅ | ✅ | ⬜ |
| Rectangular Micro QR (rMQR, all 32 sizes) | ✅ | ✅ | ⬜ |
| MaxiCode (modes 2–6) | ✅ | ✅ | ⬜ |
| Han Xin (versions 1–3, Numeric/Text/Byte) | ✅ | ✅ | ⬜ |
| Grid Matrix · DotCode | ⬜ | ⬜ | ⬜ |

### 2D — stacked

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| PDF417 (Text/Byte/Numeric, EC 0–8) | ✅ | ✅ | ✅⁴ |
| MicroPDF417 | ⬜ | ⬜ | ⬜ |
| Code 16K · Code 49 · Codablock F | ⬜ | ⬜ | ⬜ |
| GS1 DataBar Stacked / Stacked Omni / Expanded Stacked | ⬜ | ⬜ | ⬜ |

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
| Mailmark · Australia Post · Japan Post | ⬜ | ⬜ | ⬜ |

¹ QR ships a full image sampler: Otsu binarization → finder-pattern detection →
perspective recovery (via the bottom-right alignment pattern) → sub-pixel grid
sampling → decode. Robust to any-angle rotation, moderate tilt, blur and noise
(envelope documented in `tests/harness_image.rs`).

² 1D detection is the shared **`scan1d`** luminance front-end (edge detection →
run-length → normalized `LinearPattern`), feeding each symbology's decoder. The
end-to-end pixel→symbol path is verified for Code 128, Code 39 and EAN-13 in
`tests/scan1d_pipeline.rs`; it applies to any standard bar/space linear code.
DataBar (finder-pattern based) and Pharmacode need dedicated samplers.

³ Data Matrix ships an image sampler: Otsu binarization → largest-component isolation
→ solid-L finder + timing-edge line fitting → perspective corners → `imgproc` grid
sampling, with symbol size and grid chirality confirmed by Reed–Solomon. Robust to
any-angle rotation, scale, blur/noise and (size-graded) tilt; envelope in
`tests/datamatrix_image.rs`.

⁴ PDF417 (finder-less) is located by its start/stop guard columns → RANSAC edge fit →
affine grid sampling → RS-corrected decode. Handles upright, rotation ≤±10°, scale,
blur and noise; perspective keystoning is out of scope (no interior anchor). Envelope
in `tests/pdf417_image.rs`.

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
decoding** — the cheap stage of the two-stage `Detect` → `Analyze` pipeline:

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

| resolution | locate median | locate FPS | full-decode FPS | detect speedup |
|-----------:|--------------:|-----------:|----------------:|---------------:|
| 640×480    | 0.30 ms | ~3350 | ~1725 | 1.9× |
| 1280×720   | 0.83 ms | ~1200 | ~635  | 1.9× |
| 1920×1080  | 1.85 ms | ~545  | ~310  | 1.8× |

Recall 100% and false-positive rate 0% on the benchmark's planted-code frames.
Locating is comfortably real-time at every resolution; decoding then runs only on the
regions the locator hands off (and `Hints` lets already-decoded codes be skipped
across frames).

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

# Decode a PNG (tries QR, Data Matrix, PDF417, and the 1D front-end)
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
4-point homography, sub-pixel grid sampling — the 2D sampler toolkit), **`scan1d`**
(generic 1D luminance front-end), **`render`** (Encoding → grayscale) and
**`transform`** (rotation/perspective/blur/noise) for the test harness.

Correctness is anchored to independent reference vectors, not self-consistency: the
ISO/IEC 18004 QR example, the ISO/IEC 16022 Data Matrix example, ISO/IEC 15438 +
ZXing vectors for PDF417, BWIPP-rendered patterns for DataBar, and documented
check-digit/pattern references for the 1D codes — plus exhaustive encode→decode→
re-encode identity tests, and full image-pipeline tests (encode → render → transform
→ sample → decode) for QR and the 1D front-end.

## License

MIT © Karpelès Lab Inc.
