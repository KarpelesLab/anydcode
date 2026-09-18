//! Deterministic pseudo-random robustness sweep over the linear and postal decoders.
//!
//! The camera front-end feeds every noisy scanline to every decoder in both
//! directions, so a decoder must never panic on malformed input, and anything it does
//! accept must be a well-formed symbol its own encoder can render again.
#![cfg(all(
    feature = "decode",
    feature = "encode",
    feature = "linear",
    feature = "postal"
))]

use anyd::codes::codabar::{CodabarDecoder, CodabarEncoder};
use anyd::codes::code11::{Code11Decoder, Code11Encoder};
use anyd::codes::code39::{Code39Decoder, Code39Encoder};
use anyd::codes::code93::{Code93Decoder, Code93Encoder};
use anyd::codes::code128::{Code128Decoder, Code128Encoder};
use anyd::codes::dxfilm::{DxFilmDecoder, DxFilmEncoder};
use anyd::codes::ean::{EanDecoder, EanEncoder};
use anyd::codes::itf::{ItfDecoder, ItfEncoder};
use anyd::codes::msi::{MsiCheck, MsiDecoder, MsiEncoder};
use anyd::codes::pharmacode::{PharmacodeDecoder, PharmacodeEncoder};
use anyd::codes::postal::{PostalDecoder, PostalEncoder};
use anyd::codes::telepen::{TelepenDecoder, TelepenEncoder};
use anyd::codes::twoof5::{TwoOf5Decoder, TwoOf5Encoder};
use anyd::output::{BitMatrix, Encoding, LinearPattern};
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// xorshift64* — small, deterministic, good enough to shake out edge cases.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A decoder paired with the encoder that must be able to re-render its output.
type Pair = (&'static str, Box<dyn Decode>, Box<dyn Encode>);

fn linear_pairs() -> Vec<Pair> {
    let mut pairs: Vec<Pair> = vec![
        (
            "ean",
            Box::new(EanDecoder::new()),
            Box::new(EanEncoder::new()),
        ),
        (
            "code128",
            Box::new(Code128Decoder::new()),
            Box::new(Code128Encoder::new()),
        ),
        (
            "code93",
            Box::new(Code93Decoder::new()),
            Box::new(Code93Encoder::new()),
        ),
        (
            "itf",
            Box::new(ItfDecoder::new()),
            Box::new(ItfEncoder::new()),
        ),
        (
            "itf+check",
            Box::new(ItfDecoder::with_check(true)),
            Box::new(ItfEncoder::new()),
        ),
        (
            "twoof5",
            Box::new(TwoOf5Decoder::new()),
            Box::new(TwoOf5Encoder::new()),
        ),
        (
            "codabar",
            Box::new(CodabarDecoder::new()),
            Box::new(CodabarEncoder::new()),
        ),
        (
            "plessey",
            Box::new(MsiDecoder::plessey()),
            Box::new(MsiEncoder::new()),
        ),
        (
            "telepen",
            Box::new(TelepenDecoder::new()),
            Box::new(TelepenEncoder::new()),
        ),
        (
            "telepen+check",
            Box::new(TelepenDecoder::with_check(true)),
            Box::new(TelepenEncoder::new()),
        ),
        (
            "pharmacode",
            Box::new(PharmacodeDecoder::new()),
            Box::new(PharmacodeEncoder::new()),
        ),
        (
            "pharmacode2",
            Box::new(PharmacodeDecoder::two_track()),
            Box::new(PharmacodeEncoder::new()),
        ),
    ];
    for full_ascii in [false, true] {
        for check in [false, true] {
            pairs.push((
                "code39",
                Box::new(
                    Code39Decoder::new()
                        .with_full_ascii(full_ascii)
                        .with_check_digit(check),
                ),
                Box::new(Code39Encoder::new()),
            ));
        }
    }
    for count in 0..=2 {
        pairs.push((
            "code11",
            Box::new(Code11Decoder::new().with_check_count(count)),
            Box::new(Code11Encoder::new()),
        ));
    }
    for scheme in [
        MsiCheck::None,
        MsiCheck::Mod10,
        MsiCheck::Mod11,
        MsiCheck::Mod1010,
        MsiCheck::Mod1110,
    ] {
        pairs.push((
            "msi",
            Box::new(MsiDecoder::with_check(scheme)),
            Box::new(MsiEncoder::new()),
        ));
    }
    pairs
}

/// Valid patterns from every linear encoder, as seeds for mutation.
fn linear_seeds() -> Vec<LinearPattern> {
    let mut symbols = Vec::new();
    let ean = EanEncoder::new();
    symbols.push((
        ean.build_ean13("590123412345").unwrap(),
        &ean as &dyn Encode,
    ));
    symbols.push((ean.build_ean8("9638507").unwrap(), &ean));
    symbols.push((ean.build_upca("03600029145").unwrap(), &ean));
    symbols.push((ean.build_upce("0425261").unwrap(), &ean));
    symbols.push((ean.build_ean2("53").unwrap(), &ean));
    symbols.push((ean.build_ean5("52495").unwrap(), &ean));
    let with5 = ean
        .with_addon(ean.build_ean13("978186197271").unwrap(), "51299")
        .unwrap();
    symbols.push((with5, &ean));
    let with2 = ean
        .with_addon(ean.build_upce("0123456").unwrap(), "07")
        .unwrap();
    symbols.push((with2, &ean));
    let c128 = Code128Encoder::new();
    symbols.push((c128.build_text("Ab\x0112345678x").unwrap(), &c128));
    let c39 = Code39Encoder::new();
    symbols.push((c39.build(b"CODE-39", false, false).unwrap(), &c39));
    symbols.push((c39.build(b"a$B", true, true).unwrap(), &c39));
    let c93 = Code93Encoder::new();
    symbols.push((c93.build(b"TEST93", false).unwrap(), &c93));
    symbols.push((c93.build(b"x!Y", true).unwrap(), &c93));
    let c11 = Code11Encoder::new();
    for count in 0..=2 {
        symbols.push((c11.build(b"123-45", count).unwrap(), &c11));
    }
    let itf = ItfEncoder::new();
    symbols.push((itf.build(b"123456", false).unwrap(), &itf));
    symbols.push((itf.build(b"12345", true).unwrap(), &itf));
    let t25 = TwoOf5Encoder::new();
    for s in [
        Symbology::Std2of5,
        Symbology::Iata2of5,
        Symbology::Matrix2of5,
    ] {
        symbols.push((t25.build(s, b"1234").unwrap(), &t25));
    }
    let codabar = CodabarEncoder::new();
    symbols.push((codabar.build(b'A', b"12-34", b'D').unwrap(), &codabar));
    let msi = MsiEncoder::new();
    for scheme in [
        MsiCheck::None,
        MsiCheck::Mod10,
        MsiCheck::Mod11,
        MsiCheck::Mod1010,
        MsiCheck::Mod1110,
    ] {
        symbols.push((msi.build_msi(b"1234567", scheme).unwrap(), &msi));
    }
    symbols.push((msi.build_plessey(b"C0DE").unwrap(), &msi));
    let telepen = TelepenEncoder::new();
    symbols.push((telepen.build(b"Tele", false).unwrap(), &telepen));
    symbols.push((telepen.build(b"Tele", true).unwrap(), &telepen));
    let pharma = PharmacodeEncoder::new();
    symbols.push((pharma.build(1234).unwrap(), &pharma));
    symbols.push((pharma.build_two_track(117480).unwrap(), &pharma));

    symbols
        .into_iter()
        .map(|(symbol, enc)| match enc.encode(&symbol).unwrap() {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => panic!("linear encoder produced a matrix"),
        })
        .collect()
}

/// Feed `modules` (and its mirror) to every decoder; anything accepted must re-encode.
fn probe(pairs: &[Pair], modules: &[bool]) {
    let mut pattern = LinearPattern::new();
    pattern.quiet_zone = 10;
    for mirrored in [false, true] {
        pattern.modules = modules.to_vec();
        if mirrored {
            pattern.modules.reverse();
        }
        let encoding = Encoding::Linear(pattern.clone());
        for (name, dec, enc) in pairs {
            if let Ok(symbol) = dec.decode(&encoding) {
                assert!(
                    enc.encode(&symbol).is_ok(),
                    "{name} accepted {:?} as {symbol:?} but cannot re-encode it",
                    pattern
                );
            }
        }
    }
}

#[test]
fn degenerate_patterns_do_not_panic() {
    let pairs = linear_pairs();
    probe(&pairs, &[]);
    for len in [1usize, 2, 3, 7, 50, 95, 5000] {
        probe(&pairs, &vec![true; len]);
        probe(&pairs, &vec![false; len]);
        let alternating: Vec<bool> = (0..len).map(|i| i % 2 == 0).collect();
        probe(&pairs, &alternating);
        // One narrow bar followed by a huge run.
        let mut lopsided = vec![true];
        lopsided.extend(vec![false; len]);
        lopsided.push(true);
        probe(&pairs, &lopsided);
    }
}

#[test]
fn random_patterns_do_not_panic() {
    let pairs = linear_pairs();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..20000 {
        let runs = rng.below(if round % 50 == 0 { 600 } else { 120 });
        let max_width = 1 + rng.below(4);
        let mut modules = Vec::new();
        let mut bar = rng.below(8) != 0; // mostly start on a bar, as the front-end does
        for _ in 0..runs {
            let width = 1 + rng.below(max_width);
            modules.extend(core::iter::repeat_n(bar, width));
            bar = !bar;
        }
        probe(&pairs, &modules);
    }
}

#[test]
fn mutated_symbols_do_not_panic() {
    let pairs = linear_pairs();
    let seeds = linear_seeds();
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for seed in &seeds {
        probe(&pairs, &seed.modules);
        for _ in 0..600 {
            let mut m = seed.modules.clone();
            for _ in 0..1 + rng.below(3) {
                if m.is_empty() {
                    break;
                }
                let at = rng.below(m.len());
                match rng.below(5) {
                    0 => m[at] = !m[at],
                    1 => {
                        m.remove(at);
                    }
                    2 => m.insert(at, rng.below(2) == 0),
                    3 => m.truncate(at),
                    _ => {
                        // Splice in a chunk of another symbol.
                        let other = &seeds[rng.below(seeds.len())].modules;
                        let from = rng.below(other.len());
                        let to = from + rng.below(other.len() - from);
                        m.splice(at..at, other[from..to].iter().copied());
                    }
                }
            }
            probe(&pairs, &m);
        }
    }
}

/// Feed a matrix to the postal and DX Film Edge decoders; accepted symbols must re-encode.
fn probe_matrix(m: BitMatrix) {
    let encoding = Encoding::Matrix(m);
    if let Ok(symbol) = PostalDecoder::new().decode(&encoding) {
        assert!(
            PostalEncoder::new().encode(&symbol).is_ok(),
            "postal accepted {symbol:?} but cannot re-encode it"
        );
    }
    if let Ok(symbol) = DxFilmDecoder::new().decode(&encoding) {
        assert!(
            DxFilmEncoder::new().encode(&symbol).is_ok(),
            "DX film accepted {symbol:?} but cannot re-encode it"
        );
    }
}

#[test]
fn random_matrices_do_not_panic() {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    for (w, h) in [(0, 0), (0, 3), (1, 3), (3, 0), (1, 1), (31, 2), (23, 2)] {
        probe_matrix(BitMatrix::new(w, h, 0));
    }
    // Well-formed bar grids of every interesting bar count, random bar states.
    for round in 0..4000 {
        let bars = match round % 8 {
            0 => 65,
            1 => 37,
            2 => 52,
            3 => 67,
            4 => 66,
            5 => 78,
            6 => 32,
            _ => 1 + rng.below(90),
        };
        let mut m = BitMatrix::new(2 * bars - 1, 3, 2);
        let two_state = rng.below(4) == 0;
        for i in 0..bars {
            m.set(2 * i, 1, true);
            if rng.below(2) == 0 {
                m.set(2 * i, 0, true);
            }
            if !two_state && rng.below(2) == 0 {
                m.set(2 * i, 2, true);
            }
        }
        probe_matrix(m);
    }
    // Arbitrary noise grids.
    for _ in 0..500 {
        let (w, h) = (rng.below(140), rng.below(5));
        let mut m = BitMatrix::new(w, h, 0);
        for y in 0..h {
            for x in 0..w {
                if rng.below(2) == 0 {
                    m.set(x, y, true);
                }
            }
        }
        probe_matrix(m);
    }
}
