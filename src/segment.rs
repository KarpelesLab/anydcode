//! Data segments: the mode-tagged pieces a symbol's payload is split into.
//!
//! Most symbologies let the same logical text be encoded several different ways
//! (e.g. a QR payload split into a numeric run followed by a byte run). To make
//! re-encoding *lossless* — byte-for-byte identical to the scanned symbol — we
//! preserve the exact segmentation and mode of each piece rather than collapsing
//! everything to a single decoded string.

/// The encoding mode of a single [`Segment`].
///
/// Not every mode applies to every symbology; encoders reject modes they cannot
/// represent. The set is a superset chosen to cover the roadmap in
/// [`crate::Symbology`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Digits `0-9`, packed densely (QR: 3 digits / 10 bits).
    Numeric,
    /// The restricted alphanumeric set `0-9 A-Z $%*+-./: ` and space.
    Alphanumeric,
    /// Raw 8-bit bytes, interpreted per the active ECI (default: ISO-8859-1 / UTF-8).
    Byte,
    /// Double-byte Kanji (Shift-JIS), used by QR/Han Xin.
    Kanji,
    /// Extended Channel Interpretation switch: selects the character set for the
    /// bytes that follow. Carries the ECI assignment number.
    Eci(u32),
}

impl Mode {
    /// Whether this mode carries payload bytes (as opposed to being a control switch).
    pub fn is_data(&self) -> bool {
        !matches!(self, Mode::Eci(_))
    }
}

/// One mode-tagged piece of a symbol's payload.
///
/// The interpretation of [`Segment::data`] depends on [`Segment::mode`]:
/// - `Numeric`      → ASCII digit bytes (`b'0'..=b'9'`).
/// - `Alphanumeric` → ASCII bytes from the alphanumeric set.
/// - `Byte`         → raw payload bytes as stored in the symbol.
/// - `Kanji`        → the source Shift-JIS bytes (2 per character).
/// - `Eci`          → empty; the assignment lives in the mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// How [`Segment::data`] is encoded.
    pub mode: Mode,
    /// The raw payload for this segment (see type docs for interpretation).
    pub data: Vec<u8>,
}

impl Segment {
    /// A numeric segment from ASCII digits.
    pub fn numeric(digits: impl Into<Vec<u8>>) -> Self {
        Segment {
            mode: Mode::Numeric,
            data: digits.into(),
        }
    }

    /// An alphanumeric segment.
    pub fn alphanumeric(data: impl Into<Vec<u8>>) -> Self {
        Segment {
            mode: Mode::Alphanumeric,
            data: data.into(),
        }
    }

    /// A raw byte segment.
    pub fn byte(data: impl Into<Vec<u8>>) -> Self {
        Segment {
            mode: Mode::Byte,
            data: data.into(),
        }
    }

    /// A Kanji (Shift-JIS) segment.
    pub fn kanji(data: impl Into<Vec<u8>>) -> Self {
        Segment {
            mode: Mode::Kanji,
            data: data.into(),
        }
    }

    /// An ECI mode switch (no payload).
    pub fn eci(assignment: u32) -> Self {
        Segment {
            mode: Mode::Eci(assignment),
            data: Vec::new(),
        }
    }
}
