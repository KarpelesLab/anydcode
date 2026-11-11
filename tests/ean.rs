//! End-to-end EAN/UPC round-trip tests: encode → decode → re-encode must be
//! identical across every family member, plus 2- and 5-digit add-ons. Also asserts
//! independent reference vectors for the computed check/parity digits.
#![cfg(all(feature = "decode", feature = "encode", feature = "ean"))]

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

// ---- Spec tables, restated independently ------------------------------------
//
// The parity tables below are written out from the GS1 General Specifications
// (section 5.2: number sets A/B, i.e. odd/even parity, per position), not derived
// from the crate's own tables. A left-hand character has odd parity (number set A,
// "L") when it has an odd number of dark modules and even parity (set B, "G")
// otherwise.

fn modules_of(symbol: &anyd::Symbol) -> Vec<bool> {
    match EanEncoder::new().encode(symbol).unwrap() {
        Encoding::Linear(lp) => lp.modules,
        Encoding::Matrix(_) => panic!("expected linear"),
    }
}

/// Parity letters (`O`dd / `E`ven) of `count` seven-module characters, the first at
/// module `start` and each following one `step` modules later.
fn parities(modules: &[bool], start: usize, step: usize, count: usize) -> String {
    (0..count)
        .map(|i| {
            let at = start + i * step;
            let dark = modules[at..at + 7].iter().filter(|&&m| m).count();
            if dark % 2 == 1 { 'O' } else { 'E' }
        })
        .collect()
}

#[test]
fn ean13_first_digit_selects_the_left_half_parity() {
    const TABLE: [&str; 10] = [
        "OOOOOO", "OOEOEE", "OOEEOE", "OOEEEO", "OEOOEE", "OEEOOE", "OEEEOO", "OEOEOE", "OEOEEO",
        "OEEOEO",
    ];
    let enc = EanEncoder::new();
    for (first, expected) in TABLE.iter().enumerate() {
        let sym = enc.build_ean13(&format!("{first}12345678901")).unwrap();
        let modules = modules_of(&sym);
        assert_eq!(
            &parities(&modules, 3, 7, 6),
            expected,
            "first digit {first}"
        );
        // The right half is always number set C: even parity throughout.
        assert_eq!(parities(&modules, 50, 7, 6), "EEEEEE");
        if first != 0 {
            assert_lossless(&sym);
        }
    }
}

#[test]
fn ean8_uses_odd_parity_on_the_left() {
    let modules = modules_of(&EanEncoder::new().build_ean8("9638507").unwrap());
    assert_eq!(parities(&modules, 3, 7, 4), "OOOO");
    assert_eq!(parities(&modules, 36, 7, 4), "EEEE");
}

#[test]
fn upce_parity_follows_number_system_and_check_digit() {
    // Number system 0, indexed by check digit; number system 1 is the inverse.
    const TABLE: [&str; 10] = [
        "EEEOOO", "EEOEOO", "EEOOEO", "EEOOOE", "EOEEOO", "EOOEEO", "EOOOEE", "EOEOEO", "EOEOOE",
        "EOOEOE",
    ];
    let enc = EanEncoder::new();
    for ns in 0..2u8 {
        let mut seen = [false; 10];
        for payload in 0..400u32 {
            let sym = enc
                .build_upce(&format!("{ns}{:06}", payload * 2503))
                .unwrap();
            let check = (digits_of(&sym).as_bytes()[7] - b'0') as usize;
            seen[check] = true;
            let expected: String = TABLE[check]
                .chars()
                .map(|c| match (ns, c) {
                    (0, c) => c,
                    (_, 'E') => 'O',
                    _ => 'E',
                })
                .collect();
            assert_eq!(parities(&modules_of(&sym), 3, 7, 6), expected, "{sym:?}");
        }
        assert_eq!(seen, [true; 10], "number system {ns}");
    }
}

#[test]
fn addon_parity_tables() {
    let enc = EanEncoder::new();
    // EAN-2: the value modulo 4 selects the parity pair.
    const TWO: [&str; 4] = ["OO", "OE", "EO", "EE"];
    for value in 0..100usize {
        let sym = enc.build_ean2(&format!("{value:02}")).unwrap();
        assert_eq!(parities(&modules_of(&sym), 4, 9, 2), TWO[value % 4]);
        assert_lossless(&sym);
    }
    // EAN-5: the 3-9-3-9-3 weighted sum modulo 10 selects the parity pattern.
    const FIVE: [&str; 10] = [
        "EEOOO", "EOEOO", "EOOEO", "EOOOE", "OEEOO", "OOEEO", "OOOEE", "OEOEO", "OEOOE", "OOEOE",
    ];
    let mut seen = [false; 10];
    for value in (0..100_000usize).step_by(331) {
        let text = format!("{value:05}");
        let d: Vec<usize> = text.bytes().map(|b| (b - b'0') as usize).collect();
        let checksum = (3 * (d[0] + d[2] + d[4]) + 9 * (d[1] + d[3])) % 10;
        seen[checksum] = true;
        let sym = enc.build_ean5(&text).unwrap();
        assert_eq!(
            parities(&modules_of(&sym), 4, 9, 5),
            FIVE[checksum],
            "{text}"
        );
        assert_lossless(&sym);
    }
    assert_eq!(seen, [true; 10]);
}

/// One UPC-E for every value of the last payload digit, each with the UPC-A it
/// stands for under the GS1 zero-suppression rules (manufacturer/item split by the
/// last digit: `0-2`, `3`, `4`, `5-9`). The UPC-E check digit is the UPC-A's.
/// `0654321` ⟷ `065100004327` and `0123455` ⟷ `012345000058` are the widely
/// published examples; the rest are expanded and check-summed by hand.
#[test]
fn upce_zero_suppression_reference_pairs() {
    let enc = EanEncoder::new();
    for (upce, upca) in [
        ("0123450", "012000003455"),
        ("0654321", "065100004327"),
        ("0123452", "012200003453"),
        ("0123453", "012300000451"),
        ("0123454", "012340000053"),
        ("0123455", "012345000058"),
        ("0123456", "012345000065"),
        ("0123457", "012345000072"),
        ("0123458", "012345000089"),
        ("0123459", "012345000096"),
    ] {
        // The UPC-A reference itself carries a valid check digit.
        enc.build_upca(upca).unwrap();
        let sym = enc.build_upce(upce).unwrap();
        assert_eq!(digits_of(&sym), format!("{upce}{}", &upca[11..]), "{upce}");
        // Six payload digits imply number system 0.
        assert_eq!(enc.build_upce(&upce[1..]).unwrap(), sym);
        assert_lossless(&sym);
    }
}

/// Every UPC-E payload shape round-trips, in both number systems.
#[test]
fn upce_sweep_roundtrips() {
    let enc = EanEncoder::new();
    for ns in 0..2 {
        for payload in (0..1_000_000u32).step_by(1009) {
            let sym = enc.build_upce(&format!("{ns}{payload:06}")).unwrap();
            assert_lossless(&sym);
        }
    }
}

// ---- Rejections ---------------------------------------------------------------

#[test]
fn addon_cannot_be_attached_to_an_addon() {
    let enc = EanEncoder::new();
    for standalone in [
        enc.build_ean2("12").unwrap(),
        enc.build_ean5("52495").unwrap(),
    ] {
        for addon in ["34", "12345"] {
            assert!(matches!(
                enc.with_addon(standalone.clone(), addon),
                Err(anyd::Error::InvalidParameter { .. })
            ));
        }
    }
}

#[test]
fn builders_reject_malformed_input() {
    let enc = EanEncoder::new();
    let invalid = |r: anyd::Result<anyd::Symbol>| matches!(r, Err(anyd::Error::InvalidData { .. }));
    // Wrong lengths, either side of both accepted ones.
    for digits in ["", "59012341234", "59012341234570"] {
        assert!(invalid(enc.build_ean13(digits)), "EAN-13 {digits:?}");
    }
    for digits in ["", "963850", "963850741"] {
        assert!(invalid(enc.build_ean8(digits)), "EAN-8 {digits:?}");
    }
    for digits in ["", "0360002914", "0360002914520"] {
        assert!(invalid(enc.build_upca(digits)), "UPC-A {digits:?}");
    }
    assert!(invalid(enc.build_upca("036000291453"))); // bad check digit
    assert!(invalid(enc.build_ean8("96385075"))); // bad check digit
    for digits in ["", "12345", "123456789", "2425261", "24252614", "042526a"] {
        assert!(invalid(enc.build_upce(digits)), "UPC-E {digits:?}");
    }
    for digits in ["", "1", "123", "1a"] {
        assert!(invalid(enc.build_ean2(digits)), "EAN-2 {digits:?}");
    }
    for digits in ["", "1234", "123456", "1234a"] {
        assert!(invalid(enc.build_ean5(digits)), "EAN-5 {digits:?}");
    }
    // Non-ASCII digits are not digits.
    assert!(invalid(enc.build_ean2("١٢")));

    let main = enc.build_ean13("590123412345").unwrap();
    for addon in ["", "1", "123", "1234", "123456", "1x"] {
        assert!(invalid(enc.with_addon(main.clone(), addon)), "{addon:?}");
    }
    // Only an EAN/UPC symbol can take an add-on.
    let other = anyd::Symbol::new(
        Symbology::Ean13,
        main.segments.clone(),
        SymbolMeta::Itf(Default::default()),
    );
    assert!(matches!(
        enc.with_addon(other.clone(), "12"),
        Err(anyd::Error::InvalidParameter { .. })
    ));
    assert!(matches!(
        enc.encode(&other),
        Err(anyd::Error::InvalidParameter { .. })
    ));
    // A symbol with no numeric segment, or digits that do not fit its variant.
    let mut empty = main.clone();
    empty.segments.clear();
    assert!(enc.encode(&empty).is_err());
    let mut short = main.clone();
    short.segments[0].data.pop();
    assert!(enc.encode(&short).is_err());
}

#[test]
fn decoder_rejects_malformed_rows() {
    use anyd::output::{BitMatrix, LinearPattern};
    let enc = EanEncoder::new();
    let dec = EanDecoder::new();
    let row = |modules: Vec<bool>| {
        let mut pattern = LinearPattern::new();
        pattern.modules = modules;
        Encoding::Linear(pattern)
    };
    let undecodable = |e: &Encoding| matches!(dec.decode(e), Err(anyd::Error::Undecodable { .. }));

    assert!(matches!(
        dec.decode(&Encoding::Matrix(BitMatrix::new(95, 1, 0))),
        Err(anyd::Error::Unsupported { .. })
    ));
    assert!(undecodable(&row(Vec::new())));
    assert!(undecodable(&row(vec![false; 95])));
    assert!(undecodable(&row(vec![true; 95])));

    let main = modules_of(&enc.build_ean13("590123412345").unwrap());
    let two = modules_of(&enc.build_ean2("12").unwrap());
    let five = modules_of(&enc.build_ean5("52495").unwrap());
    // Surrounding light modules (the quiet zones) do not matter.
    let mut padded = vec![false; 11];
    padded.extend(&main);
    padded.extend([false; 7]);
    assert_eq!(
        digits_of(&dec.decode(&row(padded)).unwrap()),
        "5901234123457"
    );

    // Truncated anywhere, or with a single module flipped, the row is not a symbol.
    for len in 1..main.len() {
        assert!(
            undecodable(&row(main[..len].to_vec())),
            "truncated to {len}"
        );
    }
    for at in 0..main.len() {
        let mut flipped = main.clone();
        flipped[at] = !flipped[at];
        assert!(undecodable(&row(flipped)), "module {at} flipped");
    }
    // An add-on must be separated from the main symbol, and be complete.
    let glued: Vec<bool> = main.iter().chain(&two).copied().collect();
    assert!(undecodable(&row(glued)));
    let mut cut = main.clone();
    cut.extend([false; 9]);
    cut.extend(&five[..five.len() - 1]);
    assert!(undecodable(&row(cut)));
    // Nothing may follow an add-on, standalone or attached.
    for base in [five.clone(), two.clone()] {
        let mut trailing = base;
        trailing.extend([false, true]);
        assert!(undecodable(&row(trailing)));
    }
    let mut double = main.clone();
    for addon in [&two, &two] {
        double.extend([false; 9]);
        double.extend(addon);
    }
    assert!(undecodable(&row(double)));
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::codes::ean::AddOnView;
    use anyd::output::LinearBuf;
    let enc = EanEncoder::new();
    let mains = [
        enc.build_ean13("590123412345").unwrap(),
        enc.build_ean13("4006381333931").unwrap(),
        enc.build_ean8("9638507").unwrap(),
        enc.build_upca("03600029145").unwrap(),
        enc.build_upce("0425261").unwrap(),
        enc.build_upce("1123456").unwrap(),
    ];
    let standalone = [
        enc.build_ean2("05").unwrap(),
        enc.build_ean5("52495").unwrap(),
    ];
    let mut symbols = Vec::new();
    for main in &mains {
        symbols.push(main.clone());
        for addon in ["12", "05", "52495", "90000"] {
            symbols.push(enc.with_addon(main.clone(), addon).unwrap());
        }
    }
    symbols.extend(standalone);

    for symbol in &symbols {
        let Encoding::Linear(expected) = enc.encode(symbol).unwrap() else {
            panic!("EAN/UPC encodes to a linear pattern");
        };
        let SymbolMeta::Ean(meta) = &symbol.meta else {
            panic!("EAN meta");
        };
        let digits = digits_of(symbol);
        let addon = meta.addon.as_ref().map(|a| AddOnView {
            kind: a.kind,
            digits: &a.digits,
        });
        assert_eq!(addon, meta.addon.as_ref().map(|a| a.view()));
        let mut storage = [0u8; 32];
        let mut buf = LinearBuf::new(&mut storage);
        enc.encode_into(meta.variant, digits.as_bytes(), addon, &mut buf)
            .unwrap();
        assert_eq!(buf, expected);
        assert_eq!(buf.quiet_zone, expected.quiet_zone);
        let bound = EanEncoder::max_modules(meta.variant, addon.map(|a| a.kind));
        assert_eq!(buf.len(), bound);
        // Too-small storage reports a capacity error instead of panicking.
        let mut tiny = [0u8; 2];
        let err = enc.encode_into(
            meta.variant,
            digits.as_bytes(),
            addon,
            &mut LinearBuf::new(&mut tiny),
        );
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }

    // Invalid input is rejected before anything is written.
    let two = AddOnView {
        kind: AddOnKind::Two,
        digits: b"12",
    };
    let mismatched = AddOnView {
        kind: AddOnKind::Five,
        digits: b"12",
    };
    for (variant, digits, addon) in [
        (EanVariant::Ean13, &b"5901234123458"[..], None), // bad check digit
        (EanVariant::Ean13, b"590123412345", None),       // missing check digit
        (EanVariant::UpcE, b"24252614", None),            // bad number system
        (EanVariant::Ean8, b"9638507A", None),
        (EanVariant::Ean2, b"05", Some(two)), // add-on on an add-on
        (EanVariant::Ean13, b"5901234123457", Some(mismatched)),
    ] {
        let mut storage = [0u8; 32];
        let mut buf = LinearBuf::new(&mut storage);
        assert!(enc.encode_into(variant, digits, addon, &mut buf).is_err());
        assert_eq!(buf.len(), 0);
    }
}
