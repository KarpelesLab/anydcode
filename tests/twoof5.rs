//! Standard/IATA/Matrix 2 of 5: encode → decode → re-encode identity per variant.
#![cfg(all(feature = "decode", feature = "encode", feature = "twoof5"))]

use anyd::codes::twoof5::{TwoOf5Decoder, TwoOf5Encoder};
use anyd::segment::Segment;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn assert_lossless(sym: Symbology, digits: &[u8]) {
    let enc = TwoOf5Encoder::new();
    let symbol = enc.build(sym, digits).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = TwoOf5Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, sym, "variant not recovered");
    assert_eq!(decoded.segments, vec![Segment::numeric(digits.to_vec())]);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
}

#[test]
fn variant_matrix() {
    for sym in [
        Symbology::Std2of5,
        Symbology::Iata2of5,
        Symbology::Matrix2of5,
    ] {
        assert_lossless(sym, b"0123456789");
        assert_lossless(sym, b"7");
        assert_lossless(sym, b"42");
    }
}

#[test]
fn variants_are_distinct_encodings() {
    // The three dialects differ (start/stop), so their encodings must not coincide.
    let enc = TwoOf5Encoder::new();
    let std = enc
        .encode(&enc.build(Symbology::Std2of5, b"123").unwrap())
        .unwrap();
    let iata = enc
        .encode(&enc.build(Symbology::Iata2of5, b"123").unwrap())
        .unwrap();
    let matrix = enc
        .encode(&enc.build(Symbology::Matrix2of5, b"123").unwrap())
        .unwrap();
    assert_ne!(std, iata);
    assert_ne!(std, matrix);
    assert_ne!(iata, matrix);
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::output::{Encoding, LinearBuf};
    let enc = TwoOf5Encoder::new();
    for sym in [
        Symbology::Std2of5,
        Symbology::Iata2of5,
        Symbology::Matrix2of5,
    ] {
        for digits in [&b"7"[..], b"42", b"123", b"0123456789", b"98765432109876"] {
            let symbol = enc.build(sym, digits).unwrap();
            let Encoding::Linear(expected) = enc.encode(&symbol).unwrap() else {
                panic!("2-of-5 encodes to a linear pattern");
            };
            let mut storage = [0u8; 32];
            let mut buf = LinearBuf::new(&mut storage);
            enc.encode_into(sym, digits, &mut buf).unwrap();
            assert_eq!(buf, expected);
            assert_eq!(buf.len(), TwoOf5Encoder::max_modules(sym, digits.len()));
            // Too-small storage reports a capacity error instead of panicking.
            let mut tiny = [0u8; 2];
            let err = enc.encode_into(sym, digits, &mut LinearBuf::new(&mut tiny));
            assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        }
        // Invalid input is rejected before anything is written.
        for bad in [&b""[..], b"12A4"] {
            let mut storage = [0u8; 32];
            let mut buf = LinearBuf::new(&mut storage);
            assert!(enc.encode_into(sym, bad, &mut buf).is_err());
            assert_eq!(buf.len(), 0);
        }
    }
    let mut storage = [0u8; 32];
    let err = enc.encode_into(Symbology::Itf, b"12", &mut LinearBuf::new(&mut storage));
    assert!(err.is_err());
    // The fallback bound covers every dialect.
    for n in 1..8 {
        let bound = TwoOf5Encoder::max_modules(Symbology::Itf, n);
        for sym in [
            Symbology::Std2of5,
            Symbology::Iata2of5,
            Symbology::Matrix2of5,
        ] {
            assert!(TwoOf5Encoder::max_modules(sym, n) <= bound);
        }
    }
}

/// Re-draw a module row, replacing the run at index `run` with one `width` modules wide.
fn with_run_width(modules: &[bool], run: usize, width: usize) -> Vec<bool> {
    let mut out = Vec::new();
    let (mut i, mut k) = (0, 0);
    while i < modules.len() {
        let mut j = i;
        while j < modules.len() && modules[j] == modules[i] {
            j += 1;
        }
        let w = if k == run { width } else { j - i };
        out.extend(std::iter::repeat_n(modules[i], w));
        (i, k) = (j, k + 1);
    }
    out
}

/// Standard and IATA 2 of 5 carry data in the bars only: every space is narrow. A
/// wide space (or a wide Matrix inter-character space) is not part of the symbology.
#[test]
fn wide_spaces_are_rejected() {
    use anyd::output::{Encoding, LinearPattern};
    let enc = TwoOf5Encoder::new();
    for (sym, start_runs, per_digit) in [
        (Symbology::Std2of5, 6, 10),
        (Symbology::Iata2of5, 4, 10),
        (Symbology::Matrix2of5, 6, 6),
    ] {
        let symbol = enc.build(sym, b"1234").unwrap();
        let Encoding::Linear(clean) = enc.encode(&symbol).unwrap() else {
            panic!("2 of 5 encodes to a linear pattern");
        };
        let decode = |modules: Vec<bool>| {
            let mut p = LinearPattern::new();
            p.modules = modules;
            TwoOf5Decoder::new().decode(&Encoding::Linear(p))
        };
        assert!(decode(clean.modules.clone()).is_ok());
        // The last run of the first digit is its trailing (narrow) space.
        let gap = start_runs + per_digit - 1;
        assert!(
            decode(with_run_width(&clean.modules, gap, 3)).is_err(),
            "{sym:?}: wide inter-bar space accepted"
        );
        // An oversized bar is not a wide bar either (run `start_runs` is the first
        // data bar, wide for the digit 1).
        assert!(decode(with_run_width(&clean.modules, start_runs, 3)).is_ok());
        assert!(
            decode(with_run_width(&clean.modules, start_runs, 12)).is_err(),
            "{sym:?}: 12-module bar accepted as wide"
        );
    }
}
