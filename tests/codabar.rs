//! Codabar: encode → decode → re-encode identity across start/stop pairs and the
//! full character set.

use anydcode::codes::codabar::{CodabarDecoder, CodabarEncoder, CodabarMeta};
use anydcode::segment::Segment;
use anydcode::symbol::SymbolMeta;
use anydcode::symbology::Symbology;
use anydcode::traits::{Decode, Encode};

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
    for start in [b'A', b'B', b'C', b'D'] {
        for stop in [b'A', b'B', b'C', b'D'] {
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
