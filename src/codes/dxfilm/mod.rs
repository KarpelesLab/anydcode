//! DX Film Edge — the barcode printed along the edge of 35mm film (ISO/IEC 15424
//! symbology identifier `]X…`; see the Wikipedia article on DX encoding and the
//! zxing-cpp reference reader `core/src/oned/ODDXFilmEdgeReader.cpp`).
//!
//! # Representation
//!
//! A DX Film Edge symbol is **two parallel tracks** running along the film edge: a
//! *clock* track (a fixed run of evenly spaced marks used to locate the module grid)
//! and, beside it, a *data* track carrying the film identification. Because the clock
//! track is not derivable from the data alone and the two tracks must line up module
//! for module, this is modelled as a two-row [`BitMatrix`] rather than a single
//! [`crate::output::LinearPattern`]:
//!
//! - **row 0** — the clock track;
//! - **row 1** — the data track (start pattern, data bits, stop pattern).
//!
//! `true` is a dark (opaque) module. Both rows are the same width: 31 modules for the
//! frame-number variant, 23 for the older variant without a frame number.
//!
//! # Data-track bit layout
//!
//! After a 5-module start pattern (`▓ ▓ ▓`, i.e. bar-space-bar-space-bar) come the
//! data bits — one module each, dark = 1 — then a 3-module stop pattern (`▓ ▓`).
//! With a frame number (23 data bits):
//!
//! | bits  | field                                  |
//! |-------|----------------------------------------|
//! | 0     | separator (always light)               |
//! | 1–7   | DX number part 1 (product code, 7 bits, MSB first) |
//! | 8     | separator (always light)               |
//! | 9–12  | DX number part 2 (generation, 4 bits)  |
//! | 13–18 | frame number (6 bits)                  |
//! | 19    | half-frame flag (`A` suffix)           |
//! | 20    | separator (always light)               |
//! | 21    | parity (even parity over bits 0..=20)  |
//! | 22    | separator (always light)               |
//!
//! Without a frame number (15 data bits) the run ends after part 2: bit 13 is the
//! parity over bits 0..=12 and bit 14 is a separator. The DX number is customarily
//! written `part1-part2` (e.g. `115-10`), and with a frame `part1-part2/frame` plus a
//! trailing `A` for a half-frame exposure (e.g. `115-10/11A`).
//!
//! The payload is stored as a single [`Segment::byte`] holding that canonical ASCII
//! text; [`DxFilmMeta`] records whether the frame-number variant is used.

use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// DX Film Edge has no mandatory quiet zone in this abstract module model.
const QUIET_ZONE: usize = 0;

/// Number of data bits with / without the frame number (zxing-cpp `DATA_LENGTH_*`).
const DATA_LEN_FN: usize = 23;
const DATA_LEN_NO_FN: usize = 15;

/// The parsed contents of a DX Film Edge symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DxData {
    /// DX number part 1 (product number), 1..=127.
    part1: u8,
    /// DX number part 2 (generation number), 0..=15.
    part2: u8,
    /// Frame number and half-frame flag, if the frame-number variant is used.
    frame: Option<(u8, bool)>,
}

impl DxData {
    fn data_len(&self) -> usize {
        if self.frame.is_some() {
            DATA_LEN_FN
        } else {
            DATA_LEN_NO_FN
        }
    }

    /// The canonical ASCII text form, e.g. `115-10` or `115-10/11A`.
    fn to_text(&self) -> String {
        let mut s = format!("{}-{}", self.part1, self.part2);
        if let Some((frame, half)) = self.frame {
            s.push('/');
            s.push_str(&frame.to_string());
            if half {
                s.push('A');
            }
        }
        s
    }

    /// Parse the canonical text form, validating every field range.
    fn parse(text: &str) -> Result<DxData> {
        let (p1, rest) = text
            .split_once('-')
            .ok_or_else(|| Error::invalid_data("DX number must contain '-'"))?;
        let part1: u8 = p1
            .parse()
            .map_err(|_| Error::invalid_data("DX part 1 is not a number"))?;
        if part1 == 0 || part1 > 127 {
            return Err(Error::capacity("DX part 1 must be 1..=127"));
        }
        let (p2_str, frame) = match rest.split_once('/') {
            Some((p2, frame_str)) => {
                let (digits, half) = match frame_str.strip_suffix('A') {
                    Some(d) => (d, true),
                    None => (frame_str, false),
                };
                let frame: u8 = digits
                    .parse()
                    .map_err(|_| Error::invalid_data("frame number is not a number"))?;
                if frame > 63 {
                    return Err(Error::capacity("frame number must be 0..=63"));
                }
                (p2, Some((frame, half)))
            }
            None => (rest, None),
        };
        let part2: u8 = p2_str
            .parse()
            .map_err(|_| Error::invalid_data("DX part 2 is not a number"))?;
        if part2 > 15 {
            return Err(Error::capacity("DX part 2 must be 0..=15"));
        }
        Ok(DxData {
            part1,
            part2,
            frame,
        })
    }

    /// The data-track bits (one entry per module, dark = `true`), including
    /// separators and the even-parity bit.
    fn data_bits(&self) -> Vec<bool> {
        let mut bits = vec![false; self.data_len()];
        // bit 0: separator (light).
        write_field(&mut bits, 1, 7, self.part1 as u32); // DX part 1
        // bit 8: separator (light).
        write_field(&mut bits, 9, 4, self.part2 as u32); // DX part 2
        if let Some((frame, half)) = self.frame {
            write_field(&mut bits, 13, 6, frame as u32);
            bits[19] = half;
            // bit 20: separator; bit 21: parity; bit 22: separator.
            bits[21] = even_parity(&bits[..21]);
        } else {
            // bit 13: parity; bit 14: separator.
            bits[13] = even_parity(&bits[..13]);
        }
        bits
    }
}

/// Write `value` into `bits[start..start+len]`, most-significant bit first.
fn write_field(bits: &mut [bool], start: usize, len: usize, value: u32) {
    for i in 0..len {
        bits[start + i] = (value >> (len - 1 - i)) & 1 != 0;
    }
}

/// Read `len` bits from `bits[start..]`, most-significant bit first.
fn read_field(bits: &[bool], start: usize, len: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..len {
        v = (v << 1) | bits[start + i] as u32;
    }
    v
}

/// Even parity: `true` when an odd number of the bits are set (so that the total
/// including the parity bit is even).
fn even_parity(bits: &[bool]) -> bool {
    bits.iter().filter(|&&b| b).count() % 2 == 1
}

/// The clock track for a symbol of `width` modules: a 5-module bar, then evenly
/// spaced single modules, then a 3-module bar (zxing-cpp `CLOCK_PATTERN_*`).
fn clock_row(width: usize) -> Vec<bool> {
    let mut row = vec![false; width];
    let mut x = 0usize;
    let mut dark = true;
    for run in run_lengths(width) {
        if dark {
            for m in row.iter_mut().skip(x).take(run) {
                *m = true;
            }
        }
        x += run;
        dark = !dark;
    }
    row
}

/// The clock/data-track run lengths: `5, 1×n, 3`, summing to `width`.
fn run_lengths(width: usize) -> Vec<usize> {
    let mut runs = vec![5];
    runs.extend(std::iter::repeat_n(1, width - 5 - 3));
    runs.push(3);
    runs
}

/// The full 2-row matrix for `dx`.
fn render(dx: &DxData) -> BitMatrix {
    let data = dx.data_bits();
    // start(5) + data + stop(3).
    let width = 5 + data.len() + 3;
    let mut m = BitMatrix::new(width, 2, QUIET_ZONE);

    // Row 0: clock track.
    for (x, &dark) in clock_row(width).iter().enumerate() {
        if dark {
            m.set(x, 0, true);
        }
    }

    // Row 1: data track = start pattern, data bits, stop pattern.
    // Start pattern: bar space bar space bar.
    for x in [0usize, 2, 4] {
        m.set(x, 1, true);
    }
    for (i, &bit) in data.iter().enumerate() {
        if bit {
            m.set(5 + i, 1, true);
        }
    }
    // Stop pattern: bar space bar.
    m.set(5 + data.len(), 1, true);
    m.set(5 + data.len() + 2, 1, true);
    m
}

/// Parameters required to re-encode a DX Film Edge symbol identically. The payload
/// text carries the DX/frame numbers; this records the symbol variant.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DxFilmMeta {
    /// `true` for the newer variant that carries a 6-bit frame number (31-module
    /// tracks); `false` for the older 23-module variant.
    pub has_frame_number: bool,
}

/// DX Film Edge encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DxFilmEncoder;

impl DxFilmEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Build a reproducible [`Symbol`] from the DX number parts and an optional frame
    /// number (`frame`, `half_frame`). `part1` is 1..=127, `part2` 0..=15, the frame
    /// number 0..=63.
    pub fn build(&self, part1: u8, part2: u8, frame: Option<(u8, bool)>) -> Result<Symbol> {
        let dx = DxData {
            part1,
            part2,
            frame,
        };
        // Validate ranges via the text round-trip parser.
        let text = dx.to_text();
        let dx = DxData::parse(&text)?;
        Ok(Symbol::new(
            Symbology::DxFilmEdge,
            vec![Segment::byte(text.into_bytes())],
            SymbolMeta::DxFilm(DxFilmMeta {
                has_frame_number: dx.frame.is_some(),
            }),
        ))
    }

    /// Convenience: build from the canonical text form, e.g. `"115-10"` or
    /// `"115-10/11A"`.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        let dx = DxData::parse(text)?;
        self.build(dx.part1, dx.part2, dx.frame)
    }
}

impl Encode for DxFilmEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::DxFilmEdge {
            return Err(Error::invalid_parameter(
                "DxFilmEncoder given a non-DX-Film-Edge symbol",
            ));
        }
        let text = flatten_text(&symbol.segments)?;
        let dx = DxData::parse(&text)?;
        Ok(Encoding::Matrix(render(&dx)))
    }
}

/// DX Film Edge decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DxFilmDecoder;

impl DxFilmDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }

    /// Decode the two-row module grid into a [`Symbol`].
    pub fn decode_matrix(&self, m: &BitMatrix) -> Result<Symbol> {
        if m.height() != 2 {
            return Err(Error::undecodable("DX Film Edge grid must have two rows"));
        }
        let width = m.width();
        let data_len = match width.checked_sub(5 + 3) {
            Some(DATA_LEN_FN) => DATA_LEN_FN,
            Some(DATA_LEN_NO_FN) => DATA_LEN_NO_FN,
            _ => return Err(Error::undecodable("DX Film Edge width is not valid")),
        };
        let has_fn = data_len == DATA_LEN_FN;

        // Clock track must match.
        for (x, &dark) in clock_row(width).iter().enumerate() {
            if m.get(x, 0) != dark {
                return Err(Error::undecodable("DX Film Edge clock track mismatch"));
            }
        }

        // Start pattern: bar space bar space bar.
        let start_ok = (0..5).all(|x| m.get(x, 1) == matches!(x, 0 | 2 | 4));
        if !start_ok {
            return Err(Error::undecodable("DX Film Edge start pattern mismatch"));
        }
        // Stop pattern: bar space bar.
        let stop = 5 + data_len;
        if !m.get(stop, 1) || m.get(stop + 1, 1) || !m.get(stop + 2, 1) {
            return Err(Error::undecodable("DX Film Edge stop pattern mismatch"));
        }

        let bits: Vec<bool> = (0..data_len).map(|i| m.get(5 + i, 1)).collect();

        // Separators must be light.
        let sep_ok = if has_fn {
            !bits[0] && !bits[8] && !bits[20] && !bits[22]
        } else {
            !bits[0] && !bits[8] && !bits[14]
        };
        if !sep_ok {
            return Err(Error::undecodable("DX Film Edge separator is not light"));
        }

        // Parity check (even parity over all bits except the parity and the final
        // separator).
        let parity_index = data_len - 2;
        if even_parity(&bits[..parity_index]) != bits[parity_index] {
            return Err(Error::undecodable("DX Film Edge parity mismatch"));
        }

        let part1 = read_field(&bits, 1, 7) as u8;
        if part1 == 0 {
            return Err(Error::undecodable("DX Film Edge part 1 is zero"));
        }
        let part2 = read_field(&bits, 9, 4) as u8;
        let frame = has_fn.then(|| (read_field(&bits, 13, 6) as u8, bits[19]));

        let dx = DxData {
            part1,
            part2,
            frame,
        };
        Ok(Symbol::new(
            Symbology::DxFilmEdge,
            vec![Segment::byte(dx.to_text().into_bytes())],
            SymbolMeta::DxFilm(DxFilmMeta {
                has_frame_number: has_fn,
            }),
        ))
    }
}

impl Decode for DxFilmDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "DX Film Edge decode of a linear pattern",
            }),
        }
    }
}

/// Concatenate the bytes of all data segments into UTF-8 text.
fn flatten_text(segments: &[Segment]) -> Result<String> {
    let mut out = Vec::new();
    for seg in segments {
        if matches!(seg.mode, Mode::Eci(_)) {
            return Err(Error::Unsupported {
                what: "DX Film Edge ECI segments",
            });
        }
        out.extend_from_slice(&seg.data);
    }
    String::from_utf8(out).map_err(|_| Error::invalid_data("DX Film Edge payload is not UTF-8"))
}
