# AnyDCode

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
| QR Code (v1–40, all EC levels & masks) | ✅ | ✅¹ | ⬜ |
| Micro QR Code | ⬜ | ⬜ | ⬜ |
| Rectangular Micro QR (rMQR) | ⬜ | ⬜ | ⬜ |
| Aztec · Aztec Runes | ⬜ | ⬜ | ⬜ |
| Data Matrix (ECC 200) | ⬜ | ⬜ | ⬜ |
| MaxiCode | ⬜ | ⬜ | ⬜ |
| Han Xin · Grid Matrix · DotCode | ⬜ | ⬜ | ⬜ |

### 2D — stacked

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| PDF417 · MicroPDF417 | ⬜ | ⬜ | ⬜ |
| Code 16K · Code 49 · Codablock F | ⬜ | ⬜ | ⬜ |
| GS1 DataBar Stacked / Stacked Omni / Expanded Stacked | ⬜ | ⬜ | ⬜ |

### 1D — linear

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| EAN-13 · EAN-8 · UPC-A · UPC-E | ⬜ | ⬜ | ⬜ |
| UPC/EAN add-ons: EAN-2 · EAN-5 | ⬜ | ⬜ | ⬜ |
| Code 128 · GS1-128 | ⬜ | ⬜ | ⬜ |
| Code 39 · Code 93 · Code 11 | ⬜ | ⬜ | ⬜ |
| ITF · Standard / IATA / Matrix 2 of 5 | ⬜ | ⬜ | ⬜ |
| Codabar · MSI Plessey · Plessey | ⬜ | ⬜ | ⬜ |
| Telepen · Pharmacode (1- & 2-track) · DX Film Edge | ⬜ | ⬜ | ⬜ |
| GS1 DataBar Omni (RSS-14) · Limited · Expanded | ⬜ | ⬜ | ⬜ |

### Postal (height-modulated)

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| POSTNET · PLANET · Intelligent Mail | ⬜ | ⬜ | ⬜ |
| Royal Mail (RM4SCC) · Mailmark · Dutch KIX | ⬜ | ⬜ | ⬜ |
| Australia Post · Japan Post | ⬜ | ⬜ | ⬜ |

¹ QR decode is currently **structural** — it consumes an already-sampled module
grid. Image sampling / detection front-ends are the next milestone.

> **Naming note.** GS1 DataBar was formerly "RSS": DataBar Omnidirectional = RSS-14,
> DataBar Limited = RSS Limited, DataBar Expanded = RSS Expanded. Codabar is sometimes
> called "Coda"/NW-7. These are aliases for the same `Symbology` variants above.

## Example: QR round-trip

```rust
use anydcode::codes::qr::{EcLevel, QrDecoder, QrEncoder};
use anydcode::traits::{Decode, Encode};

let encoder = QrEncoder::new();
let symbol = encoder.build_text("HELLO WORLD", EcLevel::Q).unwrap();
let encoding = encoder.encode(&symbol).unwrap();

let decoded = QrDecoder::new().decode(&encoding).unwrap();
assert_eq!(decoded.text().as_deref(), Some("HELLO WORLD"));
// Re-encoding the decoded symbol yields the identical matrix.
assert_eq!(encoder.encode(&decoded).unwrap(), encoding);
```

## Architecture

```
Symbol  ─── the lossless currency: segments + symbology-specific meta
  │
  ├─ Encode ──▶ Encoding (BitMatrix for 2D, LinearPattern for 1D)
  └─ Decode ◀── Encoding

Live video:  GrayFrame ──▶ Detect ──▶ Candidate ──▶ Analyze ──▶ Symbol
                              ▲                          │
                              └────── Hints ◀────────────┘  (skip known codes)
```

Correctness for QR is anchored to the ISO/IEC 18004 worked example (the numeric
`"01234567"` V1-M codewords and EC bytes are asserted in the test suite), plus
exhaustive round-trip tests across modes, versions and EC levels.

## License

MIT © Karpelès Lab Inc.
