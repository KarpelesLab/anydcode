//! Cross-cutting conformance checks for the linear encoders/decoders: independent
//! reference vectors that the per-symbology suites do not pin, the API misuse paths
//! (wrong `Encoding` kind, wrong symbology or metadata, empty input), and the
//! guarantee that a damaged row is never read as a *different* symbol by the codes
//! whose structure or check characters are meant to prevent exactly that.
#![cfg(all(
    feature = "decode",
    feature = "encode",
    feature = "linear",
    feature = "postal"
))]

use anyd::codes::codabar::{CodabarDecoder, CodabarEncoder, CodabarMeta};
use anyd::codes::code11::{Code11Decoder, Code11Encoder, Code11Meta};
use anyd::codes::code39::{Code39Decoder, Code39Encoder};
use anyd::codes::code93::{Code93Decoder, Code93Encoder, Code93Meta};
use anyd::codes::code128::{Code128Decoder, Code128Encoder};
use anyd::codes::dxfilm::{DxFilmDecoder, DxFilmEncoder};
use anyd::codes::ean::{EanDecoder, EanEncoder};
use anyd::codes::itf::{ItfDecoder, ItfEncoder};
use anyd::codes::msi::{MsiCheck, MsiDecoder, MsiEncoder};
use anyd::codes::pharmacode::{PharmacodeDecoder, PharmacodeEncoder};
use anyd::codes::postal::{PostalDecoder, PostalEncoder};
use anyd::codes::telepen::{TelepenDecoder, TelepenEncoder};
use anyd::codes::twoof5::{TwoOf5Decoder, TwoOf5Encoder};
use anyd::output::{BitMatrix, Encoding, LinearPattern};
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};
use anyd::{Error, Segment, Symbol};

fn linear(encoding: Encoding) -> LinearPattern {
    match encoding {
        Encoding::Linear(p) => p,
        Encoding::Matrix(_) => panic!("expected a linear pattern"),
    }
}

fn row(modules: Vec<bool>) -> Encoding {
    let mut pattern = LinearPattern::new();
    pattern.modules = modules;
    Encoding::Linear(pattern)
}

// ---- Reference vectors --------------------------------------------------------

/// ITF-14 `1 54 00141 28876 3` (the Wikipedia "ITF-14" example): the thirteen data
/// digits take the GTIN mod-10 check digit 3.
#[test]
fn itf14_reference_check_digit() {
    let enc = ItfEncoder::new();
    let symbol = enc.build(b"1540014128876", true).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    // Read without check handling, the fourteen printed digits come back.
    let raw = ItfDecoder::new().decode(&encoding).unwrap();
    assert_eq!(raw.payload_bytes(), b"15400141288763");
    let checked = ItfDecoder::with_check(true).decode(&encoding).unwrap();
    assert_eq!(checked, symbol);
    // The same fourteen digits as plain payload are the same bars.
    assert_eq!(
        enc.encode(&enc.build(b"15400141288763", false).unwrap())
            .unwrap(),
        encoding
    );
    // A different final digit is not a valid check.
    let wrong = enc
        .encode(&enc.build(b"15400141288764", false).unwrap())
        .unwrap();
    assert!(matches!(
        ItfDecoder::with_check(true).decode(&wrong),
        Err(Error::Undecodable { .. })
    ));
}

/// MSI check digits for `1234567`, worked by hand. Luhn mod 10: doubling every
/// other digit from the right gives 5+6+1+4+6+2+2 = 26, so the digit is 4. IBM mod
/// 11 (weights 2..=7 cycling from the right): 14+18+20+20+18+14+2 = 106 = 9 * 11 + 7,
/// so the digit is 11 - 7 = 4 (the value Wikipedia's MSI article gives for both).
/// The second mod-10 digit, over `12345674`, is 1.
#[test]
fn msi_reference_check_digits() {
    let enc = MsiEncoder::new();
    for (scheme, printed) in [
        (MsiCheck::None, &b"1234567"[..]),
        (MsiCheck::Mod10, b"12345674"),
        (MsiCheck::Mod11, b"12345674"),
        (MsiCheck::Mod1010, b"123456741"),
        (MsiCheck::Mod1110, b"123456741"),
    ] {
        let symbol = enc.build_msi(b"1234567", scheme).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        let raw = MsiDecoder::new().decode(&encoding).unwrap();
        assert_eq!(raw.payload_bytes(), printed, "{scheme:?}");
    }
    // Mod 11 has no digit for a remainder of 10 (`6`: 6 * 2 = 12 = 11 + 1).
    for scheme in [MsiCheck::Mod11, MsiCheck::Mod1110] {
        assert!(matches!(
            enc.build_msi(b"6", scheme),
            Err(Error::InvalidData { .. })
        ));
    }
    enc.build_msi(b"6", MsiCheck::Mod10).unwrap();
}

/// Pharmacode values straight from the definition: bar `n` from the right is worth
/// `2^n` when thin and `2 * 2^n` when thick.
#[test]
fn pharmacode_reference_bars() {
    let enc = PharmacodeEncoder::new();
    let bars = |value: u32| -> String {
        let pattern = linear(enc.encode(&enc.build(value).unwrap()).unwrap());
        let mut out = String::new();
        let mut i = 0;
        while i < pattern.modules.len() {
            let len = pattern.modules[i..]
                .iter()
                .take_while(|&&m| m == pattern.modules[i])
                .count();
            if pattern.modules[i] {
                out.push(if len == 1 { 'n' } else { 'W' });
            }
            i += len;
        }
        out
    };
    assert_eq!(bars(3), "nn");
    assert_eq!(bars(4), "nW");
    assert_eq!(bars(5), "Wn");
    assert_eq!(bars(6), "WW");
    assert_eq!(bars(7), "nnn");
    assert_eq!(bars(1234), "nnWWnWnnWW"); // 512+256+2*128+2*64+32+2*16+8+4+2*2+2*1
    assert_eq!(bars(131070), "W".repeat(16));
}

// ---- API misuse -----------------------------------------------------------------

/// Every structural decoder refuses the other kind of [`Encoding`] with an error.
#[test]
fn decoders_reject_the_wrong_encoding_kind() {
    let matrix = Encoding::Matrix(BitMatrix::new(95, 3, 0));
    let linear_decoders: Vec<(&str, Box<dyn Decode>)> = vec![
        ("ean", Box::new(EanDecoder::new())),
        ("code128", Box::new(Code128Decoder::new())),
        ("code39", Box::new(Code39Decoder::new())),
        ("code93", Box::new(Code93Decoder::new())),
        ("code11", Box::new(Code11Decoder::new())),
        ("itf", Box::new(ItfDecoder::new())),
        ("twoof5", Box::new(TwoOf5Decoder::new())),
        ("codabar", Box::new(CodabarDecoder::new())),
        ("msi", Box::new(MsiDecoder::new())),
        ("plessey", Box::new(MsiDecoder::plessey())),
        ("telepen", Box::new(TelepenDecoder::new())),
        ("pharmacode", Box::new(PharmacodeDecoder::new())),
        ("pharmacode2", Box::new(PharmacodeDecoder::two_track())),
    ];
    for (name, decoder) in &linear_decoders {
        assert!(
            matches!(
                decoder.decode(&matrix),
                Err(Error::InvalidParameter { .. } | Error::Unsupported { .. })
            ),
            "{name}"
        );
        // An empty row is never a symbol either.
        assert!(
            matches!(
                decoder.decode(&row(Vec::new())),
                Err(Error::Undecodable { .. })
            ),
            "{name}"
        );
    }
    let a_row = row(vec![true, false, true]);
    assert!(matches!(
        PostalDecoder::new().decode(&a_row),
        Err(Error::Unsupported { .. })
    ));
    assert!(matches!(
        DxFilmDecoder::new().decode(&a_row),
        Err(Error::Unsupported { .. })
    ));
}

/// Each encoder checks that the symbol it is handed is one of its own: the right
/// symbology and the right metadata variant.
#[test]
fn encoders_reject_foreign_symbols() {
    let foreign_meta = SymbolMeta::Ean(anyd::codes::ean::EanMeta::new(
        anyd::codes::ean::EanVariant::Ean13,
    ));
    let cases: Vec<(&str, Box<dyn Encode>, Symbol)> = vec![
        (
            "code39",
            Box::new(Code39Encoder::new()),
            Code39Encoder::new().build(b"A1", false, true).unwrap(),
        ),
        (
            "code93",
            Box::new(Code93Encoder::new()),
            Code93Encoder::new().build(b"A1", false).unwrap(),
        ),
        (
            "code11",
            Box::new(Code11Encoder::new()),
            Code11Encoder::new().build(b"1-2", 2).unwrap(),
        ),
        (
            "itf",
            Box::new(ItfEncoder::new()),
            ItfEncoder::new().build(b"1234", false).unwrap(),
        ),
        (
            "twoof5",
            Box::new(TwoOf5Encoder::new()),
            TwoOf5Encoder::new()
                .build(Symbology::Matrix2of5, b"12")
                .unwrap(),
        ),
        (
            "codabar",
            Box::new(CodabarEncoder::new()),
            CodabarEncoder::new().build(b'A', b"12", b'B').unwrap(),
        ),
        (
            "msi",
            Box::new(MsiEncoder::new()),
            MsiEncoder::new().build_msi(b"12", MsiCheck::Mod10).unwrap(),
        ),
        (
            "plessey",
            Box::new(MsiEncoder::new()),
            MsiEncoder::new().build_plessey(b"1F").unwrap(),
        ),
        (
            "telepen",
            Box::new(TelepenEncoder::new()),
            TelepenEncoder::new().build(b"ab", true).unwrap(),
        ),
        (
            "pharmacode",
            Box::new(PharmacodeEncoder::new()),
            PharmacodeEncoder::new().build(1234).unwrap(),
        ),
        (
            "dxfilm",
            Box::new(DxFilmEncoder::new()),
            DxFilmEncoder::new().build(115, 10, None).unwrap(),
        ),
        (
            "postal",
            Box::new(PostalEncoder::new()),
            PostalEncoder::new().build_postnet("12345").unwrap(),
        ),
    ];
    for (name, encoder, symbol) in cases {
        encoder.encode(&symbol).unwrap();

        let mut wrong_symbology = symbol.clone();
        wrong_symbology.symbology = Symbology::Ean13;
        assert!(
            matches!(
                encoder.encode(&wrong_symbology),
                Err(Error::InvalidParameter { .. } | Error::Unsupported { .. })
            ),
            "{name}: wrong symbology accepted"
        );

        // DX Film Edge keeps everything in the payload text and reads no metadata.
        if name != "dxfilm" {
            let mut wrong_meta = symbol.clone();
            wrong_meta.meta = foreign_meta.clone();
            let result = encoder.encode(&wrong_meta);
            if name == "postal" {
                // The postal variant follows the symbology; foreign metadata that does
                // not contradict it is ignored.
                assert_eq!(result.unwrap(), encoder.encode(&symbol).unwrap());
            } else {
                assert!(
                    matches!(result, Err(Error::InvalidParameter { .. })),
                    "{name}: wrong meta accepted"
                );
            }
        }
    }
}

/// Empty payloads: an error where the symbology needs data, a clean round-trip where
/// it does not.
#[test]
fn empty_payloads() {
    assert!(ItfEncoder::new().build(b"", false).is_err());
    assert!(ItfEncoder::new().build(b"", true).is_err());
    assert!(MsiEncoder::new().build_msi(b"", MsiCheck::None).is_err());
    assert!(MsiEncoder::new().build_plessey(b"").is_err());
    for symbology in [
        Symbology::Std2of5,
        Symbology::Iata2of5,
        Symbology::Matrix2of5,
    ] {
        assert!(TwoOf5Encoder::new().build(symbology, b"").is_err());
    }
    assert!(Code128Encoder::new().build_text("").is_err());

    for full_ascii in [false, true] {
        for check in [false, true] {
            let enc = Code39Encoder::new();
            let symbol = enc.build(b"", full_ascii, check).unwrap();
            let encoding = enc.encode(&symbol).unwrap();
            let decoder = Code39Decoder::new()
                .with_full_ascii(full_ascii)
                .with_check_digit(check);
            assert_eq!(decoder.decode(&encoding).unwrap(), symbol);
        }
    }
    for count in 0..=2 {
        let enc = Code11Encoder::new();
        let symbol = enc.build(b"", count).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        let decoder = Code11Decoder::new().with_check_count(count);
        assert_eq!(decoder.decode(&encoding).unwrap(), symbol);
    }
    let enc = Code93Encoder::new();
    let symbol = enc.build(b"", false).unwrap();
    assert_eq!(
        Code93Decoder::new()
            .decode(&enc.encode(&symbol).unwrap())
            .unwrap(),
        symbol
    );
    let enc = CodabarEncoder::new();
    let symbol = enc.build(b'C', b"", b'D').unwrap();
    assert_eq!(
        CodabarDecoder::new()
            .decode(&enc.encode(&symbol).unwrap())
            .unwrap(),
        symbol
    );
}

/// Option values outside their domain are reported, not clamped.
#[test]
fn out_of_range_options() {
    assert!(matches!(
        Code11Encoder::new().build(b"123", 3),
        Err(Error::InvalidParameter { .. })
    ));
    let mut out = LinearPattern::new();
    assert!(matches!(
        Code11Encoder::new().encode_into(b"123", &Code11Meta { check_count: 3 }, &mut out),
        Err(Error::InvalidParameter { .. })
    ));
    assert!(out.modules.is_empty());
    let good = Code11Encoder::new().build(b"123", 1).unwrap();
    let encoding = Code11Encoder::new().encode(&good).unwrap();
    assert!(matches!(
        Code11Decoder::new().with_check_count(3).decode(&encoding),
        Err(Error::InvalidParameter { .. })
    ));

    for (start, stop) in [(b'a', b'B'), (b'A', b'E'), (b'1', b'A'), (b'A', b'*')] {
        assert!(CodabarEncoder::new().build(start, b"12", stop).is_err());
        let mut out = LinearPattern::new();
        assert!(
            CodabarEncoder::new()
                .encode_into(b"12", &CodabarMeta { start, stop }, &mut out)
                .is_err()
        );
        assert!(out.modules.is_empty());
    }
    // Start/stop letters are not data characters.
    assert!(CodabarEncoder::new().build(b'A', b"1A2", b'B').is_err());
    // Plessey is upper-case hexadecimal.
    assert!(MsiEncoder::new().build_plessey(b"1f").is_err());
    assert!(MsiEncoder::new().build_plessey(b"1G").is_err());
}

/// Code 93 decides full-ASCII from the symbol itself: only a shift character makes
/// it so. Building base-set data with `full_ascii` therefore renders the very same
/// row as without, and that row reads back as the plain symbol.
#[test]
fn code93_full_ascii_is_detected_in_band() {
    let enc = Code93Encoder::new();
    for data in [&b"ABC-123"[..], b"$%+/", b""] {
        let plain = enc.build(data, false).unwrap();
        let flagged = enc.build(data, true).unwrap();
        assert_eq!(
            flagged.meta,
            SymbolMeta::Code93(Code93Meta { full_ascii: true })
        );
        let encoding = enc.encode(&plain).unwrap();
        assert_eq!(enc.encode(&flagged).unwrap(), encoding);
        let decoded = Code93Decoder::new().decode(&encoding).unwrap();
        assert_eq!(decoded, plain);
        assert_eq!(decoded.segments, vec![Segment::alphanumeric(data.to_vec())]);
    }
    let shifted = enc.build(b"aB", true).unwrap();
    let decoded = Code93Decoder::new()
        .decode(&enc.encode(&shifted).unwrap())
        .unwrap();
    assert_eq!(decoded, shifted);
}

// ---- Damaged rows ---------------------------------------------------------------

/// Flip each module of a valid row in turn. For symbologies protected by element
/// structure (discrete characters, fixed run counts) or by check characters, the
/// result is either rejected or — where only an element's width class survives the
/// flip — still the original symbol; it is never some other symbol.
#[test]
fn single_module_damage_never_reads_as_another_symbol() {
    let ean = EanEncoder::new();
    let c128 = Code128Encoder::new();
    let c39 = Code39Encoder::new();
    let c93 = Code93Encoder::new();
    let c11 = Code11Encoder::new();
    let codabar = CodabarEncoder::new();
    let msi = MsiEncoder::new();
    let telepen = TelepenEncoder::new();
    let cases: Vec<(&str, Box<dyn Decode>, Encoding)> = vec![
        (
            "ean13+5",
            Box::new(EanDecoder::new()),
            ean.encode(
                &ean.with_addon(ean.build_ean13("978186197271").unwrap(), "51299")
                    .unwrap(),
            )
            .unwrap(),
        ),
        (
            "upce",
            Box::new(EanDecoder::new()),
            ean.encode(&ean.build_upce("0425261").unwrap()).unwrap(),
        ),
        (
            "ean8",
            Box::new(EanDecoder::new()),
            ean.encode(&ean.build_ean8("9638507").unwrap()).unwrap(),
        ),
        (
            "code128",
            Box::new(Code128Decoder::new()),
            c128.encode(&c128.build_text("Ab\x0112345678x").unwrap())
                .unwrap(),
        ),
        (
            "code39",
            Box::new(Code39Decoder::new()),
            c39.encode(&c39.build(b"CODE-39 $/+%", false, false).unwrap())
                .unwrap(),
        ),
        (
            "code39+check",
            Box::new(
                Code39Decoder::new()
                    .with_full_ascii(true)
                    .with_check_digit(true),
            ),
            c39.encode(&c39.build(b"a$B", true, true).unwrap()).unwrap(),
        ),
        (
            "code93",
            Box::new(Code93Decoder::new()),
            c93.encode(&c93.build(b"x!Y TEST93", true).unwrap())
                .unwrap(),
        ),
        (
            "code11",
            Box::new(Code11Decoder::new().with_check_count(2)),
            c11.encode(&c11.build(b"123-4567890", 2).unwrap()).unwrap(),
        ),
        (
            "codabar",
            Box::new(CodabarDecoder::new()),
            codabar
                .encode(&codabar.build(b'A', b"12-34$:/.+", b'D').unwrap())
                .unwrap(),
        ),
        (
            "plessey",
            Box::new(MsiDecoder::plessey()),
            msi.encode(&msi.build_plessey(b"C0DE1F").unwrap()).unwrap(),
        ),
        (
            "telepen+check",
            Box::new(TelepenDecoder::with_check(true)),
            telepen
                .encode(&telepen.build(b"Telepen 1", true).unwrap())
                .unwrap(),
        ),
    ];
    for (name, decoder, encoding) in cases {
        let original = decoder.decode(&encoding).unwrap();
        let pattern = linear(encoding);
        let mut rejected = 0;
        for at in 0..pattern.modules.len() {
            let mut damaged = pattern.modules.clone();
            damaged[at] = !damaged[at];
            match decoder.decode(&row(damaged)) {
                Err(Error::Undecodable { .. }) => rejected += 1,
                Err(other) => panic!("{name}: module {at}: unexpected error {other:?}"),
                Ok(symbol) => assert_eq!(
                    symbol, original,
                    "{name}: module {at} flipped reads as another symbol"
                ),
            }
        }
        assert!(
            rejected * 2 > pattern.modules.len(),
            "{name}: only {rejected} of {} flips rejected",
            pattern.modules.len()
        );
    }
}
