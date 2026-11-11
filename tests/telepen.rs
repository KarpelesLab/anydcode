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

/// A Telepen row is bar/space pairs, so it formally ends in a narrow space — which a
/// scanner, or any row cropped to its bars like every other symbology's, never
/// delivers: it is indistinguishable from the quiet zone. Such a row must decode.
#[test]
fn decodes_without_the_trailing_space() {
    use anyd::output::Encoding;
    let enc = TelepenEncoder::new();
    for check in [false, true] {
        let symbol = enc.build(b"Telepen 123!", check).unwrap();
        let Encoding::Linear(mut pattern) = enc.encode(&symbol).unwrap() else {
            panic!("Telepen encodes to a linear pattern");
        };
        assert_eq!(pattern.modules.pop(), Some(false));
        assert_eq!(pattern.modules.last(), Some(&true));
        let decoded = TelepenDecoder::with_check(check)
            .decode(&Encoding::Linear(pattern))
            .unwrap();
        assert_eq!(decoded.segments, symbol.segments);
    }
}

/// Element widths of a module row, starting with a bar.
fn run_widths(modules: &[bool]) -> Vec<usize> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < modules.len() {
        let len = modules[i..]
            .iter()
            .take_while(|&&m| m == modules[i])
            .count();
        runs.push(len);
        i += len;
    }
    runs
}

/// Telepen maps its bit stream to elements deterministically: `010` is always a wide
/// bar + wide space, and inside an open `0 1…1 0` block only narrow bars occur. A row
/// that opens a block (narrow bar, wide space) and then shows a wide bar is not
/// something any Telepen bit stream renders to, even though a lenient reading
/// (`01` + `00` + `10`) yields the same bits as the canonical `010` + `010` — it must
/// be rejected rather than decoded to a symbol that re-encodes differently.
#[test]
fn wide_bar_inside_an_open_block_is_rejected() {
    use anyd::output::{Encoding, LinearPattern};
    let enc = TelepenEncoder::new();
    // 0x12 is `01001000` LSB first: `010` `010` `00`.
    let symbol = enc.build(&[0x12], false).unwrap();
    let Encoding::Linear(pattern) = enc.encode(&symbol).unwrap() else {
        panic!("Telepen encodes to a linear pattern");
    };
    let runs = run_widths(&pattern.modules);
    // The start guard is six pairs; then `010` `010` as two wide/wide pairs.
    assert_eq!(runs[12..16], [3, 3, 3, 3]);

    for replacement in [
        [1, 3, 3, 1, 1, 3], // `01` open, `00` (wide bar, narrow space), `10` close
        [1, 3, 3, 3, 1, 1], // `01` open, `010` (wide bar, wide space), `1`
    ] {
        let mut forged = runs[..12].to_vec();
        forged.extend(replacement);
        forged.extend(&runs[16..]);
        let mut row = LinearPattern::new();
        for (i, &w) in forged.iter().enumerate() {
            row.modules.extend(core::iter::repeat_n(i % 2 == 0, w));
        }
        assert_eq!(row.modules.len(), pattern.modules.len());
        for check in [false, true] {
            assert!(
                TelepenDecoder::with_check(check)
                    .decode(&Encoding::Linear(row.clone()))
                    .is_err(),
                "{replacement:?} accepted"
            );
        }
    }
    // The genuine row still decodes.
    let decoded = TelepenDecoder::new()
        .decode(&Encoding::Linear(pattern))
        .unwrap();
    assert_eq!(decoded.segments, symbol.segments);
}
