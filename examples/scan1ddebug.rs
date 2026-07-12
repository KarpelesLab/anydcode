// Diagnostic: what does scan1d recover on a real 1D photo, and why does EAN reject it?
use anyd::codes::ean::EanDecoder;
use anyd::output::Encoding;
use anyd::scan1d::{ScanOptions, scan_lines};
use anyd::traits::Decode;

fn main() {
    let path = std::env::args().nth(1).expect("usage: scan1ddebug <png>");
    let rgba = oxideav_png::decode_png_to_rgba(&std::fs::read(&path).unwrap()).unwrap();
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = anyd::GrayFrame::new(&luma, w, h).unwrap();

    // Reference: encode the known payload to compare recovered patterns against.
    let reference: Option<Vec<bool>> = std::env::args().nth(2).and_then(|digits| {
        use anyd::codes::ean::EanEncoder;
        use anyd::traits::Encode;
        let enc = EanEncoder::new();
        let sym = enc.build_ean13(&digits).ok()?;
        match enc.encode(&sym).ok()? {
            Encoding::Linear(p) => Some(p.modules),
            _ => None,
        }
    });
    if let Some(r) = &reference {
        println!("reference pattern: {} modules", r.len());
    }
    let as_str = |m: &[bool]| {
        m.iter()
            .map(|&b| if b { '#' } else { '.' })
            .collect::<String>()
    };

    let opts = ScanOptions::default();
    let cands = scan_lines(&frame, &opts);
    println!("frame {w}x{h}: {} scan1d candidates", cands.len());
    let dec = EanDecoder::new();
    for (i, c) in cands.iter().enumerate() {
        let mods = c.pattern.modules.len();
        let quiet = c.pattern.quiet_zone;
        let res = dec.decode(&Encoding::Linear(c.pattern.clone()));
        let verdict = match res {
            Ok(s) => format!("OK -> {:?}", s.text()),
            Err(e) => format!("ERR {e}"),
        };
        // Best-alignment Hamming distance vs reference (slide the shorter over the longer).
        let hd = reference.as_ref().map(|r| {
            let (a, b) = (&c.pattern.modules, r);
            let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
            let mut best = short.len();
            for off in 0..=(long.len() - short.len()) {
                let d = short
                    .iter()
                    .zip(&long[off..])
                    .filter(|(x, y)| x != y)
                    .count();
                best = best.min(d);
            }
            best
        });
        println!(
            "  [{i}] conf={:.3} modules={mods} quiet={quiet} hamming={hd:?} :: {verdict}",
            c.confidence
        );
        if reference.is_some() && (93..=97).contains(&mods) {
            println!("       {}", as_str(&c.pattern.modules));
        }
    }
    if let Some(r) = &reference {
        println!("  REF  {}", as_str(r));
    }
}
