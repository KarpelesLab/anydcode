// Diagnostic: how far is each grid the QR sampler tries from the symbol that was
// printed? Usage: qrdiff <png> <expected.txt>
//
// The expected text is encoded at the sampled version under every EC level, mask and
// two segmentations (optimised, and one byte segment); the closest reference is
// reported with the count of differing modules and, with -v, a map of them
// (`#`/`.` agree, `X` sampled dark where light was printed, `o` the reverse).
use anyd::codes::qr::{EcLevel, Mask, QrEncoder, QrMeta, Version, sample_grids};
use anyd::output::{BitMatrix, Encoding};
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::traits::Encode;

fn references(text: &[u8], version: u8) -> Vec<(String, BitMatrix)> {
    let enc = QrEncoder::new();
    let mut out = Vec::new();
    for level in [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H] {
        let mut builds = vec![("byte", enc.build(vec![Segment::byte(text.to_vec())], level))];
        if let Ok(t) = std::str::from_utf8(text) {
            builds.push(("opt", enc.build_text(t, level)));
        }
        for (how, built) in builds {
            let Ok(mut symbol) = built else { continue };
            for mask in 0..8 {
                let SymbolMeta::Qr(meta) = &symbol.meta else {
                    continue;
                };
                symbol.meta = SymbolMeta::Qr(QrMeta {
                    version: Version::new(version).unwrap(),
                    mask: Mask::new(mask).unwrap(),
                    ..*meta
                });
                if let Ok(Encoding::Matrix(m)) = enc.encode(&symbol) {
                    out.push((format!("{level:?}/{how}/mask{mask}"), m));
                }
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    let args: Vec<&String> = args.iter().filter(|a| *a != "-v").collect();
    let rgba = oxideav_png::decode_rgba8(&std::fs::read(args[0]).unwrap()).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();
    let mut text = std::fs::read(args[1]).unwrap();
    while text.last().is_some_and(|c| matches!(c, b'\n' | b'\r')) && args.len() < 3 {
        text.pop();
    }
    let grids = sample_grids(&frame);
    println!("{} grids", grids.len());
    let mut best: Option<(usize, usize, String, BitMatrix, BitMatrix)> = None;
    for (i, grid) in grids.iter().enumerate() {
        let dim = grid.width();
        let version = ((dim - 17) / 4) as u8;
        for (name, reference) in references(&text, version) {
            let diff = (0..dim * dim)
                .filter(|&k| grid.get(k % dim, k / dim) != reference.get(k % dim, k / dim))
                .count();
            if best
                .as_ref()
                .is_none_or(|b| diff * b.3.width().pow(2) < b.0 * dim * dim)
            {
                best = Some((diff, i, name, grid.clone(), reference));
            }
        }
    }
    let Some((diff, i, name, grid, reference)) = best else {
        println!("no reference could be built");
        return;
    };
    // Every grid of that size against the same reference.
    let per: Vec<String> = grids
        .iter()
        .filter(|g| g.width() == grid.width())
        .map(|g| {
            let d = g.width();
            (0..d * d)
                .filter(|&k| g.get(k % d, k / d) != reference.get(k % d, k / d))
                .count()
                .to_string()
        })
        .collect();
    println!("all grids of this size: {}", per.join(" "));
    let dim = grid.width();
    println!(
        "closest: grid #{i} dim {dim} vs {name}: {diff} of {} modules differ ({:.1}%)",
        dim * dim,
        100.0 * diff as f64 / (dim * dim) as f64
    );
    if verbose {
        for y in 0..dim {
            let row: String = (0..dim)
                .map(|x| match (grid.get(x, y), reference.get(x, y)) {
                    (true, true) => '#',
                    (false, false) => '.',
                    (true, false) => 'X',
                    (false, true) => 'o',
                })
                .collect();
            println!("{row}");
        }
    }
}
