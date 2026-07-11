//! DotCode structural decoding: [`BitMatrix`] → [`Symbol`].
//!
//! The decoder reverses the checkerboard fold to recover the dot bit stream, reads
//! the 2-bit mask header and the 9-bit codeword patterns, un-applies the mask,
//! verifies the Reed–Solomon check words, and reconstructs the payload
//! [`Segment`]s. The recovered (unmasked) data codewords plus geometry and mask are
//! stored in [`DotCodeMeta`] so re-encoding is byte-for-byte identical.
//!
//! Corner-forcing (masks 4–7) is not assumed on decode — consistent with the
//! encoder, which never forces corners (see [`super`]).

use super::encode::{
    BIN_LATCH, FNC1, FNC2, FNC3, LATCH_A, LATCH_B_FROM_A, LATCH_BC, UPPER_SHIFT_A, UPPER_SHIFT_B,
    is_corner,
};
use super::rs::rsencode;
use super::tables::codeword_for_pattern;
use super::{DotCodeMeta, MAX_SIZE, MIN_SIZE};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// DotCode structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DotCodeDecoder;

impl DotCodeDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        DotCodeDecoder
    }

    /// Decode a clean DotCode dot matrix into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let width = matrix.width();
        let height = matrix.height();
        if !(MIN_SIZE..=MAX_SIZE).contains(&width) || !(MIN_SIZE..=MAX_SIZE).contains(&height) {
            return Err(Error::undecodable("DotCode symbol size out of range"));
        }
        if (width + height) % 2 != 1 {
            return Err(Error::undecodable("DotCode width+height must be odd"));
        }

        // Dark-module grid (row-major).
        let mut grid = vec![false; width * height];
        for y in 0..height {
            for x in 0..width {
                grid[y * width + x] = matrix.get(x, y);
            }
        }

        let stream = unfold_dotstream(&grid, width, height);
        if stream.len() < 2 + 9 {
            return Err(Error::undecodable("DotCode symbol too small to hold data"));
        }

        // Mask header (2 bits).
        let mask = ((stream[0] as u8) << 1) | (stream[1] as u8);

        // Read 9-bit codeword patterns until an invalid pattern (pad region).
        let mut codewords = Vec::new();
        let mut pos = 2;
        while pos + 9 <= stream.len() {
            let mut pat = 0u16;
            for k in 0..9 {
                pat = (pat << 1) | stream[pos + k] as u16;
            }
            match codeword_for_pattern(pat) {
                Some(cw) => {
                    codewords.push(cw);
                    pos += 9;
                }
                None => break,
            }
        }

        // The stream holds `mask codeword` + data + ECC codewords.
        // Split off the leading mask codeword.
        if codewords.is_empty() {
            return Err(Error::undecodable("DotCode has no codewords"));
        }
        let total = codewords.len(); // = data_length + ecc_length (mask codeword is header bits)

        let data_length = data_length_for(total)
            .ok_or_else(|| Error::undecodable("DotCode codeword count is inconsistent"))?;
        let ecc_length = total - data_length;

        let masked_data = &codewords[..data_length];
        let ecc = &codewords[data_length..];

        // Verify Reed–Solomon check words over [mask, masked_data].
        let mut block = Vec::with_capacity(data_length + 1 + ecc_length);
        block.push(mask);
        block.extend_from_slice(masked_data);
        block.extend(std::iter::repeat_n(0u8, ecc_length));
        rsencode(data_length + 1, ecc_length, &mut block);
        if &block[data_length + 1..] != ecc {
            return Err(Error::ErrorCorrectionFailed);
        }

        // Un-apply the mask to recover the original data codewords.
        let data = unmask(masked_data, mask);

        let segments = codewords_to_segments(&data);

        let meta = DotCodeMeta {
            width,
            height,
            mask,
            codewords: data,
        };
        Ok(Symbol::new(
            Symbology::DotCode,
            segments,
            SymbolMeta::DotCode(meta),
        ))
    }
}

impl Decode for DotCodeDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "DotCode decode of a linear pattern",
            }),
        }
    }
}

/// Unique data-codeword count for a total of `total` (data + ECC) codewords, where
/// `ecc = 3 + data/2`. Returns `None` if no consistent split exists.
fn data_length_for(total: usize) -> Option<usize> {
    (1..=total).find(|&dl| dl + 3 + dl / 2 == total)
}

const MASK_WEIGHTS: [u8; 4] = [0, 3, 7, 17];

fn unmask(masked_data: &[u8], mask: u8) -> Vec<u8> {
    let step = MASK_WEIGHTS[mask as usize] as u16;
    masked_data
        .iter()
        .enumerate()
        .map(|(j, &m)| {
            let weight = (step * j as u16) % 113;
            ((m as u16 + 113 - weight) % 113) as u8
        })
        .collect()
}

/// Reverse of `fold_dotstream`: recover the padded dot bit stream from the grid.
fn unfold_dotstream(grid: &[bool], width: usize, height: usize) -> Vec<bool> {
    let n_dots = (height * width) / 2;
    let mut stream = vec![false; n_dots];
    let mut pos = 0usize;

    if height & 1 == 1 {
        for row in 0..height {
            for column in 0..width {
                if (column + row) & 1 == 0 && !is_corner(column, row, width, height) {
                    stream[pos] = grid[(height - row - 1) * width + column];
                    pos += 1;
                }
            }
        }
        for &idx in &[
            width - 2,
            height * width - 2,
            width * 2 - 1,
            (height - 1) * width - 1,
            0,
            (height - 1) * width,
        ] {
            stream[pos] = grid[idx];
            pos += 1;
        }
    } else {
        for column in 0..width {
            for row in 0..height {
                if (column + row) & 1 == 0 && !is_corner(column, row, width, height) {
                    stream[pos] = grid[row * width + column];
                    pos += 1;
                }
            }
        }
        for &idx in &[
            (height - 1) * width - 1,
            (height - 2) * width,
            height * width - 2,
            (height - 1) * width + 1,
            width - 1,
            0,
        ] {
            stream[pos] = grid[idx];
            pos += 1;
        }
    }
    stream
}

// ---------------------------------------------------------------------------
// Codewords → payload segments (reverse of the Annex F state machine)
// ---------------------------------------------------------------------------

/// GS1 group-separator byte emitted for a non-leading FNC1.
const GS: u8 = 0x1D;

/// Reconstruct payload [`Segment`]s from unmasked data codewords.
///
/// Produces `Byte` segments (and `Eci` control segments). This is a best-effort,
/// human-readable reconstruction; the lossless round-trip is guaranteed by the
/// codewords stored in [`DotCodeMeta`], not by these segments.
fn codewords_to_segments(cw: &[u8]) -> Vec<Segment> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut segments: Vec<Segment> = Vec::new();
    let mut mode = b'C';
    let mut i = 0usize;

    // Skip a leading reader-init (FNC3) or leading FNC1 (twodigits marker).
    if i < cw.len() && (cw[i] == FNC3 || cw[i] == FNC1) {
        i += 1;
    }

    let flush = |bytes: &mut Vec<u8>, segments: &mut Vec<Segment>| {
        if !bytes.is_empty() {
            segments.push(Segment::byte(std::mem::take(bytes)));
        }
    };

    while i < cw.len() {
        let v = cw[i];
        match mode {
            b'C' => {
                if v < 100 {
                    bytes.push(b'0' + v / 10);
                    bytes.push(b'0' + v % 10);
                    i += 1;
                } else if v == 100 {
                    // seventeen-ten: "17" + 3 pairs + "10"
                    bytes.push(b'1');
                    bytes.push(b'7');
                    for k in 0..3 {
                        if let Some(&p) = cw.get(i + 1 + k) {
                            bytes.push(b'0' + p / 10);
                            bytes.push(b'0' + p % 10);
                        }
                    }
                    bytes.push(b'1');
                    bytes.push(b'0');
                    i += 4;
                } else if v == LATCH_A {
                    mode = b'A';
                    i += 1;
                } else if (102..=105).contains(&v) {
                    // nx Shift B
                    let nx = (v - 101) as usize;
                    i += 1;
                    for _ in 0..nx {
                        if let Some(&b) = cw.get(i) {
                            emit_b(&mut bytes, b);
                            i += 1;
                        }
                    }
                } else if v == LATCH_BC {
                    mode = b'B';
                    i += 1;
                } else if v == FNC1 {
                    bytes.push(GS);
                    i += 1;
                } else if v == FNC2 {
                    i = read_eci(cw, i + 1, &mut bytes, &mut segments);
                } else if v == UPPER_SHIFT_A {
                    if let Some(&b) = cw.get(i + 1) {
                        bytes.push(b + 64);
                    }
                    i += 2;
                } else if v == UPPER_SHIFT_B {
                    if let Some(&b) = cw.get(i + 1) {
                        bytes.push(b + 160);
                    }
                    i += 2;
                } else if v == BIN_LATCH {
                    i += 1;
                    i = decode_binary(cw, i, &mut bytes, &mut segments, &mut mode);
                } else {
                    // FNC3 pad / unknown: no output
                    i += 1;
                }
            }
            b'B' => {
                if v <= 95 {
                    bytes.push(v + 32);
                    i += 1;
                } else if v == 96 {
                    bytes.push(13);
                    bytes.push(10);
                    i += 1;
                } else if (97..=100).contains(&v) {
                    bytes.push(match v {
                        97 => 9,
                        98 => 28,
                        99 => 29,
                        _ => 30,
                    });
                    i += 1;
                } else if v == LATCH_A {
                    // Shift A: one A char, stay in B
                    if let Some(&b) = cw.get(i + 1) {
                        emit_a(&mut bytes, b);
                    }
                    i += 2;
                } else if v == LATCH_B_FROM_A {
                    mode = b'A';
                    i += 1;
                } else if (103..=105).contains(&v) {
                    // nx Shift C
                    let nx = (v - 101) as usize;
                    i += 1;
                    for _ in 0..nx {
                        if let Some(&p) = cw.get(i) {
                            bytes.push(b'0' + p / 10);
                            bytes.push(b'0' + p % 10);
                            i += 1;
                        }
                    }
                } else if v == LATCH_BC {
                    mode = b'C';
                    i += 1;
                } else if v == FNC1 {
                    bytes.push(GS);
                    i += 1;
                } else if v == FNC2 {
                    i = read_eci(cw, i + 1, &mut bytes, &mut segments);
                } else if v == UPPER_SHIFT_A {
                    if let Some(&b) = cw.get(i + 1) {
                        bytes.push(b + 64);
                    }
                    i += 2;
                } else if v == UPPER_SHIFT_B {
                    if let Some(&b) = cw.get(i + 1) {
                        bytes.push(b + 160);
                    }
                    i += 2;
                } else if v == BIN_LATCH {
                    i += 1;
                    i = decode_binary(cw, i, &mut bytes, &mut segments, &mut mode);
                } else {
                    i += 1;
                }
            }
            b'A' => {
                if v <= 95 {
                    emit_a(&mut bytes, v);
                    i += 1;
                } else if (96..=101).contains(&v) {
                    // nx Shift B
                    let nx = (v - 95) as usize;
                    i += 1;
                    for _ in 0..nx {
                        if let Some(&b) = cw.get(i) {
                            emit_b(&mut bytes, b);
                            i += 1;
                        }
                    }
                } else if v == LATCH_B_FROM_A {
                    mode = b'B';
                    i += 1;
                } else if (103..=105).contains(&v) {
                    let nx = (v - 101) as usize;
                    i += 1;
                    for _ in 0..nx {
                        if let Some(&p) = cw.get(i) {
                            bytes.push(b'0' + p / 10);
                            bytes.push(b'0' + p % 10);
                            i += 1;
                        }
                    }
                } else if v == LATCH_BC {
                    mode = b'C';
                    i += 1;
                } else if v == FNC1 {
                    bytes.push(GS);
                    i += 1;
                } else if v == FNC2 {
                    i = read_eci(cw, i + 1, &mut bytes, &mut segments);
                } else if v == UPPER_SHIFT_A {
                    if let Some(&b) = cw.get(i + 1) {
                        bytes.push(b + 64);
                    }
                    i += 2;
                } else if v == UPPER_SHIFT_B {
                    if let Some(&b) = cw.get(i + 1) {
                        bytes.push(b + 160);
                    }
                    i += 2;
                } else if v == BIN_LATCH {
                    i += 1;
                    i = decode_binary(cw, i, &mut bytes, &mut segments, &mut mode);
                } else {
                    i += 1;
                }
            }
            _ => unreachable!("mode X handled inside decode_binary"),
        }
    }

    flush(&mut bytes, &mut segments);
    segments
}

/// Emit one Code Set A character value into `bytes`.
fn emit_a(bytes: &mut Vec<u8>, v: u8) {
    if v < 64 {
        bytes.push(v + 32);
    } else {
        bytes.push(v - 64);
    }
}

/// Emit one Code Set B character value into `bytes`.
fn emit_b(bytes: &mut Vec<u8>, v: u8) {
    if v <= 95 {
        bytes.push(v + 32);
    } else if v == 96 {
        bytes.push(13);
        bytes.push(10);
    } else {
        bytes.push(match v {
            97 => 9,
            98 => 28,
            99 => 29,
            _ => 30,
        });
    }
}

/// Read a non-binary ECI value starting at `cw[start]` (after FNC2). Emits an
/// [`Segment::eci`] (flushing any pending bytes) and returns the next index.
fn read_eci(cw: &[u8], start: usize, bytes: &mut Vec<u8>, segments: &mut Vec<Segment>) -> usize {
    let (eci, next) = match cw.get(start) {
        Some(&first) if first < 40 => (first as u32, start + 1),
        Some(&a) => {
            let b = cw.get(start + 1).copied().unwrap_or(0) as u32;
            let c = cw.get(start + 2).copied().unwrap_or(0) as u32;
            ((a as u32 - 40) * 12769 + b * 113 + c + 40, start + 3)
        }
        None => return start,
    };
    if !bytes.is_empty() {
        segments.push(Segment::byte(std::mem::take(bytes)));
    }
    segments.push(Segment::eci(eci));
    next
}

/// Decode a binary (mode `X`) run starting at `cw[start]`, emitting bytes/ECIs, and
/// returning the index just past the run (updating `mode` on the terminating latch).
fn decode_binary(
    cw: &[u8],
    start: usize,
    bytes: &mut Vec<u8>,
    segments: &mut Vec<Segment>,
    mode: &mut u8,
) -> usize {
    let mut i = start;
    let mut group: Vec<u8> = Vec::new();

    loop {
        if i >= cw.len() {
            flush_binary(&group, bytes, segments);
            *mode = b'C';
            return i;
        }
        let v = cw[i];
        if v <= 102 {
            group.push(v);
            i += 1;
            continue;
        }
        // Control codeword: flush accumulated binary values first.
        flush_binary(&group, bytes, segments);
        group.clear();

        if (103..=108).contains(&v) {
            // nx Shift C interrupt: n = v - 101 digit pairs, stay in binary.
            let n = (v - 101) as usize;
            i += 1;
            for _ in 0..n {
                if let Some(&p) = cw.get(i) {
                    bytes.push(b'0' + p / 10);
                    bytes.push(b'0' + p % 10);
                    i += 1;
                }
            }
            continue;
        }
        // Terminators.
        i += 1;
        *mode = match v {
            FNC3 => b'A',          // 109
            UPPER_SHIFT_A => b'B', // 110
            _ => b'C',             // 111 latch to C
        };
        return i;
    }
}

/// Decode accumulated base-103 binary codewords into bytes / ECIs.
fn flush_binary(group: &[u8], bytes: &mut Vec<u8>, segments: &mut Vec<Segment>) {
    if group.is_empty() {
        return;
    }
    // Recover the base-259 value stream, group by group (6 codewords → 5 values;
    // a trailing partial of m codewords → m-1 values).
    let mut values: Vec<u16> = Vec::new();
    let mut idx = 0;
    while idx < group.len() {
        let m = (group.len() - idx).min(6);
        if m < 2 {
            break; // Not a valid group (partial groups have >= 2 codewords).
        }
        let mut buf: u64 = 0;
        for &c in &group[idx..idx + m] {
            buf = buf * 103 + c as u64;
        }
        let size = m - 1;
        let mut lsf = Vec::with_capacity(size);
        for _ in 0..size {
            lsf.push((buf % 259) as u16);
            buf /= 259;
        }
        lsf.reverse();
        values.extend(lsf);
        idx += m;
    }

    // Interpret the value stream: 0..255 = byte, 256/257/258 = ECI markers.
    let mut k = 0;
    while k < values.len() {
        let v = values[k];
        match v {
            0..=255 => {
                bytes.push(v as u8);
                k += 1;
            }
            256 => {
                let eci = values.get(k + 1).copied().unwrap_or(0) as u32;
                flush_eci(eci, bytes, segments);
                k += 2;
            }
            257 => {
                let hi = values.get(k + 1).copied().unwrap_or(0) as u32;
                let lo = values.get(k + 2).copied().unwrap_or(0) as u32;
                flush_eci((hi << 8) | lo, bytes, segments);
                k += 3;
            }
            _ => {
                // 258
                let a = values.get(k + 1).copied().unwrap_or(0) as u32;
                let b = values.get(k + 2).copied().unwrap_or(0) as u32;
                let c = values.get(k + 3).copied().unwrap_or(0) as u32;
                flush_eci((a << 16) | (b << 8) | c, bytes, segments);
                k += 4;
            }
        }
    }
}

fn flush_eci(eci: u32, bytes: &mut Vec<u8>, segments: &mut Vec<Segment>) {
    if !bytes.is_empty() {
        segments.push(Segment::byte(std::mem::take(bytes)));
    }
    segments.push(Segment::eci(eci));
}
