//! MSI Plessey (all check schemes) and Plessey (CRC): round-trip identity.
#![cfg(all(feature = "decode", feature = "encode", feature = "msi"))]

use anyd::codes::msi::{MsiCheck, MsiDecoder, MsiEncoder, MsiMeta};
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn assert_msi_lossless(digits: &[u8], scheme: MsiCheck) {
    let enc = MsiEncoder::new();
    let symbol = enc.build_msi(digits, scheme).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = MsiDecoder::with_check(scheme).decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::MsiPlessey);
    assert_eq!(decoded.segments, vec![Segment::numeric(digits.to_vec())]);
    assert_eq!(decoded.meta, SymbolMeta::Msi(MsiMeta { check: scheme }));
    assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
}

#[test]
fn msi_all_check_schemes() {
    for scheme in [
        MsiCheck::None,
        MsiCheck::Mod10,
        MsiCheck::Mod11,
        MsiCheck::Mod1010,
        MsiCheck::Mod1110,
    ] {
        assert_msi_lossless(b"1234567", scheme);
        assert_msi_lossless(b"80523", scheme);
    }
}

#[test]
fn msi_bad_check_is_rejected() {
    // Corrupt the check digit: decoder with the matching scheme must reject it.
    let enc = MsiEncoder::new();
    let sym = enc.build_msi(b"1234567", MsiCheck::Mod10).unwrap();
    let anyd::output::Encoding::Linear(mut p) = enc.encode(&sym).unwrap() else {
        panic!("expected linear");
    };
    // Flip a module inside the last data/check character region.
    let n = p.modules.len();
    p.modules[n - 6] = !p.modules[n - 6];
    let res = MsiDecoder::with_check(MsiCheck::Mod10).decode(&anyd::output::Encoding::Linear(p));
    assert!(res.is_err());
}

/// Build an MSI module row straight from 4-bit values (no digit validation).
fn msi_row(nibbles: &[u8]) -> anyd::output::Encoding {
    let mut p = anyd::output::LinearPattern::new();
    p.modules.extend([true, true, false]);
    for &v in nibbles {
        for bit in (0..4).rev() {
            p.modules.extend([true, (v >> bit) & 1 == 1, false]);
        }
    }
    p.modules.extend([true, false, false, true]);
    anyd::output::Encoding::Linear(p)
}

/// MSI digits are BCD: a bit group of 10..=15 is not a digit and must not decode
/// (it used to come out as `:`..`?` in a "numeric" segment the encoder then refused).
#[test]
fn msi_non_bcd_group_is_rejected() {
    assert!(MsiDecoder::new().decode(&msi_row(&[1, 2, 3])).is_ok());
    for bad in 10..16 {
        for scheme in [MsiCheck::None, MsiCheck::Mod10, MsiCheck::Mod1010] {
            let res = MsiDecoder::with_check(scheme).decode(&msi_row(&[1, bad, 3, 4]));
            assert!(res.is_err(), "group {bad} accepted under {scheme:?}");
        }
    }
}

/// A row carrying only check digits has no payload; the encoder cannot produce it.
#[test]
fn msi_check_only_row_is_rejected() {
    // Luhn of the empty string is 0.
    assert!(
        MsiDecoder::with_check(MsiCheck::Mod10)
            .decode(&msi_row(&[0]))
            .is_err()
    );
    assert!(
        MsiDecoder::with_check(MsiCheck::Mod1010)
            .decode(&msi_row(&[0, 0]))
            .is_err()
    );
}

/// A Plessey row holding nothing but the (all-zero) CRC of an empty payload.
#[test]
fn plessey_crc_only_row_is_rejected() {
    let mut p = anyd::output::LinearPattern::new();
    let mut bar = true;
    let start = [3usize, 1, 3, 1, 1, 3, 3, 1];
    let crc = [1usize, 3].repeat(8);
    let stop = [3usize, 3, 1, 3, 1, 1, 3, 1, 3];
    for w in start.iter().chain(&crc).chain(&stop) {
        p.modules.extend(core::iter::repeat_n(bar, *w));
        bar = !bar;
    }
    let res = MsiDecoder::plessey().decode(&anyd::output::Encoding::Linear(p));
    assert!(res.is_err(), "empty Plessey payload accepted: {res:?}");
}

#[test]
fn plessey_roundtrip() {
    let enc = MsiEncoder::new();
    for data in [&b"1234"[..], b"ABCDEF", b"DEADBEEF", b"0"] {
        let sym = enc.build_plessey(data).unwrap();
        let encoding = enc.encode(&sym).unwrap();
        let decoded = MsiDecoder::plessey().decode(&encoding).unwrap();
        assert_eq!(decoded.symbology, Symbology::Plessey);
        assert_eq!(decoded.segments, vec![Segment::byte(data.to_vec())]);
        assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
    }
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::output::{Encoding, LinearBuf};
    let enc = MsiEncoder::new();
    let check_encode = |symbology: Symbology, data: &[u8], meta: MsiMeta, expected| {
        let Encoding::Linear(expected) = expected else {
            panic!("MSI encodes to a linear pattern");
        };
        let mut storage = [0u8; 64];
        let mut buf = LinearBuf::new(&mut storage);
        enc.encode_into(symbology, data, &meta, &mut buf).unwrap();
        assert_eq!(buf, expected);
        assert_eq!(
            buf.len(),
            MsiEncoder::max_modules(symbology, data.len(), &meta)
        );
        // Too-small storage reports a capacity error instead of panicking.
        let mut tiny = [0u8; 2];
        let err = enc.encode_into(symbology, data, &meta, &mut LinearBuf::new(&mut tiny));
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    };
    for data in [&b"1234567"[..], b"80523", b"0", b"9999999999"] {
        for check in [
            MsiCheck::None,
            MsiCheck::Mod10,
            MsiCheck::Mod11,
            MsiCheck::Mod1010,
            MsiCheck::Mod1110,
        ] {
            let Ok(symbol) = enc.build_msi(data, check) else {
                continue; // mod-11 check value 10 is unrepresentable
            };
            let expected = enc.encode(&symbol).unwrap();
            check_encode(Symbology::MsiPlessey, data, MsiMeta { check }, expected);
        }
    }
    for data in [&b"1234"[..], b"ABCDEF", b"DEADBEEF", b"0"] {
        let symbol = enc.build_plessey(data).unwrap();
        let expected = enc.encode(&symbol).unwrap();
        check_encode(Symbology::Plessey, data, MsiMeta::default(), expected);
    }
    // Invalid input is rejected.
    let mut storage = [0u8; 64];
    let mut buf = LinearBuf::new(&mut storage);
    let meta = MsiMeta::default();
    assert!(
        enc.encode_into(Symbology::MsiPlessey, b"12A", &meta, &mut buf)
            .is_err()
    );
    assert!(
        enc.encode_into(Symbology::MsiPlessey, b"", &meta, &mut buf)
            .is_err()
    );
    assert!(
        enc.encode_into(Symbology::Plessey, b"12G", &meta, &mut buf)
            .is_err()
    );
    assert!(
        enc.encode_into(Symbology::Code39, b"12", &meta, &mut buf)
            .is_err()
    );
}
