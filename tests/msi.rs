//! MSI Plessey (all check schemes) and Plessey (CRC): round-trip identity.

use anydcode::codes::msi::{MsiCheck, MsiDecoder, MsiEncoder, MsiMeta};
use anydcode::segment::Segment;
use anydcode::symbol::SymbolMeta;
use anydcode::symbology::Symbology;
use anydcode::traits::{Decode, Encode};

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
    let anydcode::output::Encoding::Linear(mut p) = enc.encode(&sym).unwrap() else {
        panic!("expected linear");
    };
    // Flip a module inside the last data/check character region.
    let n = p.modules.len();
    p.modules[n - 6] = !p.modules[n - 6];
    let res =
        MsiDecoder::with_check(MsiCheck::Mod10).decode(&anydcode::output::Encoding::Linear(p));
    assert!(res.is_err());
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
