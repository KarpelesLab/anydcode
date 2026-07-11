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

/// Build an [`Encoding`] for a symbology name + UTF-8 data. Covers a representative
/// set for the demo; unknown names return an error.
fn build(sym: &str, data: &str, opts: &str) -> Result<Encoding, String> {
    use crate::codes::*;
    let b = data.as_bytes();
    let ec = opt(opts, "ec");
    let es = |e: crate::Error| e.to_string();
    match sym {
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
            e.encode(&e.build_text(data, rmqr_ec(ec)?).map_err(es)?)
                .map_err(es)
        }
        "aztec" => {
            let e = aztec::AztecEncoder::new();
            e.encode(&e.build_text(data).map_err(es)?).map_err(es)
        }
        "datamatrix" => {
            let e = datamatrix::DataMatrixEncoder::new();
            e.encode(&e.build_text(data).map_err(es)?).map_err(es)
        }
        "pdf417" => {
            let e = pdf417::Pdf417Encoder::new();
            e.encode(&e.build_text(data, pdf417_ec(ec)?).map_err(es)?)
                .map_err(es)
        }
        "maxicode" => {
            let e = maxicode::MaxiCodeEncoder::new();
            e.encode(&e.build_text(data).map_err(es)?).map_err(es)
        }
        "code128" => {
            let e = code128::Code128Encoder::new();
            e.encode(&e.build_text(data).map_err(es)?).map_err(es)
        }
        "code39" => {
            let e = code39::Code39Encoder::new();
            e.encode(&e.build(b, true, false).map_err(es)?).map_err(es)
        }
        "code93" => {
            let e = code93::Code93Encoder::new();
            e.encode(&e.build(b, true).map_err(es)?).map_err(es)
        }
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
        "itf" => {
            let e = itf::ItfEncoder::new();
            e.encode(&e.build(b, false).map_err(es)?).map_err(es)
        }
        "codabar" => {
            let e = codabar::CodabarEncoder::new();
            e.encode(&e.build(b'A', b, b'A').map_err(es)?).map_err(es)
        }
        other => Err(format!("unknown symbology '{other}'")),
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
