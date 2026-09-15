//! QR encoding: segments → module grid.
//!
//! The core, [`QrEncoder::encode_into`], allocates nothing: it builds the codeword
//! stream and the module grid in two caller-provided buffers, and can pick the
//! smallest fitting version and the lowest-penalty mask itself. With `alloc`, the
//! [`Encode`](crate::traits::Encode) impl reproduces an exact symbol from a
//! fully-specified [`QrMeta`] (the round-trip path) and the `build*` methods make
//! fresh symbols — both on top of that core.

use super::gf;
use super::matrix::Canvas;
use super::tables::{EcBlocks, char_count_bits, ec_blocks, mode_indicator};
use super::{EcLevel, Mask, QrMeta, Version};
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

/// Longest Reed–Solomon EC block in any QR version.
const MAX_EC_PER_BLOCK: usize = 30;

/// QR Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct QrEncoder;

impl QrEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        QrEncoder
    }

    /// Length both `encode_into` buffers need to hold any version (version 40).
    pub const MAX_BUFFER_LEN: usize = Canvas::storage_len(Version(40));

    /// Length both `encode_into` buffers need for `version`: the packed
    /// `size × size` module grid, which is also enough for every codeword.
    pub const fn buffer_len(version: Version) -> usize {
        Canvas::storage_len(version)
    }

    /// Heap-free encoding of `segments` at error-correction `level`.
    ///
    /// `version` and `mask` pin those choices when given; otherwise the smallest
    /// version that fits and the lowest-penalty mask are chosen. `scratch` holds the
    /// codeword stream while `storage` receives the module grid; both must be at
    /// least [`QrEncoder::buffer_len`] bytes for the resulting version
    /// ([`QrEncoder::MAX_BUFFER_LEN`] always suffices). Returns the grid together
    /// with the pinned parameters.
    ///
    /// ```
    /// use anyd::codes::qr::{EcLevel, QrEncoder};
    /// use anyd::segment::SegmentView;
    ///
    /// let mut scratch = [0u8; QrEncoder::MAX_BUFFER_LEN];
    /// let mut storage = [0u8; QrEncoder::MAX_BUFFER_LEN];
    /// let segments = [SegmentView::alphanumeric(b"HELLO WORLD")];
    /// let (grid, meta) = QrEncoder::new()
    ///     .encode_into(&segments, EcLevel::Q, None, None, &mut scratch, &mut storage)
    ///     .unwrap();
    /// assert_eq!((grid.width(), meta.version.number()), (21, 1));
    /// ```
    ///
    /// # Errors
    /// [`Error::Capacity`] when the data does not fit (the pinned version, or any
    /// version) or a buffer is too short; [`Error::InvalidData`] for bytes a segment's
    /// mode cannot represent.
    pub fn encode_into<'a>(
        &self,
        segments: &[SegmentView<'_>],
        level: EcLevel,
        version: Option<Version>,
        mask: Option<Mask>,
        scratch: &mut [u8],
        storage: &'a mut [u8],
    ) -> Result<(MatrixBuf<'a>, QrMeta)> {
        let version = match version {
            Some(v) => v,
            None => choose_version(segments, level)?,
        };
        let ecb = ec_blocks(version, level);
        let total = ecb.total_codewords();
        if scratch.len() < total || storage.len() < Self::buffer_len(version) {
            return Err(Error::capacity("QR encode buffer too small for version"));
        }

        // The grid storage doubles as the data-codeword workspace (it is larger than
        // the codeword stream); the interleaved stream lands in `scratch`.
        codewords_into(segments, version, &ecb, storage, &mut scratch[..total])?;
        let codewords = &scratch[..total];

        let mut canvas = Canvas::new(version, storage)?;
        for (i, (x, y)) in canvas.data_path().take(total * 8).enumerate() {
            if (codewords[i / 8] >> (7 - i % 8)) & 1 != 0 {
                canvas.place_data_bit(x, y, true);
            }
        }
        canvas.place_version();

        let mask = match mask {
            Some(m) => m,
            None => {
                // Score every mask in place, undoing each (masking is an involution).
                let mut best = (Mask(0), u32::MAX);
                for m in 0..8 {
                    let candidate = Mask(m);
                    canvas.apply_mask(candidate);
                    canvas.place_format(level, candidate);
                    let penalty = canvas.penalty();
                    if penalty < best.1 {
                        best = (candidate, penalty);
                    }
                    canvas.apply_mask(candidate);
                }
                best.0
            }
        };
        canvas.apply_mask(mask);
        canvas.place_format(level, mask);
        let meta = QrMeta {
            version,
            ec_level: level,
            mask,
        };
        Ok((canvas.into_grid(), meta))
    }

    /// Heap-free convenience: encode `text` as a single numeric, alphanumeric or byte
    /// segment (the densest mode that holds every byte), at the smallest fitting
    /// version with the lowest-penalty mask. Buffers as for
    /// [`QrEncoder::encode_into`].
    ///
    /// Unlike [`QrEncoder::build_text`] this never splits the text into mixed-mode
    /// segments, so long mixed payloads may need a larger version.
    pub fn encode_text_into<'a>(
        &self,
        text: &[u8],
        level: EcLevel,
        scratch: &mut [u8],
        storage: &'a mut [u8],
    ) -> Result<(MatrixBuf<'a>, QrMeta)> {
        let mode = if !text.is_empty() && text.iter().all(u8::is_ascii_digit) {
            Mode::Numeric
        } else if !text.is_empty() && text.iter().all(|&b| alnum_value(b).is_some()) {
            Mode::Alphanumeric
        } else {
            Mode::Byte
        };
        let segment = SegmentView { mode, data: text };
        self.encode_into(&[segment], level, None, None, scratch, storage)
    }
}

#[cfg(feature = "alloc")]
impl QrEncoder {
    /// Build a reproducible [`Symbol`] from `segments` at error-correction `level`,
    /// choosing the smallest fitting version and the lowest-penalty mask. The
    /// returned symbol's [`QrMeta`] pins those choices so re-encoding is identical.
    pub fn build(&self, segments: Vec<Segment>, level: EcLevel) -> Result<Symbol> {
        let views: Vec<SegmentView<'_>> = segments.iter().map(Segment::view).collect();
        let version = choose_version(&views, level)?;
        let meta = self.render(&views, level, version, None)?.1;
        Ok(Symbol::new(
            Symbology::QrCode,
            segments,
            SymbolMeta::Qr(meta),
        ))
    }

    /// Convenience: build a symbol from UTF-8 `text`, splitting it into the
    /// cheapest mix of numeric / alphanumeric / byte segments (Kanji mode is not
    /// generated — byte mode carries UTF-8 losslessly).
    pub fn build_text(&self, text: &str, level: EcLevel) -> Result<Symbol> {
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            return self.build(vec![Segment::byte(Vec::new())], level);
        }
        // Count-field widths change across the three version bands, so optimize
        // per band and keep whichever segmentation reaches the smallest version.
        let mut best: Option<(Vec<Segment>, u8)> = None;
        for group in 0..3 {
            let Some(segs) = optimize_segments(bytes, &mode_costs(group)) else {
                continue;
            };
            let views: Vec<SegmentView<'_>> = segs.iter().map(Segment::view).collect();
            if let Ok(v) = choose_version(&views, level)
                && best.as_ref().is_none_or(|&(_, bv)| v.number() < bv)
            {
                best = Some((segs, v.number()));
            }
        }
        let (segs, _) = best.ok_or_else(|| {
            Error::capacity("data does not fit in any QR version at this EC level")
        })?;
        self.build(segs, level)
    }

    /// Run [`QrEncoder::encode_into`] over freshly allocated buffers.
    fn render(
        &self,
        segments: &[SegmentView<'_>],
        level: EcLevel,
        version: Version,
        mask: Option<Mask>,
    ) -> Result<(BitMatrix, QrMeta)> {
        let len = Self::buffer_len(version);
        let (mut scratch, mut storage) = (vec![0u8; len], vec![0u8; len]);
        let (grid, meta) = self.encode_into(
            segments,
            level,
            Some(version),
            mask,
            &mut scratch,
            &mut storage,
        )?;
        Ok((BitMatrix::from(&grid), meta))
    }
}

/// Segmenter cost model for a QR version band (0: v1–9, 1: v10–26, 2: v27–40).
#[cfg(feature = "alloc")]
fn mode_costs(group: usize) -> [ModeCost; 3] {
    fn is_digit(b: u8) -> bool {
        b.is_ascii_digit()
    }
    fn is_alnum(b: u8) -> bool {
        alnum_value(b).is_some()
    }
    fn any(_: u8) -> bool {
        true
    }
    [
        ModeCost {
            mode: Mode::Numeric,
            head_bits: 4 + [10, 12, 14][group],
            tail_bits: 0,
            char_cost_sixths: 20,
            accepts: is_digit,
        },
        ModeCost {
            mode: Mode::Alphanumeric,
            head_bits: 4 + [9, 11, 13][group],
            tail_bits: 0,
            char_cost_sixths: 33,
            accepts: is_alnum,
        },
        ModeCost {
            mode: Mode::Byte,
            head_bits: 4 + [8, 16, 16][group],
            tail_bits: 0,
            char_cost_sixths: 48,
            accepts: any,
        },
    ]
}

#[cfg(feature = "alloc")]
impl Encode for QrEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::QrCode {
            return Err(Error::invalid_parameter("QrEncoder given a non-QR symbol"));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Qr(m) => m,
            _ => return Err(Error::invalid_parameter("QR symbol missing QrMeta")),
        };
        let views: Vec<SegmentView<'_>> = symbol.segments.iter().map(Segment::view).collect();
        let (matrix, _) = self.render(&views, meta.ec_level, meta.version, Some(meta.mask))?;
        Ok(Encoding::Matrix(matrix))
    }
}

/// A most-significant-bit-first bit stream destination.
trait BitSink {
    fn push(&mut self, value: u32, len: usize) -> Result<()>;
}

/// Counts bits without storing them (for capacity checks).
struct BitCounter(usize);

impl BitSink for BitCounter {
    fn push(&mut self, _value: u32, len: usize) -> Result<()> {
        self.0 += len;
        Ok(())
    }
}

/// Writes bits into a zeroed byte slice, failing once it is full.
struct BitWriter<'s> {
    bytes: &'s mut [u8],
    bits: usize,
}

impl BitSink for BitWriter<'_> {
    fn push(&mut self, value: u32, len: usize) -> Result<()> {
        if self.bits + len > self.bytes.len() * 8 {
            return Err(Error::capacity("segments exceed selected version capacity"));
        }
        for k in (0..len).rev() {
            if (value >> k) & 1 != 0 {
                self.bytes[self.bits / 8] |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
        Ok(())
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

/// Encode the ECI assignment number using the variable-length QR encoding.
fn write_eci_assignment(w: &mut impl BitSink, n: u32) -> Result<()> {
    if n < 128 {
        w.push(n, 8)
    } else if n < 16384 {
        w.push(0b10 << 14 | n, 16)
    } else if n < 1_000_000 {
        w.push(0b110 << 21 | n, 24)
    } else {
        Err(Error::invalid_data("ECI assignment out of range"))
    }
}

/// Write one segment's header and payload bits.
fn write_segment(w: &mut impl BitSink, seg: &SegmentView<'_>, version: Version) -> Result<()> {
    // Mode indicator (4 bits) for every segment including ECI.
    w.push(mode_indicator(&seg.mode) as u32, 4)?;
    match seg.mode {
        Mode::Eci(n) => {
            write_eci_assignment(w, n)?;
            // ECI carries no payload of its own; the following segment does.
        }
        Mode::Numeric => {
            let n = seg.data.len();
            w.push(n as u32, char_count_bits(version, &seg.mode))?;
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
                w.push(val, bits)?;
            }
        }
        Mode::Alphanumeric => {
            w.push(seg.data.len() as u32, char_count_bits(version, &seg.mode))?;
            for pair in seg.data.chunks(2) {
                let a = alnum_value(pair[0])
                    .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                if let Some(&second) = pair.get(1) {
                    let b = alnum_value(second)
                        .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                    w.push(a * 45 + b, 11)?;
                } else {
                    w.push(a, 6)?;
                }
            }
        }
        Mode::Byte => {
            w.push(seg.data.len() as u32, char_count_bits(version, &seg.mode))?;
            for &b in seg.data {
                w.push(b as u32, 8)?;
            }
        }
        Mode::Kanji => {
            if !seg.data.len().is_multiple_of(2) {
                return Err(Error::invalid_data("odd-length kanji segment"));
            }
            w.push(
                (seg.data.len() / 2) as u32,
                char_count_bits(version, &seg.mode),
            )?;
            for pair in seg.data.chunks(2) {
                let v = kanji_value(pair[0], pair[1])
                    .ok_or_else(|| Error::invalid_data("bad kanji char"))?;
                w.push(v, 13)?;
            }
        }
    }
    Ok(())
}

/// Total encoded bit length of all segments at a version (headers + payload).
fn segments_bit_len(segments: &[SegmentView<'_>], version: Version) -> Result<usize> {
    let mut counter = BitCounter(0);
    for seg in segments {
        write_segment(&mut counter, seg, version)?;
    }
    Ok(counter.0)
}

/// The smallest version (at `level`) whose data capacity holds `segments`.
fn choose_version(segments: &[SegmentView<'_>], level: EcLevel) -> Result<Version> {
    for v in 1..=40 {
        let version = Version(v);
        let capacity = ec_blocks(version, level).total_data() * 8;
        // char-count sizes change across version bands; recompute per version.
        if let Ok(len) = segments_bit_len(segments, version)
            && len <= capacity
        {
            return Ok(version);
        }
    }
    Err(Error::capacity(
        "data does not fit in any QR version at this EC level",
    ))
}

/// Build the full interleaved codeword stream (data + EC) for a version into `out`
/// (`ecb.total_codewords()` bytes), using `work` (at least `ecb.total_data()`
/// bytes) for the data codewords.
fn codewords_into(
    segments: &[SegmentView<'_>],
    version: Version,
    ecb: &EcBlocks,
    work: &mut [u8],
    out: &mut [u8],
) -> Result<()> {
    let total_data = ecb.total_data();
    let data = &mut work[..total_data];
    data.fill(0);

    let mut w = BitWriter {
        bytes: data,
        bits: 0,
    };
    for seg in segments {
        write_segment(&mut w, seg, version)?;
    }
    // Terminator: up to 4 zero bits bounded by the capacity, then pad to a byte
    // boundary (the buffer is already zeroed, so both only advance the cursor).
    let data_bits = total_data * 8;
    let used = (w.bits + (data_bits - w.bits).min(4)).div_ceil(8);
    // Pad codewords with the alternating 0xEC / 0x11 bytes.
    for (i, byte) in data[used..].iter_mut().enumerate() {
        *byte = if i % 2 == 0 { 0xEC } else { 0x11 };
    }

    // Interleave data codewords column by column across the blocks.
    let blocks = ecb.total_blocks();
    let block = |bi: usize| -> &[u8] {
        let (start, len) = if bi < ecb.group1_blocks {
            (bi * ecb.group1_data, ecb.group1_data)
        } else {
            let g2 = bi - ecb.group1_blocks;
            (
                ecb.group1_blocks * ecb.group1_data + g2 * ecb.group2_data,
                ecb.group2_data,
            )
        };
        &data[start..start + len]
    };
    let mut k = 0;
    for col in 0..ecb.group1_data.max(ecb.group2_data) {
        for bi in 0..blocks {
            if let Some(&cw) = block(bi).get(col) {
                out[k] = cw;
                k += 1;
            }
        }
    }
    // Then each block's EC codewords, interleaved the same way.
    let ec_len = ecb.ec_per_block;
    let mut ec = [0u8; MAX_EC_PER_BLOCK];
    for bi in 0..blocks {
        gf::encode_into(block(bi), &mut ec[..ec_len]);
        for (col, &cw) in ec[..ec_len].iter().enumerate() {
            out[total_data + col * blocks + bi] = cw;
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    /// The full ISO/IEC 18004 worked example: numeric "01234567" at V1-M produces
    /// exactly these 16 data + 10 EC codewords. This validates mode encoding,
    /// character-count sizing, terminator and pad bytes, and RS end to end.
    #[test]
    fn iso_example_full_codewords() {
        let segments = [SegmentView::numeric(b"01234567")];
        let version = Version::new(1).unwrap();
        let ecb = ec_blocks(version, EcLevel::M);
        let (mut work, mut out) = ([0u8; 26], [0u8; 26]);
        codewords_into(&segments, version, &ecb, &mut work, &mut out).unwrap();
        let expected: [u8; 26] = [
            16, 32, 12, 86, 97, 128, 236, 17, 236, 17, 236, 17, 236, 17, 236, 17, // data
            165, 36, 212, 193, 237, 54, 199, 135, 44, 85, // EC
        ];
        assert_eq!(out, expected);
    }
}
