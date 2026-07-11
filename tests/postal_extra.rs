//! End-to-end tests for the extended postal 4-state codes: Australia Post,
//! Japan Post and Royal Mail Mailmark.
//!
//! Each code is checked against at least one INDEPENDENT published reference
//! vector (its documented worked example's bar string, cross-checked against
//! zint) and against the encode → decode → re-encode identity the crate's
//! lossless contract requires.
//!
//! Reference sources (bar strings written in the spec's `F`/`A`/`D`/`T` letters,
//! i.e. Full / Ascender / Descender / Tracker):
//!
//! - Australia Post: "Customer Barcoding Technical Specifications" Diagram 1
//!   (DPID `96184209`) and the "Guide to Printing the 4-State Barcode" Figures 3
//!   (`39549554`) and 4 (`56439111ABA 9`). The three bar strings below were
//!   derived from zint's `test_auspost` reference bitmaps for those inputs.
//! - Japan Post: the Japan Post "Zip/Barcode Manual" p.6 worked examples
//!   (`15400233-16-4-205` and `350110622-1A308`), derived from zint's
//!   `test_postal` reference bitmaps.
//! - Mailmark: the "Royal Mail Mailmark barcode C/L encoding and decoding
//!   instructions" (Sept 2015) worked examples, whose published DAFT strings are
//!   reproduced verbatim in zint's `test_mailmark`.

use anyd::Symbology;
use anyd::codes::postal::{PostalDecoder, PostalEncoder, PostalVariant};
use anyd::output::{BitMatrix, Encoding};
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::traits::{Decode, Encode};

/// Render a symbol to the spec's `F/A/D/T` bar letters, reading each bar column
/// of the 3-row matrix (the same encoding used in `tests/postal.rs`).
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

// ---- Australia Post 4-State -------------------------------------------------

#[test]
fn auspost_reference_standard_diagram1() {
    // AusPost Tech Specs Diagram 1: DPID 96184209, Standard Customer Barcode.
    let sym = PostalEncoder::new().build_auspost("96184209", "").unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 37, "Standard Customer Barcode is 37 bars");
    assert_eq!(bars, "ATFAFATFDFFADDAAFDFFTFTDFFTDADADTADAT");
    // Start and stop pairs are Ascender, Tracker.
    assert!(bars.starts_with("AT") && bars.ends_with("AT"));
}

#[test]
fn auspost_reference_guide_figure3() {
    // AusPost Guide Figure 3: DPID 39549554, Standard Customer Barcode.
    let sym = PostalEncoder::new().build_auspost("39549554", "").unwrap();
    assert_eq!(bar_letters(&sym), "ATFAFAAFTFADAATFADADAATTADAFATAATDDAT");
}

#[test]
fn auspost_reference_guide_figure4_customer2() {
    // AusPost Guide Figure 4: DPID 56439111, Customer Information "ABA 9" (has a
    // space, so the C table is used) -> Customer Barcode 2, 52 bars.
    let sym = PostalEncoder::new()
        .build_auspost("56439111", "ABA 9")
        .unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 52, "Customer Barcode 2 is 52 bars");
    assert_eq!(bars, "ATADTFADDFAAAFTFFAFAFAFFFFFAFFFFFTTDDTTAFADAFFFTTAAT");
}

#[test]
fn auspost_roundtrip_formats() {
    let enc = PostalEncoder::new();
    // Standard (numeric DPID only), Customer 2 with graphic field (space forces
    // the C table), Customer 3 with a long all-digit field, and the Null FCC.
    let cases = [
        ("96184209", ""),
        ("56439111", "ABA 9"),
        ("12345678", "9012345"),
        ("12345678", "abc#12"),
        ("00000000", ""),
        ("1234", ""),
    ];
    for (dpid, custinfo) in cases {
        let sym = enc.build_auspost(dpid, custinfo).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::AustraliaPost);
        assert_eq!(sym.symbology, Symbology::AustraliaPost);
        assert!(matches!(bar_letters(&sym).len(), 37 | 52 | 67));
        assert_lossless(&sym);
    }
}

#[test]
fn auspost_short_dpid_is_zero_padded() {
    // A DPID shorter than 8 digits is left-padded; decode recovers the padded
    // canonical form, so the symbol round-trips.
    let enc = PostalEncoder::new();
    let sym = enc.build_auspost("1234", "").unwrap();
    let decoded = PostalDecoder::new()
        .decode(&enc.encode(&sym).unwrap())
        .unwrap();
    assert_eq!(decoded.segments[0].data, b"00001234");
}

#[test]
fn auspost_rejects_bad_fields() {
    let enc = PostalEncoder::new();
    assert!(enc.build_auspost("123456789", "").is_err()); // DPID > 8 digits
    assert!(enc.build_auspost("12A45", "").is_err()); // non-digit DPID
    assert!(enc.build_auspost("12345678", "\u{00e9}").is_err()); // non-graphic-set
    // Over-long: 8 DPID + 16 customer chars = 24 > 23.
    assert!(enc.build_auspost("12345678", "1234567890123456").is_err());
}

// ---- Japan Post 4-State -----------------------------------------------------

#[test]
fn japanpost_reference_manual_example1() {
    // Zip/Barcode Manual p.6 first example.
    let sym = PostalEncoder::new()
        .build_japanpost("15400233-16-4-205")
        .unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 67, "Japan Post is always 67 bars");
    assert_eq!(
        bars,
        "FDFFTFTFFADFTTFTTFDADFADFATFTFFTDAFTFTFADTFTFDAFTTFTFTDATDATDADAFDF"
    );
    // Start pair Full, Descender; stop pair Descender, Full.
    assert!(bars.starts_with("FD") && bars.ends_with("DF"));
}

#[test]
fn japanpost_reference_manual_example2() {
    // Zip/Barcode Manual p.6 second example.
    let sym = PostalEncoder::new()
        .build_japanpost("350110622-1A308")
        .unwrap();
    assert_eq!(
        bar_letters(&sym),
        "FDDFAFTFFTTFFTFFTFTTDAFFDAFDATFTFFTDATFTTDFAFTTADFTDATDATDATDAFTFDF"
    );
}

#[test]
fn japanpost_roundtrip() {
    let enc = PostalEncoder::new();
    for data in [
        "15400233-16-4-205",
        "350110622-1A308",
        "1234",
        "123456-AB",
        "ABCDEFGHIJ",
        "1234567890ABCDE",
    ] {
        let sym = enc.build_japanpost(data).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::JapanPost);
        assert_eq!(sym.symbology, Symbology::JapanPost);
        assert_lossless(&sym);
    }
}

#[test]
fn japanpost_lowercase_is_uppercased_by_caller() {
    // The alphabet is upper-case; a lower-case input is rejected (callers should
    // upper-case first, matching the stored canonical form).
    assert!(PostalEncoder::new().build_japanpost("abc").is_err());
    // Symbols outside digits/'-'/A-Z are rejected.
    assert!(PostalEncoder::new().build_japanpost("12,34").is_err());
    // Too many symbol characters (letters expand to two intermediate chars).
    assert!(PostalEncoder::new().build_japanpost("ABCDEFGHIJK").is_err());
}

// ---- Royal Mail Mailmark 4-State --------------------------------------------
//
// The `expected` DAFT strings are the published worked examples from the Royal
// Mail "barcode C/L encoding and decoding instructions" (Sept 2015).

#[test]
fn mailmark_reference_barcode_c_example1() {
    // RMMBCED Example 1.
    let sym = PostalEncoder::new()
        .build_mailmark("1100000000000XY11     ")
        .unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 66, "Mailmark Barcode C is 66 bars");
    assert_eq!(
        bars,
        "TTDTTATTDTAATTDTAATTDTAATTDTTDDAATAADDATAATDDFAFTDDTAADDDTAAFDFAFF"
    );
}

#[test]
fn mailmark_reference_barcode_c_example2() {
    // RMMBCED Example 2.
    let sym = PostalEncoder::new()
        .build_mailmark("21B2254800659JW5O9QA6Y")
        .unwrap();
    assert_eq!(
        bar_letters(&sym),
        "DAATATTTADTAATTFADDDDTTFTFDDDDFFDFDAFTADDTFFTDDATADTTFATTDAFDTFDDA"
    );
}

#[test]
fn mailmark_reference_barcode_l_example1() {
    // RMMBLED Example 1: Barcode L, 78 bars.
    let sym = PostalEncoder::new()
        .build_mailmark("11000000000000000XY11    ")
        .unwrap();
    let bars = bar_letters(&sym);
    assert_eq!(bars.len(), 78, "Mailmark Barcode L is 78 bars");
    assert_eq!(
        bars,
        "TTDTTATDDTTATTDTAATTDTAATDDTTATTDTTDATFTAATDDTAATDDTATATFAADDAATAATDDTAADFTFTA"
    );
}

#[test]
fn mailmark_reference_barcode_l_example2() {
    // RMMBLED Example 2.
    let sym = PostalEncoder::new()
        .build_mailmark("41038422416563762EF61AH8T")
        .unwrap();
    assert_eq!(
        bar_letters(&sym),
        "DTTFATTDDTATTTATFTDFFFTFDFDAFTTTADTTFDTFDDDTDFDDFTFAADTFDTDTDTFAATAFDDTAATTDTT"
    );
}

#[test]
fn mailmark_roundtrip() {
    let enc = PostalEncoder::new();
    for data in [
        "1100000000000XY11     ",    // Barcode C, international
        "21B2254800659JW5O9QA6Y",    // Barcode C
        "0100000000009JA500AA0A",    // Barcode C
        "34A6789012345OV123JQ4U",    // Barcode C
        "11000000000000000XY11    ", // Barcode L
        "41038422416563762EF61AH8T", // Barcode L
    ] {
        let sym = enc.build_mailmark(data).unwrap();
        assert_eq!(variant_of(&sym), PostalVariant::Mailmark);
        assert_eq!(sym.symbology, Symbology::Mailmark);
        assert!(matches!(bar_letters(&sym).len(), 66 | 78));
        assert_lossless(&sym);
    }
}

#[test]
fn mailmark_short_input_is_space_padded() {
    // A 21-character input is padded to the 22-character Barcode C canonical form.
    let enc = PostalEncoder::new();
    let sym = enc.build_mailmark("1100000000000XY11    ").unwrap();
    assert_eq!(sym.segments[0].data.len(), 22);
    assert_eq!(sym.segments[0].data, b"1100000000000XY11     ");
    assert_lossless(&sym);
}

#[test]
fn mailmark_rejects_bad_fields() {
    let enc = PostalEncoder::new();
    assert!(enc.build_mailmark("0000000000000").is_err()); // too short (< 14)
    assert!(enc.build_mailmark("5100000000000XY11     ").is_err()); // Format 5 > 4
    assert!(enc.build_mailmark("1000000000000XY11     ").is_err()); // Version 0 < 1
    assert!(enc.build_mailmark("110000000000!XY11     ").is_err()); // non-alnum
}

// ---- Cross-code disambiguation ----------------------------------------------

#[test]
fn distinct_codes_decode_to_their_own_symbology() {
    // Australia Post 67-bar, Japan Post 67-bar and Mailmark 78-bar all share bar
    // counts with each other or with RM4SCC/KIX; each must still decode to its
    // own symbology.
    let enc = PostalEncoder::new();
    let dec = PostalDecoder::new();
    let cases: [(Symbol, Symbology); 4] = [
        (
            enc.build_auspost("12345678", "9012345").unwrap(),
            Symbology::AustraliaPost,
        ),
        (
            enc.build_japanpost("15400233-16-4-205").unwrap(),
            Symbology::JapanPost,
        ),
        (
            enc.build_mailmark("41038422416563762EF61AH8T").unwrap(),
            Symbology::Mailmark,
        ),
        (enc.build_rm4scc("BX11LT1A").unwrap(), Symbology::RoyalMail),
    ];
    for (sym, expected) in cases {
        let decoded = dec.decode(&enc.encode(&sym).unwrap()).unwrap();
        assert_eq!(decoded.symbology, expected);
    }
}
