//! Aztec high-level encodation: payload bytes ⇄ the mode-switched bit stream, plus
//! the word bit-stuffing applied before Reed–Solomon.
//!
//! The encoder is a deterministic greedy state machine over the five modes (Upper,
//! Lower, Mixed, Punct, Digit) plus the binary shift. Mode changes use single-step
//! latches, routed through a small breadth-first search when no direct latch exists,
//! and a Punct **shift** for isolated punctuation. Because it is deterministic, a
//! payload always yields the same bit stream, which is what makes re-encoding of a
//! decoded symbol identical.

use super::tables::{
    DIGIT, LOWER, MIXED, PUNCT, UPPER, char_for_code, code_for_char, mode_bits, punct_pair_for_code,
};

/// A grow-able most-significant-first bit buffer.
#[derive(Default)]
pub struct BitVec {
    bits: Vec<bool>,
}

impl BitVec {
    fn new() -> Self {
        BitVec::default()
    }

    /// Append the low `len` bits of `value`, most-significant first.
    pub fn push(&mut self, value: u32, len: usize) {
        for k in (0..len).rev() {
            self.bits.push((value >> k) & 1 != 0);
        }
    }

    /// The accumulated bits.
    pub fn into_bits(self) -> Vec<bool> {
        self.bits
    }
}

/// Whether `b` cannot be represented in any of the five character sets and must be
/// carried by the binary shift.
fn is_binary(b: u8) -> bool {
    code_for_char(UPPER, b).is_none()
        && code_for_char(LOWER, b).is_none()
        && code_for_char(MIXED, b).is_none()
        && code_for_char(PUNCT, b).is_none()
        && code_for_char(DIGIT, b).is_none()
}

/// Directed single-step latch edges: `(from, to, code, bits)`. `PUNCT` is reached
/// only via a shift, so it is not a latch target here.
const LATCH_EDGES: &[(usize, usize, u32, usize)] = &[
    (UPPER, LOWER, 28, 5),
    (UPPER, MIXED, 29, 5),
    (UPPER, DIGIT, 30, 5),
    (LOWER, DIGIT, 30, 5),
    (LOWER, MIXED, 29, 5),
    (MIXED, UPPER, 29, 5),
    (DIGIT, UPPER, 14, 4),
];

/// The shortest latch path from `from` to `to` as a sequence of `(code, bits)`.
fn latch_path(from: usize, to: usize) -> Vec<(u32, usize)> {
    if from == to {
        return Vec::new();
    }
    // Breadth-first over the four latchable modes.
    let mut prev: [Option<(usize, u32, usize)>; 5] = [None; 5];
    let mut visited = [false; 5];
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(from);
    visited[from] = true;
    while let Some(node) = queue.pop_front() {
        if node == to {
            break;
        }
        for &(f, t, code, bits) in LATCH_EDGES {
            if f == node && !visited[t] {
                visited[t] = true;
                prev[t] = Some((node, code, bits));
                queue.push_back(t);
            }
        }
    }
    // Reconstruct.
    let mut path = Vec::new();
    let mut cur = to;
    while cur != from {
        let (p, code, bits) = prev[cur].expect("modes are fully connected");
        path.push((code, bits));
        cur = p;
    }
    path.reverse();
    path
}

/// Emit the latch sequence from `*mode` to `target`, updating `*mode`.
fn emit_latch(out: &mut BitVec, mode: &mut usize, target: usize) {
    for (code, bits) in latch_path(*mode, target) {
        out.push(code, bits);
    }
    *mode = target;
}

/// Emit a run of bytes via the binary shift, splitting long runs as needed. The
/// current mode (which must have a binary-shift code) is preserved.
fn emit_binary(out: &mut BitVec, mode: usize, run: &[u8]) {
    let mut idx = 0;
    while idx < run.len() {
        let chunk = (run.len() - idx).min(2078);
        out.push(31, mode_bits(mode)); // B/S
        if chunk <= 31 {
            out.push(chunk as u32, 5);
        } else {
            out.push(0, 5);
            out.push((chunk - 31) as u32, 11);
        }
        for &byte in &run[idx..idx + chunk] {
            out.push(byte as u32, 8);
        }
        idx += chunk;
    }
}

/// Encode `data` into the Aztec high-level bit stream.
pub fn encode(data: &[u8]) -> Vec<bool> {
    let mut out = BitVec::new();
    let mut mode = UPPER;
    let mut i = 0;
    let n = data.len();
    while i < n {
        let b = data[i];
        if is_binary(b) {
            let mut j = i;
            while j < n && is_binary(data[j]) {
                j += 1;
            }
            if mode == DIGIT {
                emit_latch(&mut out, &mut mode, UPPER);
            }
            emit_binary(&mut out, mode, &data[i..j]);
            i = j;
            continue;
        }
        // Digits always require the Digit mode.
        if b.is_ascii_digit() {
            if mode != DIGIT {
                emit_latch(&mut out, &mut mode, DIGIT);
            }
            out.push(code_for_char(DIGIT, b).unwrap() as u32, 4);
            i += 1;
            continue;
        }
        // Stay in the current mode if possible (covers spaces in every mode).
        if let Some(code) = code_for_char(mode, b) {
            out.push(code as u32, mode_bits(mode));
            i += 1;
            continue;
        }
        if b.is_ascii_uppercase() {
            emit_latch(&mut out, &mut mode, UPPER);
            out.push(code_for_char(UPPER, b).unwrap() as u32, 5);
        } else if b.is_ascii_lowercase() {
            emit_latch(&mut out, &mut mode, LOWER);
            out.push(code_for_char(LOWER, b).unwrap() as u32, 5);
        } else if let Some(code) = code_for_char(MIXED, b) {
            emit_latch(&mut out, &mut mode, MIXED);
            out.push(code as u32, 5);
        } else if let Some(code) = code_for_char(PUNCT, b) {
            // Punct shift: P/S in the current mode, then one Punct code.
            out.push(0, mode_bits(mode));
            out.push(code as u32, 5);
        } else {
            unreachable!("byte {b} is neither binary nor representable");
        }
        i += 1;
    }
    out.into_bits()
}

/// A bit reader over a `bool` slice.
struct Reader<'a> {
    bits: &'a [bool],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bits: &'a [bool]) -> Self {
        Reader { bits, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bits.len() - self.pos
    }

    fn read(&mut self, n: usize) -> Option<u32> {
        if self.remaining() < n {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bits[self.pos] as u32;
            self.pos += 1;
        }
        Some(v)
    }
}

/// Append the byte(s) produced by a Punct code (single char or two-byte combination).
fn decode_punct(code: u16, out: &mut Vec<u8>) {
    if let Some(pair) = punct_pair_for_code(code) {
        out.extend_from_slice(&pair);
    } else if let Some(byte) = char_for_code(PUNCT, code) {
        out.push(byte);
    }
}

/// Decode a high-level bit stream back into payload bytes. Trailing padding (all
/// ones, shorter than one codeword) is ignored via the "insufficient bits" rule.
pub fn decode(bits: &[bool]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut r = Reader::new(bits);
    let mut mode = UPPER;
    loop {
        let mb = mode_bits(mode);
        if r.remaining() < mb {
            break;
        }
        let code = r.read(mb).unwrap() as u16;
        match code {
            0 => {
                // P/S: read one Punct code.
                match r.read(5) {
                    Some(pc) => decode_punct(pc as u16, &mut out),
                    None => break,
                }
            }
            31 if mode != DIGIT => {
                // Binary shift.
                let mut len = match r.read(5) {
                    Some(v) => v as usize,
                    None => break,
                };
                if len == 0 {
                    match r.read(11) {
                        Some(v) => len = v as usize + 31,
                        None => break,
                    }
                }
                if r.remaining() < len * 8 {
                    break;
                }
                for _ in 0..len {
                    out.push(r.read(8).unwrap() as u8);
                }
            }
            _ => match mode {
                UPPER => match code {
                    28 => mode = LOWER,
                    29 => mode = MIXED,
                    30 => mode = DIGIT,
                    _ => push_char(UPPER, code, &mut out),
                },
                LOWER => match code {
                    28 => {
                        // U/S: one Upper char.
                        match r.read(5) {
                            Some(uc) => push_char(UPPER, uc as u16, &mut out),
                            None => break,
                        }
                    }
                    29 => mode = MIXED,
                    30 => mode = DIGIT,
                    _ => push_char(LOWER, code, &mut out),
                },
                MIXED => match code {
                    28 => mode = LOWER,
                    29 => mode = UPPER,
                    30 => mode = PUNCT,
                    _ => push_char(MIXED, code, &mut out),
                },
                DIGIT => match code {
                    14 => mode = UPPER,
                    15 => {
                        // U/S: one Upper char.
                        match r.read(5) {
                            Some(uc) => push_char(UPPER, uc as u16, &mut out),
                            None => break,
                        }
                    }
                    _ => push_char(DIGIT, code, &mut out),
                },
                _ => {}
            },
        }
    }
    out
}

/// Push the character for `(mode, code)` if it is a data code.
fn push_char(mode: usize, code: u16, out: &mut Vec<u8>) {
    if let Some(byte) = char_for_code(mode, code) {
        out.push(byte);
    }
}

/// Apply Aztec bit-stuffing for `word_size`-bit words: an all-zero or all-one leading
/// group in a word gets a complementary bit inserted, preventing all-0/all-1 words.
/// The tail is padded with 1-bits to a word boundary.
pub fn stuff_bits(bits: &[bool], w: usize) -> Vec<bool> {
    let mut out = Vec::new();
    let n = bits.len();
    let mut i = 0;
    while i < n {
        let mut all_one = true;
        let mut all_zero = true;
        for j in 0..w - 1 {
            let bit = if i + j < n { bits[i + j] } else { true };
            if bit {
                all_zero = false;
            } else {
                all_one = false;
            }
        }
        if all_one {
            out.extend(std::iter::repeat_n(true, w - 1));
            out.push(false);
            i += w - 1;
        } else if all_zero {
            out.extend(std::iter::repeat_n(false, w - 1));
            out.push(true);
            i += w - 1;
        } else {
            for j in 0..w {
                out.push(if i + j < n { bits[i + j] } else { true });
            }
            i += w;
        }
    }
    out
}

/// Reverse [`stuff_bits`]: remove the inserted complementary bits.
pub fn unstuff_bits(bits: &[bool], w: usize) -> Vec<bool> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + w <= bits.len() {
        let word = &bits[i..i + w];
        let head = &word[0..w - 1];
        if head.iter().all(|&b| b) || head.iter().all(|&b| !b) {
            out.extend_from_slice(head);
        } else {
            out.extend_from_slice(word);
        }
        i += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independently derived from the ISO/IEC 24778 character tables (Upper and Digit
    // sets, with the Upper→Digit D/L latch): "AB12".
    //   A = 2 (00010), B = 3 (00011), D/L = 30 (11110), 1 = 3 (0011), 2 = 4 (0100)
    #[test]
    fn highlevel_upper_digit_vector() {
        let bits = encode(b"AB12");
        let expected = [
            false, false, false, true, false, // A = 2
            false, false, false, true, true, // B = 3
            true, true, true, true, false, // D/L = 30
            false, false, true, true, // 1 -> code 3
            false, true, false, false, // 2 -> code 4
        ];
        assert_eq!(bits, expected.to_vec());
    }

    // Independently derived from the ISO/IEC 24778 character *and* latch tables:
    // "aZ!" exercises the Lower latch, the two-step Lower→Upper route (D/L then U/L,
    // as the standard latch table prescribes) and a Punct shift.
    //   L/L=28(11100) a=2(00010) D/L=30(11110) U/L=14(1110) Z=27(11011)
    //   P/S=0(00000) !=6(00110)
    #[test]
    fn highlevel_lower_punct_vector() {
        let bits = encode(b"aZ!");
        let expected = [
            true, true, true, false, false, // L/L = 28
            false, false, false, true, false, // a = 2
            true, true, true, true, false, // D/L = 30
            true, true, true, false, // U/L = 14 (4 bits)
            true, true, false, true, true, // Z = 27
            false, false, false, false, false, // P/S = 0
            false, false, true, true, false, // ! = 6
        ];
        assert_eq!(bits, expected.to_vec());
    }

    // Binary-shift format (independently derived): a single 0x80 byte.
    //   B/S=31(11111) length=1(00001) 0x80(10000000)
    #[test]
    fn highlevel_binary_shift_vector() {
        let bits = encode(&[0x80]);
        let expected = [
            true, true, true, true, true, // B/S = 31
            false, false, false, false, true, // length = 1
            true, false, false, false, false, false, false, false, // 0x80
        ];
        assert_eq!(bits, expected.to_vec());
    }

    #[test]
    fn highlevel_roundtrip_modes() {
        for payload in [
            &b"HELLO"[..],
            b"Hello, World!",
            b"abc XYZ 0123 def.",
            b"mixed\x01\x02control",
            b"\x00\xff\x80binary\xfe",
            b"UPPERlower123!@#",
        ] {
            let bits = encode(payload);
            let decoded = decode(&bits);
            assert_eq!(decoded, payload, "roundtrip failed for {payload:?}");
        }
    }

    #[test]
    fn stuffing_roundtrip() {
        for w in [6usize, 8, 10] {
            let raw = encode(b"Some data to stuff 000111 \x00\xff");
            let stuffed = stuff_bits(&raw, w);
            assert_eq!(stuffed.len() % w, 0);
            let unstuffed = unstuff_bits(&stuffed, w);
            // Unstuffed reproduces the raw bits plus trailing 1-pad; decoding both
            // yields the same payload.
            assert_eq!(decode(&unstuffed), decode(&raw));
        }
    }
}
