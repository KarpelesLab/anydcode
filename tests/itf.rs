//! Interleaved 2 of 5: encode → decode → re-encode identity across the option matrix.
#![cfg(all(feature = "decode", feature = "encode", feature = "itf"))]

use anyd::codes::itf::{ItfDecoder, ItfEncoder};
use anyd::segment::Segment;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

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

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::codes::itf::ItfMeta;
    use anyd::output::{Encoding, LinearBuf};
    let enc = ItfEncoder::new();
    for (digits, check) in [
        (&b"00"[..], false),
        (b"1234", false),
        (b"9876543210", false),
        (b"12345", true),
        (b"1234567890123", true),
    ] {
        let symbol = enc.build(digits, check).unwrap();
        let Encoding::Linear(expected) = enc.encode(&symbol).unwrap() else {
            panic!("ITF encodes to a linear pattern");
        };
        let meta = ItfMeta { check };
        let mut storage = [0u8; 32];
        let mut buf = LinearBuf::new(&mut storage);
        enc.encode_into(digits, &meta, &mut buf).unwrap();
        assert_eq!(buf, expected);
        assert_eq!(buf.len(), ItfEncoder::max_modules(digits.len(), &meta));
        // Too-small storage reports a capacity error instead of panicking.
        let mut tiny = [0u8; 2];
        let err = enc.encode_into(digits, &meta, &mut LinearBuf::new(&mut tiny));
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }
    // Invalid input is rejected.
    let meta = ItfMeta { check: false };
    for bad in [&b""[..], b"123", b"12A4"] {
        let mut storage = [0u8; 32];
        let mut buf = LinearBuf::new(&mut storage);
        assert!(enc.encode_into(bad, &meta, &mut buf).is_err());
        assert_eq!(buf.len(), 0);
    }
}
