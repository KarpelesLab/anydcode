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

/// Bit-cost model for one candidate mode, used by [`optimize_segments`].
///
/// Per-character costs are expressed in *sixths of a bit* so the fractional
/// densities of numeric (10 bits / 3 digits) and alphanumeric (11 bits / 2
/// characters) stay exact in integer arithmetic.
#[derive(Debug, Clone)]
pub struct ModeCost {
    /// The mode segments produced by this entry are tagged with.
    pub mode: Mode,
    /// Bits paid when opening a segment (mode indicator + character count field).
    pub head_bits: u32,
    /// Bits paid when closing a segment (self-terminating modes, e.g. Han Xin).
    pub tail_bits: u32,
    /// Cost of one payload byte, in sixths of a bit.
    pub char_cost_sixths: u32,
    /// Whether a byte is representable in this mode.
    pub accepts: fn(u8) -> bool,
}

/// Split `data` into the cheapest sequence of mode-tagged segments under `costs`.
///
/// Dynamic program over (position, mode): each byte either extends the current
/// segment or closes it (paying `tail_bits`, rounding the segment up to whole
/// bits) and opens a new one (paying `head_bits`). Per-character costs are the
/// average density, so a closed segment's cost can undershoot the true encoding
/// by a couple of bits when its length leaves a partial final group — callers
/// re-measure candidates with their exact bit writer before committing to a
/// version or size.
///
/// Returns `None` when some byte is representable in no mode (possible only
/// when `costs` lacks an accept-everything byte mode, e.g. Micro QR M1/M2).
pub fn optimize_segments(data: &[u8], costs: &[ModeCost]) -> Option<Vec<Segment>> {
    const INF: u64 = u64::MAX / 2;
    let round_up_bits = |c: u64| c.div_ceil(6) * 6;
    if data.is_empty() {
        return Some(Vec::new());
    }
    // dp[j]: cheapest cost (in sixths) of the prefix consumed so far, with the
    // open segment in mode j — its head paid, its tail not yet.
    let mut dp: Vec<u64> = costs
        .iter()
        .map(|c| {
            if (c.accepts)(data[0]) {
                u64::from(c.head_bits) * 6 + u64::from(c.char_cost_sixths)
            } else {
                INF
            }
        })
        .collect();
    // choice[i][j]: the mode index active at byte i-1 when byte i is taken in mode j.
    let mut choice = vec![vec![0u8; costs.len()]; data.len()];
    for (i, &b) in data.iter().enumerate().skip(1) {
        let mut next = vec![INF; costs.len()];
        for (j, cj) in costs.iter().enumerate() {
            if !(cj.accepts)(b) {
                continue;
            }
            // Stay in mode j, or close a mode-k segment and open a fresh j.
            let mut best = dp[j];
            let mut pick = j;
            for (k, ck) in costs.iter().enumerate() {
                if k == j || dp[k] >= INF {
                    continue;
                }
                let switched = round_up_bits(dp[k] + u64::from(ck.tail_bits) * 6)
                    + u64::from(cj.head_bits) * 6;
                if switched < best {
                    best = switched;
                    pick = k;
                }
            }
            if best >= INF {
                continue;
            }
            next[j] = best + u64::from(cj.char_cost_sixths);
            choice[i][j] = pick as u8;
        }
        dp = next;
    }
    // Close the final segment and pick the cheapest ending mode.
    let mut mode_idx = (0..costs.len())
        .min_by_key(|&j| dp[j].saturating_add(u64::from(costs[j].tail_bits) * 6))?;
    if dp[mode_idx] >= INF {
        return None;
    }
    // Walk the choices back to tag every byte with its mode.
    let mut tags = vec![0u8; data.len()];
    for i in (0..data.len()).rev() {
        tags[i] = mode_idx as u8;
        mode_idx = choice[i][mode_idx] as usize;
    }
    // Merge runs of equal tags into segments.
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=data.len() {
        if i == data.len() || tags[i] != tags[start] {
            out.push(Segment {
                mode: costs[tags[start] as usize].mode.clone(),
                data: data[start..i].to_vec(),
            });
            start = i;
        }
    }
    Some(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn is_digit(b: u8) -> bool {
        b.is_ascii_digit()
    }
    fn is_alnum(b: u8) -> bool {
        matches!(b, b'0'..=b'9' | b'A'..=b'Z' | b' ' | b'$' | b'%' | b'*' | b'+' | b'-' | b'.' | b'/' | b':')
    }
    fn any(_: u8) -> bool {
        true
    }

    /// QR version-group-0 costs (versions 1–9).
    fn qr_costs() -> Vec<ModeCost> {
        vec![
            ModeCost {
                mode: Mode::Numeric,
                head_bits: 4 + 10,
                tail_bits: 0,
                char_cost_sixths: 20,
                accepts: is_digit,
            },
            ModeCost {
                mode: Mode::Alphanumeric,
                head_bits: 4 + 9,
                tail_bits: 0,
                char_cost_sixths: 33,
                accepts: is_alnum,
            },
            ModeCost {
                mode: Mode::Byte,
                head_bits: 4 + 8,
                tail_bits: 0,
                char_cost_sixths: 48,
                accepts: any,
            },
        ]
    }

    #[test]
    fn empty_input_yields_no_segments() {
        assert_eq!(optimize_segments(b"", &qr_costs()), Some(Vec::new()));
    }

    #[test]
    fn digits_pick_numeric() {
        let segs = optimize_segments(b"0123456789", &qr_costs()).unwrap();
        assert_eq!(segs, vec![Segment::numeric(b"0123456789".to_vec())]);
    }

    #[test]
    fn hello_world_picks_alphanumeric() {
        let segs = optimize_segments(b"HELLO WORLD", &qr_costs()).unwrap();
        assert_eq!(segs, vec![Segment::alphanumeric(b"HELLO WORLD".to_vec())]);
    }

    #[test]
    fn lowercase_falls_back_to_byte() {
        let segs = optimize_segments(b"hello", &qr_costs()).unwrap();
        assert_eq!(segs, vec![Segment::byte(b"hello".to_vec())]);
    }

    #[test]
    fn short_digit_run_stays_merged() {
        // Splitting "12" out of an alphanumeric run costs a 14-bit numeric header
        // plus a 13-bit re-open; keeping it alphanumeric costs ~11 bits.
        let segs = optimize_segments(b"AB12CD", &qr_costs()).unwrap();
        assert_eq!(segs, vec![Segment::alphanumeric(b"AB12CD".to_vec())]);
    }

    #[test]
    fn long_digit_run_splits_out() {
        let segs =
            optimize_segments(b"ABCDEF00000000000000000000000000ABCDEF", &qr_costs()).unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::alphanumeric(b"ABCDEF".to_vec()),
                Segment::numeric(b"00000000000000000000000000".to_vec()),
                Segment::alphanumeric(b"ABCDEF".to_vec()),
            ]
        );
    }

    #[test]
    fn mixed_url_splits_sensibly() {
        let segs = optimize_segments(
            b"https://example.com/A0123456789012345678901234",
            &qr_costs(),
        )
        .unwrap();
        // The lowercase prefix must be byte; the long digit tail must be numeric.
        assert_eq!(segs.first().unwrap().mode, Mode::Byte);
        assert_eq!(segs.last().unwrap().mode, Mode::Numeric);
        let flat: Vec<u8> = segs.iter().flat_map(|s| s.data.clone()).collect();
        assert_eq!(
            flat,
            b"https://example.com/A0123456789012345678901234".to_vec()
        );
    }

    #[test]
    fn unrepresentable_byte_returns_none() {
        // No byte mode on offer (Micro QR M2-style): lowercase cannot be encoded.
        let costs: Vec<ModeCost> = qr_costs().into_iter().take(2).collect();
        assert_eq!(optimize_segments(b"abc", &costs), None);
        // But digits still work.
        let segs = optimize_segments(b"123", &costs).unwrap();
        assert_eq!(segs, vec![Segment::numeric(b"123".to_vec())]);
    }

    #[test]
    fn multibyte_utf8_never_splits_mid_character() {
        let text = "abc日本語123456789012345".as_bytes();
        let segs = optimize_segments(text, &qr_costs()).unwrap();
        let flat: Vec<u8> = segs.iter().flat_map(|s| s.data.clone()).collect();
        assert_eq!(flat, text.to_vec());
        for s in &segs {
            if s.mode == Mode::Byte {
                assert!(
                    std::str::from_utf8(&s.data).is_ok(),
                    "byte segment split a UTF-8 char"
                );
            }
        }
    }
}
