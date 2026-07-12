//! Raw `wasm32` FFI shim for the browser demo (no `wasm-bindgen`).
//!
//! Only compiled under the `wasm` feature. JS drives it like this:
//! - `alloc(len) -> ptr`, write input into wasm memory at `ptr`, call an entry point,
//!   read the returned buffer, then `dealloc(ptr, len)` both sides.
//! - Entry points return a packed `u64` = `(ptr << 32) | len` describing an owned
//!   output buffer in wasm memory (0 on failure). JS reads `len` bytes at `ptr`,
//!   parses them, then frees with `dealloc(ptr, len)`.
//!
//! Output formats:
//! - `encode`: `[kind:u8][w:u32 LE][h:u32 LE][quiet:u32 LE][bits: w*h]` where kind is
//!   0=matrix, 1=linear (each bit byte is 1 for a dark module); OR on failure
//!   `[2][utf8 error message]` so the UI can explain what was rejected.
//! - `decode` / `locate`: a UTF-8 JSON string.

#![allow(clippy::missing_safety_doc)]

use crate::GrayFrame;
use crate::output::Encoding;
use crate::segment::Segment;
use crate::traits::{Decode, Encode};

/// Allocate `len` bytes in wasm memory and return the pointer (caller fills it).
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let boxed = vec![0u8; len].into_boxed_slice();
    Box::into_raw(boxed) as *mut u8
}

/// Free a buffer previously produced by `alloc` or returned by an entry point.
///
/// # Safety
/// `ptr`/`len` must exactly describe a live allocation from this module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let slice = core::ptr::slice_from_raw_parts_mut(ptr, len);
    unsafe { drop(Box::from_raw(slice)) };
}

/// Pack an owned output buffer into `(ptr << 32) | len` and leak it for JS to read.
fn export(buf: Vec<u8>) -> u64 {
    let boxed = buf.into_boxed_slice();
    let len = boxed.len() as u64;
    let ptr = Box::into_raw(boxed) as *const u8 as u64;
    (ptr << 32) | len
}

unsafe fn input(ptr: *const u8, len: usize) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(ptr, len) }
}

/// Encode `data` as `symbology` (all UTF-8) with `opts` (a `key=value;…` string, e.g.
/// `ec=H`). Returns the module grid, or a `[2][message]` error buffer.
///
/// # Safety
/// Pointers must describe valid buffers of the given lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn encode(
    sym_ptr: *const u8,
    sym_len: usize,
    data_ptr: *const u8,
    data_len: usize,
    opt_ptr: *const u8,
    opt_len: usize,
) -> u64 {
    let sym = core::str::from_utf8(unsafe { input(sym_ptr, sym_len) }).unwrap_or("");
    let data = core::str::from_utf8(unsafe { input(data_ptr, data_len) }).unwrap_or("");
    let opts = core::str::from_utf8(unsafe { input(opt_ptr, opt_len) }).unwrap_or("");
    match build(sym, data, opts) {
        Ok(enc) => export(serialize_encoding(&enc)),
        Err(msg) => {
            let mut buf = vec![2u8];
            buf.extend_from_slice(msg.as_bytes());
            export(buf)
        }
    }
}

/// Read `key`'s value out of a `key=value;key=value` options string.
fn opt<'a>(opts: &'a str, key: &str) -> Option<&'a str> {
    opts.split([';', '&'])
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
}

/// Decode all codes in a `w`×`h` 8-bit luminance frame; returns a JSON array
/// `[{"symbology":..,"text":..}]` (empty array if none).
///
/// # Safety
/// `luma_ptr` must point to `w*h` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn decode(w: usize, h: usize, luma_ptr: *const u8) -> u64 {
    let luma = unsafe { input(luma_ptr, w * h) };
    let frame = match GrayFrame::new(luma, w, h) {
        Ok(f) => f,
        Err(_) => return export(b"[]".to_vec()),
    };

    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |sym: crate::Symbol| {
        let name = sym.symbology.to_string();
        let text = sym.text().unwrap_or_default();
        if !out.iter().any(|(n, t)| *n == name && *t == text) {
            out.push((name, text));
        }
    };

    if let Ok(s) = crate::codes::qr::scan(&frame) {
        push(s);
    }
    if let Ok(s) = crate::codes::datamatrix::scan(&frame) {
        push(s);
    }
    if let Some(s) = crate::codes::pdf417::scan(&frame) {
        push(s);
    }
    let candidates = crate::scan1d::scan_lines(&frame, &crate::scan1d::ScanOptions::default());
    let linear: [Box<dyn Decode>; 6] = [
        Box::new(crate::codes::code128::Code128Decoder::new()),
        Box::new(crate::codes::ean::EanDecoder::new()),
        Box::new(crate::codes::code93::Code93Decoder::new()),
        Box::new(crate::codes::code39::Code39Decoder::new()),
        Box::new(crate::codes::itf::ItfDecoder::new()),
        Box::new(crate::codes::codabar::CodabarDecoder::new()),
    ];
    for cand in &candidates {
        for dec in &linear {
            if let Some(s) = crate::scan1d::try_decode(cand, dec.as_ref()) {
                push(s);
            }
        }
    }

    let mut json = String::from("[");
    for (i, (name, text)) in out.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"symbology\":");
        json_str(&mut json, name);
        json.push_str(",\"text\":");
        json_str(&mut json, text);
        json.push('}');
    }
    json.push(']');
    export(json.into_bytes())
}

/// Locate (position-only, no decode) every candidate code in a `w`×`h` luminance
/// frame; returns JSON `[{"family":..,"corners":[[x,y],..]}]`.
///
/// # Safety
/// `luma_ptr` must point to `w*h` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn locate(w: usize, h: usize, luma_ptr: *const u8) -> u64 {
    let luma = unsafe { input(luma_ptr, w * h) };
    let frame = match GrayFrame::new(luma, w, h) {
        Ok(f) => f,
        Err(_) => return export(b"[]".to_vec()),
    };
    let candidates = crate::detect::locate(&frame, &crate::detect::LocateOptions::default());

    let mut json = String::from("[");
    for (i, c) in candidates.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let family = match c.symbology.map(|s| s.dimension()) {
            Some(crate::Dimension::Matrix) => "matrix",
            Some(crate::Dimension::Stacked) => "stacked",
            Some(crate::Dimension::Postal) => "postal",
            _ => "linear",
        };
        json.push_str("{\"family\":\"");
        json.push_str(family);
        json.push_str("\",\"corners\":[");
        for (j, p) in c.location.outline.corners.iter().enumerate() {
            if j > 0 {
                json.push(',');
            }
            json.push('[');
            push_num(&mut json, p.x);
            json.push(',');
            push_num(&mut json, p.y);
            json.push(']');
        }
        json.push_str("]}");
    }
    json.push(']');
    export(json.into_bytes())
}

fn push_num(s: &mut String, v: f32) {
    // Integer-ish pixel coordinates; one decimal is plenty for the overlay.
    let r = (v * 10.0).round() / 10.0;
    s.push_str(&r.to_string());
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn serialize_encoding(enc: &Encoding) -> Vec<u8> {
    let mut buf = Vec::new();
    match enc {
        Encoding::Matrix(m) => {
            buf.push(0);
            buf.extend_from_slice(&(m.width() as u32).to_le_bytes());
            buf.extend_from_slice(&(m.height() as u32).to_le_bytes());
            buf.extend_from_slice(&(m.quiet_zone as u32).to_le_bytes());
            for y in 0..m.height() {
                for x in 0..m.width() {
                    buf.push(m.get(x, y) as u8);
                }
            }
        }
        Encoding::Linear(p) => {
            buf.push(1);
            buf.extend_from_slice(&(p.modules.len() as u32).to_le_bytes());
            buf.extend_from_slice(&1u32.to_le_bytes());
            buf.extend_from_slice(&(p.quiet_zone as u32).to_le_bytes());
            for &b in &p.modules {
                buf.push(b as u8);
            }
        }
    }
    buf
}

/// Read a boolean-ish option (`on`/`1`/`true`/`yes` are true), defaulting to `def`.
fn opt_bool(opts: &str, key: &str, def: bool) -> bool {
    match opt(opts, key) {
        None => def,
        Some(v) => matches!(v, "on" | "1" | "true" | "yes"),
    }
}

/// Whether an option value means "auto" (absent, empty, or the literal `auto`).
fn is_auto(v: Option<&str>) -> bool {
    matches!(v, None | Some("") | Some("auto"))
}

/// Build an [`Encoding`] for a symbology name + UTF-8 data. Covers every implemented
/// symbology with its meaningful options; unknown names return an error.
fn build(sym: &str, data: &str, opts: &str) -> Result<Encoding, String> {
    use crate::codes::*;
    let b = data.as_bytes();
    let ec = opt(opts, "ec");
    let es = |e: crate::Error| e.to_string();
    match sym {
        // ---------------- 2D: matrix ----------------
        "qr" => {
            let e = qr::QrEncoder::new();
            e.encode(&e.build_text(data, qr_ec(ec)?).map_err(es)?)
                .map_err(es)
        }
        "microqr" => {
            let e = microqr::MicroQrEncoder::new();
            let s = e
                .build(vec![Segment::byte(b.to_vec())], micro_ec(ec)?)
                .map_err(es)?;
            e.encode(&s).map_err(es)
        }
        "rmqr" => {
            let e = rmqr::RmqrEncoder::new();
            let s = e
                .build_text_with(data, rmqr_ec(ec)?, rmqr_size(opt(opts, "size"))?)
                .map_err(es)?;
            e.encode(&s).map_err(es)
        }
        "aztec" => {
            let e = aztec::AztecEncoder::new();
            e.encode(&e.build_text(data).map_err(es)?).map_err(es)
        }
        "aztecrunes" => {
            let v: u8 = data
                .trim()
                .parse()
                .map_err(|_| "Aztec Rune value must be an integer 0–255".to_string())?;
            let e = aztec::AztecEncoder::new();
            e.encode(&e.build_rune(v)).map_err(es)
        }
        "datamatrix" => {
            let e = datamatrix::DataMatrixEncoder::new();
            let s = match opt(opts, "size") {
                o if is_auto(o) => e.build_text(data).map_err(es)?,
                Some(v) => {
                    let n: usize = v
                        .parse()
                        .map_err(|_| "Data Matrix size must be a number or 'auto'".to_string())?;
                    e.build_sized(b, n).map_err(es)?
                }
                None => unreachable!(),
            };
            e.encode(&s).map_err(es)
        }
        "maxicode" => {
            let e = maxicode::MaxiCodeEncoder::new();
            let mode: u8 = match opt(opts, "mode") {
                o if is_auto(o) => 4,
                Some(v) => v
                    .parse()
                    .map_err(|_| "MaxiCode mode must be 2–6".to_string())?,
                None => unreachable!(),
            };
            let s = if (4..=6).contains(&mode) {
                e.build_mode(b, mode).map_err(es)?
            } else if mode == 2 || mode == 3 {
                let parts: Vec<&str> = data.splitn(4, '|').collect();
                if parts.len() != 4 {
                    return Err(
                        "Structured MaxiCode (mode 2/3) needs 'postcode|country|service|message'"
                            .to_string(),
                    );
                }
                let country: u16 = parts[1]
                    .trim()
                    .parse()
                    .map_err(|_| "MaxiCode country code must be 0–999".to_string())?;
                let service: u16 = parts[2]
                    .trim()
                    .parse()
                    .map_err(|_| "MaxiCode service class must be 0–999".to_string())?;
                e.build_structured(mode, parts[0].trim(), country, service, parts[3].as_bytes())
                    .map_err(es)?
            } else {
                return Err("MaxiCode mode must be 2–6".to_string());
            };
            e.encode(&s).map_err(es)
        }
        "hanxin" => {
            let e = hanxin::HanXinEncoder::new();
            e.encode(&e.build_text(data, hanxin_ec(ec)?).map_err(es)?)
                .map_err(es)
        }
        "dotcode" => {
            let e = dotcode::DotCodeEncoder::new();
            let s = match opt(opts, "mode") {
                Some("gs1") => e.build_gs1_reduced(b).map_err(es)?,
                _ => e.build_bytes(b).map_err(es)?,
            };
            e.encode(&s).map_err(es)
        }
        "gridmatrix" => {
            let e = gridmatrix::GridMatrixEncoder::new();
            let ver = opt(opts, "version");
            let s = match ver {
                o if is_auto(o) => {
                    if is_auto(ec) {
                        e.build_text(data).map_err(es)?
                    } else {
                        e.build_with_ec(b, gm_ec(ec)?).map_err(es)?
                    }
                }
                Some(v) => {
                    let n: u8 = v
                        .parse()
                        .map_err(|_| "Grid Matrix version must be 1–13".to_string())?;
                    let version = gridmatrix::Version::new(n)
                        .ok_or_else(|| "Grid Matrix version must be 1–13".to_string())?;
                    let level = if is_auto(ec) {
                        gridmatrix::EcLevel::L3
                    } else {
                        gm_ec(ec)?
                    };
                    e.build_sized(b, version, level).map_err(es)?
                }
                None => unreachable!(),
            };
            e.encode(&s).map_err(es)
        }

        // ---------------- 2D: stacked ----------------
        "pdf417" => {
            let e = pdf417::Pdf417Encoder::new();
            let level = pdf417_ec(ec)?;
            let s = match opt(opts, "cols") {
                o if is_auto(o) => e.build_text(data, level).map_err(es)?,
                Some(v) => {
                    let n: usize = v
                        .parse()
                        .map_err(|_| "PDF417 columns must be 1–30".to_string())?;
                    e.build_sized(vec![Segment::alphanumeric(b.to_vec())], level, Some(n))
                        .map_err(es)?
                }
                None => unreachable!(),
            };
            e.encode(&s).map_err(es)
        }
        "micropdf417" => {
            let e = pdf417::MicroPdf417Encoder::new();
            let s = match opt(opts, "cols") {
                o if is_auto(o) => e.build_text(data).map_err(es)?,
                Some(v) => {
                    let n: usize = v
                        .parse()
                        .map_err(|_| "MicroPDF417 columns must be 1–4".to_string())?;
                    e.build_sized(vec![Segment::alphanumeric(b.to_vec())], Some(n))
                        .map_err(es)?
                }
                None => unreachable!(),
            };
            e.encode(&s).map_err(es)
        }
        "code16k" => {
            let e = code16k::Code16kEncoder::new();
            let s = match opt(opts, "rows") {
                o if is_auto(o) => e.build(b).map_err(es)?,
                Some(v) => {
                    let n: usize = v
                        .parse()
                        .map_err(|_| "Code 16K rows must be 2–16".to_string())?;
                    e.build_rows(b, Some(n)).map_err(es)?
                }
                None => unreachable!(),
            };
            e.encode(&s).map_err(es)
        }
        "code49" => {
            let e = code49::Code49Encoder::new();
            let s = match opt(opts, "rows") {
                o if is_auto(o) => e.build(b).map_err(es)?,
                Some(v) => {
                    let n: usize = v
                        .parse()
                        .map_err(|_| "Code 49 rows must be 2–8".to_string())?;
                    e.build_rows(b, Some(n)).map_err(es)?
                }
                None => unreachable!(),
            };
            e.encode(&s).map_err(es)
        }
        "codablockf" => {
            let e = codablockf::CodablockFEncoder::new();
            e.encode(&e.build(b).map_err(es)?).map_err(es)
        }

        // ---------------- 1D: EAN/UPC ----------------
        "ean13" => {
            let e = ean::EanEncoder::new();
            e.encode(&e.build_ean13(data).map_err(es)?).map_err(es)
        }
        "ean8" => {
            let e = ean::EanEncoder::new();
            e.encode(&e.build_ean8(data).map_err(es)?).map_err(es)
        }
        "upca" => {
            let e = ean::EanEncoder::new();
            e.encode(&e.build_upca(data).map_err(es)?).map_err(es)
        }
        "upce" => {
            let e = ean::EanEncoder::new();
            e.encode(&e.build_upce(data).map_err(es)?).map_err(es)
        }
        "ean2" => {
            let e = ean::EanEncoder::new();
            e.encode(&e.build_ean2(data).map_err(es)?).map_err(es)
        }
        "ean5" => {
            let e = ean::EanEncoder::new();
            e.encode(&e.build_ean5(data).map_err(es)?).map_err(es)
        }

        // ---------------- 1D: Code 128 / GS1-128 ----------------
        "code128" => {
            let e = code128::Code128Encoder::new();
            e.encode(&e.build_text(data).map_err(es)?).map_err(es)
        }
        "gs1_128" => {
            let e = code128::Code128Encoder::new();
            let input: Vec<code128::Code128Input> = b
                .iter()
                .map(|&c| {
                    if c == 0x1D {
                        code128::Code128Input::Fnc1
                    } else {
                        code128::Code128Input::Data(c)
                    }
                })
                .collect();
            e.encode(&e.build_gs1(&input).map_err(es)?).map_err(es)
        }

        // ---------------- 1D: Code 39 / 93 / 11 ----------------
        "code39" => {
            let e = code39::Code39Encoder::new();
            let full = opt_bool(opts, "fullascii", true);
            let check = opt_bool(opts, "check", false);
            e.encode(&e.build(b, full, check).map_err(es)?).map_err(es)
        }
        "code93" => {
            let e = code93::Code93Encoder::new();
            let full = opt_bool(opts, "fullascii", true);
            e.encode(&e.build(b, full).map_err(es)?).map_err(es)
        }
        "code11" => {
            let e = code11::Code11Encoder::new();
            let count: u8 = match opt(opts, "check") {
                None => 1,
                Some(v) => v
                    .parse()
                    .map_err(|_| "Code 11 check digits must be 0, 1 or 2".to_string())?,
            };
            e.encode(&e.build(b, count).map_err(es)?).map_err(es)
        }

        // ---------------- 1D: 2-of-5 ----------------
        "itf" => {
            let e = itf::ItfEncoder::new();
            e.encode(&e.build(b, opt_bool(opts, "check", false)).map_err(es)?)
                .map_err(es)
        }
        "std2of5" => two_of_5(crate::Symbology::Std2of5, b, es),
        "iata2of5" => two_of_5(crate::Symbology::Iata2of5, b, es),
        "matrix2of5" => two_of_5(crate::Symbology::Matrix2of5, b, es),

        // ---------------- 1D: other linear ----------------
        "codabar" => {
            let e = codabar::CodabarEncoder::new();
            let start = codabar_guard(opt(opts, "start"))?;
            let stop = codabar_guard(opt(opts, "stop"))?;
            e.encode(&e.build(start, b, stop).map_err(es)?).map_err(es)
        }
        "msi" => {
            let e = msi::MsiEncoder::new();
            e.encode(&e.build_msi(b, msi_check(opt(opts, "check"))?).map_err(es)?)
                .map_err(es)
        }
        "plessey" => {
            let e = msi::MsiEncoder::new();
            e.encode(&e.build_plessey(b).map_err(es)?).map_err(es)
        }
        "telepen" => {
            let e = telepen::TelepenEncoder::new();
            e.encode(&e.build(b, opt_bool(opts, "check", true)).map_err(es)?)
                .map_err(es)
        }
        "pharmacode" => {
            let v: u32 = data
                .trim()
                .parse()
                .map_err(|_| "Pharmacode value must be an integer 3–131070".to_string())?;
            let e = pharmacode::PharmacodeEncoder::new();
            e.encode(&e.build(v).map_err(es)?).map_err(es)
        }
        "pharmacode2" => {
            let v: u32 = data.trim().parse().map_err(|_| {
                "Two-track Pharmacode value must be an integer 4–64570080".to_string()
            })?;
            let e = pharmacode::PharmacodeEncoder::new();
            e.encode(&e.build_two_track(v).map_err(es)?).map_err(es)
        }
        "dxfilm" => {
            let e = dxfilm::DxFilmEncoder::new();
            e.encode(&e.build_text(data).map_err(es)?).map_err(es)
        }

        // ---------------- GS1 DataBar ----------------
        "databar_omni" => {
            let e = databar::DataBarEncoder::new();
            e.encode(&e.build_omni(b).map_err(es)?).map_err(es)
        }
        "databar_limited" => {
            let e = databar::DataBarEncoder::new();
            e.encode(&e.build_limited(b).map_err(es)?).map_err(es)
        }
        "databar_stacked" => {
            let e = databar::DataBarEncoder::new();
            e.encode(&e.build_stacked(b).map_err(es)?).map_err(es)
        }
        "databar_stacked_omni" => {
            let e = databar::DataBarEncoder::new();
            e.encode(&e.build_stacked_omni(b).map_err(es)?).map_err(es)
        }
        "databar_expanded" => {
            let e = databar::DataBarEncoder::new();
            e.encode(&e.build_expanded(b).map_err(es)?).map_err(es)
        }
        "databar_expanded_stacked" => {
            let e = databar::DataBarEncoder::new();
            let cols: usize = match opt(opts, "cols") {
                None => 2,
                Some(v) => v
                    .parse()
                    .map_err(|_| "DataBar Expanded Stacked columns must be 1–11".to_string())?,
            };
            e.encode(&e.build_expanded_stacked(b, cols).map_err(es)?)
                .map_err(es)
        }

        // ---------------- Postal ----------------
        "postnet" => {
            let e = postal::PostalEncoder::new();
            e.encode(&e.build_postnet(data).map_err(es)?).map_err(es)
        }
        "planet" => {
            let e = postal::PostalEncoder::new();
            e.encode(&e.build_planet(data).map_err(es)?).map_err(es)
        }
        "imb" => {
            let e = postal::PostalEncoder::new();
            let mut it = data.split_whitespace();
            let tracking = it.next().unwrap_or("");
            let routing = it.next().unwrap_or("");
            e.encode(&e.build_imb(tracking, routing).map_err(es)?)
                .map_err(es)
        }
        "rm4scc" => {
            let e = postal::PostalEncoder::new();
            e.encode(&e.build_rm4scc(data).map_err(es)?).map_err(es)
        }
        "kix" => {
            let e = postal::PostalEncoder::new();
            e.encode(&e.build_kix(data).map_err(es)?).map_err(es)
        }
        "japanpost" => {
            let e = postal::PostalEncoder::new();
            e.encode(&e.build_japanpost(data).map_err(es)?).map_err(es)
        }
        "mailmark" => {
            let e = postal::PostalEncoder::new();
            e.encode(&e.build_mailmark(data).map_err(es)?).map_err(es)
        }
        "auspost" => {
            let e = postal::PostalEncoder::new();
            let (dpid, custinfo) = data.split_once(' ').unwrap_or((data, ""));
            e.encode(&e.build_auspost(dpid, custinfo).map_err(es)?)
                .map_err(es)
        }

        other => Err(format!("unknown symbology '{other}'")),
    }
}

/// Build a 2-of-5 variant symbol (Standard / IATA / Matrix).
fn two_of_5(
    sym: crate::Symbology,
    data: &[u8],
    es: impl Fn(crate::Error) -> String,
) -> Result<Encoding, String> {
    use crate::traits::Encode;
    let e = crate::codes::twoof5::TwoOf5Encoder::new();
    e.encode(&e.build(sym, data).map_err(&es)?).map_err(&es)
}

/// Map a Codabar start/stop option (`A`–`D`, default `A`) to its byte.
fn codabar_guard(v: Option<&str>) -> Result<u8, String> {
    match v {
        None | Some("") | Some("A") => Ok(b'A'),
        Some("B") => Ok(b'B'),
        Some("C") => Ok(b'C'),
        Some("D") => Ok(b'D'),
        Some(o) => Err(format!(
            "Codabar start/stop must be A, B, C or D (got '{o}')"
        )),
    }
}

fn qr_ec(ec: Option<&str>) -> Result<crate::codes::qr::EcLevel, String> {
    use crate::codes::qr::EcLevel::*;
    match ec {
        None | Some("M") => Ok(M),
        Some("L") => Ok(L),
        Some("Q") => Ok(Q),
        Some("H") => Ok(H),
        Some(o) => Err(format!("QR error-correction must be L/M/Q/H (got '{o}')")),
    }
}
fn micro_ec(ec: Option<&str>) -> Result<crate::codes::microqr::MicroEcLevel, String> {
    use crate::codes::microqr::MicroEcLevel::*;
    match ec {
        None | Some("M") => Ok(M),
        Some("Detection" | "D") => Ok(Detection),
        Some("L") => Ok(L),
        Some("Q") => Ok(Q),
        Some(o) => Err(format!(
            "Micro QR error-correction must be Detection/L/M/Q (got '{o}')"
        )),
    }
}
fn rmqr_ec(ec: Option<&str>) -> Result<crate::codes::rmqr::RmqrEcLevel, String> {
    use crate::codes::rmqr::RmqrEcLevel::*;
    match ec {
        None | Some("M") => Ok(M),
        Some("H") => Ok(H),
        Some(o) => Err(format!("rMQR error-correction must be M/H (got '{o}')")),
    }
}
fn pdf417_ec(ec: Option<&str>) -> Result<crate::codes::pdf417::EcLevel, String> {
    let n: u8 = match ec {
        None => 2,
        Some(s) => s
            .parse()
            .map_err(|_| "PDF417 EC level must be 0–8".to_string())?,
    };
    crate::codes::pdf417::EcLevel::new(n).ok_or_else(|| "PDF417 EC level must be 0–8".to_string())
}
fn rmqr_size(size: Option<&str>) -> Result<crate::codes::rmqr::SizeStrategy, String> {
    use crate::codes::rmqr::SizeStrategy::*;
    match size {
        None | Some("") | Some("balanced") => Ok(Balanced),
        Some("min") => Ok(MinHeight),
        Some("max") => Ok(MaxHeight),
        Some(o) => Err(format!("rMQR size must be balanced/min/max (got '{o}')")),
    }
}
fn hanxin_ec(ec: Option<&str>) -> Result<crate::codes::hanxin::EcLevel, String> {
    use crate::codes::hanxin::EcLevel::*;
    match ec {
        None | Some("L1") => Ok(L1),
        Some("L2") => Ok(L2),
        Some("L3") => Ok(L3),
        Some("L4") => Ok(L4),
        Some(o) => Err(format!("Han Xin EC level must be L1–L4 (got '{o}')")),
    }
}
fn gm_ec(ec: Option<&str>) -> Result<crate::codes::gridmatrix::EcLevel, String> {
    use crate::codes::gridmatrix::EcLevel::*;
    match ec {
        Some("L1") => Ok(L1),
        Some("L2") => Ok(L2),
        Some("L3") => Ok(L3),
        Some("L4") => Ok(L4),
        Some("L5") => Ok(L5),
        Some(o) => Err(format!("Grid Matrix EC level must be L1–L5 (got '{o}')")),
        None => Ok(L3),
    }
}
fn msi_check(check: Option<&str>) -> Result<crate::codes::msi::MsiCheck, String> {
    use crate::codes::msi::MsiCheck;
    match check {
        None | Some("none") => Ok(MsiCheck::None),
        Some("mod10") => Ok(MsiCheck::Mod10),
        Some("mod11") => Ok(MsiCheck::Mod11),
        Some("mod1010") => Ok(MsiCheck::Mod1010),
        Some("mod1110") => Ok(MsiCheck::Mod1110),
        Some(o) => Err(format!(
            "MSI check must be none/mod10/mod11/mod1010/mod1110 (got '{o}')"
        )),
    }
}
