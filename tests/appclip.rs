//! App Clip Code tests: byte-exact URL compression against Apple's own framework
//! output, payload codec round-trips, URL canonicalization, and SVG generation.
#![cfg(feature = "appclip")]

use anyd::codes::appclip;

/// URL → compressed payload pairs verified byte-for-byte against Apple's
/// `URLCompression.framework` (via rs/appclipcode's `TestCompareBytes`). This is the
/// strongest anchor the port has: every Huffman table, trie context walk, template
/// matcher and tie-break must agree with Apple to reproduce these.
const APPLE_VECTORS: &[(&str, &str)] = &[
    ("https://example.com", "0000000000000000000000008e33db36"),
    ("https://a.co", "00000000000000000000000000004e2f"),
    ("https://www.apple.com", "000000000000000000000478a3cee226"),
    ("https://example.com/path", "000000000000000000238cf6cdb5d582"),
    ("https://appclip.example.com", "000000000000000000000000ae33db36"),
    ("https://app.nextdns.io?p=", "0000000000000000d02424a9f120a6e4"),
    ("https://app.nextdns.io?p=a", "000000000000001a0484953e2414dc85"),
    ("https://app.nextdns.io?p=a&p1=b", "000000000001a0484953e2414dc85031"),
    ("https://app.nextdns.io?p=a&", "000000000902424a9f120a6e3ccda046"),
    ("https://app.nextdns.io?p=abcdef", "000000003409092a7c4829b90b858ed5"),
    ("https://app.nextdns.io?&p=a&&p1=b", "000000000001a0484953e2414dc85031"),
    ("https://app.nextdns.io?x=a", "0000000000902424a9f120a6e720cea5"),
    ("https://app.nextdns.io/bag", "000000000000003409092a7c4829b809"),
    ("https://app.nextdns.io/biz", "000000000000003409092a7c4829b80a"),
    ("https://app.nextdns.io/cat", "000000000000003409092a7c4829b812"),
    ("https://app.nextdns.io/shop?p=a", "0000000000003409092a7c4829b87785"),
    ("https://app.nextdns.io/use", "000000000000003409092a7c4829b894"),
    ("https://app.nextdns.io/a?p=abcdef", "00048121254f89053722fb320b858ed5"),
    ("https://app.nextdns.io/id?p=a&p1=b", "0000000003409092a7c4829b83e85031"),
    ("https://app.nextdns.io/id?p=abcdef", "00000068121254f89053707d0b858ed5"),
    ("https://qr.netflix.com/C/AAAA", "0000000116d5992f39664d1bfa2cb2cb"),
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn compression_matches_apple_framework() {
    for &(url, want) in APPLE_VECTORS {
        let payload = appclip::compress_url(url)
            .unwrap_or_else(|e| panic!("compress {url}: {e:?}"));
        assert_eq!(hex(&payload), want, "payload mismatch for {url}");
    }
}

/// Full pipeline round-trip: URL → payload → ring bits → payload → URL. Queries in
/// the vectors normalize (`?&p=a&&p1=b` → `?p=a&p1=b`), so compare through a second
/// compression rather than string equality.
#[test]
fn full_roundtrip_through_ring_bits() {
    let urls = [
        "https://example.com",
        "https://example.com/",
        "https://example.com/path",
        "https://example.com/A/",
        "https://example.com/a-b",
        "https://example.com/12345",
        "https://example.com/product/A",
        "https://example.com?x=y",
        "https://example.com?x=123",
        "https://example.com?x=a-b&z=456",
        "https://example.com/product/123?x=A&y=456",
        "https://appclip.example.com",
        "https://www.apple.com",
        "https://app.nextdns.io/shop?p=a",
        "https://app.nextdns.io/id?p=a&p1=b",
        "https://qr.netflix.com/C/AAAA",
        "https://karpeleslab.github.io",
        "https://example.com/watch?v=123",
    ];
    for url in urls {
        let payload = appclip::compress_url(url)
            .unwrap_or_else(|e| panic!("compress {url}: {e:?}"));
        let bits = appclip::encode_payload(&payload)
            .unwrap_or_else(|e| panic!("encode {url}: {e:?}"));
        assert!(bits.len() >= 129, "{url}: bit vector too short");
        let back = appclip::decode_payload(&bits)
            .unwrap_or_else(|e| panic!("decode bits {url}: {e:?}"));
        assert_eq!(hex(&back), hex(&payload), "payload round-trip for {url}");
        // The decompressor canonicalizes some spellings the way Apple's does (e.g.
        // `?x=y` → `/?x=y`), so the recovered URL is asserted to be a *fixpoint*:
        // compressing and decompressing it again must reproduce it exactly.
        let recovered = appclip::decompress_url(&back)
            .unwrap_or_else(|e| panic!("decompress {url}: {e:?}"));
        let recompressed = appclip::compress_url(&recovered)
            .unwrap_or_else(|e| panic!("recompress {recovered}: {e:?}"));
        let again = appclip::decompress_url(&recompressed)
            .unwrap_or_else(|e| panic!("re-decompress {recovered}: {e:?}"));
        assert_eq!(again, recovered, "canonical fixpoint for {url}");
    }
}

/// Characters outside a component's raw-allowed set canonicalize to the same payload
/// as their pre-escaped form (Apple normalizes both spellings identically).
#[test]
fn canonical_escapes_match() {
    let pairs = [
        ("https://example.com/[", "https://example.com/%5B"),
        ("https://example.com/]", "https://example.com/%5D"),
        ("https://example.com?[", "https://example.com?%5B"),
        ("https://example.com#]", "https://example.com#%5D"),
    ];
    for (a, b) in pairs {
        let pa = appclip::compress_url(a).unwrap_or_else(|e| panic!("{a}: {e:?}"));
        let pb = appclip::compress_url(b).unwrap_or_else(|e| panic!("{b}: {e:?}"));
        assert_eq!(hex(&pa), hex(&pb), "{a} vs {b}");
    }
}

#[test]
fn rejects_invalid_urls() {
    for url in [
        "http://example.com",           // not https
        "https://",                     // no host
        "https://user@example.com",     // user info
        "https://example.com:8443",     // port
        "https://exämple.com",          // non-ASCII host
        "https://xn--e1afmkfd.xn--p1ai", // punycode
        "https://example",              // no TLD
        // Legitimate URL that simply does not fit 128 compressed bits: rejected
        // with a capacity error rather than silently truncated.
        "https://karpeleslab.github.io/anydcode/scanner-docs",
    ] {
        assert!(
            appclip::compress_url(url).is_err(),
            "{url} should be rejected"
        );
    }
}

#[test]
fn svg_generation_structure() {
    let svg =
        appclip::generate_svg("https://example.com", &appclip::Options::default()).unwrap();
    assert!(svg.contains("data-design=\"Fingerprint\""));
    assert!(svg.contains("<circle"));
    for ring in 1..=5 {
        assert!(svg.contains(&format!("ring-{ring}")), "missing ring {ring}");
    }
    assert!(svg.contains("data-logo-type=\"Camera\""));
    // At least a dozen arcs must be drawn.
    assert!(svg.matches("<path d=\"M ").count() >= 12);

    // NFC variant swaps the glyph.
    let opts = appclip::Options {
        logo: appclip::LogoKind::Nfc,
        ..Default::default()
    };
    let nfc = appclip::generate_svg("https://example.com", &opts).unwrap();
    assert!(nfc.contains("data-logo-type=\"phone\""));

    // All 18 templates resolve.
    for i in 0..18 {
        assert!(appclip::Options::with_template(i).is_some(), "template {i}");
    }
    assert!(appclip::Options::with_template(18).is_none());
}
