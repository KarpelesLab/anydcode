//! `anyd` — command-line encoder/decoder for AnyDCode barcodes.
//!
//! Built only when the `cli` feature is enabled (it pulls in `oxideav-png` for
//! PNG I/O; the library itself stays dependency-free). Build with:
//! `cargo build --features cli`.
//!
//! Usage:
//!   anyd encode <symbology> <data> [--format png|unicode|svg] [--out FILE]
//!                                  [--scale N] [--ec L|M|Q|H|0-8] [--invert]
//!   anyd decode <image.png>
//!   anyd list
//!   anyd help

use std::collections::BTreeMap;
use std::process::ExitCode;

use anyd::GrayFrame;
use anyd::Symbol;
use anyd::output::Encoding;
use anyd::render::render;
use anyd::symbology::Symbology;
use anyd::traits::Encode;

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

/// `value_opts` names the options that consume a value, `flag_opts` the bare flags;
/// anything else is rejected so a typo cannot be silently ignored. A lone `--` ends
/// option parsing (the rest is positional, e.g. data that itself starts with `--`).
fn parse_opts(args: &[String], value_opts: &[&str], flag_opts: &[&str]) -> Result<Opts, String> {
    let mut positional = Vec::new();
    let mut options = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        let Some(rest) = a.strip_prefix("--") else {
            positional.push(a.clone());
            continue;
        };
        if rest.is_empty() {
            positional.extend_from_slice(&args[i..]);
            break;
        }
        let (key, inline) = match rest.split_once('=') {
            Some((k, v)) => (k, Some(v)),
            None => (rest, None),
        };
        if flag_opts.contains(&key) {
            if inline.is_some() {
                return Err(format!("option --{key} does not take a value"));
            }
            options.insert(key.to_string(), "true".to_string());
        } else if value_opts.contains(&key) {
            let value = match inline {
                Some(v) => v.to_string(),
                None => {
                    let v = args
                        .get(i)
                        .ok_or_else(|| format!("option --{key} needs a value"))?;
                    i += 1;
                    v.clone()
                }
            };
            options.insert(key.to_string(), value);
        } else {
            return Err(format!("unknown option '--{key}'"));
        }
    }
    Ok(Opts {
        positional,
        options,
    })
}

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

fn cmd_encode(args: &[String]) -> Result<(), String> {
    let opts = parse_opts(args, &["format", "out", "scale", "ec"], &["invert"])?;
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
                Some(path) => {
                    std::fs::write(path, &bytes).map_err(|e| format!("writing {path}: {e}"))?
                }
                None => return Err("--out FILE is required for --format png".into()),
            }
            eprintln!("wrote {} bytes to {}", bytes.len(), out.unwrap());
        }
        "svg" => {
            let svg = render_svg(&encoding, scale.max(1))?;
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
        Some(path) => std::fs::write(path, s).map_err(|e| format!("writing {path}: {e}")),
        None => print_out(s),
    }
}

/// Write `s` to stdout. A reader that closes the pipe early (`anyd ... | head`) ends
/// the output quietly, where `println!` would panic.
fn print_out(s: &str) -> Result<(), String> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(s.as_bytes()).and_then(|()| stdout.flush()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(format!("writing to stdout: {e}"))
        }
        _ => Ok(()),
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
            let s = e.build_text(data, lvl).map_err(err)?;
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

/// Bar height, in modules, of a rendered linear symbol (as `anyd::render` draws it).
const LINEAR_HEIGHT_MODULES: usize = 24;

/// Upper bound on the pixels of a rendered PNG (a 16384 x 16384 canvas): far beyond
/// any printable symbol, yet small enough that a mistyped `--scale` reports an error
/// instead of exhausting memory.
const MAX_PNG_PIXELS: usize = 1 << 28;

/// Output size in pixels (quiet zone included) at `scale` pixels per module, or an
/// error when the product overflows.
fn scaled_size(encoding: &Encoding, scale: usize) -> Result<(usize, usize), String> {
    let (w_mod, h_mod, qz) = match encoding {
        Encoding::Matrix(m) => (m.width(), m.height(), m.quiet_zone),
        Encoding::Linear(p) => (p.modules.len(), LINEAR_HEIGHT_MODULES, p.quiet_zone),
    };
    let side = |modules: usize| (modules + 2 * qz).checked_mul(scale);
    match (side(w_mod), side(h_mod)) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err(format!("--scale {scale} is too large")),
    }
}

fn render_png(encoding: &Encoding, scale: usize) -> Result<Vec<u8>, String> {
    let scale = scale.max(1);
    let (w, h) = scaled_size(encoding, scale)?;
    if w.checked_mul(h).is_none_or(|n| n > MAX_PNG_PIXELS) || w.max(h) > u32::MAX as usize {
        return Err(format!(
            "--scale {scale} gives a {w}x{h} image (limit: {MAX_PNG_PIXELS} pixels)"
        ));
    }
    let img = render(encoding, scale);
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

fn render_svg(encoding: &Encoding, scale: usize) -> Result<String, String> {
    let mut rects = String::new();
    let (h_mod, qz) = match encoding {
        Encoding::Matrix(m) => (m.height(), m.quiet_zone),
        Encoding::Linear(p) => (LINEAR_HEIGHT_MODULES, p.quiet_zone),
    };
    // Every rect coordinate below is bounded by these, so they cannot overflow either.
    let (width, height) = scaled_size(encoding, scale)?;
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
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\" shape-rendering=\"crispEdges\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\
         <g fill=\"#000000\">{rects}</g></svg>\n"
    ))
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

fn cmd_decode(args: &[String]) -> Result<(), String> {
    let opts = parse_opts(args, &[], &[])?;
    let path = opts
        .positional
        .first()
        .ok_or("usage: anyd decode <image.png>")?;
    let bytes = std::fs::read(path).map_err(|e| format!("reading {path}: {e}"))?;
    let found = decode_png_bytes(&bytes)?;

    if found.is_empty() {
        return Err("no barcode found in image".into());
    }
    let mut report = String::new();
    for sym in &found {
        let name = sym.symbology.to_string();
        match sym.text() {
            Some(t) => report.push_str(&format!("{name}: {t}\n")),
            None => report.push_str(&format!("{name}: <binary> {}\n", hex(&sym.payload_bytes()))),
        }
    }
    print_out(&report)
}

/// Upper bound on the pixels of a PNG accepted for decoding (as [`MAX_PNG_PIXELS`]).
/// Checked against the header up front, so a file that merely *declares* gigantic
/// dimensions is refused before the PNG decoder sizes any buffer from them.
const MAX_DECODE_PIXELS: u64 = 1 << 28;

/// The `(width, height)` an IHDR-first PNG declares, if `bytes` has that shape.
fn png_declared_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((w, h))
}

/// Decode every symbol found in a PNG file's bytes.
fn decode_png_bytes(bytes: &[u8]) -> Result<Vec<Symbol>, String> {
    if let Some((w, h)) = png_declared_size(bytes)
        && u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS
    {
        return Err(format!(
            "PNG is {w}x{h}, over the {MAX_DECODE_PIXELS}-pixel limit"
        ));
    }
    let rgba = oxideav_png::decode_png_to_rgba(bytes).map_err(|e| format!("decoding PNG: {e}"))?;
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma = rgba_to_luma(&rgba.data);
    let frame = GrayFrame::new(&luma, w, h).map_err(|e| e.to_string())?;

    // One shared decode entry point (see `anyd::pipeline::scan_all`) drives every
    // front-end, so the CLI and the WebAssembly demo can never disagree on what decodes.
    Ok(anyd::pipeline::scan_all(&frame))
}

/// ITU-R BT.601 luma from RGBA, composited over a white page: transparent pixels
/// usually carry black RGB, and a code exported on a transparent background must
/// not read as solid black.
fn rgba_to_luma(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let y = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
            let a = p[3] as u32;
            ((y * a + 255 * (255 - a)) / 255) as u8
        })
        .collect()
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
        "\nDecodable from PNG: QR, Micro QR, rMQR, Data Matrix, Aztec, PDF417, MicroPDF417, \
         App Clip Code, and 1D (Code 128, EAN/UPC, Code 39/93, ITF, Codabar)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_does_not_swallow_positional() {
        let opts =
            parse_opts(&args(&["qr", "--invert", "HELLO"]), &["scale"], &["invert"]).unwrap();
        assert_eq!(opts.positional, ["qr", "HELLO"]);
        assert!(opts.options.contains_key("invert"));
    }

    #[test]
    fn double_dash_ends_options() {
        let opts = parse_opts(
            &args(&["code128", "--scale", "3", "--", "--5"]),
            &["scale"],
            &["invert"],
        )
        .unwrap();
        assert_eq!(opts.positional, ["code128", "--5"]);
        assert_eq!(opts.options.get("scale").map(String::as_str), Some("3"));
    }

    #[test]
    fn absurd_scale_is_an_error_not_a_panic() {
        let enc = build_encoding("qr", "HELLO", None).unwrap();
        for scale in [usize::MAX, usize::MAX / 28, 1 << 40, 200_000] {
            assert!(render_png(&enc, scale).is_err(), "png scale {scale}");
        }
        assert!(render_svg(&enc, usize::MAX).is_err());
        assert!(render_svg(&enc, usize::MAX / 28).is_err());
        let bars = build_encoding("code128", "HELLO", None).unwrap();
        assert!(render_png(&bars, usize::MAX / 2).is_err());
        assert!(render_svg(&bars, usize::MAX / 2).is_err());
        // Sane scales still render (0 is clamped to 1).
        assert!(render_png(&enc, 0).is_ok());
        assert!(render_png(&bars, 3).is_ok());
        assert!(render_svg(&enc, 8).unwrap().contains("<svg"));
    }

    #[test]
    fn transparent_pixels_read_as_light_background() {
        assert_eq!(rgba_to_luma(&[0, 0, 0, 0]), [255]);
        assert_eq!(rgba_to_luma(&[0, 0, 0, 255]), [0]);
        assert_eq!(rgba_to_luma(&[255, 255, 255, 255]), [255]);
        assert_eq!(rgba_to_luma(&[0, 0, 0, 128]), [127]);

        // A code exported with a transparent background: black modules over
        // fully transparent (0,0,0,0) pixels.
        let enc = build_encoding("qr", "TRANSPARENT", None).unwrap();
        let img = render(&enc, 6);
        let data: Vec<u8> = img
            .pixels()
            .iter()
            .flat_map(|&p| [0, 0, 0, if p < 128 { 255 } else { 0 }])
            .collect();
        let png = oxideav_png::PngImage {
            width: img.width() as u32,
            height: img.height() as u32,
            pixel_format: oxideav_png::PngPixelFormat::Rgba,
            stride: img.width() * 4,
            data,
            palette: Vec::new(),
        };
        let bytes = oxideav_png::encode_png_image(&png).unwrap();
        let found = decode_png_bytes(&bytes).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text().as_deref(), Some("TRANSPARENT"));
    }

    /// A header declaring absurd dimensions is an error, never a panic or a huge
    /// allocation, whatever the (tiny) pixel data says.
    #[test]
    fn png_with_gigantic_declared_size_is_rejected() {
        // 0x7fffffff x 0x7fffffff, 16-bit RGBA, with a valid 10-byte IDAT.
        let png: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\x7f\xff\xff\xff\x7f\xff\xff\xff\x10\x06\0\0\0\x44\x59\xd7\x25\
            \0\0\0\x0bIDAT\x78\x9c\x63\x60\x80\x01\0\0\x0a\0\x01\x7f\x80\x74\x5e\
            \0\0\0\0IEND\xae\x42\x60\x82";
        assert!(decode_png_bytes(png).is_err());
        assert!(decode_png_bytes(b"").is_err());
        assert!(decode_png_bytes(b"\x89PNG\r\n\x1a\n").is_err());
    }

    #[test]
    fn unknown_and_malformed_options_are_errors() {
        let parse = |list: &[&str]| parse_opts(&args(list), &["scale"], &["invert"]);
        assert!(parse(&["qr", "x", "--sacle", "3"]).is_err());
        assert!(parse(&["qr", "x", "--scale"]).is_err());
        assert!(parse(&["qr", "x", "--invert=1"]).is_err());
        let opts = parse(&["qr", "x", "--scale=4"]).unwrap();
        assert_eq!(opts.options.get("scale").map(String::as_str), Some("4"));
    }
}
