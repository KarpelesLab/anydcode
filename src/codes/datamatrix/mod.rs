//! Data Matrix (ISO/IEC 16022) ECC 200 encoder and structural decoder.
//!
//! Layout:
//! - [`gf`]        — GF(256) arithmetic (primitive `0x12D`) and Reed–Solomon.
//! - `tables`      — the spec's per-size data/EC codeword and region geometry.
//! - `placement`   — Annex F module placement and finder/timing borders.
//! - `encode`      — [`Symbol`] → [`BitMatrix`], or heap-free into a `MatrixBuf`.
//! - `decode`      — [`BitMatrix`] → [`Symbol`].
//!
//! # Scope
//!
//! Every ECC 200 symbol size is implemented: the square sizes 10×10 … 144×144
//! (including the multi-region larger sizes and interleaved Reed–Solomon blocks),
//! the six rectangular sizes of ISO/IEC 16022 (8×18 … 16×48) and the eighteen of the
//! DMRE extension (ISO/IEC 21471, 8×48 … 26×64).
//!
//! All six encodation schemes are read and written — **ASCII** (with digit-pair
//! packing and upper shift), **C40**, **Text**, **ANSI X12**, **EDIFACT** and
//! **Base256** — along with FNC1 (GS1 Data Matrix), the 05 and 06 Macros and ECI.
//! Structured Append and Reader Programming symbols are reported as unsupported.
//!
//! The builders choose between ASCII and Base256 only; the other schemes are
//! written from a [`Symbol`] that names them, which is what the decoder returns.
//!
//! # Segments
//!
//! A symbol is a sequence of segments, each with an [`Encodation`] in
//! [`DataMatrixMeta::encodations`]: one per run of a scheme, and one per FNC1 or ECI.
//! Concatenated, their bytes are what a reader transmits — a field-separator FNC1 is
//! the GS byte, a Macro its header and trailer.
//!
//! # Losslessness
//!
//! [`DataMatrixEncoder::build`] splits the payload into a *canonical* alternating
//! sequence of ASCII runs (bytes `< 128`) and Base256 runs (bytes `>= 128`). This is
//! exactly the segmentation the decoder reconstructs from the codeword stream, so
//! `encode(decode(encoding)) == encoding` holds.
//!
//! A symbol segmented any other way, by any encoder, re-encodes identically too:
//!
//! - The decoder merges adjacent ASCII runs, except where that would change how
//!   digits pair up: a digit encoded on its own and followed by another digit ends
//!   its segment, since the encoder packs two adjacent digits of one segment into a
//!   single codeword.
//! - A C40, Text or X12 run ends with an unlatch unless it ends the symbol. With
//!   one codeword left, that codeword is the unlatch if the run is the last segment
//!   and data otherwise; a run that was instead followed by a pad there decodes with
//!   an empty ASCII segment after it. Two C40 or Text values left over are padded
//!   with a Shift 1; a single one cannot be written.
//! - An EDIFACT run ends with its unlatch value unless it comes within two
//!   codewords of the end of the symbol.
//! - An FNC1 directly after a C40 or Text segment is part of that run, and so is a
//!   segment of the same encodation after it. Where the symbol unlatched instead,
//!   an empty ASCII segment says so.
//! - A Base256 length field of 0 means "to the end of the symbol". A Base256 run
//!   that is the last segment and ends on the last data codeword is written that
//!   way, as zint, ZXing and libdmtx do (so their symbols re-encode identically); a
//!   run that carries its byte count there instead decodes with an empty ASCII
//!   segment after it. For the same reason an empty Base256 segment is only accepted
//!   as the last codewords of a symbol it fills exactly.
//!
//! A codeword stream outside these rules — a dangling Shift 2, stray bits after an
//! EDIFACT unlatch, an ECI written longer than it needs to be — is reported as
//! undecodable rather than read approximately. Padding is not checked.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix
//! [`Mode::Byte`]: crate::segment::Mode::Byte

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(feature = "decode")]
mod decode;
#[cfg(feature = "encode")]
mod encode;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
pub mod gf;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod placement;
#[cfg(feature = "scan")]
mod sample;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod tables;

#[cfg(feature = "decode")]
pub use decode::DataMatrixDecoder;
#[cfg(feature = "encode")]
pub use encode::DataMatrixEncoder;
#[cfg(feature = "scan")]
pub use sample::{DataMatrixScanner, sample_grid, scan};

// ---- ASCII-encodation control codewords (ISO/IEC 16022 Table 2) ----------------
/// Pad / end-of-message.
const PAD: u8 = 129;
/// Latch to C40 encodation.
const C40_LATCH: u8 = 230;
/// Latch to Base256 encodation.
const BASE256_LATCH: u8 = 231;
/// FNC1.
const FNC1: u8 = 232;
/// Structured Append.
#[cfg(feature = "decode")]
const STRUCTURED_APPEND: u8 = 233;
/// Reader Programming.
#[cfg(feature = "decode")]
const READER_PROGRAMMING: u8 = 234;
/// Upper shift (the next ASCII value gets 128 added).
const UPPER_SHIFT: u8 = 235;
/// 05 Macro.
const MACRO05: u8 = 236;
/// 06 Macro.
const MACRO06: u8 = 237;
/// Latch to ANSI X12 encodation.
const X12_LATCH: u8 = 238;
/// Latch to Text encodation.
const TEXT_LATCH: u8 = 239;
/// Latch to EDIFACT encodation.
const EDIFACT_LATCH: u8 = 240;
/// Extended Channel Interpretation.
const ECI: u8 = 241;
/// In C40, Text and X12 encodation: return to ASCII.
const UNLATCH: u8 = 254;
/// In EDIFACT encodation: the 6-bit value that returns to ASCII.
const EDIFACT_UNLATCH: u8 = 31;
/// The byte a field-separator FNC1 is transmitted as.
const GS: u8 = 0x1D;
/// The bytes a Macro codeword appends to the message.
const MACRO_TRAILER: &[u8] = b"\x1e\x04";

/// The bytes a Macro codeword prefixes the message with.
fn macro_header(encodation: Encodation) -> &'static [u8] {
    if encodation == Encodation::Macro06 {
        b"[)>\x1e06\x1d"
    } else {
        b"[)>\x1e05\x1d"
    }
}

/// The data-encodation scheme used for one segment.
///
/// More schemes may be added; match with a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Encodation {
    /// ASCII encodation: digit pairs, direct byte values, and upper shift. Also the
    /// encodation of an ECI segment ([`Mode::Eci`](crate::segment::Mode::Eci)).
    Ascii,
    /// Base256 encodation: a latch, length field, and randomized raw bytes.
    Base256,
    /// C40 encodation: three values to two codewords; digits, upper-case letters
    /// and space take one value, every other byte two or more.
    C40,
    /// Text encodation: as C40 with lower-case letters in the basic set.
    Text,
    /// ANSI X12 encodation: three characters to two codewords; digits, upper-case
    /// letters, space, CR, `*` and `>` only, in a multiple of three.
    X12,
    /// EDIFACT encodation: four characters to three codewords; bytes `0x20..=0x5E`.
    Edifact,
    /// An FNC1 character. The segment's data is what a reader transmits for it:
    /// nothing in the first or second codeword position, where it flags the data
    /// format (GS1 in the first), and the separator byte GS (`0x1D`) anywhere else.
    /// Directly after a C40 or Text segment it is written inside that run, and a
    /// segment of the same encodation following it continues the run.
    Fnc1,
    /// The 05 Macro codeword, which stands for the header `[)>␞05␝` and the trailer
    /// `␞␄`. It takes two segments: the symbol's first, holding the header bytes,
    /// and its last, holding the trailer bytes.
    Macro05,
    /// The 06 Macro codeword: as [`Encodation::Macro05`] with the header `[)>␞06␝`.
    Macro06,
}

/// Data Matrix parameters needed to re-encode a symbol identically.
///
/// Heap-free encoding takes the same information as borrowed arguments of
/// `DataMatrixEncoder::encode_into` (the symbol size and an `&[Encodation]`).
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataMatrixMeta {
    /// Full symbol width in modules (e.g. `10` for a 10×10 symbol, `18` for 8×18).
    pub width: usize,
    /// Full symbol height in modules; equal to `width` for a square symbol.
    pub height: usize,
    /// The encodation used for each segment, in symbol order. Parallel to
    /// [`crate::Symbol::segments`].
    pub encodations: Vec<Encodation>,
}
