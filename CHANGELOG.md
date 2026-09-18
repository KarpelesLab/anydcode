# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Live detection.** On camera-like scenes the locate → crop → decode loop read about
  half the codes it was shown and reported dozens of values that were not there, at
  ~1 s per decode pass. It now reads 97.5% of 1D and 83% of 2D codes on the same scenes
  with no wrong values, in tens of milliseconds (`examples/liveeval.rs`,
  `tests/live_pipeline.rs`):
  - `scan1d` treated a whole scan line as one barcode and required both crop edges to
    be light; it now thresholds locally, cuts each line into quiet-zone-delimited spans,
    fits module width and bar/space bias jointly, and samples band-averaged profiles at
    any angle.
  - `pipeline::scan_1d` believed any single decode although half its decoders have no
    check character; readings now need cross-scanline consensus, quiet zones, a
    plausible length, and must not be a fragment of a longer reading.
  - The EAN/UPC width reader ignored quiet zones, reading a UPC-E out of the left half
    of any EAN-13 and out of other symbologies and text.
  - `detect::locate` only recognised barcodes within ~15° of the frame axes, was blinded
    by uneven lighting (global Otsu), let a single false finder hit turn a text blob into
    a top-ranked "QR" that swallowed the barcode inside it, and gave decoders no
    orientation. It is now gradient-orientation based with local binarization, and
    reports each linear region's reading axis and an oriented box.
  - The App Clip scanner decoded ordinary barcodes and text into fluent-looking URLs
    (its payload RS is far too weak for thousands of hypotheses per frame); it now
    requires real ring structure and the template byte. Aztec no longer "decodes" a
    blank patch (all-zero codewords are a valid RS codeword).
  - QR and App Clip scanning no longer spend hundreds of milliseconds on code-free
    frames.
  - `FrameDetector` matched `Hints` by exact fingerprint equality, which sensor noise
    and hand shake defeat on every frame; it now matches by position plus a tolerant
    fingerprint.
  - Web demo: a wasm trap or failed fetch left a decode worker "busy" forever, silently
    disabling decoding for the session; the crop batch was truncated before barcodes
    were prioritised, so busy scenes never decoded theirs.
- Encoders now conform where they did not (output unreadable by other tools): Aztec
  orientation marks, Data Matrix 144×144 block interleave, Micro QR M1/M3 padding,
  Code 16K symbol value 106, one-track Pharmacode gap, Grid Matrix numeral/byte modes,
  DotCode corner-forced masks.
- Decoders now read what other encoders produce: DataBar Expanded methods 3–14, PDF417
  byte shift 913, Code 128 / Code 16K / Codablock F FNC4, Code 49 numeric mode, Code 16K
  modes 5/6, rMQR ECI, Aztec latched punctuation and FLG(n), DotCode macros and error
  correction, Code 39 / Code 11 at 3:1 print ratio, Telepen cropped to its last bar.
- Panics on hostile input: IMb, Mailmark, Han Xin, DataBar Expanded, DotCode and MaxiCode
  decoders; App Clip URLs with non-ASCII near the scheme; `MatrixBuf::new` overflow; CLI
  `--scale`, giant PNG headers, closed stdout.
- Many decoders accepted malformed or non-canonical input (out-of-range numeric groups,
  aliased Kanji, empty payloads, over-wide elements, unvalidated termination bars, Micro
  QR M1 "error correction") that either mis-decoded or could not re-encode.
- CLI: option parsing (flags swallowing data, unknown options ignored, surplus
  arguments), alpha-channel PNGs decoding as solid black.

### Added

- `pipeline::scan_1d_at`, `scan_stacked_at`, `scan_linear_at`: decode a located linear
  region along its known reading axis (1D readers, then the PDF417 family).
- `scan1d::scan_spans`, `scan1d::refine_axis`, `ScanOptions::around`;
  `imgproc::orient::gradient_angle_peaks`; `codes::ean::decode_edges_within`;
  `detect::fingerprint_distance`.
- `Location::rotation` and an oriented `outline` on linear `detect::locate` candidates.

### Changed

- `LocateOptions::anisotropy` is now a rotation-invariant gradient coherence (default
  `0.7`) and `edge_density` a fraction of edge pixels; `ScanOptions::angles_deg` accepts
  any angle.
- One-track Pharmacode renders with the standard two-module gap (`max_modules` 78).

## [0.1.4](https://github.com/KarpelesLab/anydcode/compare/v0.1.3...v0.1.4) - 2026-09-15

### Added

- no_std core, alloc/std tiers, per-symbology and encode/decode/scan gates

### Other

- tests, docs: run heap-free encoders without alloc; document encode_into
- heap-free encode_into
- heap-free code-set planner
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free encode_into
- heap-free LinearSink/LinearBuf/MatrixBuf; Code 39 encode_into

## [0.1.3](https://github.com/KarpelesLab/anydcode/compare/v0.1.2...v0.1.3) - 2026-07-29

### Other

- automatic mode selection for QR-family and Han Xin build_text
- image sampler — camera detection for all 34 size variants
- image sampler — camera detection for all 32 rectangular sizes
- image samplers — camera detection via native fiducials

### Added

- automatic mode selection: `build_text` for QR, Micro QR (new), rMQR and Han Xin
  now splits text into the cheapest mix of numeric / alphanumeric / byte segments
  via a shared cost-model dynamic program (`segment::optimize_segments`)

## [0.1.2](https://github.com/KarpelesLab/anydcode/compare/v0.1.1...v0.1.2) - 2026-07-18

### Fixed

- live decode starved by runaway QR fallbacks on cluttered crops
- *(demo)* unify decode path + cache-bust wasm so the demo actually decodes
- *(ean)* consensus floor on edge decode to reject phantom barcodes
- *(detect)* isolate 1D barcodes from busy scenes via per-tile labelling

### Other

- pin explicit stable discriminants
- bring README in line with current detection status
- apply rustfmt across recent additions
- camera detection of printed App Clip Codes
- implement Apple App Clip Codes (URL codec + SVG generation)
- record a real quiet zone on encoded matrices
- decode linear codes at any rotation
- decode small-module curved captures — format brute force + biased sampling
- stop lock judder and stale locks that outlive the code
- keep the live demo's main thread cheap; bridge linear region seams
- gate scene texture out of the locator and stop fragmenting codes
- parallel full-frame 2D worker so QR works without the locator
- bound QR and 1D scans; split scan_2d/scan_1d; add decode2d
- capture now records what the HUD shows (tracked codes + composited overlay)
- track located candidates to kill acquiring-reticle noise
- track decoded codes across frames via the locate pass
- persist decoded codes with a hold window to stop lock flicker
- robust EAN/UPC edge-width decode for real camera captures
- add scan1ddebug tool + curved-bottle EAN fixture
- add locdebug example + real EAN-13 scene fixture
- Fix capture freeze: reuse live detection state instead of synchronous re-decode
- Merge impl/full-generator: all 51 symbologies + options in the web generator
- Web generator: support every implemented symbology with all its options
- add SizeStrategy (Balanced/MinHeight/MaxHeight) for shape control
- add capture button (saves native frame PNG + detection JSON) and request 1080p
- Add gated degrade example for realistic jsQR-parity benchmarking
- QR read_format: cross-validate both format copies, keep higher-confidence one
- QR sampler: timing-anchored dewarp + per-module majority midpoint sampling
- Add sharp real-world QR fixture + gated qrdebug example (diagnosis: timing-pitch drift → per-module errors)
- Add multi-anchor non-planar (TPS) dewarping to the QR sampler
- Merge impl/qr-realworld: real-world robustness for QR/DataMatrix/PDF417 samplers
- Harden QR image sampler for real-world photos (adaptive binarization + local sampling)
- Add real-world QR photo regression fixture (curved bottle, blurred)

## [0.1.1](https://github.com/KarpelesLab/anydcode/compare/v0.1.0...v0.1.1) - 2026-07-11

### Other

- Fix broken rustdoc links to private items (dotcode, gridmatrix, detect)
- Fix live-camera scanning + add sci-fi target-lock HUD to the demo
- Link the live WASM demo in README
- cargo fmt (wasm demo)
- Add WASM demo: raw wasm32 FFI, browser page, GitHub Pages workflow
- Mark Code 16K/49/Codablock F + DataBar stacked implemented; complete catalog
- Merge impl/stacked-linear: Code 16K, Code 49, Codablock F
- Merge impl/databar-stacked: DataBar Stacked / Stacked-Omni / Expanded-Stacked
- Implement GS1 DataBar stacked variants
- Pre-wire stubs for remaining symbologies (Han Xin, Grid Matrix, DotCode, Code 16K/49, Codablock F, DX Film Edge)
- Fix byte_char_slices lint in codabar test
- Fix clippy byte_char_slices lint (stable 1.97) in aztec tables
- Add CI, crates.io, docs.rs, and license badges to README
