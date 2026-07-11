# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/KarpelesLab/anydcode/compare/v0.1.0...v0.2.0) - 2026-07-11

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
