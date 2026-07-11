//! `anyd` — command-line encoder/decoder for AnyDCode barcodes.
//!
//! Built only when the `cli` feature is enabled (it pulls in `oxideav-png` for
//! PNG I/O; the library itself stays dependency-free). Build with:
//! `cargo build --features cli`.
//!
//! Usage:
//!   anyd encode <symbology> <data> [--format png|unicode|svg] [--out FILE]
//!                                  [--scale N] [--ec L|M|Q|H|0-8] [--invert]
//!   anyd decode <image.png> [--symbology <name>]
//!   anyd list
//!   anyd help

use std::collections::BTreeMap;
use std::process::ExitCode;

use anyd::GrayFrame;
use anyd::Symbol;
use anyd::output::Encoding;
use anyd::render::render;
use anyd::segment::Segment;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("encode") => cmd_encode(&args[1..]),
        Some("decode") => cmd_decode(&args[1..]),
        Some("list") => {
            print_symbologies();
            Ok(())
        }
        None | Some("help" | "-h" | "--help") => {
            print_help();
            Ok(())
        }
        Some(other) => Err(format!(
            "unknown command '{other}' (try: encode, decode, list, help)"
        )),
    }
}

/// Split args into positionals and `--key value` / `--key=value` / `--flag` options.
struct Opts {
    positional: Vec<String>,
    options: BTreeMap<String, String>,
}

fn parse_opts(args: &[String]) -> Opts {
    let mut positional = Vec::new();
    let mut options = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(rest) = a.strip_prefix("--") {
            if let Some((k, v)) = rest.split_once('=') {
                options.insert(k.to_string(), v.to_string());
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                options.insert(rest.to_string(), args[i + 1].clone());
                i += 1;
            } else {
                options.insert(rest.to_string(), "true".to_string());
            }
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    Opts {
        positional,
        options,
    }
}

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

fn cmd_encode(args: &[String]) -> Result<(), String> {
    let opts = parse_opts(args);
    if opts.positional.len() < 2 {
        return Err("usage: anyd encode <symbology> <data> [--format ...] [--out ...]".into());
    }
    let symbology = opts.positional[0].to_lowercase();
    let data = &opts.positional[1];
    let ec = opts.options.get("ec").map(String::as_str);

    let encoding = build_encoding(&symbology, data, ec)?;

    let format = opts
        .options
        .get("format")
        .map(String::as_str)
        .unwrap_or("unicode");
    let scale: usize = opts
        .options
        .get("scale")
        .map(|s| s.parse().map_err(|_| "invalid --scale".to_string()))
        .transpose()?
        .unwrap_or(8);
    let invert = opts.options.contains_key("invert");
    let out = opts.options.get("out");

    match format {
        "png" => {
            let bytes = render_png(&encoding, scale)?;
            match out {
                Some(path) => std::fs::write(path, &bytes).map_err(|e| e.to_string())?,
                None => return Err("--out FILE is required for --format png".into()),
            }
            eprintln!("wrote {} bytes to {}", bytes.len(), out.unwrap());
        }
        "svg" => {
            let svg = render_svg(&encoding, scale.max(1));
            write_text(out, &svg)?;
        }
        "unicode" | "text" => {
            let text = render_unicode(&encoding, invert);
            write_text(out, &text)?;
        }
        other => return Err(format!("unknown --format '{other}' (png, svg, unicode)")),
    }
    Ok(())
}

fn write_text(out: Option<&String>, s: &str) -> Result<(), String> {
    match out {
        Some(path) => std::fs::write(path, s).map_err(|e| e.to_string()),
        None => {
            println!("{s}");
            Ok(())
        }
    }
}

/// Build the abstract [`Encoding`] for a symbology name + data string.
fn build_encoding(symbology: &str, data: &str, ec: Option<&str>) -> Result<Encoding, String> {
    use anyd::codes::*;
    let bytes = data.as_bytes();
    let err = |e: anyd::Error| e.to_string();

    let (sym, enc): (Symbol, Encoding) = match symbology {
        "qr" | "qrcode" => {
            let e = qr::QrEncoder::new();
            let s = e.build_text(data, qr_ec(ec)?).map_err(err)?;
            let g = e.encode(&s).map_err(err)?;
            (s, g)
        }
        "microqr" => {
            let e = microqr::MicroQrEncoder::new();
            let lvl = micro_ec(ec)?;
            let s = e
                .build(vec![Segment::byte(bytes.to_vec())], lvl)
                .map_err(err)?;
            let g = e.encode(&s).map_err(err)?;
            (s, g)
        }
        "rmqr" => {
            let e = rmqr::RmqrEncoder::new();
            let s = e.build_text(data, rmqr_ec(ec)?).map_err(err)?;
            let g = e.encode(&s).map_err(err)?;
            (s, g)
        }
        "datamatrix" | "dm" => {
            encode_via(datamatrix::DataMatrixEncoder::new(), |e| e.build_text(data))?
        }
        "aztec" => encode_via(aztec::AztecEncoder::new(), |e| e.build_text(data))?,
        "maxicode" => encode_via(maxicode::MaxiCodeEncoder::new(), |e| e.build_text(data))?,
        "pdf417" => {
            let e = pdf417::Pdf417Encoder::new();
            let s = e.build_text(data, pdf417_ec(ec)?).map_err(err)?;
            let g = e.encode(&s).map_err(err)?;
            (s, g)
        }
        "code128" => encode_via(code128::Code128Encoder::new(), |e| e.build_text(data))?,
        "code39" => encode_via(code39::Code39Encoder::new(), |e| {
            e.build(bytes, true, false)
        })?,
        "code93" => encode_via(code93::Code93Encoder::new(), |e| e.build(bytes, true))?,
        "code11" => encode_via(code11::Code11Encoder::new(), |e| e.build(bytes, 1))?,
        "ean13" => encode_via(ean::EanEncoder::new(), |e| e.build_ean13(data))?,
        "ean8" => encode_via(ean::EanEncoder::new(), |e| e.build_ean8(data))?,
        "upca" => encode_via(ean::EanEncoder::new(), |e| e.build_upca(data))?,
        "upce" => encode_via(ean::EanEncoder::new(), |e| e.build_upce(data))?,
        "itf" => encode_via(itf::ItfEncoder::new(), |e| e.build(bytes, false))?,
        "std2of5" | "2of5" => encode_via(twoof5::TwoOf5Encoder::new(), |e| {
            e.build(Symbology::Std2of5, bytes)
        })?,
        "codabar" => encode_via(codabar::CodabarEncoder::new(), |e| {
            e.build(b'A', bytes, b'A')
        })?,
        "telepen" => encode_via(telepen::TelepenEncoder::new(), |e| e.build(bytes, true))?,
        "pharmacode" => {
            let value: u32 = data
                .parse()
                .map_err(|_| "pharmacode data must be an integer".to_string())?;
            encode_via(pharmacode::PharmacodeEncoder::new(), |e| e.build(value))?
        }
        other => return Err(format!("unsupported symbology '{other}' (see: anyd list)")),
    };
    let _ = sym;
    Ok(enc)
}

/// Helper: build a symbol with `f` then encode it, mapping errors to strings.
fn encode_via<E, F>(encoder: E, f: F) -> Result<(Symbol, Encoding), String>
where
    E: Encode,
    F: FnOnce(&E) -> anyd::Result<Symbol>,
{
    let symbol = f(&encoder).map_err(|e| e.to_string())?;
    let encoding = encoder.encode(&symbol).map_err(|e| e.to_string())?;
    Ok((symbol, encoding))
}

fn qr_ec(ec: Option<&str>) -> Result<anyd::codes::qr::EcLevel, String> {
    use anyd::codes::qr::EcLevel::*;
    Ok(match ec.map(str::to_lowercase).as_deref() {
        None => M,
        Some("l") => L,
        Some("m") => M,
        Some("q") => Q,
        Some("h") => H,
        Some(o) => return Err(format!("QR --ec must be L/M/Q/H, got '{o}'")),
    })
}

fn micro_ec(ec: Option<&str>) -> Result<anyd::codes::microqr::MicroEcLevel, String> {
    use anyd::codes::microqr::MicroEcLevel::*;
    Ok(match ec.map(str::to_lowercase).as_deref() {
        None => M,
        Some("detection" | "d") => Detection,
        Some("l") => L,
        Some("m") => M,
        Some("q") => Q,
        Some(o) => return Err(format!("Micro QR --ec must be Detection/L/M/Q, got '{o}'")),
    })
}

fn rmqr_ec(ec: Option<&str>) -> Result<anyd::codes::rmqr::RmqrEcLevel, String> {
    use anyd::codes::rmqr::RmqrEcLevel::*;
    Ok(match ec.map(str::to_lowercase).as_deref() {
        None => M,
        Some("m") => M,
        Some("h") => H,
        Some(o) => return Err(format!("rMQR --ec must be M/H, got '{o}'")),
    })
}

fn pdf417_ec(ec: Option<&str>) -> Result<anyd::codes::pdf417::EcLevel, String> {
    let level: u8 = match ec {
        None => 2,
        Some(s) => s
            .parse()
            .map_err(|_| "PDF417 --ec must be 0..=8".to_string())?,
    };
    anyd::codes::pdf417::EcLevel::new(level).ok_or_else(|| "PDF417 --ec must be 0..=8".into())
}

// ---------------------------------------------------------------------------
// renderers
// ---------------------------------------------------------------------------

fn render_png(encoding: &Encoding, scale: usize) -> Result<Vec<u8>, String> {
    let img = render(encoding, scale.max(1));
    let png = oxideav_png::PngImage {
        width: img.width() as u32,
        height: img.height() as u32,
        pixel_format: oxideav_png::PngPixelFormat::Gray8,
        stride: img.width(),
        data: img.pixels().to_vec(),
        palette: Vec::new(),
    };
    oxideav_png::encode_png_image(&png).map_err(|e| e.to_string())
}

/// Render to the terminal using half-block characters (2 module rows per line).
fn render_unicode(encoding: &Encoding, invert: bool) -> String {
    let margin = 2usize;
    type DarkFn = Box<dyn Fn(usize, usize) -> bool>;
    let (w, h, dark): (usize, usize, DarkFn) = match encoding {
        Encoding::Matrix(m) => {
            let (mw, mh) = (m.width(), m.height());
            let m = m.clone();
            (mw, mh, Box::new(move |x, y| m.get(x, y)))
        }
        Encoding::Linear(p) => {
            let modules = p.modules.clone();
            let bar_h = 3usize; // a few rows tall for on-screen legibility
            (
                modules.len(),
                bar_h,
                Box::new(move |x, _y| modules.get(x).copied().unwrap_or(false)),
            )
        }
    };
    let total_w = w + 2 * margin;
    let total_h = h + 2 * margin;
    // `on` = a module we want to render bright/filled. Codes are dark-on-light;
    // by default we print dark modules as filled blocks (scans on a light terminal).
    let cell = |x: usize, y: usize| -> bool {
        if x < margin || y < margin || x >= margin + w || y >= margin + h {
            invert // quiet zone: light (filled only when inverted)
        } else {
            dark(x - margin, y - margin) ^ invert
        }
    };
    let mut out = String::new();
    let mut y = 0;
    while y < total_h {
        for x in 0..total_w {
            let top = cell(x, y);
            let bot = y + 1 < total_h && cell(x, y + 1);
            out.push(match (top, bot) {
                (true, true) => '\u{2588}',  // █
                (true, false) => '\u{2580}', // ▀
                (false, true) => '\u{2584}', // ▄
                (false, false) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    out
}

fn render_svg(encoding: &Encoding, scale: usize) -> String {
    let mut rects = String::new();
    let (w_mod, h_mod, qz) = match encoding {
        Encoding::Matrix(m) => (m.width(), m.height(), m.quiet_zone),
        Encoding::Linear(p) => (p.modules.len(), 24, p.quiet_zone),
    };
    let width = (w_mod + 2 * qz) * scale;
    let height = (h_mod + 2 * qz) * scale;
    let mut push_rect = |x: usize, y: usize, w: usize, h: usize| {
        rects.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
            x * scale,
            y * scale,
            w * scale,
            h * scale
        ));
    };
    match encoding {
        Encoding::Matrix(m) => {
            for y in 0..m.height() {
                // Merge horizontal runs of dark modules into one rect.
                let mut x = 0;
                while x < m.width() {
                    if m.get(x, y) {
                        let start = x;
                        while x < m.width() && m.get(x, y) {
                            x += 1;
                        }
                        push_rect(start + qz, y + qz, x - start, 1);
                    } else {
                        x += 1;
                    }
                }
            }
        }
        Encoding::Linear(p) => {
            let mut x = 0;
            while x < p.modules.len() {
                if p.modules[x] {
                    let start = x;
                    while x < p.modules.len() && p.modules[x] {
                        x += 1;
                    }
                    push_rect(start + qz, qz, x - start, h_mod);
                } else {
                    x += 1;
                }
            }
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\" shape-rendering=\"crispEdges\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\
         <g fill=\"#000000\">{rects}</g></svg>\n"
    )
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

fn cmd_decode(args: &[String]) -> Result<(), String> {
    let opts = parse_opts(args);
    let path = opts
        .positional
        .first()
        .ok_or("usage: anyd decode <image.png>")?;
    let bytes = std::fs::read(path).map_err(|e| format!("reading {path}: {e}"))?;

    let rgba = oxideav_png::decode_png_to_rgba(&bytes).map_err(|e| format!("decoding PNG: {e}"))?;
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    // ITU-R BT.601 luma from RGBA.
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = GrayFrame::new(&luma, w, h).map_err(|e| e.to_string())?;

    let mut found: Vec<(String, Symbol)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut record = |sym: Symbol| {
        let key = (sym.symbology, sym.text().unwrap_or_default());
        if seen.insert(key) {
            found.push((sym.symbology.to_string(), sym));
        }
    };

    // 2D samplers.
    if let Ok(s) = anyd::codes::qr::scan(&frame) {
        record(s);
    }
    if let Ok(s) = anyd::codes::datamatrix::scan(&frame) {
        record(s);
    }
    if let Some(s) = anyd::codes::pdf417::scan(&frame) {
        record(s);
    }

    // 1D front-end: scan lines, then try the checksummed linear decoders.
    let candidates = anyd::scan1d::scan_lines(&frame, &anyd::scan1d::ScanOptions::default());
    let linear: Vec<Box<dyn Decode>> = vec![
        Box::new(anyd::codes::code128::Code128Decoder::new()),
        Box::new(anyd::codes::ean::EanDecoder::new()),
        Box::new(anyd::codes::code93::Code93Decoder::new()),
        Box::new(anyd::codes::code39::Code39Decoder::new()),
        Box::new(anyd::codes::itf::ItfDecoder::new()),
        Box::new(anyd::codes::codabar::CodabarDecoder::new()),
    ];
    for cand in &candidates {
        for dec in &linear {
            if let Some(s) = anyd::scan1d::try_decode(cand, dec.as_ref()) {
                record(s);
            }
        }
    }

    if found.is_empty() {
        return Err("no barcode found in image".into());
    }
    for (name, sym) in &found {
        match sym.text() {
            Some(t) => println!("{name}: {t}"),
            None => println!("{name}: <binary> {}", hex(&sym.payload_bytes())),
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// help / list
// ---------------------------------------------------------------------------

fn print_help() {
    println!(
        "anyd — AnyDCode barcode encoder/decoder\n\n\
         USAGE:\n\
         \x20 anyd encode <symbology> <data> [--format png|unicode|svg] [--out FILE] [--scale N] [--ec L|M|Q|H|0-8] [--invert]\n\
         \x20 anyd decode <image.png>\n\
         \x20 anyd list\n\n\
         EXAMPLES:\n\
         \x20 anyd encode qr \"HELLO\" --format unicode\n\
         \x20 anyd encode qr \"https://example.com\" --format png --out qr.png --scale 8\n\
         \x20 anyd encode ean13 5901234123457 --format svg --out barcode.svg\n\
         \x20 anyd decode qr.png"
    );
}

fn print_symbologies() {
    println!("Encodable symbology names for `anyd encode`:");
    for name in [
        "qr",
        "microqr",
        "rmqr",
        "datamatrix",
        "aztec",
        "maxicode",
        "pdf417",
        "code128",
        "code39",
        "code93",
        "code11",
        "ean13",
        "ean8",
        "upca",
        "upce",
        "itf",
        "std2of5",
        "codabar",
        "telepen",
        "pharmacode",
    ] {
        println!("  {name}");
    }
    println!(
        "\nDecodable from PNG: QR, Data Matrix, PDF417, and 1D (Code 128, EAN/UPC, Code 39/93, ITF, Codabar)."
    );
}
