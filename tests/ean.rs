//! End-to-end EAN/UPC round-trip tests: encode → decode → re-encode must be
//! identical across every family member, plus 2- and 5-digit add-ons. Also asserts
//! independent reference vectors for the computed check/parity digits.

use anyd::Symbology;
use anyd::codes::ean::{AddOnKind, EanDecoder, EanEncoder, EanVariant};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::traits::{Decode, Encode};

/// The single numeric segment's digits, as a String.
fn digits_of(symbol: &anyd::Symbol) -> String {
    let seg = symbol
        .segments
        .iter()
        .find(|s| matches!(s.mode, anyd::Mode::Numeric))
        .expect("numeric segment");
    String::from_utf8(seg.data.clone()).unwrap()
}

fn ean_meta(symbol: &anyd::Symbol) -> (EanVariant, Option<(AddOnKind, String)>) {
    match &symbol.meta {
        SymbolMeta::Ean(m) => (
            m.variant,
            m.addon
                .as_ref()
                .map(|a| (a.kind, String::from_utf8(a.digits.clone()).unwrap())),
        ),
        _ => panic!("not an EAN symbol"),
    }
}

/// Encode, decode, and assert the decoded symbol re-encodes to the identical row.
fn assert_lossless(symbol: &anyd::Symbol) {
    let enc = EanEncoder::new();
    let encoding = enc.encode(symbol).unwrap();
    let decoded = EanDecoder::new().decode(&encoding).unwrap();

    // Decoded segments and meta must match, and re-encode byte-for-byte.
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");
    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding not identical");
}

fn module_len(symbol: &anyd::Symbol) -> usize {
    match EanEncoder::new().encode(symbol).unwrap() {
        Encoding::Linear(lp) => lp.modules.len(),
        Encoding::Matrix(_) => panic!("expected linear"),
    }
}

// ---- Reference vectors (independent sources) -------------------------------

#[test]
fn ean13_reference_check_digit() {
    // 590123412345 → check digit 7 (GS1 check-digit calculator; common example).
    let sym = EanEncoder::new().build_ean13("590123412345").unwrap();
    assert_eq!(digits_of(&sym), "5901234123457");
    assert_eq!(module_len(&sym), 95);
}

#[test]
fn upca_reference_check_digit() {
    // 03600029145 → check digit 2 (Wikipedia, "Universal Product Code").
    let sym = EanEncoder::new().build_upca("03600029145").unwrap();
    assert_eq!(digits_of(&sym), "036000291452");
    assert_eq!(module_len(&sym), 95);
}

#[test]
fn upce_reference_pair() {
    // UPC-E 0425261(4) expands to UPC-A 042100005264 (Wikipedia). Building from the
    // 7-digit form must compute check digit 4.
    let sym = EanEncoder::new().build_upce("0425261").unwrap();
    assert_eq!(digits_of(&sym), "04252614");
    assert_eq!(module_len(&sym), 51);
    // Full 8-digit form verifies rather than recomputes.
    let sym8 = EanEncoder::new().build_upce("04252614").unwrap();
    assert_eq!(digits_of(&sym8), "04252614");
    // A wrong check digit is rejected.
    assert!(EanEncoder::new().build_upce("04252619").is_err());
}

#[test]
fn ean5_reference_checksum() {
    // 52495 ($24.95) → checksum 1 (Wikipedia, "EAN-5"). The checksum only selects the
    // parity, so we assert the module pattern is the canonical 47-module width.
    let sym = EanEncoder::new().build_ean5("52495").unwrap();
    assert_eq!(digits_of(&sym), "52495");
    assert_eq!(module_len(&sym), 47);
}

#[test]
fn ean8_reference_check_digit() {
    // 9638507 → check digit 4 (GS1 mod-10 over 7 digits).
    let sym = EanEncoder::new().build_ean8("9638507").unwrap();
    assert_eq!(digits_of(&sym), "96385074");
    assert_eq!(module_len(&sym), 67);
}

// ---- Round-trip across all six types ---------------------------------------

#[test]
fn roundtrip_ean13() {
    let sym = EanEncoder::new().build_ean13("590123412345").unwrap();
    assert_eq!(ean_meta(&sym).0, EanVariant::Ean13);
    assert_eq!(sym.symbology, Symbology::Ean13);
    assert_lossless(&sym);
}

#[test]
fn roundtrip_ean8() {
    let sym = EanEncoder::new().build_ean8("9638507").unwrap();
    assert_eq!(ean_meta(&sym).0, EanVariant::Ean8);
    assert_lossless(&sym);
}

#[test]
fn roundtrip_upca() {
    let sym = EanEncoder::new().build_upca("03600029145").unwrap();
    assert_eq!(ean_meta(&sym).0, EanVariant::UpcA);
    assert_lossless(&sym);
}

#[test]
fn roundtrip_upce_both_number_systems() {
    for code in ["04252614", "0425261", "1123456"] {
        let sym = EanEncoder::new().build_upce(code).unwrap();
        assert_eq!(ean_meta(&sym).0, EanVariant::UpcE);
        assert_eq!(sym.symbology, Symbology::UpcE);
        assert_lossless(&sym);
    }
}

#[test]
fn roundtrip_ean2_standalone() {
    for code in ["00", "01", "02", "03", "37", "99"] {
        let sym = EanEncoder::new().build_ean2(code).unwrap();
        assert_eq!(ean_meta(&sym).0, EanVariant::Ean2);
        assert_eq!(module_len(&sym), 20);
        assert_lossless(&sym);
    }
}

#[test]
fn roundtrip_ean5_standalone() {
    for code in ["52495", "00000", "12345", "99999"] {
        let sym = EanEncoder::new().build_ean5(code).unwrap();
        assert_eq!(ean_meta(&sym).0, EanVariant::Ean5);
        assert_lossless(&sym);
    }
}

// ---- Main symbol + add-on --------------------------------------------------

#[test]
fn roundtrip_ean13_with_ean5_addon() {
    let enc = EanEncoder::new();
    let base = enc.build_ean13("978030640615").unwrap(); // ISBN-style
    let sym = enc.with_addon(base, "52495").unwrap();
    let (variant, addon) = ean_meta(&sym);
    assert_eq!(variant, EanVariant::Ean13);
    assert_eq!(addon, Some((AddOnKind::Five, "52495".to_string())));
    assert_eq!(module_len(&sym), 95 + 9 + 47);
    assert_lossless(&sym);
}

#[test]
fn roundtrip_ean13_with_ean2_addon() {
    let enc = EanEncoder::new();
    let base = enc.build_ean13("590123412345").unwrap();
    let sym = enc.with_addon(base, "05").unwrap();
    assert_eq!(ean_meta(&sym).1, Some((AddOnKind::Two, "05".to_string())));
    assert_lossless(&sym);
}

#[test]
fn roundtrip_upca_with_ean2_addon() {
    let enc = EanEncoder::new();
    let base = enc.build_upca("03600029145").unwrap();
    let sym = enc.with_addon(base, "12").unwrap();
    assert_lossless(&sym);
}

#[test]
fn roundtrip_upce_with_ean5_addon() {
    let enc = EanEncoder::new();
    let base = enc.build_upce("04252614").unwrap();
    let sym = enc.with_addon(base, "12345").unwrap();
    assert_lossless(&sym);
}

#[test]
fn roundtrip_ean8_with_ean5_addon() {
    let enc = EanEncoder::new();
    let base = enc.build_ean8("9638507").unwrap();
    let sym = enc.with_addon(base, "54321").unwrap();
    assert_lossless(&sym);
}

// ---- Decode-side behaviour -------------------------------------------------

#[test]
fn leading_zero_ean13_decodes_as_upca() {
    // An EAN-13 with an explicit leading zero is, by definition, a UPC-A. It must
    // re-encode identically but be reported as UPC-A after a decode round-trip.
    let enc = EanEncoder::new();
    let sym = enc.build_ean13("0123456789012").unwrap();
    let encoding = enc.encode(&sym).unwrap();
    let decoded = EanDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::UpcA);
    assert_eq!(digits_of(&decoded), "123456789012");
    // Still module-identical when re-encoded.
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn upca_and_leading_zero_ean13_share_pattern() {
    let enc = EanEncoder::new();
    // UPC-A 036000291452 is identical to EAN-13 0036000291452.
    let ean = enc.build_ean13("0036000291452").unwrap();
    let upca = enc.build_upca("036000291452").unwrap();
    assert_eq!(enc.encode(&ean).unwrap(), enc.encode(&upca).unwrap());
}

#[test]
fn rejects_bad_check_digit() {
    // 5901234123458 has a wrong check digit (correct is 7).
    assert!(EanEncoder::new().build_ean13("5901234123458").is_err());
}

#[test]
fn rejects_non_digits() {
    assert!(EanEncoder::new().build_ean13("59012341234A").is_err());
    assert!(EanEncoder::new().build_ean8("ABCDEFG").is_err());
}

#[test]
fn manual_symbol_roundtrips() {
    // A symbol assembled by hand (not via a builder) encodes and decodes cleanly.
    use anyd::Symbol;
    use anyd::codes::ean::EanMeta;
    let sym = Symbol::new(
        Symbology::Ean13,
        vec![Segment::numeric(b"4006381333931".to_vec())],
        SymbolMeta::Ean(EanMeta::new(EanVariant::Ean13)),
    );
    assert_lossless(&sym);
}
