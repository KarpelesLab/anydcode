//! Interleaved 2 of 5: encode → decode → re-encode identity across the option matrix.

use anydcode::codes::itf::{ItfDecoder, ItfEncoder};
use anydcode::segment::Segment;
use anydcode::symbology::Symbology;
use anydcode::traits::{Decode, Encode};

fn assert_lossless(digits: &[u8], check: bool) {
    let enc = ItfEncoder::new();
    let symbol = enc.build(digits, check).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = ItfDecoder::with_check(check).decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::Itf);
    assert_eq!(decoded.segments, vec![Segment::numeric(digits.to_vec())]);
    assert_eq!(decoded.meta, symbol.meta);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
}

#[test]
fn option_matrix() {
    assert_lossless(b"1234", false);
    assert_lossless(b"00", false);
    assert_lossless(b"9876543210", false);
    // With a mod-10 check digit; data length must make the total even.
    assert_lossless(b"12345", true);
    assert_lossless(b"1234567", true);
}

#[test]
fn odd_length_rejected() {
    assert!(ItfEncoder::new().build(b"123", false).is_err());
    assert!(ItfEncoder::new().build(b"1234", true).is_err());
}

#[test]
fn non_digit_rejected() {
    assert!(ItfEncoder::new().build(b"12AB", false).is_err());
}
