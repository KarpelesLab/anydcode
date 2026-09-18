//! DotCode integration tests: an independent reference bitmap plus
//! encode→decode→re-encode round-trip identity across code sets and sizes.
#![cfg(all(feature = "decode", feature = "encode", feature = "dotcode"))]

use anyd::codes::dotcode::{DotCodeDecoder, DotCodeEncoder};
use anyd::output::Encoding;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// Render an [`Encoding::Matrix`] to `'0'`/`'1'` rows for comparison with the
/// documented dot bitmaps.
fn matrix_rows(encoding: &Encoding) -> Vec<String> {
    match encoding {
        Encoding::Matrix(m) => (0..m.height())
            .map(|y| {
                (0..m.width())
                    .map(|x| if m.get(x, y) { '1' } else { '0' })
                    .collect()
            })
            .collect(),
        Encoding::Linear(_) => panic!("DotCode must produce a matrix"),
    }
}

/// INDEPENDENT REFERENCE VECTOR.
///
/// AIM ISS DotCode Rev 4.0 Figure 6 (top-right), reproduced as zint
/// `backend/tests/test_dotcode.c` `test_encode` vector /*6*/ (auto mask → 1) for the
/// GS1 payload `[17]070620[10]ABC123456`. The bracketed AIs reduce to the byte
/// string `1707062010ABC123456` (AI 17 is fixed-length 6, AI 10 is last); this
/// reduction is itself cross-checked by zint dump vector /*24*/. Building through the
/// public API and rendering the matrix must reproduce the documented dots exactly.
#[test]
fn reference_bitmap_rev4_figure6() {
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

    let encoder = DotCodeEncoder::new();
    let symbol = encoder.build_gs1_reduced(b"1707062010ABC123456").unwrap();
    let encoding = encoder.encode(&symbol).unwrap();
    assert_eq!(matrix_rows(&encoding), expected);
}

/// Encode → decode → re-encode identity, plus meta and payload recovery.
fn assert_roundtrip(symbol: &anyd::Symbol) {
    let encoder = DotCodeEncoder::new();
    let decoder = DotCodeDecoder::new();

    let encoding = encoder.encode(symbol).unwrap();
    let decoded = decoder.decode(&encoding).unwrap();

    // Re-encoding the decoded symbol reproduces the identical matrix.
    let reencoded = encoder.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoded matrix differs");

    // The pinned re-encode metadata survives the round-trip exactly.
    assert_eq!(decoded.symbology, Symbology::DotCode);
    match (&symbol.meta, &decoded.meta) {
        (SymbolMeta::DotCode(a), SymbolMeta::DotCode(b)) => assert_eq!(a, b, "meta differs"),
        _ => panic!("expected DotCodeMeta"),
    }
}

#[test]
fn roundtrip_across_code_sets_and_sizes() {
    let encoder = DotCodeEncoder::new();

    let cases: &[&[u8]] = &[
        b"A",
        b"AB",
        b"Hello, World!",
        b"ABCDE12345678",
        b"1234567890",
        b"1712345610",
        b"00",
        b"\x00ABCD1234567890",
        b"99aA[{00\x00",
        b"The quick brown fox jumps over the lazy dog 0123456789",
        b"lower case letters and DIGITS 42 mixed",
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        b"9999999999999999999999999999999999999999",
    ];
    for &data in cases {
        let symbol = encoder.build_bytes(data).unwrap();
        assert_roundtrip(&symbol);
        // Payload bytes must be recovered verbatim by the decoder.
        let decoded = DotCodeDecoder::new()
            .decode(&encoder.encode(&symbol).unwrap())
            .unwrap();
        assert_eq!(
            decoded.payload_bytes(),
            data,
            "payload mismatch for {data:?}"
        );
    }
}

#[test]
fn roundtrip_binary_and_high_bytes() {
    let encoder = DotCodeEncoder::new();
    let cases: Vec<Vec<u8>> = vec![
        vec![0x80, 0x81, 0x82, 0x83, 0x31, 0x32, 0x33, 0x34],
        b"abcde\x80\x81\x82\x83\x84\xff".to_vec(),
        (0u16..=255).map(|b| b as u8).collect(),
        vec![0xff; 40],
        b"text then binary \x80\x90\xa0\xb0 and more".to_vec(),
    ];
    for data in &cases {
        let symbol = encoder.build_bytes(data).unwrap();
        assert_roundtrip(&symbol);
        let decoded = DotCodeDecoder::new()
            .decode(&encoder.encode(&symbol).unwrap())
            .unwrap();
        assert_eq!(decoded.payload_bytes(), *data, "binary payload mismatch");
    }
}

#[test]
fn roundtrip_user_width() {
    let encoder = DotCodeEncoder::new();
    for width in [7usize, 13, 20, 30] {
        let symbol = encoder
            .build_bytes_width(b"WIDTH TEST 12345", width)
            .unwrap();
        assert_roundtrip(&symbol);
    }
}

#[test]
fn roundtrip_gs1_reduced() {
    let encoder = DotCodeEncoder::new();
    let symbol = encoder.build_gs1_reduced(b"1707062010ABC123456").unwrap();
    assert_roundtrip(&symbol);
}

#[test]
fn rejects_non_dotcode_symbol() {
    // A tampered matrix (all dark) should fail the Reed–Solomon check.
    let encoder = DotCodeEncoder::new();
    let symbol = encoder.build_bytes(b"CHECKSUM").unwrap();
    let mut encoding = encoder.encode(&symbol).unwrap();
    if let Encoding::Matrix(m) = &mut encoding {
        // Flip several data modules to force an RS mismatch.
        let w = m.width();
        for y in 0..m.height() {
            for x in 0..w {
                if (x + y) % 2 == 0 {
                    m.set(x, y, !m.get(x, y));
                }
            }
        }
    }
    assert!(DotCodeDecoder::new().decode(&encoding).is_err());
}

/// Encode hand-built (unmasked, already padded) data codewords straight from meta.
fn symbol_from_codewords(width: usize, height: usize, codewords: &[u8]) -> anyd::Symbol {
    anyd::Symbol::new(
        Symbology::DotCode,
        vec![],
        SymbolMeta::DotCode(anyd::codes::dotcode::DotCodeMeta {
            width,
            height,
            mask: 0,
            codewords: codewords.to_vec(),
        }),
    )
}

#[test]
fn hostile_codeword_streams_never_panic() {
    // A symbol with valid Reed-Solomon check words can carry any codeword sequence,
    // including shifts whose operand is out of range for the shifted set (an Upper
    // Shift B operand above 95 would overflow a byte) or that run off the end.
    let enc = DotCodeEncoder::new();
    let dec = DotCodeDecoder::new();
    let mut seed = 0xD07C_0DE5_EED0_0001u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut streams: Vec<Vec<u8>> = vec![
        vec![111, 112, 106, 106, 106, 106, 106, 106, 106],
        vec![110, 112, 106, 106, 106, 106, 106, 106, 106],
        vec![106, 111, 96, 111, 112, 110, 100, 110, 112],
        vec![112, 102, 102, 102, 102, 102, 102, 102, 111],
        vec![108, 112, 112, 112, 106, 106, 106, 106, 111],
    ];
    for _ in 0..600 {
        // Bias towards the control codewords 96..=112.
        streams.push(
            (0..9)
                .map(|_| match next() % 3 {
                    0 => 96 + (next() % 17) as u8,
                    _ => (next() % 113) as u8,
                })
                .collect(),
        );
    }
    for stream in &streams {
        // 9 data + 7 check codewords: 2 + 16 * 9 = 146 dots fit 21 x 14 exactly.
        let encoding = enc.encode(&symbol_from_codewords(21, 14, stream)).unwrap();
        let decoded = dec.decode(&encoding).unwrap();
        assert_eq!(enc.encode(&decoded).unwrap(), encoding, "stream {stream:?}");
    }
}

#[test]
fn codewords_that_do_not_fit_are_an_error() {
    // 21 x 14 holds 147 dots: 2 mask bits and 16 codewords (9 data + 7 check). One
    // data codeword more needs 17 and must be refused, not silently truncated.
    let enc = DotCodeEncoder::new();
    assert!(enc.encode(&symbol_from_codewords(21, 14, &[1; 9])).is_ok());
    assert!(
        enc.encode(&symbol_from_codewords(21, 14, &[1; 10]))
            .is_err()
    );
    assert!(enc.encode(&symbol_from_codewords(5, 6, &[1; 40])).is_err());
}

#[test]
fn decoder_never_panics_on_garbage() {
    let mut seed = 0xD07C_0DE0_F022_0001u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let dec = DotCodeDecoder::new();
    for (w, h) in [
        (0usize, 0usize),
        (4, 5),
        (5, 5),
        (5, 6),
        (6, 5),
        (7, 10),
        (10, 7),
        (13, 56),
        (56, 13),
        (21, 14),
        (200, 199),
        (199, 200),
        (201, 200),
        (200, 200),
    ] {
        for density in [0u64, 1, 2, 4] {
            let mut m = anyd::output::BitMatrix::new(w, h, 3);
            for y in 0..h {
                for x in 0..w {
                    m.set(x, y, (x + y) % 2 == 0 && next() % 4 < density);
                }
            }
            let _ = dec.decode(&Encoding::Matrix(m));
        }
    }
    // Valid symbols with flipped dots: never a panic.
    let enc = DotCodeEncoder::new();
    for len in [1usize, 5, 20, 80, 300] {
        let payload: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let symbol = enc.build_bytes(&payload).unwrap();
        let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
            panic!("expected matrix");
        };
        let (w, h) = (clean.width(), clean.height());
        for flips in [1usize, 3, 10, 100] {
            for _ in 0..10 {
                let mut m = clean.clone();
                for _ in 0..flips {
                    let (x, y) = ((next() % w as u64) as usize, (next() % h as u64) as usize);
                    let v = m.get(x, y);
                    m.set(x, y, !v);
                }
                let _ = dec.decode(&Encoding::Matrix(m));
            }
        }
    }
}

/// `zint -b DOTCODE -d Ba` (13 x 10). No plain mask lights this symbol's edges well
/// enough, so zint settles on a corner-forced mask (4-7): the six corner dots are lit
/// over whatever the check words put there, and Reed-Solomon repairs them on reading.
#[rustfmt::skip]
const ZINT_BA_FORCED_CORNERS: &[&str] = &[
    "1000001010101",
    "0101010000000",
    "0000100000001",
    "0100010101010",
    "1010100000101",
    "0101000101000",
    "0010001000100",
    "0001000001000",
    "1000100010101",
    "0100010101010",
];

#[test]
fn matches_zint_forced_corner_mask() {
    let mut m = anyd::output::BitMatrix::new(13, 10, 3);
    for (y, row) in ZINT_BA_FORCED_CORNERS.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            m.set(x, y, c == b'1');
        }
    }
    let reference = Encoding::Matrix(m);
    let decoded = DotCodeDecoder::new().decode(&reference).unwrap();
    assert_eq!(decoded.payload_bytes(), b"Ba");
    let SymbolMeta::DotCode(meta) = &decoded.meta else {
        panic!("expected DotCodeMeta");
    };
    assert!((4..=7).contains(&meta.mask), "mask {}", meta.mask);
    // Both the decoded symbol and a fresh build reproduce zint's dots exactly.
    let enc = DotCodeEncoder::new();
    assert_eq!(enc.encode(&decoded).unwrap(), reference);
    let built = enc.build_bytes(b"Ba").unwrap();
    assert_eq!(enc.encode(&built).unwrap(), reference);
}

#[test]
fn corrects_damaged_dots() {
    // A flipped dot turns its 5-of-9 pattern into an invalid one: an erasure. A
    // symbol with `nc` check codewords recovers from `nc` erasures, so any `nc / 2`
    // flipped dots (each spoiling at most one codeword) must always be repaired.
    let mut seed = 0xD07C_0DE0_EC00_0001u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let enc = DotCodeEncoder::new();
    let dec = DotCodeDecoder::new();
    for payload in [
        &b"A"[..],
        b"DotCode 2026",
        b"0123456789012345678901234567890123456789",
        &[0x80, 0xFF, 0x00, 0x7F, 0xC3, 0xA9],
        &[b'x'; 400],
    ] {
        let symbol = enc.build_bytes(payload).unwrap();
        let SymbolMeta::DotCode(meta) = &symbol.meta else {
            panic!("expected DotCodeMeta");
        };
        let nc = 3 + meta.codewords.len() / 2;
        // Interleaved blocks (long messages) each hold a share of the check words.
        let blocks = (meta.codewords.len() + 1 + nc).div_ceil(112);
        let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
            panic!("expected matrix");
        };
        let (w, h) = (clean.width(), clean.height());
        for _ in 0..20 {
            let mut m = clean.clone();
            for _ in 0..(nc / blocks - 1) / 2 {
                let (x, y) = loop {
                    let (x, y) = ((next() % w as u64) as usize, (next() % h as u64) as usize);
                    if (x + y) % 2 == 0 {
                        break (x, y);
                    }
                };
                let v = m.get(x, y);
                m.set(x, y, !v);
            }
            let damaged = Encoding::Matrix(m);
            let decoded = dec.decode(&damaged).unwrap();
            assert_eq!(decoded.payload_bytes(), payload);
            // Re-encoding yields the pristine symbol, not the damaged one (unless the
            // damage is exactly what a corner-forced mask would have produced).
            let again = enc.encode(&decoded).unwrap();
            assert!(again == Encoding::Matrix(clean.clone()) || again == damaged);
        }
    }
}

#[test]
fn random_payloads_round_trip_at_any_width() {
    // Payloads drawn from alphabets that stress each code set, the binary latch and
    // the digit-pair look-ahead, at automatic and forced widths. Every build must
    // decode back to the same bytes and re-encode identically; a refusal must be a
    // clean capacity error.
    let mut seed = 0xD07C_0DE0_B17D_0001u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alphabets: [&[u8]; 6] = [
        b"0123456789",
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        b"abcxyz 0189,.-\r\n",
        b"\x00\x01\x09\x1c\x1d\x1e\x1fAB12",
        b"\x80\x9f\xa0\xff\xc3\xa9 aZ09",
        b"17010203101710",
    ];
    let enc = DotCodeEncoder::new();
    let dec = DotCodeDecoder::new();
    for round in 0..400 {
        let alphabet = alphabets[round % alphabets.len()];
        let len = 1 + (next() % 60) as usize;
        let payload: Vec<u8> = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let built = if round % 3 == 0 {
            enc.build_bytes_width(&payload, 5 + (next() % 196) as usize)
        } else {
            enc.build_bytes(&payload)
        };
        let symbol = match built {
            Ok(symbol) => symbol,
            Err(anyd::Error::Capacity { .. }) => continue,
            Err(e) => panic!("unexpected error {e} for {payload:?}"),
        };
        let encoding = enc.encode(&symbol).unwrap();
        let decoded = dec.decode(&encoding).unwrap();
        assert_eq!(decoded.payload_bytes(), payload, "payload {payload:?}");
        assert_eq!(decoded.meta, symbol.meta, "payload {payload:?}");
        assert_eq!(enc.encode(&decoded).unwrap(), encoding);
    }
}

#[test]
fn macro_messages_decode_to_the_original_bytes() {
    // "[)>RS05GS...RSEOT" (and 06, 12, or any other two-digit format with a bare EOT)
    // is compacted to Latch B plus a macro codeword 97-100; the decoder must expand
    // the header and trailer again rather than read 97-100 as HT/FS/GS/RS.
    let enc = DotCodeEncoder::new();
    for payload in [
        &b"[)>\x1e05\x1dABC123\x1e\x04"[..],
        b"[)>\x1e06\x1dABC123\x1e\x04",
        b"[)>\x1e12\x1dabc\x1e\x04",
        b"[)>\x1e07ABC\x04",
        b"[)>\x1e99\x1d12345678\x1e\x04",
        // Not macros: no EOT, or a leading special that merely looks similar.
        b"[)>\x1e05\x1dABC",
        b"\x1dABC",
        b"\tTAB\x1e\x04",
    ] {
        let symbol = enc.build_bytes(payload).unwrap();
        let decoded = DotCodeDecoder::new()
            .decode(&enc.encode(&symbol).unwrap())
            .unwrap();
        assert_eq!(decoded.payload_bytes(), payload, "payload {payload:?}");
        assert_roundtrip(&symbol);
    }
}
