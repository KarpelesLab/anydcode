//! Apple **App Clip Code**: the circular five-ring code iOS scans to launch an App
//! Clip.
//!
//! Apple has never published this format; everything here follows the public
//! reverse engineering of Apple's `AppClipCodeGenerator` and
//! `URLCompression.framework` ([rs/appclipcode], MIT), which this module reimplements
//! from scratch in Rust. The trained Huffman frequency tables in `data/` are byte
//! dumps of the framework's models (~1.7 MB), which is why the whole module sits
//! behind the default-on `appclip` cargo feature — lean builds can opt out with
//! `default-features = false`.
//!
//! # Format in one paragraph
//!
//! An HTTPS URL is compressed to at most 128 bits by multi-context Huffman coders
//! (trained tries for hosts and paths, TLD tables, a path wordbook, LEB128 numerics),
//! split into two Reed–Solomon-protected blocks plus a GF(16) metadata block,
//! scrambled, permuted through a fixed 128-entry LUT and laid out on five concentric
//! rings of 17/23/26/29/33 positions: the first 128 bits choose which positions carry
//! an arc versus a gap, and the remaining bits color each visible arc in either the
//! foreground or a derived third color. See `doc/SPEC.md` of [rs/appclipcode] for the
//! full write-up.
//!
//! Encoding and structural decoding (bits → URL) are implemented; camera detection of
//! the printed circle is not yet wired into the live pipeline.
//!
//! [rs/appclipcode]: https://github.com/rs/appclipcode
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "appclip")] {
//! use anyd::codes::appclip;
//!
//! let svg = appclip::generate_svg("https://example.com", &appclip::Options::default()).unwrap();
//! assert!(svg.contains("data-design=\"Fingerprint\""));
//!
//! // Structural round-trip: bits back to the URL.
//! let payload = appclip::compress_url("https://example.com").unwrap();
//! let bits = appclip::encode_payload(&payload).unwrap();
//! let back = appclip::decode_payload(&bits).unwrap();
//! assert_eq!(appclip::decompress_url(&back).unwrap(), "https://example.com");
//! # }
//! ```

mod codec;
mod gf;
mod huffman;
mod svg;
mod tables;
mod url;

pub use codec::{decode_payload, encode_payload};
pub use svg::{Color, LogoKind, Palette, template_palette, third_color};
pub use url::{compress_url, decompress_url};

use crate::error::Result;

/// Generation options: colors and the center glyph.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Color palette; defaults to Apple template 1 (black on white).
    pub palette: Palette,
    /// Center glyph; camera-scan by default.
    pub logo: LogoKind,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            palette: template_palette(1).expect("template 1 exists"),
            logo: LogoKind::Camera,
        }
    }
}

impl Options {
    /// Options using one of Apple's 18 predefined color templates.
    pub fn with_template(index: usize) -> Option<Options> {
        Some(Options {
            palette: template_palette(index)?,
            logo: LogoKind::Camera,
        })
    }
}

/// Generate the SVG for `url`. The URL must be HTTPS and compressible to 128 bits —
/// short hosts and common path shapes fit; arbitrary long URLs may not.
pub fn generate_svg(url: &str, opts: &Options) -> Result<String> {
    let payload = compress_url(url)?;
    let bits = encode_payload(&payload)?;
    Ok(svg::render_svg(&bits, &opts.palette, url, opts.logo))
}

/// Decode a full ring bit vector (gap bits + color stream) back to its URL.
pub fn decode_bits(bits: &[bool]) -> Result<String> {
    let payload = decode_payload(bits)?;
    decompress_url(&payload)
}
