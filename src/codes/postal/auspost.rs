//! Australia Post 4-State Customer Barcode.
//!
//! Implements the "Customer Barcoding Technical Specifications" bar structure:
//! a start pair, a two-digit Format Control Code (FCC), an eight-digit Delivery
//! Point Identifier (DPID / sorting code), an optional Customer Information
//! field, filler bars, four Reed–Solomon check symbols (over `GF(64)`,
//! primitive polynomial `0x43`, generator roots `α^1..α^4`) and a stop pair.
//!
//! Bar values follow the specification / zint convention `0 = Full`,
//! `1 = Ascender`, `2 = Descender`, `3 = Tracker`. Three bar layouts exist,
//! selected automatically from the input (as in zint's `BARCODE_AUSPOST`):
//!
//! | Format          | FCC | bars | Customer Information            |
//! |-----------------|-----|------|---------------------------------|
//! | Standard        | 11  |  37  | none                            |
//! | Customer 2      | 59  |  52  | up to 8 (N table) / 5 (C table) |
//! | Customer 3      | 62  |  67  | up to 15 (N) / 10 (C table)     |
//! | Null (DPID = 0) | 00  |  as above |                            |
//!
//! The **N table** encodes each digit as two bar values; the **C table** encodes
//! each character of the 64-symbol graphic set as three bar values. zint chooses
//! the C table whenever the Customer Information contains a non-digit.
//!
//! ## Round-trip and the N/C ambiguity
//!
//! The choice of encoding table for the Customer Information field is *not*
//! recorded in the barcode, and the C-table is a bijection onto every triple of
//! bar values while the letters `A`–`Z` map to triples drawn only from
//! `{0,1,2}` — the same alphabet the N-table uses. A Customer Information field
//! of purely upper-case letters is therefore genuinely indistinguishable from
//! some digit string. This decoder resolves the field as **numeric when it
//! parses cleanly as N-table pairs, otherwise as a raw byte field** (C-table).
//! Fields containing a space, `#` or a lower-case letter always force the C
//! table (their triples contain a `3`), so they round-trip exactly; a caller
//! that needs an unambiguous alphabetic field should include such a character.
//!
//! Reference: verified against zint's `test_auspost` diagrams (AusPost Tech
//! Specs Diagram 1 "96184209" and Guide Figures 3/4), see `tests/postal_extra.rs`.

use super::rs::{Gf, RsEncoder};
use super::{BarState, PostalVariant};
use crate::error::{Error, Result};
use crate::segment::{Mode, Segment};

/// The 64-symbol graphic character set indexing the C table.
const GDSET: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz #";

/// N table: each digit `0..=9` maps to two bar values.
const N_TABLE: [[u8; 2]; 10] = [
    [0, 0],
    [0, 1],
    [0, 2],
    [1, 0],
    [1, 1],
    [1, 2],
    [2, 0],
    [2, 1],
    [2, 2],
    [3, 0],
];

/// C table: each graphic character maps to three bar values (a bijection onto
/// all 64 triples over `{0,1,2,3}`).
const C_TABLE: [[u8; 3]; 64] = [
    [2, 2, 2],
    [3, 0, 0],
    [3, 0, 1],
    [3, 0, 2],
    [3, 1, 0],
    [3, 1, 1],
    [3, 1, 2],
    [3, 2, 0],
    [3, 2, 1],
    [3, 2, 2],
    [0, 0, 0],
    [0, 0, 1],
    [0, 0, 2],
    [0, 1, 0],
    [0, 1, 1],
    [0, 1, 2],
    [0, 2, 0],
    [0, 2, 1],
    [0, 2, 2],
    [1, 0, 0],
    [1, 0, 1],
    [1, 0, 2],
    [1, 1, 0],
    [1, 1, 1],
    [1, 1, 2],
    [1, 2, 0],
    [1, 2, 1],
    [1, 2, 2],
    [2, 0, 0],
    [2, 0, 1],
    [2, 0, 2],
    [2, 1, 0],
    [2, 1, 1],
    [2, 1, 2],
    [2, 2, 0],
    [2, 2, 1],
    [0, 2, 3],
    [0, 3, 0],
    [0, 3, 1],
    [0, 3, 2],
    [0, 3, 3],
    [1, 0, 3],
    [1, 1, 3],
    [1, 2, 3],
    [1, 3, 0],
    [1, 3, 1],
    [1, 3, 2],
    [1, 3, 3],
    [2, 0, 3],
    [2, 1, 3],
    [2, 2, 3],
    [2, 3, 0],
    [2, 3, 1],
    [2, 3, 2],
    [2, 3, 3],
    [3, 0, 3],
    [3, 1, 3],
    [3, 2, 3],
    [3, 3, 0],
    [3, 3, 1],
    [3, 3, 2],
    [3, 3, 3],
    [0, 0, 3],
    [0, 1, 3],
];

/// The two FCC digits for each format index (`fccs[]` in zint).
const FCCS: [[u8; 2]; 7] = [*b"00", *b"11", *b"59", *b"62", *b"45", *b"87", *b"92"];

/// Start and stop pair: `Ascender, Tracker`.
const START_STOP: [u8; 2] = [1, 3];

/// A bar value `0..=3` as a [`BarState`].
fn value_to_bar(v: u8) -> BarState {
    match v {
        0 => BarState::Full,
        1 => BarState::Ascender,
        2 => BarState::Descender,
        _ => BarState::Tracker,
    }
}

/// A [`BarState`] as a bar value `0..=3`.
fn bar_to_value(b: BarState) -> u8 {
    match b {
        BarState::Full => 0,
        BarState::Ascender => 1,
        BarState::Descender => 2,
        BarState::Tracker => 3,
    }
}

/// The C-table index of a graphic-set byte.
fn gdset_index(byte: u8) -> Option<usize> {
    GDSET.iter().position(|&c| c == byte)
}

/// The `GF(64)` Reed–Solomon parameters (4 check symbols, first root `α^1`).
fn rs_check(triples: &[u8]) -> Vec<u8> {
    let gf = Gf::new(0x43, 6);
    RsEncoder::new(&gf, 4, 1).encode(triples)
}

/// Australia Post format, chosen from the padded DPID and Customer Information
/// exactly as zint's `BARCODE_AUSPOST` does.
struct Layout {
    /// Index into [`FCCS`].
    fcc_idx: usize,
    /// Whether the Customer Information is C-table (has a non-digit).
    c_table: bool,
    /// Total filler target (`23`, `38` or `53`) before RS and stop.
    filler_target: usize,
}

/// Validate the DPID and Customer Information and pick the format.
fn plan(dpid: &[u8], custinfo: &[u8]) -> Result<Layout> {
    if dpid.len() != 8 || !dpid.iter().all(u8::is_ascii_digit) {
        return Err(Error::invalid_data("Australia Post DPID must be 8 digits"));
    }
    // `length` mirrors zint: 8 (DPID) + customer information.
    let length = 8 + custinfo.len();
    if length > 23 {
        return Err(Error::capacity(
            "Australia Post input too long (maximum 23)",
        ));
    }
    if custinfo.iter().any(|&b| gdset_index(b).is_none()) {
        return Err(Error::invalid_data(
            "Australia Post customer information must be graphic-set (0-9 A-Z a-z space #)",
        ));
    }
    let c_table = custinfo.iter().any(|&b| !b.is_ascii_digit());

    let (fcc_idx, filler_target) = if custinfo.is_empty() {
        (1, 23) // FCC 11 Standard Customer Barcode
    } else if length > 18 {
        if c_table {
            return Err(Error::capacity(
                "Australia Post customer information over 10 characters must be digits only",
            ));
        }
        (3, 53) // FCC 62 Customer Barcode 3
    } else if length > 16 || (length > 13 && c_table) {
        (3, 53)
    } else {
        (2, 38) // FCC 59 Customer Barcode 2
    };

    // All-zero DPID selects the Null FCC 00 (bar count is unaffected).
    let fcc_idx = if dpid.iter().all(|&b| b == b'0') {
        0
    } else {
        fcc_idx
    };
    Ok(Layout {
        fcc_idx,
        c_table,
        filler_target,
    })
}

/// Encode a padded 8-digit DPID and Customer Information into bar states.
pub(super) fn encode(dpid: &[u8], custinfo: &[u8]) -> Result<Vec<BarState>> {
    let layout = plan(dpid, custinfo)?;

    let mut dest: Vec<u8> = Vec::new();
    dest.extend_from_slice(&START_STOP);
    for &d in &FCCS[layout.fcc_idx] {
        dest.extend_from_slice(&N_TABLE[(d - b'0') as usize]);
    }
    for &d in dpid {
        dest.extend_from_slice(&N_TABLE[(d - b'0') as usize]);
    }
    if layout.c_table {
        for &c in custinfo {
            dest.extend_from_slice(&C_TABLE[gdset_index(c).unwrap()]);
        }
    } else {
        for &d in custinfo {
            dest.extend_from_slice(&N_TABLE[(d - b'0') as usize]);
        }
    }
    // Filler bars (Tracker) up to the format target.
    while dest.len() != layout.filler_target {
        dest.push(3);
    }

    // Reed–Solomon over 6-bit triples of the FCC/DPID/customer/filler region,
    // skipping the two start bars.
    let triples: Vec<u8> = dest[2..]
        .chunks_exact(3)
        .map(|c| (c[0] << 4) | (c[1] << 2) | c[2])
        .collect();
    for &sym in &rs_check(&triples) {
        dest.push((sym >> 4) & 3);
        dest.push((sym >> 2) & 3);
        dest.push(sym & 3);
    }

    dest.extend_from_slice(&START_STOP);
    Ok(dest.iter().map(|&v| value_to_bar(v)).collect())
}

/// The bar count for each supported format.
fn format_ok(n: usize) -> bool {
    matches!(n, 37 | 52 | 67)
}

/// Decode an Australia Post bar sequence into its DPID and Customer Information
/// segments. Returns `Err` (so the dispatcher can fall through) if the framing,
/// tables or Reed–Solomon check do not validate.
pub(super) fn decode(bars: &[BarState]) -> Result<(PostalVariant, Vec<Segment>)> {
    let n = bars.len();
    if !format_ok(n) {
        return Err(Error::undecodable("Australia Post bar count invalid"));
    }
    let v: Vec<u8> = bars.iter().map(|&b| bar_to_value(b)).collect();
    if v[..2] != START_STOP || v[n - 2..] != START_STOP {
        return Err(Error::undecodable("Australia Post start/stop bars invalid"));
    }

    // Verify the Reed–Solomon check over the FCC/DPID/customer/filler region
    // (everything between the start pair and the 12 RS bars).
    let data_region = &v[2..n - 14]; // exclude start(2), RS(12), stop(2)
    let rs_region = &v[n - 14..n - 2];
    if !data_region.len().is_multiple_of(3) {
        return Err(Error::undecodable("Australia Post data region misaligned"));
    }
    let triples: Vec<u8> = data_region
        .chunks_exact(3)
        .map(|c| (c[0] << 4) | (c[1] << 2) | c[2])
        .collect();
    let expected = rs_check(&triples);
    let got: Vec<u8> = rs_region
        .chunks_exact(3)
        .map(|c| (c[0] << 4) | (c[1] << 2) | c[2])
        .collect();
    if expected != got {
        return Err(Error::undecodable("Australia Post Reed-Solomon mismatch"));
    }

    // FCC (2 digits) then DPID (8 digits), both N-table. The FCC is validated
    // implicitly by the RS check and re-derived on encode, so it is skipped.
    for i in 0..2 {
        let pair = [data_region[i * 2], data_region[i * 2 + 1]];
        if !N_TABLE.contains(&pair) {
            return Err(Error::undecodable("Australia Post FCC invalid"));
        }
    }
    let mut dpid = Vec::with_capacity(8);
    for i in 2..10 {
        let pair = [data_region[i * 2], data_region[i * 2 + 1]];
        let d = N_TABLE
            .iter()
            .position(|&e| e == pair)
            .ok_or_else(|| Error::undecodable("Australia Post DPID invalid"))?;
        dpid.push(b'0' + d as u8);
    }

    // Customer Information + filler occupies the middle of the data region
    // (after FCC(4 bars) and DPID(16 bars)).
    let mid = &data_region[20..];
    let custinfo = decode_custinfo(mid);

    let mut segments = vec![Segment::numeric(dpid)];
    match custinfo {
        CustInfo::None => {}
        CustInfo::Numeric(bytes) => segments.push(Segment::numeric(bytes)),
        CustInfo::Bytes(bytes) => segments.push(Segment::byte(bytes)),
    }
    Ok((PostalVariant::AustraliaPost, segments))
}

/// A decoded Customer Information field.
enum CustInfo {
    /// No customer information (Standard Customer Barcode).
    None,
    /// Digits, decoded via the N table.
    Numeric(Vec<u8>),
    /// Graphic characters, decoded via the C table (raw bytes: the graphic set
    /// includes lower-case and `#`, which the alphanumeric mode cannot hold).
    Bytes(Vec<u8>),
}

/// Decode the Customer-Information + filler region, preferring the numeric
/// (N-table) interpretation. See the module docs on the N/C ambiguity.
fn decode_custinfo(mid: &[u8]) -> CustInfo {
    if mid.iter().all(|&x| x == 3) {
        return CustInfo::None;
    }
    if let Some(digits) = parse_n(mid) {
        return CustInfo::Numeric(digits);
    }
    if let Some(bytes) = parse_c(mid) {
        return CustInfo::Bytes(bytes);
    }
    CustInfo::None
}

/// Parse the region as N-table digit pairs followed by Tracker filler.
fn parse_n(mid: &[u8]) -> Option<Vec<u8>> {
    let mut end = mid.len();
    while end > 0 && mid[end - 1] == 3 {
        end -= 1;
    }
    let core = &mid[..end];
    if !core.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(core.len() / 2);
    for pair in core.chunks_exact(2) {
        let d = N_TABLE.iter().position(|&e| e == [pair[0], pair[1]])? as u8;
        out.push(b'0' + d);
    }
    Some(out)
}

/// Parse the region as C-table triples followed by Tracker filler. Because a
/// filler triple `(3,3,3)` is itself a valid C-table character, the split point
/// is taken as the *fewest* characters whose remaining bars are all Tracker
/// (i.e. maximal filler) — the interpretation the encoder produces.
fn parse_c(mid: &[u8]) -> Option<Vec<u8>> {
    let max_chars = mid.len() / 3;
    for chars in 0..=max_chars {
        let consumed = chars * 3;
        if mid[consumed..].iter().any(|&x| x != 3) {
            continue;
        }
        let mut out = Vec::with_capacity(chars);
        let mut ok = true;
        for tri in mid[..consumed].chunks_exact(3) {
            match C_TABLE.iter().position(|&e| e == [tri[0], tri[1], tri[2]]) {
                Some(i) => out.push(GDSET[i]),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(out);
        }
    }
    None
}

/// Extract the `(dpid, custinfo)` byte fields from an Australia Post symbol's
/// segments for re-encoding.
pub(super) fn fields(segments: &[Segment]) -> Result<(Vec<u8>, Vec<u8>)> {
    let dpid = segments
        .first()
        .filter(|s| matches!(s.mode, Mode::Numeric))
        .map(|s| s.data.clone())
        .ok_or_else(|| Error::invalid_data("Australia Post symbol missing DPID segment"))?;
    let custinfo = segments.get(1).map(|s| s.data.clone()).unwrap_or_default();
    Ok((dpid, custinfo))
}
