//! Postal (height-modulated / 4-state) symbologies.
//!
//! This module implements the height-modulated postal barcodes, where data is
//! carried by the *height* of equal-width, equally-spaced vertical bars rather
//! than by bar/space widths. Two physical families are covered:
//!
//! - **2-state** (bars are *tall* or *short*): USPS **POSTNET**
//!   ([`Symbology::Postnet`]) and **PLANET** ([`Symbology::Planet`]).
//! - **4-state** (each bar is *full*, *ascender*, *descender* or *tracker*):
//!   USPS **Intelligent Mail** ([`Symbology::IntelligentMail`], IMb / OneCode),
//!   Royal Mail **RM4SCC** ([`Symbology::RoyalMail`]) and Dutch **KIX**
//!   ([`Symbology::KixCode`]).
//!
//! Australia Post, Japan Post and Mailmark remain [`Error::Unsupported`].
//!
//! ## Output representation & BitMatrix cell convention
//!
//! A single-row [`LinearPattern`] cannot express bar heights, so postal codes
//! render to [`Encoding::Matrix`] with a **fixed height of 3 module rows** and
//! **one bar per two columns** (bar column, then a 1-module light gap):
//!
//! - Row `0` (top) — the **ascender** band.
//! - Row `1` (middle) — the **tracker** band; dark for *every* bar (all four
//!   states share the central tracking region).
//! - Row `2` (bottom) — the **descender** band.
//!
//! A bar occupies even columns `0, 2, 4, …`; odd columns are an all-light gap.
//! `N` bars therefore produce a `(2N-1) × 3` matrix. Each bar's three cells
//! encode its state:
//!
//! | State       | top | mid | bot |
//! |-------------|-----|-----|-----|
//! | `Tracker`   |  ·  |  #  |  ·  |
//! | `Ascender`  |  #  |  #  |  ·  |
//! | `Descender` |  ·  |  #  |  #  |
//! | `Full`      |  #  |  #  |  #  |
//!
//! For the 2-state codes a **tall** bar maps to `Ascender` and a **short** bar
//! to `Tracker`, so the middle row stays dark for both and the whole family
//! shares one convention. The mapping is a bijection, so the decoder recovers
//! the exact bar states — and thus the exact segments and [`PostalMeta`] — from
//! the matrix, giving `encode(decode(x)) == x`.
//!
//! ## Lossless model
//!
//! The human-readable payload lives verbatim in the symbol's [`Segment`]s and
//! excludes any computed check character (POSTNET/PLANET mod-10 digit, RM4SCC
//! checksum) or CRC (IMb); those are recomputed on encode and verified on
//! decode. [`PostalMeta`] records only which family member produced the bars.
//!
//! - POSTNET/PLANET: one [`Segment::numeric`] of ZIP/data digits (no check digit).
//! - IMb: two numeric segments — the 20-digit *tracking code* then the 0/5/9/11
//!   digit *routing code* (an empty segment when there is no routing code).
//! - RM4SCC/KIX: one [`Segment::alphanumeric`] over the `0-9 A-Z` alphabet (no
//!   check character; KIX has none by definition).
//!
//! [`Symbology::Postnet`]: crate::symbology::Symbology::Postnet
//! [`Symbology::Planet`]: crate::symbology::Symbology::Planet
//! [`Symbology::IntelligentMail`]: crate::symbology::Symbology::IntelligentMail
//! [`Symbology::RoyalMail`]: crate::symbology::Symbology::RoyalMail
//! [`Symbology::KixCode`]: crate::symbology::Symbology::KixCode
//! [`Error::Unsupported`]: crate::error::Error::Unsupported
//! [`Encoding::Matrix`]: crate::output::Encoding::Matrix
//! [`LinearPattern`]: crate::output::LinearPattern
//! [`Segment`]: crate::segment::Segment
//! [`Segment::numeric`]: crate::segment::Segment::numeric
//! [`Segment::alphanumeric`]: crate::segment::Segment::alphanumeric

mod imb;
mod postnet;
mod rm4scc;

use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Number of module rows in a rendered postal matrix (ascender / tracker /
/// descender bands).
const ROWS: usize = 3;

/// Light-module margin recorded around a postal matrix. Postal clear zones are
/// specified physically (in inches), not in modules; this is a nominal value and
/// is not load-bearing for the round-trip.
const QUIET: usize = 0;

/// The state of a single postal bar (its height / extension pattern).
///
/// The tracker band (matrix row 1) is dark for all four states; the ascender
/// (row 0) and descender (row 2) bands vary. For the 2-state codes only
/// [`BarState::Ascender`] (tall) and [`BarState::Tracker`] (short) occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarState {
    /// Tracker only (neither ascender nor descender).
    Tracker,
    /// Ascender + tracker (a tall bar, for 2-state codes).
    Ascender,
    /// Descender + tracker.
    Descender,
    /// Full bar (both ascender and descender).
    Full,
}

impl BarState {
    /// Build a state from the presence of an ascender and a descender.
    fn from_parts(ascender: bool, descender: bool) -> BarState {
        match (ascender, descender) {
            (true, true) => BarState::Full,
            (true, false) => BarState::Ascender,
            (false, true) => BarState::Descender,
            (false, false) => BarState::Tracker,
        }
    }

    /// Whether the ascender band is dark.
    fn has_ascender(self) -> bool {
        matches!(self, BarState::Ascender | BarState::Full)
    }

    /// Whether the descender band is dark.
    fn has_descender(self) -> bool {
        matches!(self, BarState::Descender | BarState::Full)
    }
}

/// Which member of the postal family a [`Symbol`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PostalVariant {
    /// USPS POSTNET (2-state).
    Postnet,
    /// USPS PLANET (2-state).
    Planet,
    /// USPS Intelligent Mail Barcode (4-state, 65 bars).
    IntelligentMail,
    /// Royal Mail RM4SCC (4-state).
    RoyalMail,
    /// Dutch KIX / PostNL (4-state, no checksum or framing bars).
    KixCode,
}

impl PostalVariant {
    /// The [`Symbology`] this variant corresponds to.
    fn symbology(self) -> Symbology {
        match self {
            PostalVariant::Postnet => Symbology::Postnet,
            PostalVariant::Planet => Symbology::Planet,
            PostalVariant::IntelligentMail => Symbology::IntelligentMail,
            PostalVariant::RoyalMail => Symbology::RoyalMail,
            PostalVariant::KixCode => Symbology::KixCode,
        }
    }

    /// The variant a [`Symbology`] maps to, if it is a postal one.
    fn from_symbology(s: Symbology) -> Option<PostalVariant> {
        Some(match s {
            Symbology::Postnet => PostalVariant::Postnet,
            Symbology::Planet => PostalVariant::Planet,
            Symbology::IntelligentMail => PostalVariant::IntelligentMail,
            Symbology::RoyalMail => PostalVariant::RoyalMail,
            Symbology::KixCode => PostalVariant::KixCode,
            _ => return None,
        })
    }
}

/// Parameters required to re-encode a postal symbol identically.
///
/// The payload (ZIP/tracking/routing digits or the RM4SCC alphabet string) lives
/// in the symbol's [`Segment`]s; the only re-encoding parameter is which family
/// member produced the bars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostalMeta {
    /// The postal family member.
    pub variant: PostalVariant,
}

impl PostalMeta {
    /// Metadata for the given variant.
    pub fn new(variant: PostalVariant) -> Self {
        PostalMeta { variant }
    }
}

/// Postal (height-modulated / 4-state) encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct PostalEncoder;

impl PostalEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        PostalEncoder
    }

    /// Build a POSTNET symbol from ZIP/data digits (5, 9 or 11 are typical). The
    /// mod-10 check digit is computed on encode and must not be included here.
    pub fn build_postnet(&self, digits: &str) -> Result<Symbol> {
        postnet::validate_digits(digits.as_bytes())?;
        Ok(numeric_symbol(
            PostalVariant::Postnet,
            digits.as_bytes().to_vec(),
        ))
    }

    /// Build a PLANET symbol from data digits (11 or 13 are typical). The mod-10
    /// check digit is computed on encode and must not be included here.
    pub fn build_planet(&self, digits: &str) -> Result<Symbol> {
        postnet::validate_digits(digits.as_bytes())?;
        Ok(numeric_symbol(
            PostalVariant::Planet,
            digits.as_bytes().to_vec(),
        ))
    }

    /// Build an Intelligent Mail (IMb) symbol from a 20-digit tracking code and a
    /// routing code of 0, 5, 9 or 11 digits (`""` for none).
    pub fn build_imb(&self, tracking: &str, routing: &str) -> Result<Symbol> {
        imb::validate(tracking.as_bytes(), routing.as_bytes())?;
        Ok(Symbol::new(
            Symbology::IntelligentMail,
            vec![
                Segment::numeric(tracking.as_bytes().to_vec()),
                Segment::numeric(routing.as_bytes().to_vec()),
            ],
            SymbolMeta::Postal(PostalMeta::new(PostalVariant::IntelligentMail)),
        ))
    }

    /// Build an RM4SCC symbol from a `0-9 A-Z` string (postcode + DPS). The
    /// checksum character is computed on encode.
    pub fn build_rm4scc(&self, data: &str) -> Result<Symbol> {
        rm4scc::validate(data.as_bytes())?;
        Ok(alnum_symbol(
            PostalVariant::RoyalMail,
            data.as_bytes().to_vec(),
        ))
    }

    /// Build a Dutch KIX symbol from a `0-9 A-Z` string (no checksum or framing).
    pub fn build_kix(&self, data: &str) -> Result<Symbol> {
        rm4scc::validate(data.as_bytes())?;
        Ok(alnum_symbol(
            PostalVariant::KixCode,
            data.as_bytes().to_vec(),
        ))
    }
}

impl Encode for PostalEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        let variant =
            PostalVariant::from_symbology(symbol.symbology).ok_or(Error::Unsupported {
                what: "non-postal symbology passed to PostalEncoder",
            })?;
        if let SymbolMeta::Postal(meta) = &symbol.meta
            && meta.variant != variant
        {
            return Err(Error::invalid_parameter(
                "PostalMeta variant disagrees with symbol symbology",
            ));
        }

        let bars = match variant {
            PostalVariant::Postnet => postnet::encode(false, &numeric_data(symbol)?)?,
            PostalVariant::Planet => postnet::encode(true, &numeric_data(symbol)?)?,
            PostalVariant::IntelligentMail => {
                let (tracking, routing) = imb_fields(symbol)?;
                imb::encode(&tracking, &routing)?
            }
            PostalVariant::RoyalMail => rm4scc::encode(false, &alnum_data(symbol)?)?,
            PostalVariant::KixCode => rm4scc::encode(true, &alnum_data(symbol)?)?,
        };
        Ok(Encoding::Matrix(bars_to_matrix(&bars)))
    }
}

/// Postal (height-modulated / 4-state) decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct PostalDecoder;

impl PostalDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        PostalDecoder
    }
}

impl Decode for PostalDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let matrix = match encoding {
            Encoding::Matrix(m) => m,
            Encoding::Linear(_) => {
                return Err(Error::Unsupported {
                    what: "postal decode of a linear pattern",
                });
            }
        };
        let bars = matrix_to_bars(matrix)?;
        let n = bars.len();
        let four_state = bars
            .iter()
            .any(|b| matches!(b, BarState::Descender | BarState::Full));

        if n == 65 {
            // Only the IMb uses exactly 65 bars.
            let (variant, segments) = imb::decode(&bars)?;
            return Ok(build_symbol(variant, segments));
        }
        if !four_state && n >= 7 && (n - 2) % 5 == 0 {
            // POSTNET / PLANET: 2-state, tall framing bars, groups of five.
            let (variant, digits) = postnet::decode(&bars)?;
            return Ok(build_symbol(variant, vec![Segment::numeric(digits)]));
        }
        if four_state && n % 4 == 2 && n >= 6 {
            // RM4SCC: start bar + 4-bar groups (incl. checksum) + stop bar.
            let (variant, chars) = rm4scc::decode_rm4scc(&bars)?;
            return Ok(build_symbol(variant, vec![Segment::alphanumeric(chars)]));
        }
        if four_state && n % 4 == 0 && n >= 4 {
            // KIX: bare 4-bar groups, no framing or checksum.
            let (variant, chars) = rm4scc::decode_kix(&bars)?;
            return Ok(build_symbol(variant, vec![Segment::alphanumeric(chars)]));
        }
        Err(Error::undecodable("unrecognized postal bar layout"))
    }
}

// ---- Symbol / matrix helpers ----------------------------------------------

/// Assemble a symbol carrying a single numeric segment for `variant`.
fn numeric_symbol(variant: PostalVariant, digits: Vec<u8>) -> Symbol {
    Symbol::new(
        variant.symbology(),
        vec![Segment::numeric(digits)],
        SymbolMeta::Postal(PostalMeta::new(variant)),
    )
}

/// Assemble a symbol carrying a single alphanumeric segment for `variant`.
fn alnum_symbol(variant: PostalVariant, data: Vec<u8>) -> Symbol {
    Symbol::new(
        variant.symbology(),
        vec![Segment::alphanumeric(data)],
        SymbolMeta::Postal(PostalMeta::new(variant)),
    )
}

/// Assemble a decoded symbol with the correct symbology and metadata.
fn build_symbol(variant: PostalVariant, segments: Vec<Segment>) -> Symbol {
    Symbol::new(
        variant.symbology(),
        segments,
        SymbolMeta::Postal(PostalMeta::new(variant)),
    )
}

/// The bytes of the first numeric segment.
fn numeric_data(symbol: &Symbol) -> Result<Vec<u8>> {
    symbol
        .segments
        .iter()
        .find(|s| matches!(s.mode, Mode::Numeric))
        .map(|s| s.data.clone())
        .ok_or_else(|| Error::invalid_data("postal symbol has no numeric segment"))
}

/// The bytes of the first data segment (numeric or alphanumeric).
fn alnum_data(symbol: &Symbol) -> Result<Vec<u8>> {
    symbol
        .segments
        .iter()
        .find(|s| matches!(s.mode, Mode::Numeric | Mode::Alphanumeric))
        .map(|s| s.data.clone())
        .ok_or_else(|| Error::invalid_data("postal symbol has no data segment"))
}

/// The IMb `(tracking, routing)` fields from the symbol's two numeric segments.
fn imb_fields(symbol: &Symbol) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut numerics = symbol
        .segments
        .iter()
        .filter(|s| matches!(s.mode, Mode::Numeric));
    let tracking = numerics
        .next()
        .map(|s| s.data.clone())
        .ok_or_else(|| Error::invalid_data("IMb symbol missing tracking segment"))?;
    let routing = numerics.next().map(|s| s.data.clone()).unwrap_or_default();
    Ok((tracking, routing))
}

/// Render a bar sequence to the 3-row [`BitMatrix`] (see module docs).
fn bars_to_matrix(bars: &[BarState]) -> BitMatrix {
    let width = if bars.is_empty() {
        0
    } else {
        2 * bars.len() - 1
    };
    let mut m = BitMatrix::new(width, ROWS, QUIET);
    for (i, &bar) in bars.iter().enumerate() {
        let col = i * 2;
        if bar.has_ascender() {
            m.set(col, 0, true);
        }
        m.set(col, 1, true); // tracker band: dark for every bar
        if bar.has_descender() {
            m.set(col, 2, true);
        }
    }
    m
}

/// Recover the bar sequence from a 3-row postal [`BitMatrix`].
fn matrix_to_bars(m: &BitMatrix) -> Result<Vec<BarState>> {
    if m.height() != ROWS {
        return Err(Error::undecodable("postal matrix must be 3 rows tall"));
    }
    let w = m.width();
    if w == 0 || w.is_multiple_of(2) {
        return Err(Error::undecodable("postal matrix width must be odd"));
    }
    let n = w.div_ceil(2);
    let mut bars = Vec::with_capacity(n);
    for i in 0..n {
        let col = i * 2;
        if !m.get(col, 1) {
            return Err(Error::undecodable("postal bar missing its tracker module"));
        }
        bars.push(BarState::from_parts(m.get(col, 0), m.get(col, 2)));
        // The gap column following each bar (if any) must be all light.
        let gap = col + 1;
        if gap < w && (m.get(gap, 0) || m.get(gap, 1) || m.get(gap, 2)) {
            return Err(Error::undecodable("postal gap column is not light"));
        }
    }
    Ok(bars)
}
