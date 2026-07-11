//! End-to-end postal (height-modulated / 4-state) tests.
//!
//! Each implemented code is checked against an independent reference vector and
//! against the encode → decode → re-encode identity that the crate's lossless
//! contract requires.
//!
//! Reference sources:
//! - IMb: USPS-B-3200 "Intelligent Mail Barcode 4-State Specification" Rev H,
//!   Appendix C, Examples 1–4 (the canonical `01234567094987654321` tracking
//!   code with routing codes none / `01234` / `012345678` / `01234567891`).
//! - RM4SCC checksum: the worked example on Wikipedia / HandWiki "RM4SCC"
//!   (top values `2 6 1 1 4 5 1 2`, bottom `6 4 2 2 4 6 2 5` → check character
//!   `I`), which corresponds to the data string `BX11LT1A`.
//! - POSTNET/PLANET: the `7-4-2-1-0` digit weighting and mod-10 check digit
//!   (USPS DMM; Wikipedia "POSTNET" / "PLANET").

use anyd::Symbology;
use anyd::codes::postal::{PostalDecoder, PostalEncoder, PostalVariant};
use anyd::output::{BitMatrix, Encoding};
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::traits::{Decode, Encode};

/// Render a symbol and return its bar states as the spec's `F/A/D/T` letters,
/// reading each bar column of the 3-row matrix.
fn bar_letters(symbol: &Symbol) -> String {
    let matrix = match PostalEncoder::new().encode(symbol).unwrap() {
        Encoding::Matrix(m) => m,
        Encoding::Linear(_) => panic!("expected a matrix"),
    };
    matrix_letters(&matrix)
}

fn matrix_letters(m: &BitMatrix) -> String {
    assert_eq!(m.height(), 3, "postal matrix must be 3 rows");
    let n = m.width().div_ceil(2);
    (0..n)
        .map(|i| {
            let c = i * 2;
            assert!(m.get(c, 1), "tracker band must be dark");
            match (m.get(c, 0), m.get(c, 2)) {
                (true, true) => 'F',
                (true, false) => 'A',
                (false, true) => 'D',
                (false, false) => 'T',
            }
        })
        .collect()
}

fn variant_of(symbol: &Symbol) -> PostalVariant {
    match &symbol.meta {
        SymbolMeta::Postal(m) => m.variant,
        _ => panic!("not a postal symbol"),
    }
}

/// encode → decode → assert the decoded symbol matches and re-encodes identically.
fn assert_lossless(symbol: &Symbol) {
    let enc = PostalEncoder::new();
    let encoding = enc.encode(symbol).unwrap();
    let decoded = PostalDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, symbol.symbology, "symbology differs");
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");
    assert_eq!(
        enc.encode(&decoded).unwrap(),
        encoding,
        "re-encoding not identical"
    );
}

// ---- Intelligent Mail Barcode: USPS-B-3200 Appendix C ----------------------

/// Tracking code shared by all four spec examples.
const IMB_TRACKING: &str = "01234567094987654321";

#[test]
fn imb_reference_example_1_no_routing() {
    let sym = PostalEncoder::new().build_imb(IMB_TRACKING, "").unwrap();
    assert_eq!(
        bar_letters(&sym),
        "ATTFATTDTTADTAATTDTDTATTDAFDDFADFDFTFFFFFTATFAAAATDFFTDAADFTFDTDT"
    );
}

#[test]
fn imb_reference_example_2_zip5() {
    let sym = PostalEncoder::new()
        .build_imb(IMB_TRACKING, "01234")
        .unwrap();
    assert_eq!(
        bar_letters(&sym),
        "DTTAFADDTTFTDTFTFDTDDADADAFADFATDDFTAAAFDTTADFAAATDFDTDFADDDTDFFT"
    );
}

#[test]
fn imb_reference_example_3_zip9() {
    let sym = PostalEncoder::new()
        .build_imb(IMB_TRACKING, "012345678")
        .unwrap();
    assert_eq!(
        bar_letters(&sym),
        "ADFTTAFDTTTTFATTADTAAATFTFTATDAAAFDDADATATDTDTTDFDTDATADADTDFFTFA"
    );
}

#[test]
fn imb_reference_example_4_zip11() {
    let sym = PostalEncoder::new()
        .build_imb(IMB_TRACKING, "01234567891")
        .unwrap();
    assert_eq!(
        bar_letters(&sym),
        "AADTFFDFTDADTAADAATFDTDDAAADDTDTTDAFADADDDTFFFDDTTTADFAAADFTDAADA"
    );
}

#[test]
fn imb_roundtrip_all_routing_lengths() {
    let enc = PostalEncoder::new();
    for routing in ["", "01234", "012345678", "01234567891"] {
        let sym = enc.build_imb(IMB_TRACKING, routing).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::IntelligentMail);
        assert_eq!(sym.symbology, Symbology::IntelligentMail);
        // Every rendered IMb is exactly 65 bars.
        assert_eq!(bar_letters(&sym).len(), 65);
        assert_lossless(&sym);
    }
}

#[test]
fn imb_roundtrip_varied_payloads() {
    let enc = PostalEncoder::new();
    let cases = [
        ("00000000000000000000", ""),
        ("01234000000000000000", "99999"),
        ("94999999999999999999", "99999999999"),
        ("53379777234994321247", "01234567891"),
    ];
    for (tracking, routing) in cases {
        let sym = enc.build_imb(tracking, routing).unwrap();
        assert_lossless(&sym);
    }
}

#[test]
fn imb_rejects_bad_fields() {
    let enc = PostalEncoder::new();
    // Tracking must be 20 digits.
    assert!(enc.build_imb("0123", "").is_err());
    // Barcode ID second digit must be 0-4.
    assert!(enc.build_imb("05234567094987654321", "").is_err());
    // Routing must be 0/5/9/11 digits.
    assert!(enc.build_imb(IMB_TRACKING, "1234").is_err());
    // Non-digits rejected.
    assert!(enc.build_imb("0123456709498765432X", "").is_err());
}

// ---- POSTNET / PLANET ------------------------------------------------------

#[test]
fn postnet_reference_check_digit() {
    // ZIP 12345: digit sum 15 → check digit 5 (mod-10). 5 data digits + check =
    // 6 groups of 5 bars, plus two framing bars = 32 bars.
    let sym = PostalEncoder::new().build_postnet("12345").unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 32);
    // Framing bars are tall; POSTNET has no descenders/full bars.
    assert!(bars.starts_with('A') && bars.ends_with('A'));
    assert!(bars.chars().all(|c| c == 'A' || c == 'T'));
    // Leading frame + digit 1 (00011 → TTTAA) + digit 2 (00101 → TTATA) ...
    assert!(bars.starts_with("ATTTAATTATA"));
}

#[test]
fn planet_is_postnet_complement() {
    // PLANET uses the bitwise-complement patterns: three tall bars per group.
    let sym = PostalEncoder::new().build_planet("12345").unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 32);
    assert!(bars.starts_with('A') && bars.ends_with('A'));
    // Digit 1 POSTNET 00011 → PLANET 11100 → AAATT (after the frame bar).
    assert!(bars.starts_with("AAAATT"));
}

#[test]
fn postnet_roundtrip_lengths() {
    let enc = PostalEncoder::new();
    for zip in ["12345", "207700001", "12345678901"] {
        let sym = enc.build_postnet(zip).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::Postnet);
        assert_lossless(&sym);
    }
}

#[test]
fn planet_roundtrip_lengths() {
    let enc = PostalEncoder::new();
    for digits in ["01234567891", "0123456789012"] {
        let sym = enc.build_planet(digits).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::Planet);
        assert_lossless(&sym);
    }
}

#[test]
fn postnet_planet_do_not_collide() {
    // The two-tall (POSTNET) and three-tall (PLANET) groups are disjoint, so a
    // decoded symbol keeps its variant.
    let enc = PostalEncoder::new();
    let postnet = enc.build_postnet("12345").unwrap();
    let planet = enc.build_planet("12345").unwrap();
    let dec = PostalDecoder::new();
    let p1 = dec.decode(&enc.encode(&postnet).unwrap()).unwrap();
    let p2 = dec.decode(&enc.encode(&planet).unwrap()).unwrap();
    assert_eq!(p1.symbology, Symbology::Postnet);
    assert_eq!(p2.symbology, Symbology::Planet);
}

#[test]
fn postnet_rejects_non_digits() {
    assert!(PostalEncoder::new().build_postnet("12A45").is_err());
    assert!(PostalEncoder::new().build_postnet("").is_err());
}

// ---- RM4SCC / KIX ----------------------------------------------------------

#[test]
fn rm4scc_reference_checksum() {
    // Worked example: data BX11LT1A → check character I (see module docs). The
    // rendered symbol is start + 9 four-bar groups (8 data + checksum) + stop =
    // 1 + 36 + 1 = 38 bars, framed by an ascender and a full bar.
    let sym = PostalEncoder::new().build_rm4scc("BX11LT1A").unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 38);
    assert!(bars.starts_with('A'), "RM4SCC start bar is an ascender");
    assert!(bars.ends_with('F'), "RM4SCC stop bar is a full bar");

    // Decode and confirm the checksum character preceding the stop bar is 'I'.
    // The bars for the checksum char occupy positions 33..37 (0-based) of the
    // start-stripped body; re-encoding 'BX11LT1A' + 'I' as bare KIX groups must
    // reproduce those bars.
    let check_group = &bars[1 + 8 * 4..1 + 9 * 4];
    let expected = bar_letters(&PostalEncoder::new().build_kix("I").unwrap());
    assert_eq!(check_group, expected);
}

#[test]
fn rm4scc_roundtrip() {
    let enc = PostalEncoder::new();
    for data in ["BX11LT1A", "LU17XG", "SN345TB1A", "0"] {
        let sym = enc.build_rm4scc(data).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::RoyalMail);
        assert_eq!(sym.symbology, Symbology::RoyalMail);
        assert_lossless(&sym);
    }
}

#[test]
fn kix_roundtrip_no_checksum() {
    let enc = PostalEncoder::new();
    for data in ["2500GG11XY", "1234AB", "ABCDEFGHIJ"] {
        let sym = enc.build_kix(data).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::KixCode);
        assert_eq!(sym.symbology, Symbology::KixCode);
        // KIX bars are exactly four per character, no framing.
        assert_eq!(bar_letters(&sym).len(), data.len() * 4);
        assert_lossless(&sym);
    }
}

#[test]
fn kix_and_rm4scc_are_distinguished() {
    // KIX has 4·N bars (≡0 mod 4); RM4SCC has 4·M+2 bars (≡2 mod 4), so a decoder
    // never confuses the two.
    let enc = PostalEncoder::new();
    let dec = PostalDecoder::new();
    let kix = enc.build_kix("ABCD").unwrap();
    let rm = enc.build_rm4scc("ABCD").unwrap();
    assert_eq!(
        dec.decode(&enc.encode(&kix).unwrap()).unwrap().symbology,
        Symbology::KixCode
    );
    assert_eq!(
        dec.decode(&enc.encode(&rm).unwrap()).unwrap().symbology,
        Symbology::RoyalMail
    );
}

#[test]
fn rm4scc_rejects_lowercase_and_symbols() {
    assert!(PostalEncoder::new().build_rm4scc("ab12").is_err());
    assert!(PostalEncoder::new().build_kix("12-34").is_err());
}

// ---- Unsupported variants --------------------------------------------------

#[test]
fn unsupported_symbologies_error() {
    use anyd::Segment;
    let enc = PostalEncoder::new();
    // Australia Post / Japan Post / Mailmark remain unsupported.
    let sym = Symbol::new(
        Symbology::AustraliaPost,
        vec![Segment::numeric(b"1234".to_vec())],
        SymbolMeta::Generic,
    );
    assert!(enc.encode(&sym).is_err());
}
