//! Deterministic no-panic fuzzing of the stacked / DataBar structural decoders.
//!
//! Decoders take untrusted input, so they must fail cleanly — never panic — on random
//! grids, on valid symbols with random module flips, and on symbols whose metadata
//! carries arbitrary (but renderable) symbol values. Whenever a damaged symbol still
//! decodes, the result must itself re-encode without panicking.
#![cfg(all(
    feature = "decode",
    feature = "encode",
    feature = "pdf417",
    feature = "code16k",
    feature = "code49",
    feature = "codablockf",
    feature = "databar"
))]

use anyd::codes::codablockf::{CodablockFDecoder, CodablockFEncoder};
use anyd::codes::code16k::{Code16kDecoder, Code16kEncoder, Code16kMeta};
use anyd::codes::code49::{Code49Decoder, Code49Encoder};
use anyd::codes::databar::{DataBarDecoder, DataBarEncoder};
use anyd::codes::pdf417::{
    EcLevel, MicroPdf417Decoder, MicroPdf417Encoder, Pdf417Decoder, Pdf417Encoder,
};
use anyd::output::{BitMatrix, Encoding, LinearPattern};
use anyd::segment::Segment;
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// xorshift64* — small, deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn bytes(&mut self, len: usize, max: u8) -> Vec<u8> {
        (0..len)
            .map(|_| (self.next() % (max as u64 + 1)) as u8)
            .collect()
    }
}

fn random_matrix(rng: &mut Rng, width: usize, height: usize) -> BitMatrix {
    let mut m = BitMatrix::new(width, height, 1);
    // Vary the dark density so long runs (wide elements) occur as well as noise.
    let density = 1 + rng.below(9);
    for y in 0..height {
        for x in 0..width {
            if rng.below(10) < density {
                m.set(x, y, true);
            }
        }
    }
    m
}

fn flip_matrix(rng: &mut Rng, m: &BitMatrix, flips: usize) -> BitMatrix {
    let mut out = m.clone();
    for _ in 0..flips {
        let (x, y) = (rng.below(m.width()), rng.below(m.height()));
        out.set(x, y, !m.get(x, y));
    }
    out
}

/// Decode `encoding` with `dec`; a successful decode must also re-encode cleanly.
fn exercise(dec: &dyn Decode, enc: &dyn Encode, encoding: &Encoding) {
    if let Ok(symbol) = dec.decode(encoding) {
        let _ = enc.encode(&symbol);
    }
}

fn fuzz_matrix_decoder(
    seed: u64,
    dec: &dyn Decode,
    enc: &dyn Encode,
    dims: &[(usize, usize)],
    valid: &[Encoding],
) {
    let mut rng = Rng(seed);
    for round in 0..60 {
        let (w, h) = dims[round % dims.len()];
        exercise(dec, enc, &Encoding::Matrix(random_matrix(&mut rng, w, h)));
    }
    for encoding in valid {
        let Encoding::Matrix(m) = encoding else {
            panic!("expected a matrix");
        };
        for _ in 0..60 {
            let flips = 1 + rng.below(12);
            let damaged = flip_matrix(&mut rng, m, flips);
            exercise(dec, enc, &Encoding::Matrix(damaged));
        }
    }
}

#[test]
fn pdf417_never_panics() {
    let enc = Pdf417Encoder::new();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut valid = Vec::new();
    for level in [0u8, 1, 2, 4] {
        let segments = vec![
            Segment::alphanumeric(b"Fuzz: PDF417 (mixed) text, 12345".to_vec()),
            Segment::numeric(b"000123456789012345678901234567890123456789012345".to_vec()),
            Segment::byte(rng.bytes(13, 255)),
        ];
        let symbol = enc.build(segments, EcLevel::new(level).unwrap()).unwrap();
        valid.push(enc.encode(&symbol).unwrap());
    }
    // 17·c + 69 wide, 3 module rows per codeword row; the last is 30 columns × 90 rows.
    let dims = [(86, 9), (103, 30), (154, 12), (120, 9), (579, 270)];
    fuzz_matrix_decoder(1, &Pdf417Decoder::new(), &enc, &dims, &valid);
}

#[test]
fn micropdf417_never_panics() {
    let enc = MicroPdf417Encoder::new();
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut valid = Vec::new();
    for cols in 1..=4 {
        let segments = vec![
            Segment::alphanumeric(b"Micro fuzz".to_vec()),
            Segment::byte(rng.bytes(7, 255)),
        ];
        let symbol = enc.build_sized(segments, Some(cols)).unwrap();
        valid.push(enc.encode(&symbol).unwrap());
    }
    let dims = [(38, 22), (55, 16), (82, 12), (99, 8), (99, 88), (40, 22)];
    fuzz_matrix_decoder(2, &MicroPdf417Decoder::new(), &enc, &dims, &valid);
}

#[test]
fn code16k_never_panics() {
    let enc = Code16kEncoder::new();
    let dec = Code16kDecoder::new();
    let mut rng = Rng(0x0dd_ba11);
    let valid: Vec<Encoding> = [&b"Code 16K fuzz"[..], b"\x01\x02ab\x7f", b"0123456789"]
        .iter()
        .map(|d| enc.encode(&enc.build(d).unwrap()).unwrap())
        .collect();
    fuzz_matrix_decoder(
        3,
        &dec,
        &enc,
        &[(70, 2), (70, 16), (70, 17), (71, 4)],
        &valid,
    );

    // Arbitrary symbol values are renderable (the checks are derived), so they reach
    // the payload reconstruction with every mode / code-set combination.
    for _ in 0..3000 {
        let rows = 2 + rng.below(15);
        let mut values = rng.bytes(rows * 5 - 2, 106);
        values[0] = 7 * (rows as u8 - 2) + rng.below(7) as u8;
        let symbol = Symbol::new(
            Symbology::Code16k,
            Vec::new(),
            SymbolMeta::Code16k(Code16kMeta { rows, values }),
        );
        let encoding = enc.encode(&symbol).unwrap();
        if let Ok(decoded) = dec.decode(&encoding) {
            assert_eq!(enc.encode(&decoded).unwrap(), encoding);
        }
    }
}

#[test]
fn code49_never_panics() {
    let enc = Code49Encoder::new();
    let valid: Vec<Encoding> = [&b"CODE 49 FUZZ"[..], b"lower\x01case", b"0123456789012345"]
        .iter()
        .map(|d| enc.encode(&enc.build(d).unwrap()).unwrap())
        .collect();
    let dims = [(70, 2), (70, 8), (70, 9), (69, 3)];
    fuzz_matrix_decoder(4, &Code49Decoder::new(), &enc, &dims, &valid);
}

#[test]
fn codablockf_never_panics() {
    let enc = CodablockFEncoder::new();
    let valid: Vec<Encoding> = [
        &b"Codablock F fuzz input, long enough for several rows"[..],
        b"\x01\x02ab",
    ]
    .iter()
    .map(|d| enc.encode(&enc.build(d).unwrap()).unwrap())
    .collect();
    let dims = [(101, 2), (112, 5), (739, 44), (24, 3), (750, 45)];
    fuzz_matrix_decoder(5, &CodablockFDecoder::new(), &enc, &dims, &valid);
}

#[test]
fn databar_never_panics() {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let mut rng = Rng(0xda7a_ba20);

    // Linear: random rows at every accepted element count, then damaged symbols.
    for round in 0..400 {
        let len = [96, 79, 102, 134, 151, 543, 20, 1][round % 8] + rng.below(3);
        let modules: Vec<bool> = (0..len).map(|_| rng.below(3) == 0).collect();
        let pattern = LinearPattern {
            modules,
            quiet_zone: 1,
        };
        exercise(&dec, &enc, &Encoding::Linear(pattern));
    }
    let linear = [
        enc.build_omni(b"0950110153001").unwrap(),
        enc.build_limited(b"1234567890123").unwrap(),
        enc.build_expanded(b"0198898765432106320201234515991231")
            .unwrap(),
        enc.build_expanded(b"10ABCdef123\x1d21xyz").unwrap(),
    ];
    for symbol in &linear {
        let Encoding::Linear(p) = enc.encode(symbol).unwrap() else {
            panic!("expected a linear pattern");
        };
        for _ in 0..200 {
            let mut damaged = p.clone();
            for _ in 0..1 + rng.below(6) {
                let i = rng.below(damaged.modules.len());
                damaged.modules[i] = !damaged.modules[i];
            }
            exercise(&dec, &enc, &Encoding::Linear(damaged));
        }
    }

    // Stacked matrices.
    let valid = [
        enc.encode(&enc.build_stacked(b"0950110153001").unwrap())
            .unwrap(),
        enc.encode(&enc.build_stacked_omni(b"0950110153001").unwrap())
            .unwrap(),
        enc.encode(
            &enc.build_expanded_stacked(b"0198898765432106320201234515991231", 2)
                .unwrap(),
        )
        .unwrap(),
        enc.encode(
            &enc.build_expanded_stacked(b"10ABCdef123\x1d21xyz", 3)
                .unwrap(),
        )
        .unwrap(),
    ];
    let dims = [(50, 3), (50, 5), (53, 1), (102, 5), (151, 9), (543, 13)];
    fuzz_matrix_decoder(6, &dec, &enc, &dims, &valid);
}
