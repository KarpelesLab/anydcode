//! DotCode encoding: data encodation (Code Set A/B/C + binary), symbol sizing,
//! padding, mask selection and dot placement.
//!
//! The Code Set A/B/C state machine, binary latch/shift handling, symbol sizing,
//! padding and the checkerboard fold are ports of the algorithm in
//! AIM ISS DotCode Rev 4.0 (Annexes A/F), following zint's `backend/dotcode.c`
//! reference implementation. See [`super`] for the lossless-round-trip design.

use super::rs::rsencode;
use super::tables::DC_DOT_PATTERNS;
use super::{DotCodeMeta, MAX_SIZE, MIN_SIZE};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

// Special codeword values (shared by encoder and decoder).
pub(crate) const LATCH_A: u8 = 101;
pub(crate) const LATCH_B_FROM_A: u8 = 102;
pub(crate) const LATCH_BC: u8 = 106; // Latch B (from C) / Latch C (from A/B)
pub(crate) const FNC1: u8 = 107;
pub(crate) const FNC2: u8 = 108;
pub(crate) const FNC3: u8 = 109; // Reader Init, or Bin-terminate-latch-A
pub(crate) const UPPER_SHIFT_A: u8 = 110;
pub(crate) const UPPER_SHIFT_B: u8 = 111;
pub(crate) const BIN_LATCH: u8 = 112;

// ---------------------------------------------------------------------------
// Data encodation state machine (Annex F)
// ---------------------------------------------------------------------------

#[inline]
fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

fn is_twodigits(src: &[u8], pos: usize) -> bool {
    pos + 1 < src.len() && is_digit(src[pos]) && is_digit(src[pos + 1])
}

fn to_int2(src: &[u8], pos: usize) -> u8 {
    (src[pos] - b'0') * 10 + (src[pos + 1] - b'0')
}

/// Consecutive digits ahead of `pos`, capped at `max`.
fn cnt_digits(src: &[u8], pos: usize, max: usize) -> usize {
    let end = (pos + max).min(src.len());
    let mut i = pos;
    while i < end && is_digit(src[i]) {
        i += 1;
    }
    i - pos
}

fn datum_a(src: &[u8], pos: usize) -> bool {
    pos < src.len() && src[pos] <= 95
}

/// 0 = not encodable in B, 1 = single char, 2 = CR/LF pair.
fn datum_b(src: &[u8], pos: usize) -> i32 {
    if pos >= src.len() {
        return 0;
    }
    let c = src[pos];
    if (32..=127).contains(&c) {
        return 1;
    }
    match c {
        9 | 28 | 29 | 30 => return 1, // HT, FS, GS, RS
        _ => {}
    }
    if pos + 1 < src.len() && c == 13 && src[pos + 1] == 10 {
        return 2; // CR/LF
    }
    0
}

fn datum_c(src: &[u8], pos: usize) -> bool {
    is_twodigits(src, pos)
}

fn seventeen_ten(src: &[u8], pos: usize) -> bool {
    pos + 9 < src.len()
        && src[pos] == b'1'
        && src[pos + 1] == b'7'
        && src[pos + 8] == b'1'
        && src[pos + 9] == b'0'
        && cnt_digits(src, pos + 2, 6) >= 6
}

fn ahead_c(src: &[u8], pos: usize) -> usize {
    let mut count = 0;
    let mut i = pos;
    while i < src.len() && datum_c(src, i) {
        count += 1;
        i += 2;
    }
    count
}

fn try_c(src: &[u8], pos: usize) -> usize {
    if pos < src.len() && is_digit(src[pos]) {
        let ahead = ahead_c(src, pos);
        if ahead > ahead_c(src, pos + 1) {
            return ahead;
        }
    }
    0
}

fn ahead_a(src: &[u8], pos: usize) -> usize {
    let mut count = 0;
    let mut i = pos;
    while i < src.len() && datum_a(src, i) && try_c(src, i) < 2 {
        count += 1;
        i += 1;
    }
    count
}

/// Returns (chars consumed, number of codewords `nx`).
fn ahead_b(src: &[u8], pos: usize) -> (usize, usize) {
    let mut count = 0;
    let mut i = pos;
    loop {
        if i >= src.len() {
            break;
        }
        let incr = datum_b(src, i);
        if incr == 0 || try_c(src, i) >= 2 {
            break;
        }
        count += 1;
        i += incr as usize;
    }
    (i - pos, count)
}

fn binary(src: &[u8], pos: usize) -> bool {
    pos < src.len() && src[pos] >= 128
}

/// Binary buffer that packs 5 base-259 values into 6 base-103 codewords.
struct BinBuf {
    buf: u64,
    size: i32,
}

impl BinBuf {
    fn new() -> Self {
        BinBuf { buf: 0, size: 0 }
    }

    fn empty(&mut self, out: &mut Vec<u8>) {
        if self.size != 0 {
            let mut lawrencium = [0i32; 6];
            let mut buf = self.buf;
            let n = self.size as usize + 1;
            for slot in lawrencium.iter_mut().take(n) {
                *slot = (buf % 103) as i32;
                buf /= 103;
            }
            for i in 0..n {
                out.push(lawrencium[self.size as usize - i] as u8);
            }
        }
        self.buf = 0;
        self.size = 0;
    }

    fn append(&mut self, out: &mut Vec<u8>, val: u32) {
        self.buf *= 259;
        self.buf += val as u64;
        self.size += 1;
        if self.size == 5 {
            self.empty(out);
        }
    }
}

/// Encode one data segment into DotCode codewords (before padding / masking / ECC).
///
/// Returns the codeword stream and whether it ends in binary (mode `X`) — a single
/// segment is both the first and last, so macro/EOT handling is applied in full.
pub(crate) fn encode_message(
    source: &[u8],
    eci: u32,
    gs1: bool,
    reader_init: bool,
) -> (Vec<u8>, bool) {
    const LEAD_SPECIALS: [u8; 4] = [9, 28, 29, 30]; // HT, FS, GS, RS
    let length = source.len();
    let mut out: Vec<u8> = Vec::new();
    let mut mode = b'C';
    let mut inside_macro: i32 = 0;
    let mut bin = BinBuf::new();
    let mut position = 0usize;

    let last_eot = length > 0 && source[length - 1] == 4;
    let last_rseot = last_eot && length > 1 && source[length - 2] == 30;

    // ---- First-segment leading handling ----
    if reader_init {
        out.push(FNC3);
    } else if !gs1 && eci == 0 && length >= 2 && is_twodigits(source, 0) {
        out.push(FNC1);
    } else if !source.is_empty() && LEAD_SPECIALS.contains(&source[0]) {
        out.push(LATCH_A);
        out.push(source[0] + 64);
        mode = b'A';
        position += 1;
    } else if length > 5
        && source[0] == b'['
        && source[1] == b')'
        && source[2] == b'>'
        && source[3] == 30
        && last_eot
    {
        let format_050612 = (source[4] == b'0' && (source[5] == b'5' || source[5] == b'6'))
            || (source[4] == b'1' && source[5] == b'2');
        inside_macro = 0;
        if length > 6 && format_050612 && source[6] == 29 && last_rseot {
            inside_macro = match source[5] {
                b'5' => 97,
                b'6' => 98,
                _ => 99,
            };
        } else if !format_050612 && is_twodigits(source, 4) {
            inside_macro = 100;
        }
        if inside_macro != 0 {
            out.push(LATCH_BC); // Latch B
            mode = b'B';
            out.push(inside_macro as u8);
            if inside_macro == 100 {
                out.push((source[4] - b'0') + 16);
                out.push((source[5] - b'0') + 16);
                position += 6;
            } else {
                position += 7;
            }
        }
    }

    // ---- ECI ----
    if eci > 0 {
        if mode == b'X' {
            if eci <= 0xFF {
                bin.append(&mut out, 256);
                bin.append(&mut out, eci);
            } else if eci <= 0xFFFF {
                bin.append(&mut out, 257);
                bin.append(&mut out, eci >> 8);
                bin.append(&mut out, eci & 0xFF);
            } else {
                bin.append(&mut out, 258);
                bin.append(&mut out, eci >> 16);
                bin.append(&mut out, (eci >> 8) & 0xFF);
                bin.append(&mut out, eci & 0xFF);
            }
        } else {
            out.push(FNC2);
            if eci <= 39 {
                out.push(eci as u8);
            } else {
                let a = (eci - 40) / 12769;
                let b = ((eci - 40) - 12769 * a) / 113;
                let c = (eci - 40) - 12769 * a - 113 * b;
                out.push((a + 40) as u8);
                out.push(b as u8);
                out.push(c as u8);
            }
        }
    }

    // ---- Main loop ----
    while position < length {
        // Macro end-of-data steps.
        if inside_macro != 0 && inside_macro != 100 && position + 2 == length {
            position += 2;
            continue;
        }
        if inside_macro == 100 && position + 1 == length {
            position += 1;
            continue;
        }

        match mode {
            b'C' => {
                // Step C2
                if seventeen_ten(source, position) {
                    out.push(100);
                    out.push(to_int2(source, position + 2));
                    out.push(to_int2(source, position + 4));
                    out.push(to_int2(source, position + 6));
                    position += 10;
                    continue;
                }
                if datum_c(source, position) || (gs1 && source[position] == 0x1D) {
                    if source[position] == 0x1D {
                        out.push(FNC1);
                        position += 1;
                    } else {
                        out.push(to_int2(source, position));
                        position += 2;
                    }
                    continue;
                }
                // Step C3
                if binary(source, position) {
                    if position + 1 < length && is_digit(source[position + 1]) {
                        if source[position] - 128 < 32 {
                            out.push(UPPER_SHIFT_A);
                            out.push(source[position] - 128 + 64);
                        } else {
                            out.push(UPPER_SHIFT_B);
                            out.push(source[position] - 128 - 32);
                        }
                        position += 1;
                    } else {
                        out.push(BIN_LATCH);
                        mode = b'X';
                    }
                    continue;
                }
                // Step C4
                let m = ahead_a(source, position);
                let (_, nx) = ahead_b(source, position);
                if m > nx {
                    out.push(LATCH_A);
                    mode = b'A';
                } else if (1..=4).contains(&nx) {
                    out.push(101 + nx as u8); // nx Shift B
                    for _ in 0..nx {
                        position = emit_b_char(source, position, &mut out);
                    }
                } else {
                    out.push(LATCH_BC); // Latch B
                    mode = b'B';
                }
            }
            b'B' => {
                // Step D1
                let n = try_c(source, position);
                if n >= 2 {
                    if n <= 4 {
                        out.push(103 + (n as u8 - 2)); // nx Shift C
                        for _ in 0..n {
                            out.push(to_int2(source, position));
                            position += 2;
                        }
                    } else {
                        out.push(LATCH_BC); // Latch C
                        mode = b'C';
                    }
                    continue;
                }
                // Step D2
                if gs1 && source[position] == 0x1D {
                    out.push(FNC1);
                    position += 1;
                    continue;
                }
                if datum_b(source, position) != 0 {
                    let mut done = false;
                    let c = source[position];
                    if (32..=127).contains(&c) {
                        out.push(c - 32);
                        done = true;
                    } else if c == 13 {
                        out.push(96); // CR/LF
                        position += 1;
                        done = true;
                    } else if position != 0 {
                        // HT/FS/GS/RS in first position would look like a macro.
                        out.push(match c {
                            9 => 97,
                            28 => 98,
                            29 => 99,
                            _ => 100,
                        });
                        done = true;
                    }
                    if done {
                        position += 1;
                        continue;
                    }
                }
                // Step D3
                if binary(source, position) {
                    if datum_b(source, position + 1) != 0 {
                        if source[position] - 128 < 32 {
                            out.push(UPPER_SHIFT_A);
                            out.push(source[position] - 128 + 64);
                        } else {
                            out.push(UPPER_SHIFT_B);
                            out.push(source[position] - 128 - 32);
                        }
                        position += 1;
                    } else {
                        out.push(BIN_LATCH);
                        mode = b'X';
                    }
                    continue;
                }
                // Step D4
                if ahead_a(source, position) == 1 {
                    out.push(LATCH_A); // Shift A
                    if source[position] < 32 {
                        out.push(source[position] + 64);
                    } else {
                        out.push(source[position] - 32);
                    }
                    position += 1;
                } else {
                    out.push(LATCH_B_FROM_A); // Latch A
                    mode = b'A';
                }
            }
            b'A' => {
                // Step E1
                let n = try_c(source, position);
                if n >= 2 {
                    if n <= 4 {
                        out.push(103 + (n as u8 - 2)); // nx Shift C
                        for _ in 0..n {
                            out.push(to_int2(source, position));
                            position += 2;
                        }
                    } else {
                        out.push(LATCH_BC); // Latch C
                        mode = b'C';
                    }
                    continue;
                }
                // Step E2
                if gs1 && source[position] == 0x1D {
                    out.push(FNC1);
                    position += 1;
                    continue;
                }
                if datum_a(source, position) {
                    if source[position] < 32 {
                        out.push(source[position] + 64);
                    } else {
                        out.push(source[position] - 32);
                    }
                    position += 1;
                    continue;
                }
                // Step E3
                if binary(source, position) {
                    if datum_a(source, position + 1) {
                        if source[position] - 128 < 32 {
                            out.push(UPPER_SHIFT_A);
                            out.push(source[position] - 128 + 64);
                        } else {
                            out.push(UPPER_SHIFT_B);
                            out.push(source[position] - 128 - 32);
                        }
                        position += 1;
                    } else {
                        out.push(BIN_LATCH);
                        mode = b'X';
                    }
                    continue;
                }
                // Step E4
                let (_, nx) = ahead_b(source, position);
                if (1..=6).contains(&nx) {
                    out.push(95 + nx as u8); // nx Shift B
                    for _ in 0..nx {
                        position = emit_b_char(source, position, &mut out);
                    }
                } else {
                    out.push(LATCH_B_FROM_A); // Latch B
                    mode = b'B';
                }
            }
            _ => {
                // mode == b'X' (binary)
                // Step F1
                let n = try_c(source, position);
                if n >= 2 {
                    bin.empty(&mut out);
                    if n <= 7 {
                        out.push(101 + n as u8); // Interrupt for nx Shift C
                        for _ in 0..n {
                            out.push(to_int2(source, position));
                            position += 2;
                        }
                    } else {
                        out.push(UPPER_SHIFT_B); // Terminate with Latch to C
                        mode = b'C';
                    }
                    continue;
                }
                // Step F2
                if binary(source, position)
                    || binary(source, position + 1)
                    || binary(source, position + 2)
                    || binary(source, position + 3)
                {
                    bin.append(&mut out, source[position] as u32);
                    position += 1;
                    continue;
                }
                // Step F3
                bin.empty(&mut out);
                if ahead_a(source, position) > ahead_b(source, position).1 {
                    out.push(FNC3); // Terminate with Latch to A
                    mode = b'A';
                } else {
                    out.push(UPPER_SHIFT_A); // Terminate with Latch to B
                    mode = b'B';
                }
            }
        }
    }

    if mode == b'X' && bin.size != 0 {
        bin.empty(&mut out);
    }

    (out, mode == b'X')
}

/// Emit one Code Set B character at `source[pos]`, returning the next position.
fn emit_b_char(source: &[u8], pos: usize, out: &mut Vec<u8>) -> usize {
    let c = source[pos];
    if c >= 32 {
        out.push(c - 32);
        pos + 1
    } else if c == 13 {
        out.push(96); // CR/LF consumes two source bytes
        pos + 2
    } else {
        out.push(match c {
            9 => 97,
            28 => 98,
            29 => 99,
            _ => 100,
        });
        pos + 1
    }
}

// ---------------------------------------------------------------------------
// Symbol sizing (Annex 5.2.2)
// ---------------------------------------------------------------------------

fn min_dots_for(data_length: usize) -> usize {
    9 * (data_length + 3 + data_length / 2) + 2
}

/// Choose (width, height) for `data_length` codewords, optionally honouring a
/// user-requested `width`. Port of zint's automatic and user-width sizing.
pub(crate) fn select_size(data_length: usize, width: Option<usize>) -> Result<(usize, usize)> {
    let min_area = min_dots_for(data_length) * 2;

    let (mut width, mut height) = if let Some(w) = width {
        if !(MIN_SIZE..=MAX_SIZE).contains(&w) {
            return Err(Error::invalid_parameter(
                "DotCode width must be between 5 and 200",
            ));
        }
        let mut h = min_area.div_ceil(w);
        if h < 5 {
            h = 5;
        }
        if (w + h) % 2 == 0 {
            h += 1;
        }
        (w, h)
    } else {
        let h = ((min_area as f64 * 0.666).sqrt()) as f32;
        let w = ((min_area as f64 * 1.5).sqrt()) as f32;
        let mut height = h as usize;
        let mut width = w as usize;
        if (width + height) % 2 == 1 {
            if width * height < min_area {
                width += 1;
                height += 1;
            }
        } else if (h as f64) * (width as f64) < (w as f64) * (height as f64) {
            width += 1;
            if width * height < min_area {
                width -= 1;
                height += 1;
                if width * height < min_area {
                    width += 2;
                }
            }
        } else {
            height += 1;
            if width * height < min_area {
                width += 1;
                height -= 1;
                if width * height < min_area {
                    height += 2;
                }
            }
        }
        (width, height)
    };

    if height > MAX_SIZE || width > MAX_SIZE {
        return Err(Error::capacity(
            "DotCode data too large for a 200x200 symbol",
        ));
    }
    if width < MIN_SIZE {
        width = MIN_SIZE;
    }
    if height < MIN_SIZE {
        height = MIN_SIZE;
    }
    Ok((width, height))
}

/// Append pad codewords (Latch B / binary-terminate) to fill the symbol, mirroring
/// zint. Returns the padded codeword vector.
pub(crate) fn add_padding(
    mut codewords: Vec<u8>,
    width: usize,
    height: usize,
    binary_finish: bool,
) -> Vec<u8> {
    let n_dots = (height * width) / 2;
    let min_dots = min_dots_for(codewords.len());
    let mut padding_dots = n_dots as i64 - min_dots as i64;
    let mut data_length = codewords.len();

    if padding_dots >= 9 {
        let mut is_first = true;
        while padding_dots >= 9 {
            if padding_dots < 18 && (data_length & 1) == 0 {
                padding_dots -= 9;
            } else if padding_dots >= 18 {
                if (data_length & 1) == 0 {
                    padding_dots -= 9;
                } else {
                    padding_dots -= 18;
                }
            } else {
                break;
            }
            if is_first && binary_finish {
                codewords.push(FNC3);
            } else {
                codewords.push(LATCH_BC);
            }
            data_length += 1;
            is_first = false;
        }
    }
    codewords
}

// ---------------------------------------------------------------------------
// Masking, ECC and dot placement
// ---------------------------------------------------------------------------

const MASK_WEIGHTS: [u8; 4] = [0, 3, 7, 17];

/// Build `[mask, masked_data..., ecc...]` from unmasked data codewords.
fn build_masked_block(data: &[u8], mask: u8) -> Vec<u8> {
    let data_length = data.len();
    let ecc_length = 3 + data_length / 2;
    let mut block = Vec::with_capacity(data_length + 1 + ecc_length);
    block.push(mask);
    let step = MASK_WEIGHTS[mask as usize] as u16;
    for (j, &d) in data.iter().enumerate() {
        let weight = (step * j as u16) % 113;
        block.push(((weight + d as u16) % 113) as u8);
    }
    block.extend(std::iter::repeat_n(0u8, ecc_length));
    rsencode(data_length + 1, ecc_length, &mut block);
    block
}

/// Convert the masked codeword block into the dot bit stream (mask = 2 bits, each
/// codeword = 9-bit pattern), MSB first.
fn make_dotstream(block: &[u8]) -> Vec<bool> {
    let mut bits = Vec::new();
    // Mask value as two bits.
    for i in (0..2).rev() {
        bits.push((block[0] >> i) & 1 == 1);
    }
    for &cw in &block[1..] {
        let pat = DC_DOT_PATTERNS[cw as usize];
        for i in (0..9).rev() {
            bits.push((pat >> i) & 1 == 1);
        }
    }
    bits
}

/// Is `(column, row)` a reserved corner dot (holds one of the final six bits)?
pub(crate) fn is_corner(column: usize, row: usize, width: usize, height: usize) -> bool {
    if column == 0 && row == 0 {
        return true;
    }
    if height & 1 == 1 {
        if (column == width - 2 && row == 0) || (column == width - 1 && row == 1) {
            return true;
        }
        if column == 0 && row == height - 1 {
            return true;
        }
    } else {
        if column == width - 1 && row == 0 {
            return true;
        }
        if (column == 0 && row == height - 2) || (column == 1 && row == height - 1) {
            return true;
        }
    }
    (column == width - 2 && row == height - 1) || (column == width - 1 && row == height - 2)
}

/// Fold the padded dot stream onto the checkerboard, returning a dark-module grid
/// (row-major, `height * width`). Non-data (checkerboard-off) cells are light.
pub(crate) fn fold_dotstream(stream: &[bool], width: usize, height: usize) -> Vec<bool> {
    let mut grid = vec![false; width * height];
    let mut pos = 0usize;

    if height & 1 == 1 {
        // Horizontal folding: data cells stored vertically flipped.
        for row in 0..height {
            for column in 0..width {
                if (column + row) & 1 == 0 {
                    if is_corner(column, row, width, height) {
                        // Filled below.
                    } else {
                        grid[(height - row - 1) * width + column] = stream[pos];
                        pos += 1;
                    }
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
            grid[idx] = stream[pos];
            pos += 1;
        }
    } else {
        // Vertical folding: data cells stored in place.
        for column in 0..width {
            for row in 0..height {
                if (column + row) & 1 == 0 {
                    if is_corner(column, row, width, height) {
                        // Filled below.
                    } else {
                        grid[row * width + column] = stream[pos];
                        pos += 1;
                    }
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
            grid[idx] = stream[pos];
            pos += 1;
        }
    }
    grid
}

/// Render a padded codeword set + geometry + mask into the final dark-module grid.
pub(crate) fn render_grid(data: &[u8], width: usize, height: usize, mask: u8) -> Vec<bool> {
    let block = build_masked_block(data, mask);
    let mut stream = make_dotstream(&block);
    let n_dots = (height * width) / 2;
    // Pad the tail with lit dots.
    while stream.len() < n_dots {
        stream.push(true);
    }
    fold_dotstream(&stream, width, height)
}

// ---------------------------------------------------------------------------
// Mask scoring (Annex A)
// ---------------------------------------------------------------------------

const SCORE_UNLIT_EDGE: i64 = -99999;

fn get_dot(grid: &[bool], height: usize, width: usize, x: i64, y: i64) -> bool {
    x >= 0
        && x < width as i64
        && y >= 0
        && y < height as i64
        && grid[(y as usize) * width + x as usize]
}

fn clr_col(grid: &[bool], height: usize, width: usize, x: usize) -> bool {
    let mut y = x & 1;
    while y < height {
        if get_dot(grid, height, width, x as i64, y as i64) {
            return false;
        }
        y += 2;
    }
    true
}

fn clr_row(grid: &[bool], height: usize, width: usize, y: usize) -> bool {
    let mut x = y & 1;
    while x < width {
        if get_dot(grid, height, width, x as i64, y as i64) {
            return false;
        }
        x += 2;
    }
    true
}

fn col_penalty(grid: &[bool], height: usize, width: usize) -> i64 {
    let mut penalty = 0i64;
    let mut local = 0i64;
    for x in 1..width - 1 {
        if clr_col(grid, height, width, x) {
            local = if local == 0 {
                height as i64
            } else {
                local * height as i64
            };
        } else if local != 0 {
            penalty += local;
            local = 0;
        }
    }
    penalty + local
}

fn row_penalty(grid: &[bool], height: usize, width: usize) -> i64 {
    let mut penalty = 0i64;
    let mut local = 0i64;
    for y in 1..height - 1 {
        if clr_row(grid, height, width, y) {
            local = if local == 0 {
                width as i64
            } else {
                local * width as i64
            };
        } else if local != 0 {
            penalty += local;
            local = 0;
        }
    }
    penalty + local
}

/// Port of zint's `dc_score_array()`.
pub(crate) fn score_array(grid: &[bool], height: usize, width: usize) -> i64 {
    let h = height as i64;
    let w = width as i64;
    let penalty = row_penalty(grid, height, width) + col_penalty(grid, height, width);

    // Top edge.
    let (mut sum, mut first, mut last) = (0i64, -1i64, -1i64);
    let mut x = 0i64;
    while x < w {
        if get_dot(grid, height, width, x, 0) {
            if first < 0 {
                first = x;
            }
            last = x;
            sum += 1;
        }
        x += 2;
    }
    if sum == 0 {
        return SCORE_UNLIT_EDGE;
    }
    let mut worstedge = (sum + last - first) * h;

    // Bottom edge.
    let (mut sum, mut first, mut last) = (0i64, -1i64, -1i64);
    let mut x = (width as i64) & 1;
    while x < w {
        if get_dot(grid, height, width, x, h - 1) {
            if first < 0 {
                first = x;
            }
            last = x;
            sum += 1;
        }
        x += 2;
    }
    if sum == 0 {
        return SCORE_UNLIT_EDGE;
    }
    let edge = (sum + last - first) * h;
    if edge < worstedge {
        worstedge = edge;
    }

    // Left edge.
    let (mut sum, mut first, mut last) = (0i64, -1i64, -1i64);
    let mut y = 0i64;
    while y < h {
        if get_dot(grid, height, width, 0, y) {
            if first < 0 {
                first = y;
            }
            last = y;
            sum += 1;
        }
        y += 2;
    }
    if sum == 0 {
        return SCORE_UNLIT_EDGE;
    }
    let edge = (sum + last - first) * w;
    if edge < worstedge {
        worstedge = edge;
    }

    // Right edge.
    let (mut sum, mut first, mut last) = (0i64, -1i64, -1i64);
    let mut y = (height as i64) & 1;
    while y < h {
        if get_dot(grid, height, width, w - 1, y) {
            if first < 0 {
                first = y;
            }
            last = y;
            sum += 1;
        }
        y += 2;
    }
    if sum == 0 {
        return SCORE_UNLIT_EDGE;
    }
    let edge = (sum + last - first) * w;
    if edge < worstedge {
        worstedge = edge;
    }

    // Isolated-dot / cross-pattern penalties.
    let mut sum = 0i64;
    for y in 0..height as i64 {
        let mut x = y & 1;
        while x < w {
            if !get_dot(grid, height, width, x - 1, y - 1)
                && !get_dot(grid, height, width, x + 1, y - 1)
                && !get_dot(grid, height, width, x - 1, y + 1)
                && !get_dot(grid, height, width, x + 1, y + 1)
                && (!get_dot(grid, height, width, x, y)
                    || (!get_dot(grid, height, width, x - 2, y)
                        && !get_dot(grid, height, width, x, y - 2)
                        && !get_dot(grid, height, width, x + 2, y)
                        && !get_dot(grid, height, width, x, y + 2)))
            {
                sum += 1;
            }
            x += 2;
        }
    }

    worstedge - sum * sum - penalty
}

/// Select the best mask in `0..=3` for the padded data, matching zint's tie-break
/// (highest index wins on equal score). Corner-forcing (masks 4–7) is intentionally
/// not applied — see [`super`].
pub(crate) fn select_mask(data: &[u8], width: usize, height: usize) -> u8 {
    let mut best_mask = 0u8;
    let mut high = i64::MIN;
    for m in 0..4u8 {
        let grid = render_grid(data, width, height, m);
        let score = score_array(&grid, height, width);
        if m == 0 || score >= high {
            high = score;
            best_mask = m;
        }
    }
    best_mask
}

// ---------------------------------------------------------------------------
// Public encoder
// ---------------------------------------------------------------------------

/// DotCode encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DotCodeEncoder;

impl DotCodeEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        DotCodeEncoder
    }

    /// Build a DotCode [`Symbol`] from raw bytes, auto-selecting size and mask.
    pub fn build_bytes(&self, data: &[u8]) -> Result<Symbol> {
        self.build_opts(data, 0, false, false, None)
    }

    /// Build a DotCode [`Symbol`] from raw bytes with a user-chosen width (`5..=200`).
    pub fn build_bytes_width(&self, data: &[u8], width: usize) -> Result<Symbol> {
        self.build_opts(data, 0, false, false, Some(width))
    }

    /// Build a GS1 DotCode [`Symbol`] from an already GS1-reduced byte string
    /// (Application Identifier parsing is the caller's responsibility; FNC1 group
    /// separators are the GS byte `0x1D`).
    pub fn build_gs1_reduced(&self, data: &[u8]) -> Result<Symbol> {
        self.build_opts(data, 0, true, false, None)
    }

    /// Full build entry point.
    pub(crate) fn build_opts(
        &self,
        data: &[u8],
        eci: u32,
        gs1: bool,
        reader_init: bool,
        width: Option<usize>,
    ) -> Result<Symbol> {
        if data.is_empty() {
            return Err(Error::invalid_data("DotCode input is empty"));
        }
        if eci > 811799 {
            return Err(Error::invalid_parameter(
                "DotCode ECI out of range (0..=811799)",
            ));
        }
        let (codewords, binary_finish) = encode_message(data, eci, gs1, reader_init);
        if codewords.is_empty() {
            return Err(Error::invalid_data("DotCode produced no codewords"));
        }
        let (width, height) = select_size(codewords.len(), width)?;
        let padded = add_padding(codewords, width, height, binary_finish);
        let mask = select_mask(&padded, width, height);
        let meta = DotCodeMeta {
            width,
            height,
            mask,
            codewords: padded,
        };
        let segments = if eci > 0 {
            vec![Segment::eci(eci), Segment::byte(data.to_vec())]
        } else {
            vec![Segment::byte(data.to_vec())]
        };
        Ok(Symbol::new(
            Symbology::DotCode,
            segments,
            SymbolMeta::DotCode(meta),
        ))
    }
}

/// Render meta into a [`BitMatrix`]. Shared by [`Encode`] and internal validation.
pub(crate) fn render_meta(meta: &DotCodeMeta) -> Result<Encoding> {
    if meta.codewords.is_empty() {
        return Err(Error::invalid_parameter("DotCode meta has no codewords"));
    }
    if meta.mask > 3 {
        return Err(Error::invalid_parameter("DotCode mask must be 0..=3"));
    }
    if !(MIN_SIZE..=MAX_SIZE).contains(&meta.width) || !(MIN_SIZE..=MAX_SIZE).contains(&meta.height)
    {
        return Err(Error::invalid_parameter("DotCode size out of range"));
    }
    if (meta.width + meta.height) % 2 != 1 {
        return Err(Error::invalid_parameter("DotCode width+height must be odd"));
    }
    for &cw in &meta.codewords {
        if cw >= 113 {
            return Err(Error::invalid_parameter("DotCode codeword out of range"));
        }
    }
    let grid = render_grid(&meta.codewords, meta.width, meta.height, meta.mask);
    let mut matrix = BitMatrix::new(meta.width, meta.height, 3);
    for (i, &dark) in grid.iter().enumerate() {
        if dark {
            matrix.set(i % meta.width, i / meta.width, true);
        }
    }
    Ok(Encoding::Matrix(matrix))
}

impl Encode for DotCodeEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::DotCode {
            return Err(Error::invalid_parameter(
                "DotCodeEncoder given a non-DotCode symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::DotCode(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "DotCode symbol missing DotCodeMeta",
                ));
            }
        };
        render_meta(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codewords (with padding) must match zint's `ZINT_DEBUG_TEST` dumps for the
    /// documented AIM ISS DotCode Rev 4.0 / zint `test_input` vectors. These are
    /// independent reference values from zint `backend/tests/test_dotcode.c`.
    fn dump(data: &[u8], eci: u32, gs1: bool, width: Option<usize>) -> Vec<u8> {
        let (cw, bf) = encode_message(data, eci, gs1, false);
        let (w, h) = select_size(cw.len(), width).unwrap();
        add_padding(cw, w, h, bf)
    }

    #[test]
    fn zint_input_vectors() {
        // /* 0*/ "A" width 13 -> 66 21 6A
        assert_eq!(dump(b"A", 0, false, Some(13)), vec![102, 33, 106]);
        // /* 8*/ "\0" width 13 -> 65 40 6A
        assert_eq!(dump(b"\x00", 0, false, Some(13)), vec![101, 64, 106]);
        // /*15*/ "\x7f" width 13 -> 66 5F 6A (ShiftB DEL PAD)
        assert_eq!(dump(b"\x7f", 0, false, Some(13)), vec![102, 95, 106]);
        // /*23*/ "1712345610" -> 6B 64 0C 22 38 (FNC1 17..10 12 34 56)
        assert_eq!(
            dump(b"1712345610", 0, false, None),
            vec![107, 100, 12, 34, 56]
        );
        // /*33*/ "ABCDE12345678" -> 6A 21 22 23 24 25 69 0C 22 38 4E
        assert_eq!(
            dump(b"ABCDE12345678", 0, false, None),
            vec![106, 33, 34, 35, 36, 37, 105, 12, 34, 56, 78]
        );
        // /*34*/ "\0ABCD1234567890" (len 15) -> 65 40 21 22 23 24 6A 0C 22 38 4E 5A 6A
        assert_eq!(
            dump(b"\x00ABCD1234567890", 0, false, None),
            vec![101, 64, 33, 34, 35, 36, 106, 12, 34, 56, 78, 90, 106]
        );
        // /*41*/ "00" width 13 -> 6B 00 6A
        assert_eq!(dump(b"00", 0, false, Some(13)), vec![107, 0, 106]);
        // /*29*/ "99aA[{00\0" (len 9) -> 6B 63 6A 41 21 3B 5B 10 10 65 40
        assert_eq!(
            dump(b"99aA[{00\x00", 0, false, None),
            vec![107, 99, 106, 65, 33, 59, 91, 16, 16, 101, 64]
        );
        // /*35*/ DATA_MODE "abcde\x80\x81\x82\x83\x84\xff" -> 6A 41 42 43 44 45 70 31 5A 35 21 5A 5F 02 31
        assert_eq!(
            dump(b"abcde\x80\x81\x82\x83\x84\xff", 0, false, None),
            vec![106, 65, 66, 67, 68, 69, 112, 49, 90, 53, 33, 90, 95, 2, 49]
        );
        // /*37*/ DATA_MODE "\x80\x81\x82\x83\x31\x32\x33\x34" -> 70 13 56 0A 59 2C 67 0C 22
        assert_eq!(
            dump(b"\x80\x81\x82\x83\x31\x32\x33\x34", 0, false, None),
            vec![112, 19, 86, 10, 89, 44, 103, 12, 34]
        );
        // /*16*/ macro: "[)>\x1e05\x1dA\x1e\x04" -> 6A 61 21 (LatchB Macro97 A)
        assert_eq!(
            dump(b"[)>\x1e05\x1dA\x1e\x04", 0, false, None),
            vec![106, 97, 33]
        );
    }

    /// ECI codeword vectors (zint `test_input` /*0*/-/*6*/, single char "A").
    #[test]
    fn zint_eci_vectors() {
        // /* 1*/ eci 3, "A" -> 6C 03 66 21
        assert_eq!(
            encode_message(b"A", 3, false, false).0,
            vec![108, 3, 102, 33]
        );
        // /* 4*/ eci 899, "A" -> 6C 28 07 44 66 21
        assert_eq!(
            encode_message(b"A", 899, false, false).0,
            vec![108, 40, 7, 68, 102, 33]
        );
        // /* 6*/ eci 811799, "A" -> 6C 67 40 50 66 21
        assert_eq!(
            encode_message(b"A", 811799, false, false).0,
            vec![108, 103, 64, 80, 102, 33]
        );
    }

    #[test]
    fn size_parity_is_odd() {
        for len in 1..40 {
            let (w, h) = select_size(len, None).unwrap();
            assert_eq!((w + h) % 2, 1, "len {len}: {w}x{h} not odd parity");
            assert!(w >= MIN_SIZE && h >= MIN_SIZE);
        }
    }
}

#[cfg(test)]
mod bitmap_tests {
    use super::*;

    fn grid_to_rows(grid: &[bool], width: usize, height: usize) -> Vec<String> {
        (0..height)
            .map(|r| {
                (0..width)
                    .map(|c| if grid[r * width + c] { '1' } else { '0' })
                    .collect()
            })
            .collect()
    }

    // GS1 reduction of "[17]070620[10]ABC123456" (AI 17 fixed 6, AI 10 last).
    // Cross-checked against zint dump vector /*24*/ for the "17..10" path.
    const GS1_REDUCED: &[u8] = b"1707062010ABC123456";

    /// zint `test_dotcode.c` test_encode /*2*/ — ISS DotCode Rev 4.0 Figure 5
    /// (Mask = 0, user-forced). Independent reference bitmap.
    #[test]
    fn rev4_figure5_mask0() {
        let expected = [
            "10101000100010000000001",
            "01000101010001010000000",
            "00100010001000101000100",
            "01010001010000000101010",
            "00001000100010100010101",
            "00000101010101010000010",
            "00100010101000000010001",
            "00010100000101000100010",
            "00001000001000001010101",
            "01010101010001000001010",
            "10100000100010001000101",
            "01000100000100010101000",
            "10000010000010100010001",
            "00010000010100010101010",
            "10101000001000101010001",
            "01000001010101010000010",
        ];
        let (cw, bf) = encode_message(GS1_REDUCED, 0, true, false);
        let (w, h) = select_size(cw.len(), None).unwrap();
        assert_eq!((w, h), (23, 16), "size mismatch");
        let padded = add_padding(cw, w, h, bf);
        let grid = render_grid(&padded, w, h, 0);
        assert_eq!(grid_to_rows(&grid, w, h), expected);
    }

    /// zint test_encode /*6*/ — Rev 4.0 Figure 6 top-right, auto mask selection
    /// resolves to Mask = 1. Validates the mask-scoring path end-to-end.
    #[test]
    fn rev4_figure6_auto_mask() {
        let expected = [
            "10000000001010001000101",
            "01010101000100000101000",
            "00100010000000100000001",
            "01010001000001000001000",
            "10101010100000001010101",
            "00000100010100000100010",
            "00000000001010101010001",
            "00010001010001000001000",
            "00101010101000001010001",
            "01000100000001010000000",
            "10101000101000101000001",
            "00010101000100010101010",
            "10001000001010100000101",
            "01010001010001000001010",
            "10000010101010100010101",
            "01000101000101010101010",
        ];
        let (cw, bf) = encode_message(GS1_REDUCED, 0, true, false);
        let (w, h) = select_size(cw.len(), None).unwrap();
        let padded = add_padding(cw, w, h, bf);
        let mask = select_mask(&padded, w, h);
        assert_eq!(mask, 1, "auto mask should resolve to 1");
        let grid = render_grid(&padded, w, h, mask);
        assert_eq!(grid_to_rows(&grid, w, h), expected);
    }
}
