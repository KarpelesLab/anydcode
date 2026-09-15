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

use super::Encodation;
use super::gf::{self, MAX_EC_PER_BLOCK};
use super::placement::{QUIET_ZONE, draw_borders, mapping_to_symbol, occupancy_bytes, place};
use super::tables::{LARGEST_SQUARE, SquareSpec, smallest_square_for, square_by_size};
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

/// ASCII codeword: pad / end-of-message.
const PAD: u8 = 129;
/// ASCII codeword: latch to Base256 encodation.
const BASE256_LATCH: u8 = 231;
/// ASCII codeword: upper shift (the next ASCII value gets 128 added).
const UPPER_SHIFT: u8 = 235;

/// Scratch bytes needed to render `spec`: every codeword (data + EC) followed by the
/// placement occupancy bitmap.
const fn scratch_len_for(spec: &SquareSpec) -> usize {
    let ms = spec.mapping_size();
    spec.total_cw() + occupancy_bytes(ms, ms)
}

/// Data Matrix encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataMatrixEncoder;

impl DataMatrixEncoder {
    /// Scratch bytes sufficient for any symbol size (the 144×144 symbol):
    /// its 2178 codewords plus a 2178-byte placement occupancy bitmap.
    pub const MAX_SCRATCH_LEN: usize = scratch_len_for(&LARGEST_SQUARE);

    /// Storage bytes sufficient for any symbol size (a bit-packed 144×144 grid).
    pub const MAX_STORAGE_LEN: usize =
        MatrixBuf::bytes_for(LARGEST_SQUARE.symbol_size, LARGEST_SQUARE.symbol_size);

    /// A new encoder.
    pub fn new() -> Self {
        DataMatrixEncoder
    }

    /// Scratch bytes [`DataMatrixEncoder::encode_into`] needs for a square symbol of
    /// side `symbol_size` (total codewords plus the placement occupancy bitmap), or
    /// `None` if `symbol_size` is not a Data Matrix square size. Never exceeds
    /// [`DataMatrixEncoder::MAX_SCRATCH_LEN`].
    pub const fn scratch_len(symbol_size: usize) -> Option<usize> {
        match square_by_size(symbol_size) {
            Some(spec) => Some(scratch_len_for(&spec)),
            None => None,
        }
    }

    /// Storage bytes the output [`MatrixBuf`] needs for a square symbol of side
    /// `symbol_size`, or `None` if it is not a Data Matrix square size. Never exceeds
    /// [`DataMatrixEncoder::MAX_STORAGE_LEN`].
    pub const fn storage_len(symbol_size: usize) -> Option<usize> {
        match square_by_size(symbol_size) {
            Some(spec) => Some(MatrixBuf::bytes_for(spec.symbol_size, spec.symbol_size)),
            None => None,
        }
    }

    /// Heap-free rendering of a fully specified symbol: the byte-mode `segments`, each
    /// encoded with the parallel entry of `encodations`, into a `symbol_size ×
    /// symbol_size` symbol. This is exactly what [`Encode`](crate::traits::Encode)
    /// does with a [`Symbol`](crate::Symbol) and its
    /// [`DataMatrixMeta`](super::DataMatrixMeta).
    ///
    /// `scratch` holds the codewords and placement state (at least
    /// [`DataMatrixEncoder::scratch_len`] bytes); `storage` receives the module bits
    /// (at least [`DataMatrixEncoder::storage_len`] bytes). The returned grid has the
    /// 1-module Data Matrix quiet zone.
    ///
    /// # Errors
    /// - [`Error::InvalidParameter`] for an unknown size or mismatched `encodations`;
    /// - [`Error::Unsupported`] for a non-byte segment;
    /// - [`Error::Capacity`] if the data does not fit the symbol or a buffer is too
    ///   small.
    pub fn encode_into<'a>(
        &self,
        segments: &[SegmentView<'_>],
        encodations: &[Encodation],
        symbol_size: usize,
        scratch: &mut [u8],
        storage: &'a mut [u8],
    ) -> Result<MatrixBuf<'a>> {
        if encodations.len() != segments.len() {
            return Err(Error::invalid_parameter(
                "encodation count does not match segment count",
            ));
        }
        check_modes(segments.iter().map(|s| s.mode))?;
        let spec = spec_for(symbol_size)?;
        render_into(
            encodations
                .iter()
                .copied()
                .zip(segments.iter().map(|s| s.data)),
            Some(spec),
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
            Some(sz) => Some(spec_for(sz)?),
            None => None,
        };
        render_into(canonical_runs(data), forced, scratch, storage)
    }
}

#[cfg(feature = "alloc")]
impl DataMatrixEncoder {
    /// Build a reproducible [`Symbol`] from raw `data`, choosing a canonical
    /// ASCII/Base256 segmentation and the smallest fitting square symbol size.
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        self.build_inner(data, None)
    }

    /// Like [`DataMatrixEncoder::build`] but forcing a specific square symbol size
    /// (full side length in modules, e.g. `20` for a 20×20 symbol).
    pub fn build_sized(&self, data: &[u8], symbol_size: usize) -> Result<Symbol> {
        self.build_inner(data, Some(symbol_size))
    }

    /// Convenience: build from UTF-8 `text`.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        self.build(text.as_bytes())
    }

    fn build_inner(&self, data: &[u8], forced: Option<usize>) -> Result<Symbol> {
        let forced = forced.map(spec_for).transpose()?;
        let limit = forced.map_or(LARGEST_SQUARE.data_cw, |s| s.data_cw);
        let mut cw = vec![0u8; limit];
        let n = write_codewords(canonical_runs(data), &mut cw).ok_or(Error::capacity(
            if forced.is_some() {
                "data does not fit in the requested size"
            } else {
                "data exceeds the largest square Data Matrix"
            },
        ))?;
        let spec = match forced {
            Some(s) => s,
            None => smallest_square_for(n).expect("codeword count within the largest size"),
        };
        let (encodations, segments): (Vec<Encodation>, Vec<Segment>) = canonical_runs(data)
            .map(|(e, b)| (e, Segment::byte(b.to_vec())))
            .unzip();
        let meta = DataMatrixMeta {
            symbol_size: spec.symbol_size,
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
        let spec = spec_for(meta.symbol_size)?;

        let mut scratch = vec![0u8; scratch_len_for(&spec)];
        let mut storage = vec![0u8; MatrixBuf::bytes_for(spec.symbol_size, spec.symbol_size)];
        let segs = meta
            .encodations
            .iter()
            .copied()
            .zip(symbol.segments.iter().map(|s| s.data.as_slice()));
        let buf = render_into(segs, Some(spec), &mut scratch, &mut storage)?;
        Ok(Encoding::Matrix(BitMatrix::from(&buf)))
    }
}

/// The square spec for `symbol_size`, or an `InvalidParameter` error.
fn spec_for(symbol_size: usize) -> Result<SquareSpec> {
    square_by_size(symbol_size)
        .ok_or_else(|| Error::invalid_parameter("unknown Data Matrix square size"))
}

/// Data Matrix segments carry raw bytes only.
fn check_modes(mut modes: impl Iterator<Item = Mode>) -> Result<()> {
    if modes.all(|m| matches!(m, Mode::Byte)) {
        Ok(())
    } else {
        Err(Error::Unsupported {
            what: "Data Matrix supports only Byte-mode segments",
        })
    }
}

/// Split `data` into a canonical alternating sequence of ASCII (bytes `< 128`) and
/// Base256 (bytes `>= 128`) runs. This is exactly the segmentation the decoder
/// reconstructs, keeping the round-trip lossless. Empty data yields one empty ASCII
/// run.
fn canonical_runs(data: &[u8]) -> impl Iterator<Item = (Encodation, &[u8])> + '_ {
    let mut rest = data;
    let mut first = true;
    core::iter::from_fn(move || {
        if rest.is_empty() {
            if core::mem::take(&mut first) {
                return Some((Encodation::Ascii, rest));
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
        Some((enc, run))
    })
}

/// Render `segs` into a symbol of the `forced` size (or the smallest that fits),
/// using `scratch` for codewords and placement state and `storage` for the modules.
fn render_into<'s, 'a>(
    segs: impl Iterator<Item = (Encodation, &'s [u8])>,
    forced: Option<SquareSpec>,
    scratch: &mut [u8],
    storage: &'a mut [u8],
) -> Result<MatrixBuf<'a>> {
    if let Some(spec) = forced
        && scratch.len() < scratch_len_for(&spec)
    {
        return Err(Error::capacity("Data Matrix scratch buffer too small"));
    }
    let limit = forced.map_or(LARGEST_SQUARE.data_cw, |s| s.data_cw);
    let avail = limit.min(scratch.len());
    let Some(n) = write_codewords(segs, &mut scratch[..avail]) else {
        return Err(Error::capacity(if avail < limit {
            "Data Matrix scratch buffer too small"
        } else if forced.is_some() {
            "data does not fit in the symbol size"
        } else {
            "data exceeds the largest square Data Matrix"
        }));
    };
    let spec = match forced {
        Some(s) => s,
        None => smallest_square_for(n).expect("codeword count within the largest size"),
    };
    if scratch.len() < scratch_len_for(&spec) {
        return Err(Error::capacity("Data Matrix scratch buffer too small"));
    }
    let size = spec.symbol_size;
    let mut m = MatrixBuf::new(storage, size, size, QUIET_ZONE)?;

    let (cw, occupied) = scratch.split_at_mut(spec.total_cw());
    pad_codewords(&mut cw[..spec.data_cw], n);
    add_error_correction(cw, &spec);

    let ms = spec.mapping_size();
    let (placed, fixed) = place(ms, ms, occupied, |c, bit, r, col| {
        if (cw[c] >> (7 - bit)) & 1 != 0 {
            let (x, y) = mapping_to_symbol(&spec, r, col);
            m.set(x, y, true);
        }
    });
    debug_assert_eq!(placed, spec.total_cw());
    if fixed {
        for d in 1..=2 {
            let (x, y) = mapping_to_symbol(&spec, ms - d, ms - d);
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

/// Encode all segments into `out` as the data codeword stream (before padding),
/// returning its length, or `None` if it does not fit.
fn write_codewords<'s>(
    segs: impl Iterator<Item = (Encodation, &'s [u8])>,
    out: &mut [u8],
) -> Option<usize> {
    let mut cw = Codewords { buf: out, len: 0 };
    for (enc, bytes) in segs {
        match enc {
            Encodation::Ascii => ascii_encode(&mut cw, bytes)?,
            Encodation::Base256 => base256_encode(&mut cw, bytes)?,
        }
    }
    Some(cw.len)
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
fn base256_encode(cw: &mut Codewords<'_>, bytes: &[u8]) -> Option<()> {
    cw.push(BASE256_LATCH)?;
    let count = bytes.len();
    let mut field = [0u8; 2];
    let field = if count <= 249 {
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
fn add_error_correction(full: &mut [u8], spec: &SquareSpec) {
    let bc = spec.blocks;
    let epb = spec.ec_per_block();
    let (data, ec_stream) = full.split_at_mut(spec.data_cw);
    let mut ecc = [0u8; MAX_EC_PER_BLOCK];
    for b in 0..bc {
        gf::encode_into(data.iter().skip(b).step_by(bc).copied(), &mut ecc[..epb]);
        for (e, &val) in ecc[..epb].iter().enumerate() {
            ec_stream[b + e * bc] = val;
        }
    }
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    fn codewords<'s>(segs: impl Iterator<Item = (Encodation, &'s [u8])>) -> Vec<u8> {
        let mut buf = vec![0u8; LARGEST_SQUARE.data_cw];
        let n = write_codewords(segs, &mut buf).unwrap();
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
        let cw = codewords([(Encodation::Base256, &[0x80u8, 0x81][..])].into_iter());
        // latch + 1-byte length + 2 randomized payload bytes.
        assert_eq!(cw.len(), 4);
        assert_eq!(cw[0], BASE256_LATCH);
    }

    #[test]
    fn canonical_runs_split() {
        let runs: Vec<_> = canonical_runs(&[b'A', 0x80, 0x81, b'B']).collect();
        assert_eq!(
            runs,
            vec![
                (Encodation::Ascii, &b"A"[..]),
                (Encodation::Base256, &[0x80, 0x81][..]),
                (Encodation::Ascii, &b"B"[..]),
            ]
        );
        let empty: Vec<_> = canonical_runs(b"").collect();
        assert_eq!(empty, vec![(Encodation::Ascii, &b""[..])]);
    }
}
