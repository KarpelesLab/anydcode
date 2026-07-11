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

The full symbology catalog is **modeled** (`Symbology`), but implementations land
incrementally. Query `Symbology::is_implemented()` at runtime.

| Symbology | Encode | Decode | Detect |
|-----------|:------:|:------:|:------:|
| QR Code (v1–40, all EC levels & masks) | ✅ | ✅ (structural) | ⬜ |
| Micro/rMQR, Aztec, Data Matrix, PDF417, MaxiCode, … | ⬜ | ⬜ | ⬜ |
| EAN/UPC, Code 128, Code 39/93, ITF, Codabar, DataBar, … | ⬜ | ⬜ | ⬜ |

"Structural" decode consumes an already-sampled module grid; image sampling /
detection front-ends are the next milestone.

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
