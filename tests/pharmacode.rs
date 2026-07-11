//! Pharmacode one-track and two-track: round-trip identity across the value ranges.

use anydcode::codes::pharmacode::{PharmacodeDecoder, PharmacodeEncoder};
use anydcode::segment::Segment;
use anydcode::symbology::Symbology;
use anydcode::traits::{Decode, Encode};

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
