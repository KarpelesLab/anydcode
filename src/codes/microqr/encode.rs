//! Micro QR encoding: segments → module grid.
//!
//! The core, [`MicroQrEncoder::encode_into`], allocates nothing: the codewords live
//! on the stack (Micro QR holds at most 24) and the module grid is written to a
//! caller-provided buffer. It can pick the smallest fitting version and the
//! highest-scoring mask itself. With `alloc`, the [`Encode`](crate::traits::Encode)
//! impl reproduces an exact symbol from a fully-specified [`MicroQrMeta`] (the
//! round-trip path) and the `build*` methods make fresh symbols — both on top of
//! that core.

use super::matrix::Canvas;
use super::tables::{
    char_count_bits, data_bit_capacity, ec_params, micro_mode_value, mode_indicator_bits,
    symbol_number, terminator_bits,
};
use super::{MicroEcLevel, MicroMask, MicroQrMeta, MicroVersion};
use crate::codes::qr::gf;
use crate::error::{Error, Result};
use crate::output::MatrixBuf;
#[cfg(feature = "alloc")]
use crate::output::{BitMatrix, Encoding};
use crate::segment::{Mode, SegmentView};
#[cfg(feature = "alloc")]
use crate::segment::{ModeCost, Segment, optimize_segments};
#[cfg(feature = "alloc")]
use crate::symbol::{Symbol, SymbolMeta};
#[cfg(feature = "alloc")]
use crate::symbology::Symbology;
#[cfg(feature = "alloc")]
use crate::traits::Encode;
#[cfg(feature = "alloc")]
use alloc::{vec, vec::Vec};

/// Most data codewords in any Micro QR symbol (M4-L).
const MAX_DATA_CW: usize = 16;
/// Most EC codewords in any Micro QR symbol (M4-Q).
const MAX_EC_CW: usize = 14;

/// Every version, smallest first.
const VERSIONS: [MicroVersion; 4] = [
    MicroVersion::M1,
    MicroVersion::M2,
    MicroVersion::M3,
    MicroVersion::M4,
];

/// Micro QR Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicroQrEncoder;

impl MicroQrEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        MicroQrEncoder
    }

    /// Length of the `encode_into` grid buffer that holds any version (M4).
    pub const MAX_BUFFER_LEN: usize = Canvas::storage_len(MicroVersion::M4);

    /// Length of the `encode_into` grid buffer for `version`.
    pub const fn buffer_len(version: MicroVersion) -> usize {
        Canvas::storage_len(version)
    }

    /// Heap-free encoding of `segments` at error-correction `level` into `storage`.
    ///
    /// `version` and `mask` pin those choices when given; otherwise the smallest
    /// version that fits (and supports `level` and every segment's mode) and the
    /// highest-scoring mask are chosen. `storage` must be at least
    /// [`MicroQrEncoder::buffer_len`] bytes for the resulting version
    /// ([`MicroQrEncoder::MAX_BUFFER_LEN`] always suffices). Returns the grid
    /// together with the pinned parameters.
    ///
    /// ```
    /// use anyd::codes::microqr::{MicroEcLevel, MicroQrEncoder};
    /// use anyd::segment::SegmentView;
    ///
    /// let mut storage = [0u8; MicroQrEncoder::MAX_BUFFER_LEN];
    /// let segments = [SegmentView::numeric(b"01234567")];
    /// let (grid, meta) = MicroQrEncoder::new()
    ///     .encode_into(&segments, MicroEcLevel::L, None, None, &mut storage)
    ///     .unwrap();
    /// assert_eq!((grid.width(), meta.version.number()), (13, 2));
    /// ```
    ///
    /// # Errors
    /// [`Error::Capacity`] when the data does not fit or `storage` is too short;
    /// [`Error::InvalidParameter`] for a version that does not support `level`;
    /// [`Error::InvalidData`] for bytes or modes a version cannot represent.
    pub fn encode_into<'a>(
        &self,
        segments: &[SegmentView<'_>],
        level: MicroEcLevel,
        version: Option<MicroVersion>,
        mask: Option<MicroMask>,
        storage: &'a mut [u8],
    ) -> Result<(MatrixBuf<'a>, MicroQrMeta)> {
        let version = match version {
            Some(v) => v,
            None => choose_version(segments, level)?,
        };
        if storage.len() < Self::buffer_len(version) {
            return Err(Error::capacity(
                "Micro QR encode buffer too small for version",
            ));
        }
        let sn = symbol_number(version, level).ok_or_else(|| {
            Error::invalid_parameter("unsupported Micro QR version/EC combination")
        })?;
        let (codewords, data_bits, total_bits) = codewords(segments, version, level)?;

        let mut canvas = Canvas::new(version, storage)?;
        for (i, (x, y)) in canvas.data_path().take(total_bits).enumerate() {
            // Data bits come first (M1/M3 place only the final codeword's high
            // nibble), then the EC codewords.
            let bit = if i < data_bits {
                i
            } else {
                MAX_DATA_CW * 8 + (i - data_bits)
            };
            if (codewords[bit / 8] >> (7 - bit % 8)) & 1 != 0 {
                canvas.place_data_bit(x, y, true);
            }
        }

        let mask = match mask {
            Some(m) => m,
            None => {
                // Score every mask in place, undoing each (masking is an involution);
                // ties resolve to the lowest index.
                let mut best = (MicroMask(0), None);
                for m in 0..4 {
                    let candidate = MicroMask(m);
                    canvas.apply_mask(candidate);
                    canvas.place_format(sn, candidate);
                    let score = canvas.evaluate();
                    if best.1.is_none_or(|s| score > s) {
                        best = (candidate, Some(score));
                    }
                    canvas.apply_mask(candidate);
                }
                best.0
            }
        };
        canvas.apply_mask(mask);
        canvas.place_format(sn, mask);
        let meta = MicroQrMeta {
            version,
            ec_level: level,
            mask,
        };
        Ok((canvas.into_grid(), meta))
    }

    /// Heap-free convenience: encode `text` as a single numeric, alphanumeric or byte
    /// segment (the densest mode that holds every byte), at the smallest fitting
    /// version with the highest-scoring mask. Buffer as for
    /// [`MicroQrEncoder::encode_into`].
    ///
    /// Unlike [`MicroQrEncoder::build_text`] this never splits the text into
    /// mixed-mode segments, so mixed payloads may need a larger version.
    pub fn encode_text_into<'a>(
        &self,
        text: &[u8],
        level: MicroEcLevel,
        storage: &'a mut [u8],
    ) -> Result<(MatrixBuf<'a>, MicroQrMeta)> {
        let mode = if !text.is_empty() && text.iter().all(u8::is_ascii_digit) {
            Mode::Numeric
        } else if !text.is_empty() && text.iter().all(|&b| alnum_value(b).is_some()) {
            Mode::Alphanumeric
        } else {
            Mode::Byte
        };
        let segment = SegmentView { mode, data: text };
        self.encode_into(&[segment], level, None, None, storage)
    }
}

#[cfg(feature = "alloc")]
impl MicroQrEncoder {
    /// Build a reproducible [`Symbol`] from `segments` at error-correction `level`,
    /// choosing the smallest fitting version and the highest-scoring mask. `level`
    /// must be supported by the chosen version (use [`MicroEcLevel::Detection`] to
    /// permit the tiny M1 numeric-only symbol).
    pub fn build(&self, segments: Vec<Segment>, level: MicroEcLevel) -> Result<Symbol> {
        let views: Vec<SegmentView<'_>> = segments.iter().map(Segment::view).collect();
        let mut storage = [0u8; Self::MAX_BUFFER_LEN];
        let (_, meta) = self.encode_into(&views, level, None, None, &mut storage)?;
        Ok(Symbol::new(
            Symbology::MicroQrCode,
            segments,
            SymbolMeta::MicroQr(meta),
        ))
    }

    /// Convenience: build a numeric M1-friendly / higher symbol from ASCII digits.
    pub fn build_numeric(&self, digits: &[u8], level: MicroEcLevel) -> Result<Symbol> {
        self.build(vec![Segment::numeric(digits.to_vec())], level)
    }

    /// Convenience: build a symbol from UTF-8 `text`, splitting it into the
    /// cheapest mix of the modes each version permits (M1: numeric only;
    /// M2: +alphanumeric; M3/M4: +byte). Kanji mode is not generated.
    pub fn build_text(&self, text: &str, level: MicroEcLevel) -> Result<Symbol> {
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            return self.build(vec![Segment::byte(Vec::new())], level);
        }
        // Mode availability and count-field widths differ per version, so
        // optimize per version, smallest first, and take the first fit.
        for version in VERSIONS {
            let Some(cap) = data_bit_capacity(version, level) else {
                continue; // level not supported at this version
            };
            let Some(segs) = optimize_segments(bytes, &mode_costs(version)) else {
                continue; // some byte not representable at this version
            };
            let views: Vec<SegmentView<'_>> = segs.iter().map(Segment::view).collect();
            if let Some(len) = segments_bit_len(&views, version)
                && len <= cap
            {
                return self.build(segs, level);
            }
        }
        Err(Error::capacity(
            "data does not fit any Micro QR version at this EC level",
        ))
    }
}

/// Segmenter cost model for a version: only the modes it permits.
#[cfg(feature = "alloc")]
fn mode_costs(version: MicroVersion) -> Vec<ModeCost> {
    fn is_digit(b: u8) -> bool {
        b.is_ascii_digit()
    }
    fn is_alnum(b: u8) -> bool {
        alnum_value(b).is_some()
    }
    fn any(_: u8) -> bool {
        true
    }
    let mib = mode_indicator_bits(version) as u32;
    [
        (Mode::Numeric, 20, is_digit as fn(u8) -> bool),
        (Mode::Alphanumeric, 33, is_alnum),
        (Mode::Byte, 48, any),
    ]
    .into_iter()
    .filter_map(|(mode, char_cost_sixths, accepts)| {
        let ccb = char_count_bits(version, &mode)? as u32;
        Some(ModeCost {
            mode,
            head_bits: mib + ccb,
            tail_bits: 0,
            char_cost_sixths,
            accepts,
        })
    })
    .collect()
}

#[cfg(feature = "alloc")]
impl Encode for MicroQrEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::MicroQrCode {
            return Err(Error::invalid_parameter(
                "MicroQrEncoder given a non-Micro-QR symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::MicroQr(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Micro QR symbol missing MicroQrMeta",
                ));
            }
        };
        let views: Vec<SegmentView<'_>> = symbol.segments.iter().map(Segment::view).collect();
        let mut storage = [0u8; Self::MAX_BUFFER_LEN];
        let (grid, _) = self.encode_into(
            &views,
            meta.ec_level,
            Some(meta.version),
            Some(meta.mask),
            &mut storage,
        )?;
        Ok(Encoding::Matrix(BitMatrix::from(&grid)))
    }
}

/// A most-significant-bit-first bit stream destination.
trait BitSink {
    fn push(&mut self, value: u32, len: usize);
}

/// Counts bits without storing them (for capacity checks).
struct BitCounter(usize);

impl BitSink for BitCounter {
    fn push(&mut self, _value: u32, len: usize) {
        self.0 += len;
    }
}

/// Writes bits into a zeroed byte array; bits past its end are counted but dropped,
/// so callers check [`BitWriter::bits`] against the capacity afterwards.
struct BitWriter<'s> {
    bytes: &'s mut [u8],
    bits: usize,
}

impl BitSink for BitWriter<'_> {
    fn push(&mut self, value: u32, len: usize) {
        for k in (0..len).rev() {
            if (value >> k) & 1 != 0
                && let Some(byte) = self.bytes.get_mut(self.bits / 8)
            {
                *byte |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }
}

/// The alphanumeric value of a character, or `None` if outside the set.
fn alnum_value(b: u8) -> Option<u32> {
    let v = match b {
        b'0'..=b'9' => b - b'0',
        b'A'..=b'Z' => b - b'A' + 10,
        b' ' => 36,
        b'$' => 37,
        b'%' => 38,
        b'*' => 39,
        b'+' => 40,
        b'-' => 41,
        b'.' => 42,
        b'/' => 43,
        b':' => 44,
        _ => return None,
    };
    Some(v as u32)
}

/// Convert a Shift-JIS double byte to the 13-bit QR Kanji value, if in range.
fn kanji_value(hi: u8, lo: u8) -> Option<u32> {
    let code = ((hi as u32) << 8) | lo as u32;
    let base = if (0x8140..=0x9FFC).contains(&code) {
        code - 0x8140
    } else if (0xE040..=0xEBBF).contains(&code) {
        code - 0xC140
    } else {
        return None;
    };
    Some((base >> 8) * 0xC0 + (base & 0xFF))
}

/// Write one segment's header and payload bits.
fn write_segment(w: &mut impl BitSink, seg: &SegmentView<'_>, version: MicroVersion) -> Result<()> {
    let mib = mode_indicator_bits(version);
    if mib > 0 {
        let mv = micro_mode_value(&seg.mode)
            .ok_or_else(|| Error::invalid_data("ECI unsupported in Micro QR"))?;
        w.push(mv as u32, mib);
    }
    let ccb = char_count_bits(version, &seg.mode).ok_or_else(|| {
        Error::invalid_data("segment mode not permitted at this Micro QR version")
    })?;
    match seg.mode {
        Mode::Numeric => {
            w.push(seg.data.len() as u32, ccb);
            for chunk in seg.data.chunks(3) {
                let mut val = 0u32;
                for &d in chunk {
                    if !d.is_ascii_digit() {
                        return Err(Error::invalid_data("non-digit in numeric segment"));
                    }
                    val = val * 10 + (d - b'0') as u32;
                }
                let bits = match chunk.len() {
                    3 => 10,
                    2 => 7,
                    _ => 4,
                };
                w.push(val, bits);
            }
        }
        Mode::Alphanumeric => {
            w.push(seg.data.len() as u32, ccb);
            for pair in seg.data.chunks(2) {
                if pair.len() == 2 {
                    let a = alnum_value(pair[0])
                        .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                    let b = alnum_value(pair[1])
                        .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                    w.push(a * 45 + b, 11);
                } else {
                    let a = alnum_value(pair[0])
                        .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                    w.push(a, 6);
                }
            }
        }
        Mode::Byte => {
            w.push(seg.data.len() as u32, ccb);
            for &b in seg.data {
                w.push(b as u32, 8);
            }
        }
        Mode::Kanji => {
            if !seg.data.len().is_multiple_of(2) {
                return Err(Error::invalid_data("odd-length kanji segment"));
            }
            w.push((seg.data.len() / 2) as u32, ccb);
            for pair in seg.data.chunks(2) {
                let v = kanji_value(pair[0], pair[1])
                    .ok_or_else(|| Error::invalid_data("bad kanji char"))?;
                w.push(v, 13);
            }
        }
        Mode::Eci(_) => return Err(Error::invalid_data("ECI unsupported in Micro QR")),
    }
    Ok(())
}

/// Total encoded bit length of all segments at a version, or `None` if any segment's
/// mode is not representable at that version.
fn segments_bit_len(segments: &[SegmentView<'_>], version: MicroVersion) -> Option<usize> {
    let mut counter = BitCounter(0);
    for seg in segments {
        write_segment(&mut counter, seg, version).ok()?;
    }
    Some(counter.0)
}

/// The smallest version (at `level`) whose data capacity holds `segments`.
fn choose_version(segments: &[SegmentView<'_>], level: MicroEcLevel) -> Result<MicroVersion> {
    for v in VERSIONS {
        let Some(cap) = data_bit_capacity(v, level) else {
            continue; // level not supported at this version
        };
        if let Some(len) = segments_bit_len(segments, v)
            && len <= cap
        {
            return Ok(v);
        }
    }
    Err(Error::capacity(
        "data does not fit any Micro QR version at this EC level",
    ))
}

/// Build the data and EC codewords for a version/level: data codewords at the
/// front of the returned array, EC codewords from byte [`MAX_DATA_CW`]. Also returns
/// how many leading data bits the symbol places (M1/M3 end on a 4-bit codeword)
/// and the total placed bit count.
fn codewords(
    segments: &[SegmentView<'_>],
    version: MicroVersion,
    ec: MicroEcLevel,
) -> Result<([u8; MAX_DATA_CW + MAX_EC_CW], usize, usize)> {
    let params = ec_params(version, ec)
        .ok_or_else(|| Error::invalid_parameter("unsupported Micro QR version/EC combination"))?;
    let cap_bits = data_bit_capacity(version, ec).unwrap_or(0);

    let mut out = [0u8; MAX_DATA_CW + MAX_EC_CW];
    let (data, ec_bytes) = out.split_at_mut(MAX_DATA_CW);
    let data = &mut data[..params.data_cw];
    let mut w = BitWriter {
        bytes: data,
        bits: 0,
    };
    for seg in segments {
        write_segment(&mut w, seg, version)?;
    }
    if w.bits > cap_bits {
        return Err(Error::capacity(
            "segments exceed selected Micro QR capacity",
        ));
    }
    // Terminator, bounded by remaining capacity (the array is already zeroed).
    let used = w.bits + (cap_bits - w.bits).min(terminator_bits(version));
    // Pad to a byte boundary, then with the alternating 0xEC / 0x11 codewords. The
    // 4-bit final codeword of M1/M3 is not a pad position: it stays `0000`.
    let full_cw = cap_bits / 8;
    for (i, byte) in data[..full_cw]
        .iter_mut()
        .skip(used.div_ceil(8))
        .enumerate()
    {
        *byte = if i % 2 == 0 { 0xEC } else { 0x11 };
    }

    gf::encode_into(data, &mut ec_bytes[..params.ec_cw]);
    Ok((out, cap_bits, cap_bits + params.ec_cw * 8))
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    /// M2-M numeric "01234567". The 4 data codewords are derived by hand from the
    /// ISO/IEC 18004 bit layout (mode `0`, count `1000`, then 10/10/7-bit digit
    /// groups → 0x40,0x18,0xAC,0xC3); the 6 EC codewords come from the shared QR
    /// Reed–Solomon field.
    #[test]
    fn m2_example_codewords() {
        let segments = [SegmentView::numeric(b"01234567")];
        let (cw, data_bits, total_bits) =
            codewords(&segments, MicroVersion::M2, MicroEcLevel::M).unwrap();
        assert_eq!((data_bits, total_bits), (32, 80));
        let expected_data = [64, 24, 172, 195];
        let expected_ec = [211, 226, 194, 57, 150, 107];
        assert_eq!(cw[..4], expected_data);
        assert_eq!(cw[MAX_DATA_CW..MAX_DATA_CW + 6], expected_ec);
    }

    /// ISO/IEC 18004 §7.4.10: M1/M3 pad with the alternating 0xEC / 0x11 codewords
    /// like every other version; only their final 4-bit codeword is `0000`. M3-L
    /// numeric "1" is mode `00`, count `00001`, digit `0001`, the 7-bit terminator
    /// and padding bits (3 codewords), then 7 pad codewords and the zero nibble.
    #[test]
    fn m3_pad_codewords() {
        let segments = [SegmentView::numeric(b"1")];
        let (cw, data_bits, _) = codewords(&segments, MicroVersion::M3, MicroEcLevel::L).unwrap();
        assert_eq!(data_bits, 84);
        let expected = [
            0x02, 0x20, 0x00, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x00,
        ];
        assert_eq!(cw[..11], expected);
        // Content running into the final nibble leaves no room for pad codewords.
        let segments = [SegmentView::numeric(b"12345678901234567890123")];
        let (cw, _, _) = codewords(&segments, MicroVersion::M3, MicroEcLevel::L).unwrap();
        assert_eq!(cw[10] & 0x0F, 0);
    }
}
