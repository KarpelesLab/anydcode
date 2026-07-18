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

/// Rasterize an encoded ring-bit vector to a grayscale image, mirroring the SVG
/// geometry (ring radii/rotations/positions, per-ring gap insets, arc merging across
/// gap positions) with 2×2 supersampling. Luminances mimic Apple template 1: white
/// background, black foreground arcs, mid-gray third-color arcs. The center logo is
/// omitted — the scanner never samples inside ring 1.
fn rasterize(bits: &[bool], size: usize, rotate_deg: f64) -> anyd::GrayImage {
    // Code disc at ~72% of the frame, slightly off-centre — a realistic capture (or
    // locator crop) always has some margin; a full-bleed disc on a white frame has no
    // outer-circle edge at all, which is out of envelope for the reference scanner
    // too.
    rasterize_at(bits, size, rotate_deg, 0.72, 0.53, 0.49)
}

fn rasterize_at(
    bits: &[bool],
    size: usize,
    rotate_deg: f64,
    diameter_frac: f64,
    cx_frac: f64,
    cy_frac: f64,
) -> anyd::GrayImage {
    const RINGS: [(f64, f64, usize, f64); 5] = [
        (177.2016, -78.0, 17, 7.5),
        (224.1012, -85.0, 23, 5.6),
        (271.0008, -70.0, 26, 5.0),
        (317.9004, -63.0, 29, 4.2),
        (364.8, -70.0, 33, 3.5),
    ];
    // Per-ring arc list: (start_deg, end_deg, luma) in canonical coordinates.
    let mut arcs: Vec<Vec<(f64, f64, f64)>> = Vec::new();
    let gap_bits = &bits[..128];
    let color_stream = &bits[128..];
    let mut offset = 0usize;
    let mut ci = 0usize;
    for &(_, _, n, half_gap) in RINGS.iter() {
        let bit_angle = 360.0 / n as f64;
        let state: Vec<Option<bool>> = (0..n)
            .map(|i| {
                if gap_bits[offset + i] {
                    None
                } else {
                    let c = color_stream.get(ci).copied().unwrap_or(false);
                    ci += 1;
                    Some(c)
                }
            })
            .collect();
        offset += n;
        let mut ring_arcs = Vec::new();
        let mut i = 0usize;
        while i < n {
            let Some(third) = state[i] else {
                i += 1;
                continue;
            };
            let mut span = 1usize;
            while i + span < n && state[i + span].is_none() {
                span += 1;
            }
            if i + span == n {
                span += state.iter().take_while(|p| p.is_none()).count();
            }
            let start = i as f64 * bit_angle + half_gap;
            let end = (i + span) as f64 * bit_angle - half_gap;
            let luma = if third { 136.0 / 255.0 } else { 0.0 };
            ring_arcs.push((start, end, luma));
            i += span;
        }
        arcs.push(ring_arcs);
    }

    let scale = size as f64 * diameter_frac / 800.0;
    let (cx, cy) = (size as f64 * cx_frac, size as f64 * cy_frac);
    let rot = rotate_deg.to_radians();
    // Light-gray surround so the outer circle keeps a mild real-world edge.
    let mut img = anyd::GrayImage::filled(size, size, 214);
    for py in 0..size {
        for px in 0..size {
            let mut acc = 0.0f64;
            for sy in 0..2 {
                for sx in 0..2 {
                    let fx = px as f64 + 0.25 + 0.5 * sx as f64 - cx;
                    let fy = py as f64 + 0.25 + 0.5 * sy as f64 - cy;
                    // Undo the code rotation, then to canonical units.
                    let (s, c) = (-rot).sin_cos();
                    let ux = (c * fx - s * fy) / scale;
                    let uy = (s * fx + c * fy) / scale;
                    let r = ux.hypot(uy);
                    if r > 400.0 {
                        acc += 214.0 / 255.0; // surround
                        continue;
                    }
                    let theta = uy.atan2(ux).to_degrees();
                    let mut v = 1.0f64; // background disc
                    for (ring, &(rr, ring_rot, _, _)) in RINGS.iter().enumerate() {
                        if (r - rr).abs() > 23.5 / 2.0 {
                            continue;
                        }
                        // Angle within this ring's rotated frame, normalized 0..360.
                        let a = (theta - ring_rot).rem_euclid(360.0);
                        for &(start, end, luma) in &arcs[ring] {
                            // Wrapping arcs run past 360°, so a position near 0° may
                            // fall inside via its +360° alias.
                            if (a >= start && a <= end)
                                || (a + 360.0 >= start && a + 360.0 <= end)
                            {
                                v = luma;
                                break;
                            }
                        }
                        break;
                    }
                    acc += v;
                }
            }
            img.set(px, py, (acc / 4.0 * 255.0).round() as u8);
        }
    }
    img
}

/// Camera-detection round-trip: generate → rasterize → scan on the shared pipeline.
#[test]
fn scan_rasterized_codes() {
    for (url, size, rot) in [
        ("https://example.com/shop?p=1", 500usize, 0.0f64),
        ("https://appclip.example.com", 380, 37.0),
        ("https://www.apple.com", 640, -113.0),
    ] {
        let payload = appclip::compress_url(url).unwrap();
        let bits = appclip::encode_payload(&payload).unwrap();
        let img = rasterize(&bits, size, rot);
        let sym = appclip::scan(&img.as_frame())
            .unwrap_or_else(|e| panic!("scan {url} ({size}px, {rot}°): {e:?}"));
        assert_eq!(sym.symbology, anyd::Symbology::AppClipCode);
        assert_eq!(sym.text().as_deref(), Some(url), "payload for {url}");
        let loc = sym.location.expect("scan reports a location");
        let c = loc.outline.center();
        let (ex, ey) = (size as f32 * 0.53, size as f32 * 0.49);
        assert!(
            (c.x - ex).abs() < size as f32 * 0.1 && (c.y - ey).abs() < size as f32 * 0.1,
            "{url}: located at {c:?}, expected near ({ex},{ey})"
        );
    }
}

/// The shared 2D pipeline entry must pick App Clip Codes up automatically.
#[test]
fn scan_2d_pipeline_reads_appclip() {
    let payload = appclip::compress_url("https://example.com/shop?p=1").unwrap();
    let bits = appclip::encode_payload(&payload).unwrap();
    let img = rasterize(&bits, 460, 12.0);
    let found = anyd::pipeline::scan_2d(&img.as_frame());
    assert!(
        found
            .iter()
            .any(|s| s.symbology == anyd::Symbology::AppClipCode
                && s.text().as_deref() == Some("https://example.com/shop?p=1")),
        "scan_2d results: {:?}",
        found.iter().map(|s| (s.symbology, s.text())).collect::<Vec<_>>()
    );
}

/// A frame with no circular structure must come back empty quickly, not hallucinate.
#[test]
fn scan_rejects_plain_scene() {
    // Checkerboard: plenty of edges, no rings.
    let mut img = anyd::GrayImage::filled(400, 400, 255);
    for y in 0..400 {
        for x in 0..400 {
            if (x / 25 + y / 25) % 2 == 0 {
                img.set(x, y, 30);
            }
        }
    }
    assert!(appclip::scan(&img.as_frame()).is_err());
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
