//! SVG rendering of the circular five-ring code.
//!
//! Bits 0–127 of the encoded vector are *gap* bits, one per ring position (ring
//! sizes 17/23/26/29/33): `0` draws an arc, `1` leaves a gap. The bits from 128 on
//! are the *color stream*, assigned in order to the visible positions: `0` paints the
//! foreground color, `1` the derived third color. Consecutive same-color arcs merge,
//! each arc extending clockwise across adjacent gap positions until the next visible
//! one, and every drawn arc is inset by the ring's half-gap angle at both ends.

use std::fmt::Write;

/// An RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
    }

    /// Parse `RRGGBB` (with optional leading `#`).
    pub fn parse(s: &str) -> Option<Color> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Color::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// The three colors of a code: foreground arcs, background disc, third-color arcs.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub foreground: Color,
    pub background: Color,
    pub third: Color,
}

/// The 9 base palettes of Apple's generator (foreground, third; background is white).
const BASE_PALETTES: [(Color, Color); 9] = [
    (Color::new(0x00, 0x00, 0x00), Color::new(0x88, 0x88, 0x88)),
    (Color::new(0x77, 0x77, 0x77), Color::new(0xAA, 0xAA, 0xAA)),
    (Color::new(0xFF, 0x3B, 0x30), Color::new(0xFF, 0x99, 0x99)),
    (Color::new(0xEE, 0x77, 0x33), Color::new(0xEE, 0xBB, 0x88)),
    (Color::new(0x33, 0xAA, 0x22), Color::new(0x99, 0xDD, 0x99)),
    (Color::new(0x00, 0xA6, 0xA1), Color::new(0x88, 0xDD, 0xCC)),
    (Color::new(0x00, 0x7A, 0xFF), Color::new(0x77, 0xBB, 0xFF)),
    (Color::new(0x58, 0x56, 0xD6), Color::new(0xBB, 0xBB, 0xEE)),
    (Color::new(0xCC, 0x73, 0xE1), Color::new(0xEE, 0xBB, 0xEE)),
];

const WHITE: Color = Color::new(0xFF, 0xFF, 0xFF);

/// Palette for one of Apple's 18 color templates: even indices put white arcs on the
/// colored disc, odd indices colored arcs on white.
pub fn template_palette(index: usize) -> Option<Palette> {
    let (fg, third) = *BASE_PALETTES.get(index / 2)?;
    Some(if index.is_multiple_of(2) {
        Palette {
            foreground: WHITE,
            background: fg,
            third,
        }
    } else {
        Palette {
            foreground: fg,
            background: WHITE,
            third,
        }
    })
}

/// Third color for a custom fg/bg pair: the preset when the pair matches one, the
/// midpoint otherwise.
pub fn third_color(fg: Color, bg: Color) -> Color {
    for &(pf, third) in &BASE_PALETTES {
        if (fg == pf && bg == WHITE) || (fg == WHITE && bg == pf) {
            return third;
        }
    }
    Color::new(
        ((u16::from(fg.r) + u16::from(bg.r)) / 2) as u8,
        ((u16::from(fg.g) + u16::from(bg.g)) / 2) as u8,
        ((u16::from(fg.b) + u16::from(bg.b)) / 2) as u8,
    )
}

/// Which center glyph the code carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoKind {
    /// Camera-scan glyph (the default).
    Camera,
    /// Phone glyph used for NFC-integrated codes.
    Nfc,
}

/// Ring geometry (radius, rotation °, positions, half-gap °), innermost first.
pub(super) const RINGS: [(f64, f64, usize, f64); 5] = [
    (177.2016, -78.0, 17, 7.5),
    (224.1012, -85.0, 23, 5.6),
    (271.0008, -70.0, 26, 5.0),
    (317.9004, -63.0, 29, 4.2),
    (364.8, -70.0, 33, 3.5),
];

const CENTER: f64 = 400.0;
const BG_RADIUS: f64 = 400.0;
const STROKE: f64 = 23.5;

/// Render the encoded bit vector as a self-contained SVG document.
pub(super) fn render_svg(bits: &[bool], pal: &Palette, url: &str, logo: LogoKind) -> String {
    let mut s = String::with_capacity(16 * 1024);
    s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    let _ = writeln!(
        s,
        "<svg data-design=\"Fingerprint\" data-payload=\"{}\" viewBox=\"0 0 800 800\" xmlns=\"http://www.w3.org/2000/svg\">",
        escape_xml(url)
    );
    s.push_str("    <title>App Clip Code</title>\n");
    let _ = writeln!(
        s,
        "    <circle cx=\"{CENTER:.6}\" cy=\"{CENTER:.6}\" id=\"Background\" r=\"{BG_RADIUS:.6}\" style=\"fill:{}\"/>",
        pal.background.hex()
    );
    s.push_str("    <g id=\"Markers\">\n");

    let gap_bits = &bits[..128];
    let color_stream = &bits[128..];
    let mut offset = 0usize;
    let mut color_idx = 0usize;
    for (ring, &(radius, rotation, n, half_gap)) in RINGS.iter().enumerate() {
        // Per-position state: None = gap, Some(false) = foreground, Some(true) = third.
        let state: Vec<Option<bool>> = (0..n)
            .map(|i| {
                if gap_bits[offset + i] {
                    None
                } else {
                    let c = color_stream.get(color_idx).copied().unwrap_or(false);
                    color_idx += 1;
                    Some(c)
                }
            })
            .collect();
        offset += n;

        let _ = writeln!(
            s,
            "        <g name=\"ring-{}\" transform=\"rotate({rotation:.0} {CENTER:.0} {CENTER:.0})\">",
            ring + 1
        );
        write_ring_arcs(&mut s, radius, n as f64, half_gap, &state, pal);
        s.push_str("        </g>\n");
    }
    s.push_str("    </g>\n");
    write_logo(&mut s, logo, pal);
    s.push_str("</svg>\n");
    s
}

fn write_ring_arcs(
    s: &mut String,
    radius: f64,
    n: f64,
    half_gap: f64,
    state: &[Option<bool>],
    pal: &Palette,
) {
    let bit_angle = 360.0 / n;
    let count = state.len();
    let mut i = 0usize;
    while i < count {
        let Some(third) = state[i] else {
            i += 1;
            continue;
        };
        // Extend clockwise across trailing gaps; the last visible arc also absorbs
        // the leading gaps that wrap past position 0.
        let mut span = 1usize;
        while i + span < count && state[i + span].is_none() {
            span += 1;
        }
        if i + span == count {
            span += state.iter().take_while(|p| p.is_none()).count();
        }

        let start = i as f64 * bit_angle + half_gap;
        let end = (i + span) as f64 * bit_angle - half_gap;
        let (sx, sy) = (
            CENTER + radius * (start.to_radians()).cos(),
            CENTER + radius * (start.to_radians()).sin(),
        );
        let (ex, ey) = (
            CENTER + radius * (end.to_radians()).cos(),
            CENTER + radius * (end.to_radians()).sin(),
        );
        let mut arc_span = end - start;
        if arc_span < 0.0 {
            arc_span += 360.0;
        }
        let large = usize::from(arc_span > 180.0);
        let color = if third { pal.third } else { pal.foreground };
        let _ = writeln!(
            s,
            "            <path d=\"M {ex:.6} {ey:.6} A {radius:.6} {radius:.6} 0 {large} 0 {sx:.6} {sy:.6}\" data-color=\"{}\" style=\"fill:none;stroke:{};stroke-linecap:round;stroke-miterlimit:10;stroke-width:{STROKE:.6}px\"/>",
            usize::from(third),
            color.hex()
        );
        i += span;
    }
}

// Center glyph path data (from Apple's generator output).
const CAMERA_PATHS: [&str; 3] = [
    "M56.9500008,40.9528084c-8.9051094,0-16.0505219,7.1454086-16.0505219,16.1038475c0,8.9051132,7.1454124,16.0505219,16.0505219,16.0505219s16.050518-7.1454086,16.050518-16.0505219C73.0005188,48.0448914,65.8551102,40.9528084,56.9500008,40.9528084z M56.9500008,67.7214508c-5.8656387,0-10.6647949-4.7458305-10.6647949-10.6647949c0-5.9722939,4.7458305-10.7181244,10.6647949-10.7181244s10.6647987,4.7458305,10.6647987,10.7181244C67.6147995,62.975605,62.8689651,67.7214508,56.9500008,67.7214508z",
    "M78.919487,42.1259422c-2.1862869,0-3.9992981,1.8663368-3.9992981,3.9459686c0,2.2396164,1.8130112,3.9459724,3.9992981,3.9459724c2.1329575-0.0533257,3.9459686-1.7596855,3.9459686-3.9459724C82.8654556,43.9389534,81.0524445,42.1259422,78.919487,42.1259422z",
    "M57.0033264,0C57.0033264,0,56.9500008,0,57.0033264,0C25.4888554,0,0,25.4888554,0,56.9500008s25.4888554,56.9500008,56.9500008,56.9500008s56.9500008-25.4888535,56.9500008-56.9500008C113.9000015,25.542181,88.4111481,0.053311,57.0033264,0z M93.7435455,72.733902c0,6.3455505-3.412735,9.7049713-9.8116074,9.7049713H29.9147377c-6.4522038,0-9.8116074-3.3594055-9.8116074-9.7049713V41.4860458c0-6.3455467,3.3594036-9.7049694,9.8116074-9.7049694h7.785305c2.3462524,0,3.0927849-0.4265842,4.5325356-1.9196663l2.3462524-2.5062332c1.5463943-1.6530457,3.1461105-2.4529057,6.185585-2.4529057H62.975605c3.0394707,0,4.6391945,0.79986,6.1855812,2.4529057l2.3462524,2.5062332c1.4397583,1.4930649,2.1862869,1.9196663,4.5325394,1.9196663h7.8919449c6.4522018,0,9.8116074,3.3594036,9.8116074,9.7049694V72.733902H93.7435455z",
];

const PHONE_OUTER: &str = "M53.92,0a53.92,53.92,0,1,0,53.92,53.92A53.92,53.92,0,0,0,53.92,0Zm30,91.32c-1,.78-2,1.51-3,2.21h0a47.94,47.94,0,0,1-53.92,0h0c-1-.7-2-1.44-3-2.21V36.8C24,28.47,28.47,24,36.91,24H71c8.38,0,12.9,4.51,12.9,12.84Z";
const PHONE_SCREEN: &str = "M77.89,95.42V36.8c0-5.06-1.81-6.85-6.9-6.85H68.92v.13A2.69,2.69,0,0,1,66.11,33H41.8A2.69,2.69,0,0,1,39,30.08V30H36.92c-5.14,0-7,1.79-7,6.85V95.42h0a48,48,0,0,0,47.94,0h0Z";

fn write_logo(s: &mut String, logo: LogoKind, pal: &Palette) {
    match logo {
        LogoKind::Nfc => {
            s.push_str("    <g id=\"Logo\" data-logo-type=\"phone\" transform=\"translate(293.400000 293.400000) scale(1.980000 1.980000)\">\n");
            let _ = writeln!(
                s,
                "        <path id=\"outer_circle\" d=\"{PHONE_OUTER}\" style=\"fill:{}\"/>",
                pal.foreground.hex()
            );
            let _ = writeln!(
                s,
                "        <path id=\"phone_screen\" d=\"{PHONE_SCREEN}\" style=\"fill:{};isolation:isolate\"/>",
                pal.third.hex()
            );
        }
        LogoKind::Camera => {
            s.push_str("    <g id=\"Logo\" data-logo-type=\"Camera\" transform=\"translate(293.275699 293.275699) scale(1.874000 1.874000)\">\n");
            for p in CAMERA_PATHS {
                let _ = writeln!(s, "        <path d=\"{p}\" style=\"fill:{}\"/>", pal.foreground.hex());
            }
        }
    }
    s.push_str("    </g>\n");
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
