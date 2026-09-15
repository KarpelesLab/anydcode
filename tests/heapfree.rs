//! Heap-free encoders, exercised in whatever feature set the test build has —
//! including `--no-default-features --features encode,all-codes`, where the library
//! has no allocator at all. Every output is pinned by module count and an FNV-1a
//! fingerprint of the modules, taken from the allocating encoders' verified output.
#![cfg(all(feature = "encode", feature = "all-codes"))]

use anyd::Symbology;
use anyd::codes::codabar::{CodabarEncoder, CodabarMeta};
use anyd::codes::code11::{Code11Encoder, Code11Meta};
use anyd::codes::code39::{Code39Encoder, Code39Meta};
use anyd::codes::code93::{Code93Encoder, Code93Meta};
use anyd::codes::code128::{Code128Encoder, Code128Input};
use anyd::codes::datamatrix::{DataMatrixEncoder, Encodation};
use anyd::codes::ean::{AddOnKind, AddOnView, EanEncoder, EanVariant};
use anyd::codes::itf::{ItfEncoder, ItfMeta};
use anyd::codes::microqr::{MicroEcLevel, MicroQrEncoder};
use anyd::codes::msi::{MsiCheck, MsiEncoder, MsiMeta};
use anyd::codes::pharmacode::PharmacodeEncoder;
use anyd::codes::qr::{EcLevel, QrEncoder};
use anyd::codes::telepen::{TelepenEncoder, TelepenMeta};
use anyd::codes::twoof5::TwoOf5Encoder;
use anyd::output::{LinearBuf, MatrixBuf};
use anyd::segment::SegmentView;

/// `(modules, quiet zone, FNV-1a over the module bits)` of a row.
fn row_print(row: &LinearBuf<'_>) -> (usize, usize, u64) {
    let hash = row.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, bit| {
        (h ^ u64::from(bit)).wrapping_mul(0x0100_0000_01b3)
    });
    (row.len(), row.quiet_zone, hash)
}

/// `(side, quiet zone, FNV-1a over the module bits)` of a grid.
fn grid_print(grid: &MatrixBuf<'_>) -> (usize, usize, u64) {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            hash = (hash ^ u64::from(grid.get(x, y))).wrapping_mul(0x0100_0000_01b3);
        }
    }
    assert_eq!(grid.width(), grid.height());
    (grid.width(), grid.quiet_zone, hash)
}

#[test]
fn linear_encoders() {
    let mut storage = [0u8; 64];
    macro_rules! check {
        ($name:literal, $bound:expr, $expected:expr, |$out:ident| $encode:expr) => {{
            let mut row = LinearBuf::new(&mut storage);
            let $out = &mut row;
            $encode.unwrap();
            assert!(row.len() <= $bound, "{} exceeds its bound", $name);
            assert_eq!(row_print(&row), $expected, "{}", $name);
        }};
    }

    let meta = Code39Meta {
        full_ascii: true,
        check_digit: true,
    };
    check!(
        "code39",
        Code39Encoder::max_modules(6, &meta),
        (155, 10, 7308062576259799335),
        |out| { Code39Encoder::new().encode_into(b"Code39", &meta, out) }
    );
    let meta = Code93Meta { full_ascii: true };
    check!(
        "code93",
        Code93Encoder::max_modules(6, &meta),
        (118, 10, 16752780658197167484),
        |out| { Code93Encoder::new().encode_into(b"Code93", &meta, out) }
    );
    let meta = Code11Meta { check_count: 2 };
    check!(
        "code11",
        Code11Encoder::max_modules(6, &meta),
        (78, 10, 14899550958336097655),
        |out| { Code11Encoder::new().encode_into(b"123-45", &meta, out) }
    );
    let mut symbols = [0u8; Code128Encoder::max_symbols(12)];
    check!(
        "code128",
        Code128Encoder::max_modules(Code128Encoder::max_symbols(12)),
        (145, 10, 14929686468206329093),
        |out| { Code128Encoder::new().encode_text_into(b"Hello 123456", &mut symbols, out) }
    );
    let gs1 = [
        Code128Input::Data(b'0'),
        Code128Input::Data(b'1'),
        Code128Input::Fnc1,
        Code128Input::Data(b'A'),
    ];
    let n = Code128Encoder::new()
        .plan_into(&gs1, true, &mut symbols)
        .unwrap();
    check!(
        "gs1-128",
        Code128Encoder::max_modules(n),
        (90, 10, 11184606644517711025),
        |out| { Code128Encoder::new().encode_into(&symbols[..n], out) }
    );
    let addon = AddOnView {
        kind: AddOnKind::Five,
        digits: b"52495",
    };
    check!(
        "ean13+5",
        EanEncoder::max_modules(EanVariant::Ean13, Some(AddOnKind::Five)),
        (151, 9, 490217306571611672),
        |out| {
            EanEncoder::new().encode_into(EanVariant::Ean13, b"5901234123457", Some(addon), out)
        }
    );
    check!(
        "upce",
        EanEncoder::max_modules(EanVariant::UpcE, None),
        (51, 9, 14460652102382450229),
        |out| { EanEncoder::new().encode_into(EanVariant::UpcE, b"04252614", None, out) }
    );
    let meta = ItfMeta { check: true };
    check!(
        "itf",
        ItfEncoder::max_modules(7, &meta),
        (81, 10, 622607707057438173),
        |out| { ItfEncoder::new().encode_into(b"1234567", &meta, out) }
    );
    check!(
        "iata2of5",
        TwoOf5Encoder::max_modules(Symbology::Iata2of5, 5),
        (79, 10, 6604381609848140390),
        |out| { TwoOf5Encoder::new().encode_into(Symbology::Iata2of5, b"98765", out) }
    );
    let meta = CodabarMeta {
        start: b'A',
        stop: b'D',
    };
    check!(
        "codabar",
        CodabarEncoder::max_modules(6),
        (99, 10, 17147715757905145663),
        |out| { CodabarEncoder::new().encode_into(b"40156-", &meta, out) }
    );
    let meta = MsiMeta {
        check: MsiCheck::Mod1110,
    };
    check!(
        "msi",
        MsiEncoder::max_modules(Symbology::MsiPlessey, 7, &meta),
        (115, 10, 8940245102249465627),
        |out| { MsiEncoder::new().encode_into(Symbology::MsiPlessey, b"1234567", &meta, out) }
    );
    let meta = TelepenMeta { check: true };
    check!(
        "telepen",
        TelepenEncoder::max_modules(3, &meta),
        (96, 10, 6090096761407496283),
        |out| { TelepenEncoder::new().encode_into(b"ABC", &meta, out) }
    );
    check!(
        "pharmacode",
        PharmacodeEncoder::max_modules(Symbology::Pharmacode),
        (29, 10, 1679928630445318993),
        |out| { PharmacodeEncoder::new().encode_into(Symbology::Pharmacode, 1234, out) }
    );
}

#[test]
fn matrix_encoders() {
    let mut scratch = [0u8; QrEncoder::MAX_BUFFER_LEN];
    let mut storage = [0u8; QrEncoder::MAX_BUFFER_LEN];
    let segments = [
        SegmentView::byte(b"https://example.com/"),
        SegmentView::numeric(b"0123456789012345"),
    ];
    let (grid, meta) = QrEncoder::new()
        .encode_into(
            &segments,
            EcLevel::M,
            None,
            None,
            &mut scratch,
            &mut storage,
        )
        .unwrap();
    assert_eq!((meta.version.number(), meta.mask.index()), (3, 2));
    assert_eq!(grid_print(&grid), (29, 4, 10786092422446982036), "qr");

    let mut storage = [0u8; MicroQrEncoder::MAX_BUFFER_LEN];
    let (grid, meta) = MicroQrEncoder::new()
        .encode_text_into(b"HELLO", MicroEcLevel::L, &mut storage)
        .unwrap();
    assert_eq!((meta.version.number(), meta.mask.index()), (2, 0));
    assert_eq!(grid_print(&grid), (13, 2, 15726942306471055973), "microqr");

    let mut scratch = [0u8; DataMatrixEncoder::MAX_SCRATCH_LEN];
    let mut storage = [0u8; DataMatrixEncoder::MAX_STORAGE_LEN];
    let grid = DataMatrixEncoder::new()
        .encode_data_into(
            b"Hello, Data Matrix 0123456789",
            None,
            &mut scratch,
            &mut storage,
        )
        .unwrap();
    assert_eq!(
        grid_print(&grid),
        (22, 1, 11072846817849368411),
        "datamatrix"
    );
    let segments = [SegmentView::byte(b"\x00\xffbinary")];
    let grid = DataMatrixEncoder::new()
        .encode_into(
            &segments,
            &[Encodation::Base256],
            18,
            &mut scratch,
            &mut storage,
        )
        .unwrap();
    assert_eq!(
        grid_print(&grid),
        (18, 1, 9479855055434250874),
        "datamatrix base256"
    );
}
