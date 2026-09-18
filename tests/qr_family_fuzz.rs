//! Deterministic no-panic fuzzing of the QR-family structural decoders (QR, Micro QR,
//! rMQR, Han Xin). Decoders take untrusted module grids, so on *any* input they must
//! return `Ok`/`Err` — never panic — and whatever they accept must re-encode
//! (`encode(decode(x))` is defined for every decodable `x`).
#![cfg(all(
    feature = "decode",
    feature = "encode",
    feature = "qr",
    feature = "microqr",
    feature = "rmqr",
    feature = "hanxin"
))]

use anyd::codes::hanxin::{self, HanXinDecoder, HanXinEncoder};
use anyd::codes::microqr::{MicroEcLevel, MicroQrDecoder, MicroQrEncoder};
use anyd::codes::qr::{EcLevel, QrDecoder, QrEncoder};
use anyd::codes::rmqr::{RmqrDecoder, RmqrEcLevel, RmqrEncoder, RmqrSize};
use anyd::output::{BitMatrix, Encoding};
use anyd::segment::Segment;
use anyd::symbol::Symbol;
use anyd::traits::{Decode, Encode};

/// xorshift64: a fixed-seed generator keeps every run identical.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 16) % n as u64) as usize
    }
}

fn random_matrix(rng: &mut Rng, width: usize, height: usize) -> BitMatrix {
    let mut m = BitMatrix::new(width, height, 0);
    for y in 0..height {
        for x in 0..width {
            if rng.next() & 0x100 != 0 {
                m.set(x, y, true);
            }
        }
    }
    m
}

/// Decode `matrix`; a success must re-encode without error.
fn probe(decoder: &dyn Decode, encoder: &dyn Encode, matrix: BitMatrix) -> Option<Symbol> {
    let decoded = decoder.decode(&Encoding::Matrix(matrix)).ok()?;
    encoder
        .encode(&decoded)
        .expect("a decoded symbol must re-encode");
    Some(decoded)
}

/// Random grids of `sizes`, then `symbols` under growing random damage: few flips
/// must still recover the payload, heavy damage must merely not panic.
fn fuzz(
    decoder: &dyn Decode,
    encoder: &dyn Encode,
    sizes: &[(usize, usize)],
    symbols: &[Symbol],
    rng: &mut Rng,
) {
    for &(w, h) in sizes {
        for _ in 0..40 {
            probe(decoder, encoder, random_matrix(rng, w, h));
        }
    }
    for symbol in symbols {
        let Encoding::Matrix(clean) = encoder.encode(symbol).unwrap() else {
            panic!("expected a matrix");
        };
        let (w, h) = (clean.width(), clean.height());
        for round in 0..200 {
            let mut m = clean.clone();
            // 1, 2, 3, … flips, then bursts covering up to the whole grid.
            let flips = if round < 20 {
                round / 4 + 1
            } else {
                rng.below(w * h) + 1
            };
            for _ in 0..flips {
                let (x, y) = (rng.below(w), rng.below(h));
                let v = m.get(x, y);
                m.set(x, y, !v);
            }
            let decoded = probe(decoder, encoder, m);
            if flips == 1 {
                // One module is within every EC level here (and both format copies).
                let decoded = decoded.expect("a single flipped module must be corrected");
                assert_eq!(decoded.segments, symbol.segments);
            }
        }
    }
}

#[test]
fn qr_never_panics() {
    let enc = QrEncoder::new();
    let symbols = [
        enc.build_text("HELLO WORLD 0123456789", EcLevel::L)
            .unwrap(),
        enc.build_text("mixed Case payload, 12345678", EcLevel::H)
            .unwrap(),
        enc.build(vec![Segment::byte(vec![0xA5; 400])], EcLevel::M)
            .unwrap(),
        enc.build(
            vec![
                Segment::eci(26),
                Segment::kanji(vec![0x93, 0x5F, 0xE4, 0xAA]),
            ],
            EcLevel::Q,
        )
        .unwrap(),
    ];
    let sizes = [
        (0, 0),
        (1, 1),
        (20, 20),
        (21, 21),
        (21, 25),
        (22, 22),
        (25, 25),
        (45, 45),
        (57, 57),
        (177, 177),
        (181, 181),
    ];
    let mut rng = Rng(0x0DDB_1A5E_5BAD_5EED);
    fuzz(&QrDecoder::new(), &enc, &sizes, &symbols, &mut rng);
}

#[test]
fn microqr_never_panics() {
    let enc = MicroQrEncoder::new();
    let symbols = [
        enc.build_text("12345", MicroEcLevel::L).unwrap(),
        enc.build_text("AB12", MicroEcLevel::M).unwrap(),
        enc.build_text("hello", MicroEcLevel::M).unwrap(),
        enc.build_text("Micro QR!", MicroEcLevel::Q).unwrap(),
    ];
    let sizes = [
        (0, 0),
        (9, 9),
        (11, 11),
        (13, 13),
        (15, 15),
        (17, 17),
        (19, 19),
    ];
    let mut rng = Rng(0x5EED_0F31_C0DE_1234);
    fuzz(&MicroQrDecoder::new(), &enc, &sizes, &symbols, &mut rng);
}

#[test]
fn rmqr_never_panics() {
    let enc = RmqrEncoder::new();
    let symbols = [
        enc.build_text("12345", RmqrEcLevel::M).unwrap(),
        enc.build_text("rMQR payload with Mixed 0123456789", RmqrEcLevel::H)
            .unwrap(),
        enc.build(vec![Segment::byte(vec![0x5A; 120])], RmqrEcLevel::M)
            .unwrap(),
    ];
    let mut sizes: Vec<(usize, usize)> = RmqrSize::all().map(|s| (s.width(), s.height())).collect();
    sizes.extend([(0, 0), (43, 8), (7, 43), (140, 17)]);
    let mut rng = Rng(0xC0FF_EE00_DEAD_BEEF);
    fuzz(&RmqrDecoder::new(), &enc, &sizes, &symbols, &mut rng);
}

#[test]
fn hanxin_never_panics() {
    let enc = HanXinEncoder::new();
    let symbols = [
        enc.build_text("1234567", hanxin::EcLevel::L1).unwrap(),
        enc.build_text("Han Xin, text & 42", hanxin::EcLevel::L2)
            .unwrap(),
        enc.build(vec![Segment::byte(vec![0xC3; 12])], hanxin::EcLevel::L4)
            .unwrap(),
    ];
    let sizes = [
        (0, 0),
        (21, 21),
        (23, 23),
        (24, 24),
        (25, 25),
        (27, 27),
        (29, 29),
    ];
    let mut rng = Rng(0xFACE_FEED_0BAD_F00D);
    fuzz(&HanXinDecoder::new(), &enc, &sizes, &symbols, &mut rng);
}
