//! Telepen full-ASCII: round-trip identity with and without the mod-127 check.

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
