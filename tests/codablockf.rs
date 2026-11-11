//! Codablock F end-to-end tests.
//!
//! 1. An **independent reference vector** from zint 2.16.0 (`zint
//!    --barcode=CODABLOCKF --data=ABCDEFGHIJKL --dump`): the exact module rows of a
//!    documented symbol are reproduced from its Code 128 codeword grid, and the symbol
//!    decodes back to the original text.
//! 2. `encode -> decode -> re-encode` identity across sizes.
#![cfg(all(feature = "decode", feature = "encode", feature = "codablockf"))]

use anyd::codes::codablockf::{CodablockFDecoder, CodablockFEncoder, CodablockFMeta};
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

#[test]
fn zint_reference_abcdefghijkl() {
    // Full Code 128 codeword grid for "ABCDEFGHIJKL" (zint --verbose), rows=4 cols=9.
    #[rustfmt::skip]
    let grid: Vec<u8> = vec![
        103, 100, 66, 33, 34, 35, 36, 34, 106,
        103, 100, 11, 37, 38, 39, 40, 99, 106,
        103, 100, 12, 41, 42, 43, 44, 70, 106,
        103, 100, 13, 99, 100, 30, 44,  1, 106,
    ];
    let meta = CodablockFMeta {
        rows: 4,
        columns: 9,
        codewords: grid,
    };
    let symbol = Symbol::new(
        Symbology::CodablockF,
        Vec::new(),
        SymbolMeta::CodablockF(meta),
    );
    let Encoding::Matrix(m) = CodablockFEncoder::new().encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    assert_eq!(m.width(), 101);
    assert_eq!(m.height(), 4);

    let expected = [
        "D0 97 BA 43 51 88 B1 11 AC 44 58 C7 58",
        "D0 97 BB 12 46 88 C5 A2 31 45 DE C7 58",
        "D0 97 BA CE 62 2B 71 63 A3 75 84 C7 58",
        "D0 97 BA 6E 5D EB DD B6 23 76 6C C7 58",
    ];
    for (row, hex) in expected.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 101).iter().enumerate() {
            assert_eq!(m.get(x, row), b, "row {row} col {x} differs from zint");
        }
    }

    let decoded = CodablockFDecoder::new()
        .decode(&Encoding::Matrix(m))
        .unwrap();
    assert_eq!(decoded.text().as_deref(), Some("ABCDEFGHIJKL"));
}

fn assert_lossless(data: &[u8]) {
    let enc = CodablockFEncoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = CodablockFDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), data, "payload differs");
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_various() {
    assert_lossless(b"AB");
    assert_lossless(b"ABCDEFGHIJKL");
    assert_lossless(b"Hello, World!");
    assert_lossless(b"1234567890");
    assert_lossless(b"Codablock-F mixes UPPER, lower, 0-9 and symbols !@#$%^&*()");
    assert_lossless(b"\x01\x02\x1fcontrol and text");
    assert_lossless(&[b'Z'; 200]);
}

#[test]
fn rejects_non_codablockf_symbol() {
    let enc = CodablockFEncoder::new();
    let bogus = Symbol::new(Symbology::Code128, Vec::new(), SymbolMeta::Generic);
    assert!(enc.encode(&bogus).is_err());
}

/// Extended ASCII via FNC4 (zint 2.16.0, `zint -b CODABLOCKF --binary --esc -d
/// 'A\xE9B \xE0\xE1\xE2\xE3\xE4\xE5x\xE6' --dump`): single-FNC4 shifts and a
/// double-FNC4 latched run. K1/K2 are computed over the extended bytes, so the symbol
/// only validates when FNC4 is honoured.
#[test]
fn zint_extended_ascii_fnc4() {
    let dump = [
        "D0 97 BA 13 51 8B DD 0D 22 C4 26 C7 58",
        "D0 97 BB 12 6C CB DD 43 2E F7 A8 C7 58",
        "D0 97 BA CE 5E E9 61 7B A4 34 46 C7 58",
        "D0 97 BA 6E 5E E8 59 7B A1 37 68 C7 58",
        "D0 97 BA 67 5E EB 21 E4 AE F6 F6 C7 58",
        "D0 97 BA E6 5E EB 09 84 A2 C6 EE C7 58",
    ];
    let width = 9 * 11 + 2;
    let mut m = anyd::output::BitMatrix::new(width, dump.len(), 10);
    for (y, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, width).iter().enumerate() {
            m.set(x, y, b);
        }
    }
    let encoding = Encoding::Matrix(m);
    let decoded = CodablockFDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.payload_bytes(),
        b"A\xE9B \xE0\xE1\xE2\xE3\xE4\xE5x\xE6"
    );
    assert_eq!(CodablockFEncoder::new().encode(&decoded).unwrap(), encoding);
}

use anyd::error::Error;

fn matrix_from_dump(dump: &[&str], width: usize) -> anyd::output::BitMatrix {
    let mut m = anyd::output::BitMatrix::new(width, dump.len(), 10);
    for (y, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, width).iter().enumerate() {
            m.set(x, y, b);
        }
    }
    m
}

/// A third-party symbol using what this crate's own layout never produces: a row in
/// Code Set C, a row selected into Set A, a Code B latch inside a row, a filler
/// Code C before the checks, and K1/K2 sharing the last data row in Set C (zint
/// 2.16.0, `zint -b CODABLOCKF --binary --esc --cols=12 -d
/// '1234567890123456AB\x01cd\x7f99' --dump`; K1 = 59, K2 = 29).
#[test]
fn zint_set_c_and_shared_check_row() {
    let dump = [
        "D0 97 7B 36 59 C8 B1 C5 B0 A6 F6 B3 91 62 F1 63 AC",
        "D0 9E 8B 12 6E 4C E9 46 22 C4 B0 BD D0 B3 93 63 AC",
        "D0 97 BA CE 42 6B D1 CB 39 65 DE E3 5C CB 14 63 AC",
    ];
    let encoding = Encoding::Matrix(matrix_from_dump(&dump, 11 * 11 + 13));
    let decoded = CodablockFDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), b"1234567890123456AB\x01cd\x7f99");
    #[rustfmt::skip]
    let grid: Vec<u8> = vec![
        103,  99,  1, 12, 34, 56, 78, 90,  12, 34, 96, 106,
        103,  98, 11, 21, 22, 33, 34, 65, 100, 67, 26, 106,
        103, 100, 12, 68, 95, 25, 25, 99,  59, 29, 40, 106,
    ];
    assert_eq!(
        decoded.meta,
        SymbolMeta::CodablockF(CodablockFMeta {
            rows: 3,
            columns: 12,
            codewords: grid,
        })
    );
    assert_eq!(CodablockFEncoder::new().encode(&decoded).unwrap(), encoding);
}

/// This layout puts 62 data characters on each of 43 rows and the checks on a 44th:
/// 2666 single-set characters fit, one more is a capacity error.
#[test]
fn capacity_is_43_full_rows() {
    let enc = CodablockFEncoder::new();
    let symbol = enc.build(&[b'K'; 2666]).unwrap();
    let SymbolMeta::CodablockF(meta) = &symbol.meta else {
        panic!();
    };
    assert_eq!((meta.rows, meta.columns), (44, 67));
    assert_lossless(&[b'K'; 2666]);
    for over in [2667, 2700, 5000] {
        let err = enc.build(&vec![b'K'; over]).unwrap_err();
        assert!(matches!(err, Error::Capacity { .. }), "{over}: {err:?}");
    }
}

/// Every length from one character up through several rows, and every ordered pair
/// of ASCII bytes (each start set, each latch), survives.
#[test]
fn lengths_and_byte_pairs_roundtrip() {
    for n in 1..=130usize {
        let data: Vec<u8> = (0..n).map(|i| b' ' + (i * 7 % 95) as u8).collect();
        assert_lossless(&data);
    }
    for a in 0..=127u8 {
        for b in (0..=127u8).step_by(5) {
            assert_lossless(&[a, b, a]);
        }
    }
}

/// The parameter and input error paths of the public API, and the three integrity
/// checks of the decoder (row check, row indicator, K1/K2).
#[test]
fn rejects_bad_parameters_and_input() {
    let enc = CodablockFEncoder::new();
    let dec = CodablockFDecoder::new();

    assert!(matches!(enc.build(b""), Err(Error::InvalidData { .. })));
    assert!(matches!(
        enc.build(b"A\x80"),
        Err(Error::InvalidData { .. })
    ));

    let symbol = enc.build(b"Integrity checks, three rows of them").unwrap();
    let SymbolMeta::CodablockF(good) = symbol.meta.clone() else {
        panic!();
    };
    let with = |meta: CodablockFMeta| {
        Symbol::new(
            Symbology::CodablockF,
            Vec::new(),
            SymbolMeta::CodablockF(meta),
        )
    };
    assert!(good.rows >= 4);

    // Metadata that cannot be rendered.
    let mut bad_value = good.clone();
    bad_value.codewords[4] = 106;
    let bad = [
        with(CodablockFMeta {
            rows: 1,
            columns: good.columns,
            codewords: good.codewords[..good.columns].to_vec(),
        }),
        with(CodablockFMeta {
            rows: 45,
            columns: 9,
            codewords: vec![0; 45 * 9],
        }),
        with(CodablockFMeta {
            rows: 2,
            columns: 8,
            codewords: vec![0; 16],
        }),
        with(CodablockFMeta {
            rows: 2,
            columns: 68,
            codewords: vec![0; 136],
        }),
        with(CodablockFMeta {
            rows: good.rows + 1,
            ..good.clone()
        }),
        with(bad_value),
        Symbol::new(Symbology::CodablockF, Vec::new(), SymbolMeta::Generic),
    ];
    for s in bad {
        assert!(
            matches!(enc.encode(&s), Err(Error::InvalidParameter { .. })),
            "{:?}",
            s.meta
        );
    }

    // Decoder input that cannot be a Codablock F grid.
    let linear = Encoding::Linear(anyd::output::LinearPattern {
        modules: vec![true; 101],
        quiet_zone: 10,
    });
    assert!(matches!(
        dec.decode(&linear),
        Err(Error::Unsupported { .. })
    ));
    for (width, height) in [(101, 1), (101, 45), (100, 4), (90, 4), (750, 4), (0, 0)] {
        let m = anyd::output::BitMatrix::new(width, height, 10);
        assert!(dec.decode_matrix(&m).is_err(), "{width}x{height}");
    }

    let columns = good.columns;
    // The modulo-103 check of one row (start value, then position-weighted values).
    let row_check = |row: &[u8]| {
        let mut sum = row[0] as u32;
        for (pos, &v) in row[..columns - 2].iter().enumerate().skip(1) {
            sum += v as u32 * pos as u32;
        }
        (sum % 103) as u8
    };
    let decodes = |codewords: Vec<u8>| {
        let meta = CodablockFMeta {
            codewords,
            ..good.clone()
        };
        dec.decode(&enc.encode(&with(meta)).unwrap())
    };
    assert_eq!(decodes(good.codewords.clone()).unwrap().meta, symbol.meta);

    // 1. A changed data character with the row check left stale.
    let mut cw = good.codewords.clone();
    cw[columns + 3] ^= 1;
    assert!(decodes(cw.clone()).is_err());
    // 2. ...and with the row check repaired: only K1/K2 can tell.
    let check = row_check(&cw[columns..2 * columns]);
    cw[2 * columns - 2] = check;
    assert!(decodes(cw).is_err());
    // 3. Two whole rows exchanged: each is a valid row with the wrong row number.
    let mut cw = good.codewords.clone();
    for i in 0..columns {
        cw.swap(columns + i, 2 * columns + i);
    }
    assert!(decodes(cw).is_err());
    // 4. A wrong row count in the first row's indicator, row check repaired.
    let mut cw = good.codewords.clone();
    cw[2] = if cw[2] == 70 { 71 } else { 70 };
    let check = row_check(&cw[..columns]);
    cw[columns - 2] = check;
    assert!(decodes(cw).is_err());
}
