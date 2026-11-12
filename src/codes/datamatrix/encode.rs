//! Data Matrix ECC 200 encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The [`DataMatrixEncoder`] builds a reproducible symbol from raw bytes (choosing a
//! canonical ASCII/Base256 segmentation and the smallest fitting square size) or
//! renders an already-specified [`Symbol`] exactly from its [`DataMatrixMeta`].
//!
//! Rendering itself is heap-free: [`DataMatrixEncoder::encode_into`] and
//! [`DataMatrixEncoder::encode_data_into`] write into caller-provided buffers, and the
//! [`Encode`] implementation is a thin wrapper over the same core.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix
//! [`DataMatrixMeta`]: super::DataMatrixMeta
//! [`Encode`]: crate::traits::Encode

use super::gf::{self, MAX_EC_PER_BLOCK};
use super::placement::{QUIET_ZONE, draw_borders, mapping_to_symbol, occupancy_bytes, place};
use super::tables::{LARGEST, SymbolSpec, smallest_square_for, spec_by_size};
use super::{
    BASE256_LATCH, C40_LATCH, ECI, EDIFACT_LATCH, EDIFACT_UNLATCH, Encodation, FNC1, GS,
    MACRO_TRAILER, MACRO05, MACRO06, PAD, TEXT_LATCH, UNLATCH, UPPER_SHIFT, X12_LATCH,
    macro_header,
};
use crate::error::{Error, Result};
use crate::output::MatrixBuf;
use crate::segment::{Mode, SegmentView};

#[cfg(feature = "alloc")]
use super::DataMatrixMeta;
#[cfg(feature = "alloc")]
use crate::output::{BitMatrix, Encoding};
#[cfg(feature = "alloc")]
use crate::segment::Segment;
#[cfg(feature = "alloc")]
use crate::symbol::{Symbol, SymbolMeta};
#[cfg(feature = "alloc")]
use crate::symbology::Symbology;
#[cfg(feature = "alloc")]
use crate::traits::Encode;
#[cfg(feature = "alloc")]
use alloc::{vec, vec::Vec};

/// Longest Base256 run whose byte count fits a one-codeword length field.
const BASE256_SHORT_MAX: usize = 249;

/// Scratch bytes needed to render `spec`: every codeword (data + EC) followed by the
/// placement occupancy bitmap.
const fn scratch_len_for(spec: &SymbolSpec) -> usize {
    spec.total_cw() + occupancy_bytes(spec.mapping_rows(), spec.mapping_cols())
}

/// One segment as the codeword writer sees it.
#[derive(Clone, Copy)]
struct Piece<'s> {
    encodation: Encodation,
    mode: Mode,
    data: &'s [u8],
}

impl<'s> Piece<'s> {
    const fn byte(encodation: Encodation, data: &'s [u8]) -> Self {
        Piece {
            encodation,
            mode: Mode::Byte,
            data,
        }
    }
}

/// Data Matrix encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataMatrixEncoder;

impl DataMatrixEncoder {
    /// Scratch bytes sufficient for any symbol size (the 144×144 symbol):
    /// its 2178 codewords plus a 2178-byte placement occupancy bitmap.
    pub const MAX_SCRATCH_LEN: usize = scratch_len_for(&LARGEST);

    /// Storage bytes sufficient for any symbol size (a bit-packed 144×144 grid).
    pub const MAX_STORAGE_LEN: usize = MatrixBuf::bytes_for(LARGEST.width, LARGEST.height);

    /// A new encoder.
    pub fn new() -> Self {
        DataMatrixEncoder
    }

    /// Scratch bytes [`DataMatrixEncoder::encode_into`] needs for a `width × height`
    /// symbol (total codewords plus the placement occupancy bitmap), or `None` if
    /// that is not a Data Matrix size. Never exceeds
    /// [`DataMatrixEncoder::MAX_SCRATCH_LEN`].
    pub const fn scratch_len(width: usize, height: usize) -> Option<usize> {
        match spec_by_size(width, height) {
            Some(spec) => Some(scratch_len_for(&spec)),
            None => None,
        }
    }

    /// Storage bytes the output [`MatrixBuf`] needs for a `width × height` symbol, or
    /// `None` if that is not a Data Matrix size. Never exceeds
    /// [`DataMatrixEncoder::MAX_STORAGE_LEN`].
    pub const fn storage_len(width: usize, height: usize) -> Option<usize> {
        match spec_by_size(width, height) {
            Some(spec) => Some(MatrixBuf::bytes_for(spec.width, spec.height)),
            None => None,
        }
    }

    /// Heap-free rendering of a fully specified symbol: the `segments`, each encoded
    /// with the parallel entry of `encodations`, into a `width × height` symbol. This
    /// is exactly what [`Encode`](crate::traits::Encode) does with a
    /// [`Symbol`](crate::Symbol) and its [`DataMatrixMeta`](super::DataMatrixMeta).
    ///
    /// `scratch` holds the codewords and placement state (at least
    /// [`DataMatrixEncoder::scratch_len`] bytes); `storage` receives the module bits
    /// (at least [`DataMatrixEncoder::storage_len`] bytes). The returned grid has the
    /// 1-module Data Matrix quiet zone.
    ///
    /// # Errors
    /// - [`Error::InvalidParameter`] for an unknown size or mismatched `encodations`;
    /// - [`Error::Unsupported`] for a segment that is neither byte-mode nor an ECI;
    /// - [`Error::InvalidData`] for a segment its encodation cannot express;
    /// - [`Error::Capacity`] if the data does not fit the symbol or a buffer is too
    ///   small.
    pub fn encode_into<'a>(
        &self,
        segments: &[SegmentView<'_>],
        encodations: &[Encodation],
        width: usize,
        height: usize,
        scratch: &mut [u8],
        storage: &'a mut [u8],
    ) -> Result<MatrixBuf<'a>> {
        if encodations.len() != segments.len() {
            return Err(Error::invalid_parameter(
                "encodation count does not match segment count",
            ));
        }
        check_modes(segments.iter().map(|s| s.mode))?;
        let spec = spec_for(width, height)?;
        render_into(
            encodations
                .iter()
                .zip(segments)
                .map(|(&encodation, s)| Piece {
                    encodation,
                    mode: s.mode,
                    data: s.data,
                }),
            Some(spec),
            Some(spec.data_cw),
            scratch,
            storage,
        )
    }

    /// Heap-free convenience: encode raw `data` with the same canonical ASCII/Base256
    /// segmentation as [`DataMatrixEncoder::build`], in the smallest fitting square
    /// symbol (`symbol_size: None`) or a forced one. The result is identical to
    /// encoding the symbol `build` / `build_sized` returns.
    ///
    /// Buffers are sized as for [`DataMatrixEncoder::encode_into`]; when the size is
    /// chosen automatically, [`DataMatrixEncoder::MAX_SCRATCH_LEN`] and
    /// [`DataMatrixEncoder::MAX_STORAGE_LEN`] always suffice.
    ///
    /// # Errors
    /// [`Error::InvalidParameter`] for an unknown forced size; [`Error::Capacity`] if
    /// the data does not fit or a buffer is too small for the chosen size.
    pub fn encode_data_into<'a>(
        &self,
        data: &[u8],
        symbol_size: Option<usize>,
        scratch: &mut [u8],
        storage: &'a mut [u8],
    ) -> Result<MatrixBuf<'a>> {
        let forced = match symbol_size {
            Some(sz) => Some(spec_for(sz, sz)?),
            None => None,
        };
        // No `fill`: like `build`, raw data is sized by its explicit length fields.
        render_into(canonical_runs(data), forced, None, scratch, storage)
    }
}

#[cfg(feature = "alloc")]
impl DataMatrixEncoder {
    /// Build a reproducible [`Symbol`] from raw `data`, choosing a canonical
    /// ASCII/Base256 segmentation and the smallest fitting square symbol size.
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        self.build_pieces(canonical_runs(data), None)
    }

    /// Like [`DataMatrixEncoder::build`] but forcing a specific square symbol size
    /// (full side length in modules, e.g. `20` for a 20×20 symbol).
    pub fn build_sized(&self, data: &[u8], symbol_size: usize) -> Result<Symbol> {
        self.build_rectangular(data, symbol_size, symbol_size)
    }

    /// Like [`DataMatrixEncoder::build`] but forcing a specific symbol size, square
    /// or rectangular: `width × height` in modules, e.g. `18, 8` for the 8×18
    /// symbol. The rectangular sizes are those of ISO/IEC 16022 and of DMRE
    /// (ISO/IEC 21471).
    pub fn build_rectangular(&self, data: &[u8], width: usize, height: usize) -> Result<Symbol> {
        let spec = spec_for(width, height)?;
        self.build_pieces(canonical_runs(data), Some(spec))
    }

    /// Convenience: build from UTF-8 `text`.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        self.build(text.as_bytes())
    }

    /// Build a GS1 Data Matrix from an element string: the application identifiers
    /// and their values run together, with the GS byte (`0x1D`) after each
    /// variable-length field that is not the last. The symbol opens with FNC1 and
    /// writes each GS as FNC1, in the smallest fitting square size.
    ///
    /// # Errors
    /// [`Error::InvalidData`] if `data` holds a byte outside ASCII;
    /// [`Error::Capacity`] if it exceeds the largest symbol.
    pub fn build_gs1(&self, data: &[u8]) -> Result<Symbol> {
        self.build_pieces(gs1_pieces(data)?, None)
    }

    /// Like [`DataMatrixEncoder::build_gs1`] but forcing a specific symbol size,
    /// `width × height` in modules, square or rectangular.
    pub fn build_gs1_sized(&self, data: &[u8], width: usize, height: usize) -> Result<Symbol> {
        let spec = spec_for(width, height)?;
        self.build_pieces(gs1_pieces(data)?, Some(spec))
    }

    fn build_pieces<'s>(
        &self,
        pieces: impl Iterator<Item = Piece<'s>> + Clone,
        forced: Option<SymbolSpec>,
    ) -> Result<Symbol> {
        let limit = forced.map_or(LARGEST.data_cw, |s| s.data_cw);
        let mut cw = vec![0u8; limit];
        let n = match write_codewords(pieces.clone(), &mut cw, None) {
            Ok(written) => written.len,
            Err(e) => {
                return Err(e.into_error(if forced.is_some() {
                    "data does not fit in the requested size"
                } else {
                    "data exceeds the largest square Data Matrix"
                }));
            }
        };
        let spec = match forced {
            Some(s) => s,
            None => smallest_square_for(n).expect("codeword count within the largest size"),
        };
        let (encodations, segments): (Vec<Encodation>, Vec<Segment>) = pieces
            .map(|p| (p.encodation, Segment::byte(p.data.to_vec())))
            .unzip();
        let meta = DataMatrixMeta {
            width: spec.width,
            height: spec.height,
            encodations,
        };
        Ok(Symbol::new(
            Symbology::DataMatrix,
            segments,
            SymbolMeta::DataMatrix(meta),
        ))
    }
}

#[cfg(feature = "alloc")]
impl Encode for DataMatrixEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::DataMatrix {
            return Err(Error::invalid_parameter(
                "DataMatrixEncoder given a non-Data-Matrix symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::DataMatrix(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Data Matrix symbol missing DataMatrixMeta",
                ));
            }
        };
        if meta.encodations.len() != symbol.segments.len() {
            return Err(Error::invalid_parameter(
                "encodation count does not match segment count",
            ));
        }
        check_modes(symbol.segments.iter().map(|s| s.mode))?;
        let spec = spec_for(meta.width, meta.height)?;

        let mut scratch = vec![0u8; scratch_len_for(&spec)];
        let mut storage = vec![0u8; MatrixBuf::bytes_for(spec.width, spec.height)];
        let segs = meta
            .encodations
            .iter()
            .zip(&symbol.segments)
            .map(|(&encodation, s)| Piece {
                encodation,
                mode: s.mode,
                data: &s.data,
            });
        let buf = render_into(
            segs,
            Some(spec),
            Some(spec.data_cw),
            &mut scratch,
            &mut storage,
        )?;
        Ok(Encoding::Matrix(BitMatrix::from(&buf)))
    }
}

/// A GS1 element string as segments: FNC1 in front, and FNC1 for each GS.
#[cfg(feature = "alloc")]
fn gs1_pieces(data: &[u8]) -> Result<impl Iterator<Item = Piece<'_>> + Clone> {
    if !data.is_ascii() {
        return Err(Error::invalid_data("GS1 element strings are ASCII"));
    }
    Ok(data.split(|&b| b == GS).enumerate().flat_map(|(n, field)| {
        let fnc1 = Piece::byte(Encodation::Fnc1, if n == 0 { &[] } else { &[GS] });
        let field = (!field.is_empty()).then_some(Piece::byte(Encodation::Ascii, field));
        core::iter::once(fnc1).chain(field)
    }))
}

/// The spec for a `width × height` symbol, or an `InvalidParameter` error.
fn spec_for(width: usize, height: usize) -> Result<SymbolSpec> {
    spec_by_size(width, height)
        .ok_or_else(|| Error::invalid_parameter("unknown Data Matrix symbol size"))
}

/// Data Matrix segments carry raw bytes, or are ECI switches.
fn check_modes(mut modes: impl Iterator<Item = Mode>) -> Result<()> {
    if modes.all(|m| matches!(m, Mode::Byte | Mode::Eci(_))) {
        Ok(())
    } else {
        Err(Error::Unsupported {
            what: "Data Matrix supports only Byte-mode and ECI segments",
        })
    }
}

/// Split `data` into a canonical alternating sequence of ASCII (bytes `< 128`) and
/// Base256 (bytes `>= 128`) runs. This is exactly the segmentation the decoder
/// reconstructs, keeping the round-trip lossless. Empty data yields one empty ASCII
/// run.
fn canonical_runs(data: &[u8]) -> impl Iterator<Item = Piece<'_>> + Clone + '_ {
    let mut rest = data;
    let mut first = true;
    core::iter::from_fn(move || {
        if rest.is_empty() {
            if core::mem::take(&mut first) {
                return Some(Piece::byte(Encodation::Ascii, rest));
            }
            return None;
        }
        first = false;
        let high = rest[0] >= 128;
        let end = rest
            .iter()
            .position(|&b| (b >= 128) != high)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        rest = tail;
        let enc = if high {
            Encodation::Base256
        } else {
            Encodation::Ascii
        };
        Some(Piece::byte(enc, run))
    })
}

/// Render `segs` into a symbol of the `forced` size (or the smallest that fits),
/// using `scratch` for codewords and placement state and `storage` for the modules.
fn render_into<'s, 'a>(
    segs: impl Iterator<Item = Piece<'s>>,
    forced: Option<SymbolSpec>,
    fill: Option<usize>,
    scratch: &mut [u8],
    storage: &'a mut [u8],
) -> Result<MatrixBuf<'a>> {
    if let Some(spec) = forced
        && scratch.len() < scratch_len_for(&spec)
    {
        return Err(Error::capacity("Data Matrix scratch buffer too small"));
    }
    let limit = forced.map_or(LARGEST.data_cw, |s| s.data_cw);
    let avail = limit.min(scratch.len());
    let written = match write_codewords(segs, &mut scratch[..avail], fill) {
        Ok(written) => written,
        Err(e) => {
            return Err(e.into_error(if avail < limit {
                "Data Matrix scratch buffer too small"
            } else if forced.is_some() {
                "data does not fit in the symbol size"
            } else {
                "data exceeds the largest square Data Matrix"
            }));
        }
    };
    let spec = match forced {
        Some(s) => s,
        None => smallest_square_for(written.len).expect("codeword count within the largest size"),
    };
    if scratch.len() < scratch_len_for(&spec) {
        return Err(Error::capacity("Data Matrix scratch buffer too small"));
    }
    let (cw, occupied) = scratch.split_at_mut(spec.total_cw());
    let n = written.settle(cw, &spec)?;
    let mut m = MatrixBuf::new(storage, spec.width, spec.height, QUIET_ZONE)?;

    pad_codewords(&mut cw[..spec.data_cw], n);
    add_error_correction(cw, &spec);

    let (mr, mc) = (spec.mapping_rows(), spec.mapping_cols());
    let (placed, fixed) = place(mr, mc, occupied, |c, bit, r, col| {
        if (cw[c] >> (7 - bit)) & 1 != 0 {
            let (x, y) = mapping_to_symbol(&spec, r, col);
            m.set(x, y, true);
        }
    });
    debug_assert_eq!(placed, spec.total_cw());
    if fixed {
        for d in 1..=2 {
            let (x, y) = mapping_to_symbol(&spec, mr - d, mc - d);
            m.set(x, y, true);
        }
    }
    draw_borders(&spec, &mut m);
    Ok(m)
}

/// A bounded codeword stream over a caller slice.
struct Codewords<'b> {
    buf: &'b mut [u8],
    len: usize,
}

impl Codewords<'_> {
    /// Append one codeword; `None` if the buffer is full.
    fn push(&mut self, v: u8) -> Option<()> {
        *self.buf.get_mut(self.len)? = v;
        self.len += 1;
        Some(())
    }
}

/// Why a codeword stream could not be written.
enum WriteError {
    /// The codewords do not fit the buffer.
    Full,
    /// An empty Base256 segment is followed by more codewords (see
    /// [`empty_base256_error`]).
    EmptyBase256,
    /// A segment its encodation cannot express.
    Invalid(&'static str),
}

impl WriteError {
    /// The public error, with `full` saying what overflowed.
    fn into_error(self, full: &'static str) -> Error {
        match self {
            WriteError::Full => Error::capacity(full),
            WriteError::EmptyBase256 => empty_base256_error(),
            WriteError::Invalid(what) => Error::invalid_data(what),
        }
    }
}

/// An open C40, Text, X12 or EDIFACT run: the values not yet packed into codewords.
struct Run {
    encodation: Encodation,
    buf: [u8; 4],
    n: usize,
    /// The last thing written into the run is an FNC1.
    after_fnc1: bool,
}

impl Run {
    fn new(encodation: Encodation) -> Self {
        Run {
            encodation,
            buf: [0; 4],
            n: 0,
            after_fnc1: false,
        }
    }

    /// Values per packed group: four 6-bit ones in EDIFACT, else three base-40.
    fn group(&self) -> usize {
        if self.encodation == Encodation::Edifact {
            4
        } else {
            3
        }
    }

    /// The buffered values as codewords (three in EDIFACT, else two).
    fn packed(&self) -> [u8; 3] {
        let b = self.buf;
        if self.encodation == Encodation::Edifact {
            [
                b[0] << 2 | b[1] >> 4,
                (b[1] & 15) << 4 | b[2] >> 2,
                (b[2] & 3) << 6 | b[3],
            ]
        } else {
            let v = 1600 * u16::from(b[0]) + 40 * u16::from(b[1]) + u16::from(b[2]) + 1;
            [(v >> 8) as u8, v as u8, 0]
        }
    }

    /// Append one value, writing out the group it completes.
    fn value(&mut self, cw: &mut Codewords<'_>, v: u8) -> Option<()> {
        self.buf[self.n] = v;
        self.n += 1;
        if self.n == self.group() {
            let codewords = self.group() - 1;
            for &c in &self.packed()[..codewords] {
                cw.push(c)?;
            }
            self.buf = [0; 4];
            self.n = 0;
        }
        Some(())
    }

    /// Append `bytes` in the run's encodation.
    fn bytes(
        &mut self,
        cw: &mut Codewords<'_>,
        bytes: &[u8],
    ) -> core::result::Result<(), WriteError> {
        for &byte in bytes {
            let mut vals = [0u8; 4];
            let n = match self.encodation {
                Encodation::X12 => x12_value(byte).map(|v| {
                    vals[0] = v;
                    1
                }),
                Encodation::Edifact => matches!(byte, 0x20..=0x5E).then(|| {
                    vals[0] = byte & 0x3F;
                    1
                }),
                set => Some(c40_values(set == Encodation::Text, byte, &mut vals)),
            }
            .ok_or(WriteError::Invalid(
                "byte outside the Data Matrix X12 or EDIFACT set",
            ))?;
            for &v in &vals[..n] {
                self.value(cw, v).ok_or(WriteError::Full)?;
            }
            self.after_fnc1 = false;
        }
        Ok(())
    }

    /// End the run and return to ASCII. `left` is how many data codewords the symbol
    /// has left (unknown while its size is undecided) and `more` whether anything is
    /// still to be written after the run.
    ///
    /// The unlatch is left out where a reader does without it: at the very end of
    /// the symbol, before a single last codeword that carries data, and after an
    /// EDIFACT run that stops within two codewords of the end.
    fn close(
        mut self,
        cw: &mut Codewords<'_>,
        fill: Option<usize>,
        more: bool,
    ) -> core::result::Result<(), WriteError> {
        if self.encodation == Encodation::Edifact {
            let left = fill.map(|f| f.saturating_sub(cw.len));
            if self.n == 0 && left.is_some_and(|l| l <= 2) {
                return Ok(());
            }
            // The unlatch value, then zero fill to the end of its codeword.
            self.buf[self.n] = EDIFACT_UNLATCH;
            let codewords = ((self.n + 1) * 6).div_ceil(8);
            for &c in &self.packed()[..codewords] {
                cw.push(c).ok_or(WriteError::Full)?;
            }
            return Ok(());
        }
        match self.n {
            0 => {}
            // A lone Shift 1 pads out the last triplet.
            2 if self.encodation != Encodation::X12 => {
                self.value(cw, 0).ok_or(WriteError::Full)?;
            }
            _ => {
                return Err(WriteError::Invalid(
                    "a C40, Text or X12 segment must fill its last triplet",
                ));
            }
        }
        match fill.map(|f| f.saturating_sub(cw.len)) {
            Some(0) => Ok(()),
            Some(1) if more => Ok(()),
            _ => cw.push(UNLATCH).ok_or(WriteError::Full),
        }
    }
}

/// The X12 value of `byte`, if it has one.
fn x12_value(byte: u8) -> Option<u8> {
    Some(match byte {
        b'\r' => 0,
        b'*' => 1,
        b'>' => 2,
        b' ' => 3,
        b'0'..=b'9' => byte - b'0' + 4,
        b'A'..=b'Z' => byte - b'A' + 14,
        _ => return None,
    })
}

/// Write the C40 (or, with `text`, Text) values of `byte` into `out`, returning how
/// many: one from the basic set, two through a shift, and two more in front for the
/// upper shift of a byte `>= 128`.
fn c40_values(text: bool, byte: u8, out: &mut [u8; 4]) -> usize {
    let mut n = 0;
    let mut push = |v: u8| {
        out[n] = v;
        n += 1;
    };
    if byte >= 128 {
        push(1);
        push(30);
    }
    let b = byte & 0x7F;
    // The letters of the basic set, and those reached through Shift 3.
    let (basic, shifted) = if text {
        (b'a'..=b'z', b'A'..=b'Z')
    } else {
        (b'A'..=b'Z', b'a'..=b'z')
    };
    match b {
        b' ' => push(3),
        b'0'..=b'9' => push(b - b'0' + 4),
        _ if basic.contains(&b) => push(b - basic.start() + 14),
        0..=31 => {
            push(0);
            push(b);
        }
        b'!'..=b'/' => {
            push(1);
            push(b - b'!');
        }
        b':'..=b'@' => {
            push(1);
            push(b - b':' + 15);
        }
        b'['..=b'_' => {
            push(1);
            push(b - b'[' + 22);
        }
        _ => {
            push(2);
            push(if shifted.contains(&b) {
                b - shifted.start() + 1
            } else {
                b - b'`'
            });
        }
    }
    n
}

/// A written codeword stream.
struct Written {
    /// Number of data codewords, before padding.
    len: usize,
    /// The stream ends in an empty Base256 run, whose zero length field reads as
    /// "to the end of the symbol": it is only valid if no padding follows.
    to_symbol_end: bool,
    /// Index of the one-codeword length field of a Base256 run that is the last
    /// segment, if it was written with its byte count.
    final_length_field: Option<usize>,
}

impl Written {
    /// Settle the stream in `cw` for `spec`, returning the codeword count.
    ///
    /// A Base256 run that is the last segment and ends on the symbol's last data
    /// codeword gets a zero length field ("to the end of the symbol"). Both that and
    /// the byte count are valid there; zero is what the reference encoders (zint,
    /// ZXing, libdmtx) write, so their symbols re-encode identically. A run closed
    /// with its count nevertheless is one followed by an (empty) ASCII segment.
    fn settle(&self, cw: &mut [u8], spec: &SymbolSpec) -> Result<usize> {
        let fills = self.len == spec.data_cw;
        if self.to_symbol_end && !fills {
            return Err(empty_base256_error());
        }
        if let Some(field) = self.final_length_field
            && fills
        {
            cw[field] = randomize_255(0, field + 1);
        }
        Ok(self.len)
    }
}

/// A Base256 length field of 0 means "the rest of the symbol", so an empty run can
/// only be written as the very last codewords of a symbol it fills exactly; anywhere
/// else a reader would take whatever follows (segments, padding) as its bytes.
fn empty_base256_error() -> Error {
    Error::invalid_data("an empty Base256 segment must end a symbol it fills exactly")
}

/// Encode all segments into `out` as the data codeword stream (before padding).
///
/// `fill` is the data capacity of a symbol whose size is already decided. It lets a
/// final Base256 run of 250 bytes or more that reaches exactly the end of that symbol
/// drop to the one-codeword zero length field, without which it would not fit, and
/// a C40, Text, X12 or EDIFACT run drop the unlatch a reader does not need there.
fn write_codewords<'s>(
    segs: impl Iterator<Item = Piece<'s>>,
    out: &mut [u8],
    fill: Option<usize>,
) -> core::result::Result<Written, WriteError> {
    use WriteError::{Full, Invalid};
    let mut cw = Codewords { buf: out, len: 0 };
    // Where an empty Base256 run ended, if one has been written.
    let mut open_end = None;
    let mut final_length_field = None;
    let mut run: Option<Run> = None;
    // The Macro codeword the symbol opened with, until its trailer segment closes it.
    let mut open_macro = None;
    let mut segs = segs.enumerate().peekable();
    while let Some((index, piece)) = segs.next() {
        let Piece {
            encodation, data, ..
        } = piece;
        let is_macro = matches!(encodation, Encodation::Macro05 | Encodation::Macro06);
        // Nothing but a Macro trailer, which writes no codeword, follows.
        let last = segs
            .peek()
            .is_none_or(|(_, p)| matches!(p.encodation, Encodation::Macro05 | Encodation::Macro06));
        if let Mode::Eci(_) = piece.mode
            && encodation != Encodation::Ascii
        {
            return Err(Invalid("an ECI segment takes ASCII encodation"));
        }

        // An FNC1 right after a C40 or Text run goes inside it, and so does a
        // segment of the run's encodation after that FNC1.
        if let Some(r) = run.as_mut() {
            let shifts = matches!(r.encodation, Encodation::C40 | Encodation::Text);
            if encodation == Encodation::Fnc1 && shifts {
                check_fnc1(data)?;
                r.value(&mut cw, 1).ok_or(Full)?;
                r.value(&mut cw, 27).ok_or(Full)?;
                r.after_fnc1 = true;
                continue;
            }
            if encodation == r.encodation && r.after_fnc1 {
                r.bytes(&mut cw, data)?;
                continue;
            }
        }
        if let Some(r) = run.take() {
            r.close(&mut cw, fill, !(is_macro && index > 0))?;
        }

        match encodation {
            Encodation::Ascii => match piece.mode {
                Mode::Eci(n) => eci_encode(&mut cw, n)?,
                _ => ascii_encode(&mut cw, data).ok_or(Full)?,
            },
            Encodation::Base256 => {
                let to_end =
                    last && data.len() > BASE256_SHORT_MAX && fill == Some(cw.len + 2 + data.len());
                if last && data.len() <= BASE256_SHORT_MAX {
                    final_length_field = Some(cw.len + 1);
                }
                base256_encode(&mut cw, data, to_end).ok_or(Full)?;
            }
            Encodation::C40 | Encodation::Text | Encodation::X12 | Encodation::Edifact => {
                cw.push(match encodation {
                    Encodation::C40 => C40_LATCH,
                    Encodation::Text => TEXT_LATCH,
                    Encodation::X12 => X12_LATCH,
                    _ => EDIFACT_LATCH,
                })
                .ok_or(Full)?;
                let mut r = Run::new(encodation);
                r.bytes(&mut cw, data)?;
                run = Some(r);
            }
            Encodation::Fnc1 => {
                check_fnc1(data)?;
                cw.push(FNC1).ok_or(Full)?;
            }
            Encodation::Macro05 | Encodation::Macro06 => {
                if index == 0 && data == macro_header(encodation) {
                    cw.push(if encodation == Encodation::Macro05 {
                        MACRO05
                    } else {
                        MACRO06
                    })
                    .ok_or(Full)?;
                    open_macro = Some(encodation);
                } else if open_macro == Some(encodation)
                    && segs.peek().is_none()
                    && data == MACRO_TRAILER
                {
                    open_macro = None;
                } else {
                    return Err(Invalid(
                        "a Macro takes the first segment (its header) and the last (its trailer)",
                    ));
                }
            }
        }
        if open_end.is_some_and(|end| cw.len > end) {
            return Err(WriteError::EmptyBase256);
        }
        if encodation == Encodation::Base256 && data.is_empty() {
            open_end = Some(cw.len);
        }
    }
    if let Some(r) = run.take() {
        r.close(&mut cw, fill, false)?;
    }
    if open_macro.is_some() {
        return Err(Invalid(
            "a Macro takes the first segment (its header) and the last (its trailer)",
        ));
    }
    if open_end.is_some_and(|end| cw.len > end) {
        return Err(WriteError::EmptyBase256);
    }
    Ok(Written {
        len: cw.len,
        to_symbol_end: open_end.is_some(),
        final_length_field,
    })
}

/// An FNC1 segment holds what a reader transmits for it: nothing, or GS.
fn check_fnc1(data: &[u8]) -> core::result::Result<(), WriteError> {
    if data.is_empty() || data == [GS] {
        Ok(())
    } else {
        Err(WriteError::Invalid(
            "an FNC1 segment is empty or the GS byte",
        ))
    }
}

/// Append an ECI codeword and assignment number `eci` (`0..=999999`).
fn eci_encode(cw: &mut Codewords<'_>, eci: u32) -> core::result::Result<(), WriteError> {
    let mut field = [0u8; 3];
    let field = match eci {
        0..=126 => {
            field[0] = eci as u8 + 1;
            &field[..1]
        }
        127..=16382 => {
            let v = eci - 127;
            field = [(v / 254 + 128) as u8, (v % 254 + 1) as u8, 0];
            &field[..2]
        }
        16383..=999_999 => {
            let v = eci - 16383;
            field = [
                (v / 64516 + 192) as u8,
                (v / 254 % 254 + 1) as u8,
                (v % 254 + 1) as u8,
            ];
            &field[..3]
        }
        _ => return Err(WriteError::Invalid("ECI assignment number above 999999")),
    };
    cw.push(ECI).ok_or(WriteError::Full)?;
    for &c in field {
        cw.push(c).ok_or(WriteError::Full)?;
    }
    Ok(())
}

/// Append `bytes` in ASCII encodation: digit pairs pack into one codeword, other
/// bytes below 128 map directly, and bytes `>= 128` use an upper shift.
fn ascii_encode(cw: &mut Codewords<'_>, bytes: &[u8]) -> Option<()> {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_digit() && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let val = (b - b'0') * 10 + (bytes[i + 1] - b'0');
            cw.push(130 + val)?;
            i += 2;
        } else if b < 128 {
            cw.push(b + 1)?;
            i += 1;
        } else {
            cw.push(UPPER_SHIFT)?;
            cw.push((b - 128) + 1)?;
            i += 1;
        }
    }
    Some(())
}

/// Append `bytes` in Base256 encodation (latch, length field, randomized payload).
/// With `to_end` the length field is the single codeword 0, "to the end of the
/// symbol".
fn base256_encode(cw: &mut Codewords<'_>, bytes: &[u8], to_end: bool) -> Option<()> {
    cw.push(BASE256_LATCH)?;
    let count = bytes.len();
    let mut field = [0u8; 2];
    let field = if to_end {
        &field[..1]
    } else if count <= BASE256_SHORT_MAX {
        field[0] = count as u8;
        &field[..1]
    } else {
        // The two-byte length field tops out at 1555 bytes, beyond any symbol.
        field[0] = u8::try_from(count / 250 + 249).ok()?;
        field[1] = (count % 250) as u8;
        &field[..2]
    };
    for &b in field.iter().chain(bytes) {
        let pos = cw.len + 1;
        cw.push(randomize_255(b, pos))?;
    }
    Some(())
}

/// The Base256 "255-state" randomizing algorithm (ISO/IEC 16022 §5.2.6).
fn randomize_255(value: u8, position: usize) -> u8 {
    let pseudo = ((149 * position) % 255) + 1;
    let t = value as usize + pseudo;
    (if t <= 255 { t } else { t - 256 }) as u8
}

/// Pad `cw[len..]` (the data codewords after the first `len`) using the "253-state"
/// pad algorithm.
fn pad_codewords(cw: &mut [u8], len: usize) {
    for (i, slot) in cw.iter_mut().enumerate().skip(len) {
        *slot = if i == len {
            PAD
        } else {
            let pos = i + 1;
            let pseudo = ((149 * pos) % 253) + 1;
            let t = 129 + pseudo;
            (if t > 254 { t - 254 } else { t }) as u8
        };
    }
}

/// Fill `full[data_cw..]` with the interleaved Reed–Solomon error codewords of the
/// (padded) data codewords `full[..data_cw]`.
fn add_error_correction(full: &mut [u8], spec: &SymbolSpec) {
    let bc = spec.blocks;
    let epb = spec.ec_per_block();
    let (data, ec_stream) = full.split_at_mut(spec.data_cw);
    let mut ecc = [0u8; MAX_EC_PER_BLOCK];
    for b in 0..bc {
        gf::encode_into(data.iter().skip(b).step_by(bc).copied(), &mut ecc[..epb]);
        for (e, &val) in ecc[..epb].iter().enumerate() {
            ec_stream[spec.ec_slot(b) + e * bc] = val;
        }
    }
}

/// The data codewords (before padding) of `segments` in a symbol of `fill` data
/// codewords, for the decoder's tests.
#[cfg(all(test, feature = "decode"))]
pub(super) fn codewords_of(
    segments: &[Segment],
    encodations: &[Encodation],
    fill: usize,
) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; fill];
    let pieces = encodations
        .iter()
        .zip(segments)
        .map(|(&encodation, s)| Piece {
            encodation,
            mode: s.mode,
            data: &s.data,
        });
    let written =
        write_codewords(pieces, &mut buf, Some(fill)).map_err(|e| e.into_error("does not fit"))?;
    let spec = SymbolSpec {
        data_cw: fill,
        ..LARGEST
    };
    let n = written.settle(&mut buf, &spec)?;
    buf.truncate(n);
    Ok(buf)
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    fn codewords<'s>(segs: impl Iterator<Item = Piece<'s>>) -> Vec<u8> {
        let mut buf = vec![0u8; LARGEST.data_cw];
        let Ok(Written { len: n, .. }) = write_codewords(segs, &mut buf, None) else {
            panic!("codewords do not fit");
        };
        buf.truncate(n);
        buf
    }

    /// The codewords of byte segments in a symbol of `fill` data codewords.
    fn filled(segs: &[(Encodation, &[u8])], fill: usize) -> Vec<u8> {
        let mut buf = vec![0u8; fill];
        let pieces = segs.iter().map(|&(e, d)| Piece::byte(e, d));
        let Ok(Written { len: n, .. }) = write_codewords(pieces, &mut buf, Some(fill)) else {
            panic!("codewords do not fit");
        };
        buf.truncate(n);
        buf
    }

    /// ISO/IEC 16022 worked example: "123456" ASCII-encodes to three digit-pair
    /// codewords 142, 164, 186 which exactly fill the 10×10 symbol's data capacity.
    #[test]
    fn iso_example_data_codewords() {
        assert_eq!(codewords(canonical_runs(b"123456")), vec![142, 164, 186]);
    }

    #[test]
    fn base256_length_field_and_latch() {
        let cw = codewords([Piece::byte(Encodation::Base256, &[0x80u8, 0x81])].into_iter());
        // latch + 1-byte length + 2 randomized payload bytes.
        assert_eq!(cw.len(), 4);
        assert_eq!(cw[0], BASE256_LATCH);
    }

    #[test]
    fn canonical_runs_split() {
        let runs: Vec<_> = canonical_runs(&[b'A', 0x80, 0x81, b'B'])
            .map(|p| (p.encodation, p.data))
            .collect();
        assert_eq!(
            runs,
            vec![
                (Encodation::Ascii, &b"A"[..]),
                (Encodation::Base256, &[0x80, 0x81][..]),
                (Encodation::Ascii, &b"B"[..]),
            ]
        );
        let empty: Vec<_> = canonical_runs(b"")
            .map(|p| (p.encodation, p.data))
            .collect();
        assert_eq!(empty, vec![(Encodation::Ascii, &b""[..])]);
    }

    /// ISO/IEC 16022 worked examples: "AIMAIMAIM" in C40 is the latch and three
    /// times the pair 91, 11; the same letters in lower case are that in Text.
    #[test]
    fn c40_and_text_triplets() {
        use Encodation::{C40, Text};
        let aim = [91, 11, 91, 11, 91, 11];
        // The unlatch is dropped only where the symbol ends.
        assert_eq!(
            filled(&[(C40, b"AIMAIMAIM")], 7),
            [&[230][..], &aim].concat()
        );
        assert_eq!(
            filled(&[(C40, b"AIMAIMAIM")], 8),
            [&[230][..], &aim, &[254]].concat()
        );
        assert_eq!(
            filled(&[(Text, b"aimaimaim")], 12),
            [&[239][..], &aim, &[254]].concat()
        );
        // One codeword left that carries data needs no unlatch either.
        assert_eq!(
            filled(&[(C40, b"AIM"), (Encodation::Ascii, b"x")], 4),
            [230, 91, 11, b'x' + 1]
        );
        assert_eq!(
            filled(&[(C40, b"AIM"), (Encodation::Ascii, b"x")], 5),
            [230, 91, 11, 254, b'x' + 1]
        );
        // Two values are padded with a Shift 1; one cannot be.
        assert_eq!(
            filled(&[(C40, b"AI")], 8),
            [
                230,
                ((1600 * 14 + 40 * 22 + 1) >> 8) as u8,
                ((1600 * 14 + 40 * 22 + 1) & 255) as u8,
                254
            ]
        );
        let mut buf = [0u8; 8];
        let lone = [Piece::byte(C40, b"AIMA")].into_iter();
        assert!(matches!(
            write_codewords(lone, &mut buf, Some(8)),
            Err(WriteError::Invalid(_))
        ));
    }

    /// Every byte goes through C40 and Text: basic set, the three shifts, and the
    /// upper shift in front of each for the high half.
    #[test]
    fn c40_values_cover_every_byte() {
        for text in [false, true] {
            for byte in 0..=255u8 {
                let mut vals = [0u8; 4];
                let n = c40_values(text, byte, &mut vals);
                assert!(vals[..n].iter().all(|&v| v < 40), "{byte}");
                let low = if byte >= 128 {
                    assert_eq!(vals[..2], [1, 30], "{byte}");
                    &vals[2..n]
                } else {
                    &vals[..n]
                };
                let basic = matches!(byte & 0x7F, b' ' | b'0'..=b'9')
                    || (if text { b'a'..=b'z' } else { b'A'..=b'Z' }).contains(&(byte & 0x7F));
                assert_eq!(low.len(), if basic { 1 } else { 2 }, "{byte}");
                assert!(basic || low[0] < 3, "{byte}");
            }
        }
    }

    #[test]
    fn x12_and_edifact_groups() {
        use Encodation::{Edifact, X12};
        // ZXing's reference vectors. The last character of the first has a single
        // codeword left to itself, which a reader takes as ASCII without an unlatch.
        assert_eq!(
            filled(&[(X12, b"ABC>ABC123>A"), (Encodation::Ascii, b"B")], 10),
            [238, 89, 233, 14, 192, 100, 207, 44, 31, 67]
        );
        assert_eq!(
            filled(&[(Edifact, b".A.C1.3.DATA.123DATA.123DATA")], 22),
            [
                240, 184, 27, 131, 198, 236, 238, 16, 21, 1, 187, 28, 179, 16, 21, 1, 187, 28, 179,
                16, 21, 1
            ]
        );
        // The unlatch takes the next free 6-bit slot, then zero fill.
        assert_eq!(filled(&[(Edifact, b"")], 8), [240, 31 << 2]);
        assert_eq!(filled(&[(Edifact, b"A")], 8), [240, 1 << 2 | 1, 0xF0]);
        assert_eq!(
            filled(&[(Edifact, b"AB")], 8),
            [240, 1 << 2, 2 << 4 | 7, 0xC0]
        );
        assert_eq!(
            filled(&[(Edifact, b"ABC")], 8),
            [240, 1 << 2, 2 << 4, 3 << 6 | 31]
        );
        assert_eq!(
            filled(&[(Edifact, b"ABCD")], 8),
            [240, 1 << 2, 2 << 4, 3 << 6 | 4, 31 << 2]
        );
        // Within two codewords of the end a reader returns to ASCII by itself.
        assert_eq!(
            filled(&[(Edifact, b"ABCD")], 6),
            [240, 1 << 2, 2 << 4, 3 << 6 | 4]
        );
        // Neither set takes a lower-case letter.
        for set in [X12, Edifact] {
            let mut buf = [0u8; 8];
            let bad = [Piece::byte(set, b"a")].into_iter();
            assert!(matches!(
                write_codewords(bad, &mut buf, Some(8)),
                Err(WriteError::Invalid(_))
            ));
        }
    }

    #[test]
    fn eci_field_widths() {
        let eci = |n| {
            let piece = Piece {
                encodation: Encodation::Ascii,
                mode: Mode::Eci(n),
                data: &[],
            };
            codewords([piece].into_iter())
        };
        assert_eq!(eci(0), [241, 1]);
        assert_eq!(eci(126), [241, 127]);
        assert_eq!(eci(127), [241, 128, 1]);
        assert_eq!(eci(16382), [241, 191, 254]);
        assert_eq!(eci(16383), [241, 192, 1, 1]);
        assert_eq!(eci(999_999), [241, 207, 63, 129]);
    }
}
