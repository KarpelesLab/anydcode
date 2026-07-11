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
//! ## Status
//!
//! The full [`Symbology`] catalog is modeled, but only [`Symbology::QrCode`] is
//! implemented so far. Query [`Symbology::is_implemented`] at runtime.
//!
//! ## Example: QR round-trip
//!
//! ```
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
//! ```

#![forbid(unsafe_code)]

pub mod codes;
pub mod error;
pub mod geometry;
pub mod image;
pub mod imgproc;
pub mod output;
pub mod pipeline;
pub mod render;
pub mod scan1d;
pub mod segment;
pub mod symbol;
pub mod symbology;
pub mod traits;
pub mod transform;

pub use error::{Error, Result};
pub use image::{GrayFrame, GrayImage};
pub use segment::{Mode, Segment};
pub use symbol::{Symbol, SymbolMeta};
pub use symbology::{Dimension, Symbology};
