//! Codabar: encode → decode → re-encode identity across start/stop pairs and the
//! full character set.
#![cfg(all(feature = "decode", feature = "encode", feature = "codabar"))]

use anyd::codes::codabar::{CodabarDecoder, CodabarEncoder, CodabarMeta};
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn assert_lossless(start: u8, data: &[u8], stop: u8) {
    let enc = CodabarEncoder::new();
    let symbol = enc.build(start, data, stop).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = CodabarDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::Codabar);
    assert_eq!(decoded.segments, vec![Segment::byte(data.to_vec())]);
    assert_eq!(
        decoded.meta,
        SymbolMeta::Codabar(CodabarMeta { start, stop })
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
}

#[test]
fn start_stop_pairs() {
    for start in *b"ABCD" {
        for stop in *b"ABCD" {
            assert_lossless(start, b"1234", stop);
        }
    }
}

#[test]
fn full_character_set() {
    // Every data character exercised, including all specials.
    assert_lossless(b'A', b"0123456789-$:/.+", b'B');
}

#[test]
fn rejects_invalid() {
    let enc = CodabarEncoder::new();
    assert!(enc.build(b'Z', b"1", b'A').is_err()); // bad guard
    assert!(enc.build(b'A', b"1 2", b'A').is_err()); // space not allowed
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::output::{Encoding, LinearBuf};
    let enc = CodabarEncoder::new();
    for (start, data, stop) in [
        (b'A', &b"1234567890"[..], b'B'),
        (b'C', b"", b'D'),
        (b'D', b"0123456789-$:/.+", b'A'),
        (b'B', b"$12.34", b'C'),
    ] {
        let symbol = enc.build(start, data, stop).unwrap();
        let Encoding::Linear(expected) = enc.encode(&symbol).unwrap() else {
            panic!("Codabar encodes to a linear pattern");
        };
        let meta = CodabarMeta { start, stop };
        let mut storage = [0u8; LinearBuf::bytes_for(CodabarEncoder::max_modules(16))];
        let mut buf = LinearBuf::new(&mut storage);
        enc.encode_into(data, &meta, &mut buf).unwrap();
        assert_eq!(buf, expected);
        assert!(buf.len() <= CodabarEncoder::max_modules(data.len()));
        // Too-small storage reports a capacity error instead of panicking.
        let mut tiny = [0u8; 2];
        let err = enc.encode_into(data, &meta, &mut LinearBuf::new(&mut tiny));
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }
    // Invalid input is rejected.
    let mut storage = [0u8; 64];
    let bad_guard = CodabarMeta {
        start: b'1',
        stop: b'B',
    };
    let mut buf = LinearBuf::new(&mut storage);
    assert!(enc.encode_into(b"1", &bad_guard, &mut buf).is_err());
    assert!(
        enc.encode_into(b"A", &CodabarMeta::default(), &mut buf)
            .is_err()
    );
}
