//! # AnyDCode
//!
//! A from-scratch Rust library for **decoding and encoding 1D and 2D barcodes**,
//! aiming for the widest possible symbology coverage while preserving every
//! variation of a code.
//!
//! ## Design goals
//!
//! - **Lossless round-trip.** A decoded [`Symbol`] carries not just its payload but
//!   the exact encoding decisions (segmentation, version/size, error-correction
//!   level, mask, ...). Feeding a decoded symbol straight back into the encoder
//!   reproduces the original symbol byte-for-byte. See [`codes::qr`].
//! - **Live-video ready.** The [`pipeline`] splits a cheap per-frame *detection*
//!   pass from a heavier *analysis/decode* pass, and carries [`pipeline::Hints`]
//!   forward so codes already decoded in earlier frames are not re-analyzed.
//! - **Portable input.** The video boundary is a borrowed luminance buffer
//!   ([`GrayFrame`]); callers own capture and color conversion.
//!
//! ## Cargo features
//!
//! The crate is always `#![no_std]`; everything beyond the core types is opt-in
//! (all of it is on by default).
//!
//! | Feature | Enables |
//! |---|---|
//! | `std` *(default)* | `alloc` plus float math: image scanning, transforms, `std` interop |
//! | `alloc` | the owned data model ([`Symbol`], [`Segment`], `BitMatrix`, ...) |
//! | `encode` *(default)* | encoders ([`traits::Encode`], with `alloc`) |
//! | `decode` *(default)* | structural decoders ([`traits::Decode`]); implies `alloc` |
//! | `scan` *(default)* | camera/still-image samplers, [`detect`], [`pipeline`]; implies `std` + `decode` |
//! | `all-codes` *(default)* | every symbology below |
//! | `matrix`, `stacked`, `linear`, `postal` | one symbology family |
//! | `qr`, `microqr`, `rmqr`, `aztec`, `datamatrix`, `maxicode`, `hanxin`, `dotcode`, `gridmatrix`, `appclip` | 2D matrix codes |
//! | `pdf417`, `code16k`, `code49`, `codablockf` | stacked codes |
//! | `ean`, `code128`, `code39`, `code93`, `code11`, `itf`, `twoof5`, `codabar`, `msi`, `telepen`, `pharmacode`, `dxfilm`, `databar` | linear codes |
//! | `cli` / `wasm` | the `anyd` binary / the browser-demo FFI shim |
//!
//! For example, a Code 128 + QR encoder for a `no_std` + `alloc` target:
//!
//! ```toml
//! anyd = { version = "0.1", default-features = false, features = ["alloc", "encode", "qr", "code128"] }
//! ```
//!
//! [`Symbology::is_implemented`] reports which symbologies the current build carries.
//!
//! ## Example: QR round-trip
//!
//! ```
//! # #[cfg(all(feature = "qr", feature = "encode", feature = "decode"))] {
//! use anyd::codes::qr::{EcLevel, QrDecoder, QrEncoder};
//! use anyd::traits::{Decode, Encode};
//!
//! let encoder = QrEncoder::new();
//! let symbol = encoder.build_text("HELLO WORLD", EcLevel::Q).unwrap();
//! let encoding = encoder.encode(&symbol).unwrap();
//!
//! let decoded = QrDecoder::new().decode(&encoding).unwrap();
//! assert_eq!(decoded.text().as_deref(), Some("HELLO WORLD"));
//! // Re-encoding the decoded symbol yields the identical matrix.
//! assert_eq!(encoder.encode(&decoded).unwrap(), encoding);
//! # }
//! ```

// Unsafe is forbidden everywhere except the raw-wasm FFI shim (the `wasm` feature),
// which needs it to hand buffers across the JS boundary. Library consumers never
// enable `wasm`, so the guarantee holds for them.
#![cfg_attr(not(feature = "wasm"), forbid(unsafe_code))]
#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

pub mod codes;
#[cfg(feature = "scan")]
pub mod detect;
pub mod error;
pub mod geometry;
pub mod image;
#[cfg(feature = "scan")]
pub mod imgproc;
#[allow(dead_code)]
mod math;
pub mod output;
#[cfg(feature = "scan")]
pub mod pipeline;
#[cfg(feature = "alloc")]
pub mod render;
#[cfg(feature = "scan")]
pub mod scan1d;
pub mod segment;
#[cfg(feature = "alloc")]
pub mod symbol;
pub mod symbology;
pub mod traits;
#[cfg(feature = "std")]
pub mod transform;
#[cfg(feature = "wasm")]
mod wasm;

pub use error::{Error, Result};
pub use image::GrayFrame;
#[cfg(feature = "alloc")]
pub use image::GrayImage;
pub use segment::Mode;
#[cfg(feature = "alloc")]
pub use segment::Segment;
#[cfg(feature = "alloc")]
pub use symbol::{Symbol, SymbolMeta};
pub use symbology::{Dimension, Symbology};
