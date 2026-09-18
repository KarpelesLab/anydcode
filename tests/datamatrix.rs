//! End-to-end Data Matrix round-trip tests: encode → decode → re-encode must be
//! byte-identical, across square sizes and ASCII / Base256 encodation, and must
//! survive correctable errors.
#![cfg(all(feature = "decode", feature = "encode", feature = "datamatrix"))]

use anyd::codes::datamatrix::{DataMatrixDecoder, DataMatrixEncoder};
use anyd::output::Encoding;
use anyd::traits::{Decode, Encode};

/// Assert that a payload decodes to the same segments/meta and re-encodes identically.
fn assert_lossless(data: &[u8]) {
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.payload_bytes(),
        data,
        "payload differs after decode"
    );
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments differ after decode"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs after decode");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding is not byte-identical");
}

#[test]
fn ascii_digits() {
    assert_lossless(b"123456");
    assert_lossless(b"1234567890");
    assert_lossless(b"7"); // lone digit -> ASCII char, not a pair
    assert_lossless(b"12345"); // odd digit count
}

#[test]
fn ascii_text() {
    assert_lossless(b"HELLO WORLD");
    assert_lossless(b"Data Matrix ECC 200!");
    assert_lossless(b"mix3d 12 t3xt 34 and digits 5678");
}

#[test]
fn base256_high_bytes() {
    assert_lossless(&[0x00, 0x80, 0xFF, 0xAB, 0xCD]);
    assert_lossless(&(0u16..=255).map(|b| b as u8).collect::<Vec<u8>>());
}

#[test]
fn mixed_ascii_and_base256() {
    // Alternating low/high byte runs exercise the encodation transitions.
    let mut data = b"ABC".to_vec();
    data.extend_from_slice(&[0x80, 0x90, 0xA0]);
    data.extend_from_slice(b"789012");
    data.extend_from_slice(&[0xFF]);
    assert_lossless(&data);
}

#[test]
fn segment_structure_is_recovered() {
    // "A" then a high byte then "B" must come back as ASCII / Base256 / ASCII.
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build(&[b'A', 0x80, b'B']).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    use anyd::codes::datamatrix::Encodation;
    use anyd::symbol::SymbolMeta;
    let SymbolMeta::DataMatrix(meta) = &decoded.meta else {
        panic!("expected Data Matrix meta");
    };
    assert_eq!(
        meta.encodations,
        vec![Encodation::Ascii, Encodation::Base256, Encodation::Ascii]
    );
    assert_eq!(decoded.segments.len(), 3);
}

#[test]
fn spans_many_sizes() {
    // Payloads of growing length force progressively larger square symbols.
    for n in [1usize, 5, 20, 60, 120, 260, 600, 1000] {
        let data: Vec<u8> = (0..n).map(|i| b'0' + (i % 10) as u8).collect();
        assert_lossless(&data);
    }
}

#[test]
fn forced_size_pads_correctly() {
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build_sized(b"12", 26).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let Encoding::Matrix(m) = &encoding else {
        panic!("expected matrix");
    };
    assert_eq!((m.width(), m.height()), (26, 26));
    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), b"12");
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn survives_correctable_errors() {
    let enc = DataMatrixEncoder::new();
    // A 26x26 symbol has 28 EC codewords across one block: it corrects up to 14.
    let symbol = enc.build_sized(b"RESILIENT DATA MATRIX 2026", 26).unwrap();
    let Encoding::Matrix(mut matrix) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };

    // Flip a handful of interior modules (avoiding the finder/timing borders).
    for k in 0..6 {
        let x = 3 + k * 3;
        let y = 3 + k * 2;
        let v = matrix.get(x, y);
        matrix.set(x, y, !v);
    }

    let decoded = DataMatrixDecoder::new()
        .decode(&Encoding::Matrix(matrix))
        .unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("RESILIENT DATA MATRIX 2026")
    );
}

#[test]
fn text_convenience_roundtrip() {
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build_text("https://example.com/anydcode").unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("https://example.com/anydcode")
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

/// The heap-free `encode_into` / `encode_data_into` paths write exactly the modules
/// `Encode` returns, across sizes and encodations.
#[test]
fn encode_into_matches_encode() {
    use anyd::SegmentView;
    use anyd::codes::datamatrix::Encodation;
    use anyd::output::MatrixBuf;
    use anyd::symbol::SymbolMeta;

    let enc = DataMatrixEncoder::new();
    let mut payloads: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"7".to_vec(),
        b"123456".to_vec(),
        b"HELLO WORLD".to_vec(),
        vec![b'A', 0x80, b'B'],
        (0u16..=255).map(|b| b as u8).collect(),
        (0..300).map(|i| (i * 7 % 256) as u8).collect(),
    ];
    for n in [20usize, 60, 120, 260, 600, 1000, 3116] {
        payloads.push((0..n).map(|i| b'0' + (i % 10) as u8).collect());
    }
    // Largest symbol filled with text (one codeword per byte).
    payloads.push((0..1558).map(|i| b'A' + (i % 26) as u8).collect());

    let mut scratch = [0u8; DataMatrixEncoder::MAX_SCRATCH_LEN];
    let mut storage = [0u8; DataMatrixEncoder::MAX_STORAGE_LEN];
    let check = |symbol: &anyd::Symbol, data: &[u8], forced: Option<usize>| {
        let mut scratch = [0u8; DataMatrixEncoder::MAX_SCRATCH_LEN];
        let mut storage = [0u8; DataMatrixEncoder::MAX_STORAGE_LEN];
        let Encoding::Matrix(expected) = enc.encode(symbol).unwrap() else {
            panic!("Data Matrix encodes to a matrix");
        };
        let SymbolMeta::DataMatrix(meta) = &symbol.meta else {
            panic!("expected Data Matrix meta");
        };
        let size = meta.symbol_size;
        let views: Vec<SegmentView> = symbol.segments.iter().map(|s| s.view()).collect();

        // Exactly-sized buffers suffice.
        let mut sc = vec![0u8; DataMatrixEncoder::scratch_len(size).unwrap()];
        let mut st = vec![0u8; DataMatrixEncoder::storage_len(size).unwrap()];
        assert!(sc.len() <= DataMatrixEncoder::MAX_SCRATCH_LEN);
        assert_eq!(st.len(), MatrixBuf::bytes_for(size, size));
        let buf = enc
            .encode_into(&views, &meta.encodations, size, &mut sc, &mut st)
            .unwrap();
        assert_eq!(buf, expected, "encode_into, size {size}");

        let buf = enc
            .encode_data_into(data, forced, &mut scratch, &mut storage)
            .unwrap();
        assert_eq!(buf, expected, "encode_data_into, size {size}");

        // One byte short of either buffer is a capacity error, not a panic.
        let short = sc.len() - 1;
        let err = enc.encode_into(
            &views,
            &meta.encodations,
            size,
            &mut sc[..short],
            &mut storage,
        );
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        let err = enc.encode_data_into(data, forced, &mut sc[..short], &mut storage);
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        let short = st.len() - 1;
        let err = enc.encode_into(
            &views,
            &meta.encodations,
            size,
            &mut scratch,
            &mut st[..short],
        );
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    };

    for data in &payloads {
        let symbol = enc.build(data).unwrap();
        check(&symbol, data, None);
        // Forced larger sizes exercise padding and multi-block interleaving.
        for size in [26usize, 52, 72, 104, 144] {
            if let Ok(symbol) = enc.build_sized(data, size) {
                check(&symbol, data, Some(size));
            }
        }
    }

    // Every square size, with a mixed ASCII/Base256 payload.
    let mut data = b"12AB".to_vec();
    data.extend_from_slice(&[0xC3, 0xA9]);
    for size in [
        10usize, 12, 14, 16, 18, 20, 22, 24, 26, 32, 36, 40, 44, 48, 52, 64, 72, 80, 88, 96, 104,
        120, 132, 144,
    ] {
        match enc.build_sized(&data, size) {
            Ok(symbol) => check(&symbol, &data, Some(size)),
            Err(e) => assert!(matches!(e, anyd::Error::Capacity { .. }), "size {size}"),
        }
        // A single ASCII segment with upper shifts (not the canonical split).
        let segs = [SegmentView::byte(&data)];
        let r = enc.encode_into(
            &segs,
            &[Encodation::Ascii],
            size,
            &mut scratch,
            &mut storage,
        );
        if size >= 14 {
            assert_eq!(r.unwrap().width(), size);
        } else {
            assert!(matches!(r, Err(anyd::Error::Capacity { .. })));
        }
    }

    // Oversized data, bad sizes, mismatched encodations and non-byte segments.
    let too_big = vec![b'A'; 1559];
    let err = enc.encode_data_into(&too_big, None, &mut scratch, &mut storage);
    assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    let err = enc.encode_data_into(b"HELLO", Some(11), &mut scratch, &mut storage);
    assert!(matches!(err, Err(anyd::Error::InvalidParameter { .. })));
    assert_eq!(DataMatrixEncoder::scratch_len(11), None);
    let err = enc.encode_into(
        &[SegmentView::byte(b"A")],
        &[],
        10,
        &mut scratch,
        &mut storage,
    );
    assert!(matches!(err, Err(anyd::Error::InvalidParameter { .. })));
    let err = enc.encode_into(
        &[SegmentView::numeric(b"1")],
        &[Encodation::Ascii],
        10,
        &mut scratch,
        &mut storage,
    );
    assert!(matches!(err, Err(anyd::Error::Unsupported { .. })));
    // Auto-size with tiny buffers.
    let err = enc.encode_data_into(b"HELLO WORLD", None, &mut [0u8; 4], &mut storage);
    assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    let mut tiny = [0u8; 4];
    let err = enc.encode_data_into(b"HELLO WORLD", None, &mut scratch, &mut tiny);
    assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
}

// ---- Third-party reference vector ---------------------------------------------

/// `zint -b DATAMATRIX --vers=24 -d Sz1`: the 144×144 symbol, one hex string per row
/// (most-significant bit = leftmost module, set = dark). This is the only size whose
/// Reed–Solomon blocks differ in length (8 × 156 + 2 × 155 data codewords), so its
/// interleaved EC codewords start at block 8 rather than block 0.
#[rustfmt::skip]
const ZINT_144: &[&str] = &[
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "a80121afbc27d82623e710bb883cadc637f3",
    "f4dfb080ce9af7ffbcc8dfd2acc3c0f3a9ba",
    "bd2ba7c43647d2e8ffb09ec3c599dbc2611b",
    "86aa2edb013a92b1aad8f4c6de6c7495fe44",
    "8ec0f1a77e238d693daccc5da8b9879e8b03",
    "9f2d6a89b208929e04b685d0b6674685247a",
    "8b752db00b23da3b19f0dab5945049d68663",
    "b28b9edb7d9eb9a570a2629acf73ee91165a",
    "f77a71b29cfdc540b1d871779686efa369ed",
    "957544a3c92cd312b6e72bc8889a809a2c4a",
    "e2eed9ef9539e66c37fc5833f0b04fd6ed53",
    "f34270da7c1ca63cd4c91cdcc18b7eb7897c",
    "9e15f3b5790ba2f201dad23fdedd91e04a5b",
    "fa8fb2895daee7caac9b452e83a268fd4124",
    "c18035e8384780dea9a8ee23acc7a99dbdc1",
    "dcf0deda2b3afc57dcb46c06c14098e2fa16",
    "cfca198a5369ddd6bfc6bd21d8a463c75861",
    "cf1ad4bea2c0e7d428beed7adcda30e1be3c",
    "c5bf07eaf34fc0ab45f748d9f79693a6a2d3",
    "d996b0c5d650f106b6a8325ade8426d247ce",
    "aad66bb0c243cbd6dda68dd1f0b4e791a055",
    "9bd7b0f4b6a693e5b2babc40b7cd0aa87c54",
    "ffffffffffffffffffffffffffffffffffff",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "9da915dda3a7b46009c3f4e9b404e398dbe9",
    "e110d8c74f0eaf3428b5c95ad1eb1c966608",
    "9bed9ba7feebec1ab994e8a995b8d3d5df0f",
    "e39ac2f4130ce94550b480889f774cac7234",
    "844d01c53cf599ba4bec353bfde85198a175",
    "ef966ea8270e98eb34c7ffdea93a5e9993a0",
    "ed8e5fa91867a40e6fe1255fcbfbdb892ae1",
    "ebd62ecdfb94bbf7a2f1b130a29ef683bc2a",
    "8789cbb8bc37b75e45ce5ea3915fd9b8c41d",
    "850f92ec9b088b0dee9e2c18fc728ec3651a",
    "a76cf1d44125b195afcf8f2f8aa0a5b561c9",
    "e38514e46236e2b46ea5ed92cd1206c36b4c",
    "92485bf6add9a8a70d9210d5c12939fb2cd9",
    "ef7384ed53a4dd8b8c9433c0d1a77ad8f446",
    "a5b5c3d1bc79e025fbbab59dd0f93be3b62d",
    "aabc92d0bc32cdf508a7a1dafb4a9ac33f30",
    "e7207fd10a73979c7f8c7bb7b54e59a333af",
    "80e406ff65b8d3039ee5d2baa31c6281a8b2",
    "a62a83b69933f28c55d6e5f1fa7a87d6274b",
    "db6feab368f8818bf2f4ea4ad74d56d8b67c",
    "9fa1219f8a11b462a7b6281beef1339e0943",
    "98dacaa8fb3898e286805ea2ca90baca658a",
    "ffffffffffffffffffffffffffffffffffff",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "92d8db954f67973021a112d99457259f2825",
    "d8e0a6c3180cbf5260974d42afff58d4c6c4",
    "b5cdd9e957f5f3c787cd8c87a2acfb97949f",
    "b6555281dde8d8a6eea40cd4f2b05acca262",
    "98f4abefaf39a17277cad731d0792b85b4f3",
    "b12c96da2d7adf486ecf5eb4929936ec2a6a",
    "88e745fc6623f7332d87771fd67bcf9db817",
    "a21fbac00d38b71792ba3746a9c00c9ea2a0",
    "b4316b862fa5819279ee2cadf47951fff9b3",
    "fd3398e16002a72512a2d0ecd4fdb4c8ea26",
    "a13713ad10ebacee7dc2787da257a1d02d25",
    "cac478f20586cbc3f8d21b9ca5ad94c13dd0",
    "ae618d9d95fb8f94499e7607b280258fa6c9",
    "f2693ebee9d2ac8c52b7cb9ee20de8e9099c",
    "be8d57b7f92f8f828bd474b7c0a749ec167d",
    "b6d016c6fe90e8e73ccce332e952aee9ef0a",
    "9687b7f4eb5fc7b36fa2232ddeb335fd6641",
    "c8fd12b967b4ff6bcc8542c092016ee1bc8c",
    "a36e21ae0191a5c2cfb2365fc331e3c50eff",
    "95ae5e921c4498ce6cecd26cbe126281721c",
    "d76249817095ab6597cdde01d5d5e9fafd1d",
    "a20040de47e498b632eeb6dec33418caea70",
    "ffffffffffffffffffffffffffffffffffff",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "e21c37cf67f597e7adcb9499ecba599ffacb",
    "e97e62c46ae8f993a0b848f2bafe0aa8ca54",
    "a4cb69c094d781445f90bbc98b38f78440b5",
    "926e78fecac2bd95ead7169cdb2802f39758",
    "fc9d0f9eeff5aba60f87ace380e567e5fe87",
    "bf8fd8e8e1c0a3b4d8ab224cd428bc979a46",
    "b3b9fbc4001bc2b493b5589feafbe9feb89f",
    "c916329559d48380decde660d1b0fcf62da8",
    "e50cf3d9c9adfc3bbbe9bcd59b067bd3b9b7",
    "c58668acbea8801632ed4e04f404709ad8f8",
    "e84c61fa3d5b95ac31904499e343f58fe203",
    "8b9e10ae27dcdba014eb0eecdcaa20861ca8",
    "f382e9c34985fd5767e65b919c13d5bdbd1f",
    "e02f34bd1deadbe1b4f706e0862276df5520",
    "f3d0e5fd906189a9f39ff41b9edee7aee8ef",
    "dc07aaca4962e574848b792ec67cd8d0f11e",
    "c3d82dbbe72b90113bd3ea71a2faada51437",
    "9d7dd8847154cba406ab9438b7d2c8a4237e",
    "a37d0d8293afe5e993b90211fea1ef8eeaa9",
    "eaa4dcff2482f5ebe4e04f76f35de8f6fa42",
    "ff4581e6e0d39cad6189e211b25869b9061b",
    "e08528e9078c84daf4d893eef8ef64bc00b2",
    "ffffffffffffffffffffffffffffffffffff",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "82bfafe4d59bcf09ddcad5ffe3d101ca31c7",
    "dde0729bf0e690e1429fc0689af90ad94af6",
    "a1f5b5d89b938589bdc7c399893697c03feb",
    "932128b6af609558fef61e74882d5aaaa534",
    "da2329f8856ddfe435ff88fbb352afbbf09b",
    "c13150f9d8feb0bbc6885ec0cd1ea2ef8536",
    "b2f609fe29c984a7fd8e6eefb71ded95e38d",
    "dbbb3a941968a20a86ca14a08979c4a131ca",
    "8d1d65c9098ba53009cd50f78e0a33df88eb",
    "b5c7b0c67b94bde79a930cd4957e6acf9e16",
    "db1aabcde7ebe53289dc1d47d54769ae5f7f",
    "844c22e0b92ec9503eb4ba1ec23cf2ac86f6",
    "868539cca435be0fe5f4f9ffe4b7afafa62b",
    "96b57af005d8fc9c4ed127d8f4c3b4e33b60",
    "ea6557852d6ffcd2a1c54707cad289a3d99d",
    "8c8bc0f58c1cef9c66ccda7ce0207ef86fc8",
    "f33efde5e6098f6509c4a3c5b35301fa679f",
    "8dfa5ea9ea68bbeeaaf9310ec8c27ac4e8da",
    "d3f0fdbd21cb8ee54d9cfd55b8c1f7f4d289",
    "d348e6ff70fac66eb0ac87fc9a394ee3b08c",
    "e21e4ffb8c3fc15cdfd6ed29d234b1ac08b5",
    "c2981ce93118e19fd28aede8c19b36ec96da",
    "ffffffffffffffffffffffffffffffffffff",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "d2c2898245519b8ecde529f1db1da1ab1b21",
    "dd3648a171daf5821ebc6a92a75130d78664",
    "8cd3c9f37eb3c23f139af33ff70ffda9f6ef",
    "cc2478fbba20c3742af1d56ede4866c44ece",
    "d7d23d933871da7f15f5d273bc434798ee43",
    "ddff12b9c05ee30ab4dabd7e9f608a8c510e",
    "b3654de386cb8ca2f3ff519d9be31d98d723",
    "ac69c08660aad2a55cceddd2896b828eeb4a",
    "bffe7fe06d85d3b5e7d0e3bda39a83aebc85",
    "a899d696772ab0b206decffccc216cfb6ee8",
    "813b3ff3077f8381f1d88aaf8a8d81868ce7",
    "fa42b48fb69ebde4a6873c4ef9ce60977190",
    "ab839391c73bc8e46fb3d697983415af21f5",
    "946a3e85a558bc5d86fb7366e89a56d1c8b2",
    "c060a3eda383d7ac11dc0e97a40295c7b031",
    "ce1d6cc5f28aa724b8cda882c14696d1357c",
    "d3f4ff8a986ba8fe87a8b573d8bec19a63e5",
    "ef488a947fe0e305cecf4ec697a9f2afef94",
    "91753f9cd3b7986a41cda4bbe1466bfa9ee3",
    "847a0084d284f2f6decb7b88f7266e8dcda2",
    "e85b9df4f025ffdd33d02879b52d8ff44a81",
    "bfa41eeb2560e1a9a8a086f2a759acf8644a",
    "ffffffffffffffffffffffffffffffffffff",
];

#[test]
fn matches_zint_144x144_block_interleave() {
    let mut m = anyd::output::BitMatrix::new(144, 144, 1);
    for (y, row) in ZINT_144.iter().enumerate() {
        for (i, c) in row.chars().enumerate() {
            let nibble = c.to_digit(16).unwrap();
            for k in 0..4 {
                m.set(4 * i + k, y, nibble & (8 >> k) != 0);
            }
        }
    }
    let reference = Encoding::Matrix(m);
    let decoded = DataMatrixDecoder::new().decode(&reference).unwrap();
    assert_eq!(decoded.payload_bytes(), b"Sz1");
    // Re-encoding reproduces the third-party matrix module for module.
    let enc = DataMatrixEncoder::new();
    assert_eq!(enc.encode(&decoded).unwrap(), reference);
    assert_eq!(
        enc.encode(&enc.build_sized(b"Sz1", 144).unwrap()).unwrap(),
        reference
    );
}

#[test]
fn decoder_never_panics_on_garbage() {
    // Deterministic xorshift noise: random grids of every valid (and some invalid)
    // size, then valid symbols with growing numbers of flipped modules.
    let mut seed = 0x0DA7_A3A7_121C_E5EDu64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let dec = DataMatrixDecoder::new();
    let sizes = [
        0usize, 1, 8, 9, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 32, 36, 40, 44, 48, 52, 64, 72,
        80, 88, 96, 104, 120, 132, 144, 145,
    ];
    for size in sizes {
        for density in [0u64, 1, 2, 4] {
            let mut m = anyd::output::BitMatrix::new(size, size, 1);
            for y in 0..size {
                for x in 0..size {
                    m.set(x, y, next() % 4 < density);
                }
            }
            let _ = dec.decode(&Encoding::Matrix(m));
        }
    }
    assert!(
        dec.decode(&Encoding::Matrix(anyd::output::BitMatrix::new(10, 12, 1)))
            .is_err()
    );

    let enc = DataMatrixEncoder::new();
    for len in [1usize, 9, 40, 200, 700] {
        let payload: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let symbol = enc.build(&payload).unwrap();
        let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
            panic!("expected a matrix");
        };
        let size = clean.width();
        for flips in [1usize, 2, 10, 100, 1000] {
            let mut m = clean.clone();
            for _ in 0..flips {
                let (x, y) = (
                    (next() % size as u64) as usize,
                    (next() % size as u64) as usize,
                );
                let dark = m.get(x, y);
                m.set(x, y, !dark);
            }
            let result = dec.decode(&Encoding::Matrix(m));
            // Every size corrects at least two codeword errors per block.
            if flips <= 2 {
                assert_eq!(result.unwrap(), symbol, "len {len} flips {flips}");
            }
        }
    }
}

#[test]
fn exactly_full_and_one_over_at_every_size() {
    // ASCII letters cost one codeword each: `data_cw` of them exactly fill a size and
    // one more must move to the next size (or be refused past 144x144).
    let caps = [
        (10usize, 3usize),
        (12, 5),
        (14, 8),
        (16, 12),
        (18, 18),
        (20, 22),
        (22, 30),
        (24, 36),
        (26, 44),
        (32, 62),
        (36, 86),
        (40, 114),
        (44, 144),
        (48, 174),
        (52, 204),
        (64, 280),
        (72, 368),
        (80, 456),
        (88, 576),
        (96, 696),
        (104, 816),
        (120, 1050),
        (132, 1304),
        (144, 1558),
    ];
    let enc = DataMatrixEncoder::new();
    let size_of = |s: &anyd::symbol::Symbol| match &s.meta {
        anyd::symbol::SymbolMeta::DataMatrix(m) => m.symbol_size,
        _ => unreachable!(),
    };
    for (i, &(size, cap)) in caps.iter().enumerate() {
        let full = vec![b'Q'; cap];
        assert_eq!(size_of(&enc.build(&full).unwrap()), size);
        assert_lossless(&full);
        let over = vec![b'Q'; cap + 1];
        assert!(enc.build_sized(&over, size).is_err());
        match caps.get(i + 1) {
            Some(&(next, _)) => assert_eq!(size_of(&enc.build(&over).unwrap()), next),
            None => assert!(enc.build(&over).is_err()),
        }
        // Base256 to the brim: latch + length field + bytes.
        let field = if cap - 2 <= 249 { 1 } else { 2 };
        let bin = vec![0xA5u8; cap - 1 - field];
        assert_eq!(size_of(&enc.build_sized(&bin, size).unwrap()), size);
        assert_lossless(&bin);
        let decoded = DataMatrixDecoder::new()
            .decode(&enc.encode(&enc.build_sized(&bin, size).unwrap()).unwrap())
            .unwrap();
        assert_eq!(decoded.payload_bytes(), bin);
        assert!(enc.build_sized(&vec![0xA5u8; cap - field], size).is_err());
    }
}
