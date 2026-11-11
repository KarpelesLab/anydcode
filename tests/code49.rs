//! Code 49 end-to-end tests.
//!
//! 1. **Independent reference vectors** from zint 2.16.0 (`zint --barcode=CODE49
//!    --data=... --dump`): three documented symbols (alphanumeric, letters, and a
//!    numeric-mode symbol) are reproduced module-for-module from their code-character
//!    grids, exercising the even/odd encodation-pattern tables.
//! 2. `encode -> decode -> re-encode` identity, plus payload recovery.
#![cfg(all(feature = "decode", feature = "encode", feature = "code49"))]

use anyd::codes::code49::{Code49Decoder, Code49Encoder, Code49Meta};
use anyd::output::Encoding;
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn bits_from_hex(hex: &str, width: usize) -> Vec<bool> {
    let bytes: Vec<u8> = hex
        .split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect();
    (0..width)
        .map(|i| (bytes[i / 8] >> (7 - (i % 8))) & 1 == 1)
        .collect()
}

fn assert_render(grid: Vec<u8>, rows: usize, dump: &[&str]) {
    let meta = Code49Meta { rows, grid };
    let symbol = Symbol::new(Symbology::Code49, Vec::new(), SymbolMeta::Code49(meta));
    let Encoding::Matrix(m) = Code49Encoder::new().encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    assert_eq!(m.width(), 70);
    assert_eq!(m.height(), rows);
    for (row, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 70).iter().enumerate() {
            assert_eq!(m.get(x, row), b, "row {row} col {x} differs from zint");
        }
    }
}

#[test]
fn zint_reference_abcd1234() {
    assert_render(
        vec![10, 11, 12, 13, 1, 2, 3, 3, 4, 48, 26, 9, 17, 31, 0, 37],
        2,
        &["BF 6C A7 B0 BE 99 BB E6 BC", "AE 11 36 28 3B A3 A3 D8 BC"],
    );
    // ...and it decodes back.
    let meta = Code49Meta {
        rows: 2,
        grid: vec![10, 11, 12, 13, 1, 2, 3, 3, 4, 48, 26, 9, 17, 31, 0, 37],
    };
    let symbol = Symbol::new(Symbology::Code49, Vec::new(), SymbolMeta::Code49(meta));
    let encoding = Code49Encoder::new().encode(&symbol).unwrap();
    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.text().as_deref(), Some("ABCD1234"));
}

#[test]
fn zint_reference_hello() {
    assert_render(
        vec![17, 14, 21, 21, 24, 48, 48, 46, 48, 48, 47, 47, 39, 19, 0, 3],
        2,
        &["A0 8C AE 23 BF 76 B4 DF BC", "B3 CB B7 26 20 C5 B0 49 BC"],
    );
}

#[test]
fn zint_reference_numeric() {
    // Uses Code 49 Numeric Encodation (mode value 2) — a render-only vector.
    assert_render(
        vec![5, 17, 9, 29, 22, 18, 48, 1, 48, 48, 43, 23, 8, 0, 2, 25],
        2,
        &["B0 DE A1 D3 A8 2F B5 8F 3C", "B3 CB B7 3C B2 8C 2E E2 3C"],
    );
}

fn assert_lossless(data: &[u8]) {
    let enc = Code49Encoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), data, "payload differs");
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_various() {
    assert_lossless(b"A");
    assert_lossless(b"ABCD1234");
    assert_lossless(b"HELLO WORLD");
    assert_lossless(b"CODE-49 $/+%.");
    assert_lossless(b"1234567890"); // digits encode individually (no numeric mode)
    assert_lossless(b"Mixed Case abc XYZ 42");
    assert_lossless(b"\x01\x02\x1f ctrl");
    assert_lossless(&[b'Q'; 40]);
}

#[test]
fn explicit_min_rows() {
    let enc = Code49Encoder::new();
    let symbol = enc.build_rows(b"AB", Some(5)).unwrap();
    let SymbolMeta::Code49(meta) = &symbol.meta else {
        panic!();
    };
    assert_eq!(meta.rows, 5);
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.meta, symbol.meta);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn rejects_non_code49_symbol() {
    let enc = Code49Encoder::new();
    let bogus = Symbol::new(Symbology::Code128, Vec::new(), SymbolMeta::Generic);
    assert!(enc.encode(&bogus).is_err());
}

/// Decode a zint `--dump` as a Code 49 matrix and check payload + re-encode identity.
fn assert_zint_decodes(dump: &[&str], payload: &[u8]) {
    let mut m = anyd::output::BitMatrix::new(70, dump.len(), 10);
    for (y, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 70).iter().enumerate() {
            m.set(x, y, b);
        }
    }
    let encoding = Encoding::Matrix(m);
    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), payload);
    assert_eq!(Code49Encoder::new().encode(&decoded).unwrap(), encoding);
}

/// Numeric Encodation (toggled by codeword 48, or selected by starting mode 2): three
/// base-48 codewords carry five digits (or four, offset by 100000), two carry three
/// digits and one carries a single digit. Reference symbols from zint 2.16.0
/// (`zint -b CODE49 -d ... --dump`), which uses it for every run of five or more digits.
#[test]
fn zint_numeric_encodation_decodes() {
    // Starting mode 2; 5 + 5 + 1 digits.
    assert_zint_decodes(
        &["B0 DE A1 D3 A8 2F A3 B4 3C", "B3 CB B6 44 21 B3 A1 33 3C"],
        b"12345678901",
    );
    // 5 + 4 + 3 digits.
    assert_zint_decodes(
        &["B0 DE BD 67 2B F7 A1 68 3C", "B2 67 2B 3C 31 6E 30 98 BC"],
        b"123456789012",
    );
    // Numeric shift in and out between letters; 4 + 3 digits.
    assert_zint_decodes(
        &["B8 C9 28 36 38 88 AB CE 3C", "A6 C4 36 5F 27 CB BA EE 3C"],
        b"A1234567B",
    );
    // A numeric run, back to alphanumeric, then a short digit run left unshifted.
    assert_zint_decodes(
        &[
            "AF D0 A3 1B BD 23 22 5E 3C",
            "BA 38 BB D2 33 CB B6 47 BC",
            "B3 CB B2 D0 3B 16 28 33 3C",
        ],
        b"X12345Y678",
    );
}

/// The Shift characters act on the code-character stream, of which a Numeric
/// Encodation run is just a compacted stretch: `\` is Shift 2 + `8`, and when digits
/// follow, an encoder compacts that `8` together with them, leaving the Shift directly
/// before the numeric shift. Reference symbol from zint 2.16.0
/// (`zint -b CODE49 --esc -d 'A\\12345' --dump`), code characters
/// `10 44 48 35 12 18 5`: `A`, Shift 2, numeric shift, then `812345`.
#[test]
fn zint_shift_pair_completed_by_a_numeric_run() {
    assert_zint_decodes(
        &["B0 B6 31 91 28 9C 23 CE BC", "B3 CB A3 7A 24 FB A8 B8 3C"],
        b"A\\12345",
    );
}

/// FNC1 (code character 45): in the first position it flags GS1 data and carries no
/// payload; later it separates variable-length fields and is transmitted as GS.
/// Reference symbol from zint 2.16.0 (`zint -b CODE49 --gs1 -d '[10]AB[21]X' --dump`),
/// code characters `45 1 0 10 11 45 2 1 33`.
#[test]
fn zint_gs1_fnc1_decodes() {
    assert_zint_decodes(
        &["A4 4F 20 91 20 B7 B7 74 3C", "BC B9 A2 42 39 ED 20 B3 3C"],
        b"10AB\x1d21X",
    );
}

use anyd::error::Error;

/// A symbol holds 49 code characters. An upper-case letter or digit takes one, a
/// lower-case letter or control two (the first Shift of the symbol moves into the
/// starting mode): 49 / 25 of them fit, one more is a capacity error.
#[test]
fn capacity_is_49_code_characters() {
    let enc = Code49Encoder::new();
    for (byte, max) in [(b'K', 49usize), (b'7', 49), (b'k', 25), (0x07, 25)] {
        let symbol = enc.build(&vec![byte; max]).unwrap();
        let SymbolMeta::Code49(meta) = &symbol.meta else {
            panic!();
        };
        assert_eq!((meta.rows, meta.grid.len()), (8, 64), "{byte}");
        assert_lossless(&vec![byte; max]);
        for over in [max + 1, max + 2, 300] {
            let err = enc.build(&vec![byte; over]).unwrap_err();
            assert!(matches!(err, Error::Capacity { .. }), "{byte} x {over}");
        }
    }
}

/// Every row count is reachable, both by data length and by `min_rows` padding.
#[test]
fn every_row_count_roundtrips() {
    let enc = Code49Encoder::new();
    let dec = Code49Decoder::new();
    let mut seen = [false; 9];
    for n in 1..=49usize {
        let data: Vec<u8> = (0..n).map(|i| b'A' + (i % 26) as u8).collect();
        let symbol = enc.build(&data).unwrap();
        let SymbolMeta::Code49(meta) = &symbol.meta else {
            panic!();
        };
        assert_eq!(
            meta.grid[(meta.rows - 1) * 8 + 6] as usize,
            7 * (meta.rows - 2)
        );
        seen[meta.rows] = true;
        assert_lossless(&data);
    }
    assert_eq!(
        seen,
        [false, false, true, true, true, true, true, true, true]
    );

    for rows in 2..=8usize {
        let padded = enc.build_rows(b"x", Some(rows)).unwrap();
        let SymbolMeta::Code49(meta) = &padded.meta else {
            panic!();
        };
        assert_eq!(meta.rows, rows);
        let encoding = enc.encode(&padded).unwrap();
        assert_eq!(dec.decode(&encoding).unwrap(), padded);
    }
}

/// Every ASCII byte, followed by a spread of second bytes from each character class,
/// survives the code-character planning and its inverse — at the start of the symbol
/// (where a leading Shift 1 / Shift 2 becomes starting mode 4 / 5) and after another
/// character; a sample also goes through the module matrix and back.
#[test]
fn every_byte_roundtrips_in_context() {
    let enc = Code49Encoder::new();
    let dec = Code49Decoder::new();
    for prefix in [&b""[..], b"Z"] {
        for a in 0..=127u8 {
            for b in (0..=127u8).step_by(11) {
                let mut data = prefix.to_vec();
                data.extend_from_slice(&[a, b]);
                let symbol = enc.build(&data).unwrap();
                assert_eq!(symbol.payload_bytes(), data);
                if a % 16 == 5 && b == 66 {
                    let encoding = enc.encode(&symbol).unwrap();
                    assert_eq!(dec.decode(&encoding).unwrap(), symbol, "{data:?}");
                }
            }
        }
    }
}

/// The parameter and input error paths of the public API.
#[test]
fn rejects_bad_parameters_and_input() {
    let enc = Code49Encoder::new();
    let dec = Code49Decoder::new();

    assert!(matches!(enc.build(b""), Err(Error::InvalidData { .. })));
    assert!(matches!(
        enc.build(b"A\x80"),
        Err(Error::InvalidData { .. })
    ));
    for rows in [0, 1, 9, 100] {
        assert!(
            matches!(
                enc.build_rows(b"A", Some(rows)),
                Err(Error::InvalidParameter { .. })
            ),
            "{rows} rows"
        );
    }

    // Metadata that is not a symbol. The grid stores its own check and mode
    // characters, so a grid that disagrees with itself must be refused: it would
    // render, but no decoder (this one included) would accept the result.
    let good = vec![10u8, 11, 12, 13, 1, 2, 3, 3, 4, 48, 26, 9, 17, 31, 0, 37];
    let symbol = |rows: usize, grid: Vec<u8>| {
        Symbol::new(
            Symbology::Code49,
            Vec::new(),
            SymbolMeta::Code49(Code49Meta { rows, grid }),
        )
    };
    assert!(enc.encode(&symbol(2, good.clone())).is_ok());
    let edit = |i: usize, v: u8| {
        let mut g = good.clone();
        g[i] = v;
        g
    };
    let bad = [
        symbol(1, good[..8].to_vec()),
        symbol(9, vec![48; 72]),
        symbol(3, good.clone()),
        symbol(2, good[..15].to_vec()),
        symbol(2, edit(0, 49)), // not a code character
        symbol(2, edit(0, 11)), // data changed, checks stale
        symbol(2, edit(7, 4)),  // row check
        symbol(2, edit(15, 0)), // last-row check
        symbol(2, edit(12, 0)), // symbol check
        symbol(2, edit(14, 7)), // mode character says three rows
        Symbol::new(Symbology::Code49, Vec::new(), SymbolMeta::Generic),
    ];
    for s in bad {
        assert!(
            matches!(enc.encode(&s), Err(Error::InvalidParameter { .. })),
            "{:?}",
            s.meta
        );
    }

    // Decoder input that cannot be a Code 49 grid.
    let linear = Encoding::Linear(anyd::output::LinearPattern {
        modules: vec![true; 70],
        quiet_zone: 10,
    });
    assert!(matches!(
        dec.decode(&linear),
        Err(Error::Unsupported { .. })
    ));
    for (width, height) in [(70, 1), (70, 9), (69, 4), (71, 4), (0, 0)] {
        let m = anyd::output::BitMatrix::new(width, height, 10);
        assert!(dec.decode_matrix(&m).is_err(), "{width}x{height}");
    }

    // A single-module flip anywhere in a valid symbol is caught by a pattern, parity
    // or check-character test.
    let Encoding::Matrix(m) = enc.encode(&enc.build(b"Flip me 42").unwrap()).unwrap() else {
        panic!("expected matrix");
    };
    for y in 0..m.height() {
        for x in (y..m.width()).step_by(5) {
            let mut damaged = m.clone();
            damaged.set(x, y, !m.get(x, y));
            assert!(dec.decode_matrix(&damaged).is_err(), "flip ({x}, {y})");
        }
    }
}
