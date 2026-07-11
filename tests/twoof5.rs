//! Standard/IATA/Matrix 2 of 5: encode → decode → re-encode identity per variant.

use anydcode::codes::twoof5::{TwoOf5Decoder, TwoOf5Encoder};
use anydcode::segment::Segment;
use anydcode::symbology::Symbology;
use anydcode::traits::{Decode, Encode};

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
