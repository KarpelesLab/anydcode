//! Telepen full-ASCII: round-trip identity with and without the mod-127 check.
#![cfg(all(feature = "decode", feature = "encode", feature = "telepen"))]

use anyd::codes::telepen::{TelepenDecoder, TelepenEncoder, TelepenMeta};
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn assert_lossless(data: &[u8], check: bool) {
    let enc = TelepenEncoder::new();
    let symbol = enc.build(data, check).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = TelepenDecoder::with_check(check).decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::Telepen);
    assert_eq!(decoded.segments, vec![Segment::byte(data.to_vec())]);
    assert_eq!(decoded.meta, SymbolMeta::Telepen(TelepenMeta { check }));
    assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
}

#[test]
fn option_matrix() {
    for check in [false, true] {
        assert_lossless(b"ABC", check);
        assert_lossless(b"Telepen 123!", check);
        assert_lossless(b"", check); // empty data + optional check
        assert_lossless(&[0u8, 1, 2, 127], check); // full 7-bit range extremes
    }
}

#[test]
fn rejects_non_ascii() {
    assert!(TelepenEncoder::new().build(&[0x80], false).is_err());
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::output::{Encoding, LinearBuf};
    let enc = TelepenEncoder::new();
    let all_ascii: Vec<u8> = (0..128).collect();
    for data in [
        &b"ABC"[..],
        b"",
        b"Telepen 123!",
        &[0u8, 1, 2, 127],
        &all_ascii,
    ] {
        for check in [false, true] {
            let symbol = enc.build(data, check).unwrap();
            let Encoding::Linear(expected) = enc.encode(&symbol).unwrap() else {
                panic!("Telepen encodes to a linear pattern");
            };
            let meta = TelepenMeta { check };
            let mut storage = [0u8; 512];
            let mut buf = LinearBuf::new(&mut storage);
            enc.encode_into(data, &meta, &mut buf).unwrap();
            assert_eq!(buf, expected);
            assert_eq!(buf.len(), TelepenEncoder::max_modules(data.len(), &meta));
            // Too-small storage reports a capacity error instead of panicking.
            let mut tiny = [0u8; 2];
            let err = enc.encode_into(data, &meta, &mut LinearBuf::new(&mut tiny));
            assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        }
    }
    let mut storage = [0u8; 64];
    let err = enc.encode_into(
        &[b'A', 0x80],
        &TelepenMeta::default(),
        &mut LinearBuf::new(&mut storage),
    );
    assert!(err.is_err());
}
