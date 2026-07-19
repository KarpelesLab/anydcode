# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3](https://github.com/KarpelesLab/anydcode/compare/v0.1.2...v0.1.3) - 2026-07-19

### Other

- image sampler — camera detection for all 34 size variants
- image sampler — camera detection for all 32 rectangular sizes
- image samplers — camera detection via native fiducials

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
