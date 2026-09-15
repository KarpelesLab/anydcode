//! Pharmacode one-track and two-track: round-trip identity across the value ranges.
#![cfg(all(feature = "decode", feature = "encode", feature = "pharmacode"))]

use anyd::codes::pharmacode::{PharmacodeDecoder, PharmacodeEncoder};
use anyd::segment::Segment;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn assert_one_track(value: u32) {
    let enc = PharmacodeEncoder::new();
    let symbol = enc.build(value).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = PharmacodeDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::Pharmacode);
    assert_eq!(
        decoded.segments,
        vec![Segment::numeric(value.to_string().into_bytes())]
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
}

fn assert_two_track(value: u32) {
    let enc = PharmacodeEncoder::new();
    let symbol = enc.build_two_track(value).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = PharmacodeDecoder::two_track().decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::PharmacodeTwoTrack);
    assert_eq!(
        decoded.segments,
        vec![Segment::numeric(value.to_string().into_bytes())]
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding, "re-encode differs");
}

#[test]
fn one_track_range() {
    for v in [3u32, 4, 7, 42, 255, 1234, 65535, 131069, 131070] {
        assert_one_track(v);
    }
}

#[test]
fn two_track_range() {
    for v in [4u32, 5, 6, 100, 6561, 117480, 1000000, 64570080] {
        assert_two_track(v);
    }
}

#[test]
fn out_of_range_rejected() {
    let enc = PharmacodeEncoder::new();
    assert!(enc.build(2).is_err());
    assert!(enc.build(131071).is_err());
    assert!(enc.build_two_track(3).is_err());
    assert!(enc.build_two_track(64570081).is_err());
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::output::{Encoding, LinearBuf};
    let enc = PharmacodeEncoder::new();
    let one_track = [3u32, 4, 7, 42, 255, 1234, 65535, 131069, 131070];
    let two_track = [4u32, 5, 6, 100, 6561, 117480, 1000000, 64570080];
    let cases = one_track
        .iter()
        .map(|&v| (Symbology::Pharmacode, v, enc.build(v).unwrap()))
        .chain(two_track.iter().map(|&v| {
            (
                Symbology::PharmacodeTwoTrack,
                v,
                enc.build_two_track(v).unwrap(),
            )
        }));
    for (symbology, value, symbol) in cases {
        let Encoding::Linear(expected) = enc.encode(&symbol).unwrap() else {
            panic!("Pharmacode encodes to a linear pattern");
        };
        let mut storage =
            [0u8; LinearBuf::bytes_for(PharmacodeEncoder::max_modules(Symbology::Pharmacode))];
        let mut buf = LinearBuf::new(&mut storage);
        enc.encode_into(symbology, value, &mut buf).unwrap();
        assert_eq!(buf, expected);
        assert!(buf.len() <= PharmacodeEncoder::max_modules(symbology));
        // Too-small storage reports a capacity error instead of panicking.
        let mut tiny = [0u8; 0];
        let err = enc.encode_into(symbology, value, &mut LinearBuf::new(&mut tiny));
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }
    // The bound is reached by the largest values.
    let mut storage = [0u8; 8];
    for (symbology, value) in [
        (Symbology::Pharmacode, 131070),
        (Symbology::PharmacodeTwoTrack, 64570080),
    ] {
        let mut buf = LinearBuf::new(&mut storage);
        enc.encode_into(symbology, value, &mut buf).unwrap();
        assert_eq!(buf.len(), PharmacodeEncoder::max_modules(symbology));
    }
    // Out-of-range values and foreign symbologies are rejected.
    let mut buf = LinearBuf::new(&mut storage);
    assert!(enc.encode_into(Symbology::Pharmacode, 2, &mut buf).is_err());
    assert!(
        enc.encode_into(Symbology::Pharmacode, 131071, &mut buf)
            .is_err()
    );
    assert!(
        enc.encode_into(Symbology::PharmacodeTwoTrack, 3, &mut buf)
            .is_err()
    );
    assert!(enc.encode_into(Symbology::Code39, 42, &mut buf).is_err());
}
