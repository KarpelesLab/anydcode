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
use crate::traits::Encode;
use alloc::{boxed::Box, format, string::String, string::ToString, vec, vec::Vec};

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

/// Borrow `len` bytes of wasm memory at `ptr`. An empty or null input is the empty
/// slice whatever the pointer says: `slice::from_raw_parts` is undefined behaviour on a
/// null pointer even for zero length, and a host may well pass `0` for "no bytes".
///
/// # Safety
/// For `len > 0`, a non-null `ptr` must point to `len` readable bytes.
unsafe fn input(ptr: *const u8, len: usize) -> &'static [u8] {
    if ptr.is_null() || len == 0 {
        return &[];
    }
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

/// Generate an Apple App Clip Code SVG for a UTF-8 `https://` URL. `opts` is the
/// usual `key=value;…` string: `template` (0–17), or `fg`/`bg` hex colors, and
/// `logo=cam|nfc`. Returns `[0][svg utf8]`, or `[2][utf8 error message]`.
///
/// # Safety
/// Pointers must describe valid buffers of the given lengths.
#[cfg(feature = "appclip")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn appclip(
    url_ptr: *const u8,
    url_len: usize,
    opt_ptr: *const u8,
    opt_len: usize,
) -> u64 {
    let url = core::str::from_utf8(unsafe { input(url_ptr, url_len) }).unwrap_or("");
    let opts = core::str::from_utf8(unsafe { input(opt_ptr, opt_len) }).unwrap_or("");
    match appclip_svg(url, opts) {
        Ok(svg) => {
            let mut buf = vec![0u8];
            buf.extend_from_slice(svg.as_bytes());
            export(buf)
        }
        Err(msg) => {
            let mut buf = vec![2u8];
            buf.extend_from_slice(msg.as_bytes());
            export(buf)
        }
    }
}

#[cfg(feature = "appclip")]
fn appclip_svg(url: &str, opts: &str) -> Result<String, String> {
    use crate::codes::appclip;

    let logo = match opt(opts, "logo") {
        Some("nfc") => appclip::LogoKind::Nfc,
        _ => appclip::LogoKind::Camera,
    };
    let palette = match (opt(opts, "fg"), opt(opts, "bg")) {
        (Some(fg), Some(bg)) if !fg.is_empty() && !bg.is_empty() => {
            let fg = appclip::Color::parse(fg).ok_or("foreground must be RRGGBB hex")?;
            let bg = appclip::Color::parse(bg).ok_or("background must be RRGGBB hex")?;
            appclip::Palette {
                foreground: fg,
                background: bg,
                third: appclip::third_color(fg, bg),
            }
        }
        _ => {
            let idx: usize = match opt(opts, "template") {
                o if is_auto(o) => 1,
                Some(v) => v.parse().map_err(|_| "template must be 0-17")?,
                None => unreachable!(),
            };
            appclip::template_palette(idx).ok_or("template must be 0-17")?
        }
    };
    appclip::generate_svg(url, &appclip::Options { palette, logo }).map_err(|e| e.to_string())
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
    let Some(frame) = (unsafe { frame_input(w, h, luma_ptr) }) else {
        return export(b"[]".to_vec());
    };

    // Same shared decode entry point as the CLI (`crate::pipeline::scan_all`), so the
    // demo and the command line always agree on what a frame decodes to.
    export(symbols_json(&crate::pipeline::scan_all(&frame)))
}

/// Decode only the **linear-textured** symbologies — 1D, and the stacked PDF417 family —
/// in a `w`×`h` luminance frame; returns the same JSON shape as `decode`.
///
/// This exists for the live pipeline: a crop the locator classified as *linear* should
/// never pay for the 2D samplers — their finder/geometry fallbacks are the expensive
/// part of a scan, and on a text-heavy crop they can dominate the whole dispatch while
/// the 1D read itself takes a millisecond (see [`crate::pipeline::scan_1d`]).
///
/// `axis` is the code's reading axis in radians as reported by `locate` (clockwise
/// from +x); pass `NaN` when unknown and the crop's orientation is searched instead.
///
/// # Safety
/// `luma_ptr` must point to `w*h` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn decode1d(w: usize, h: usize, luma_ptr: *const u8, axis: f32) -> u64 {
    let Some(frame) = (unsafe { frame_input(w, h, luma_ptr) }) else {
        return export(b"[]".to_vec());
    };
    export(symbols_json(&crate::pipeline::scan_linear_at(&frame, axis)))
}

/// Borrow a `w`×`h` luminance frame from wasm memory, or `None` for dimensions that do
/// not describe a frame (zero-sized, or `w*h` overflowing).
///
/// # Safety
/// `luma_ptr` must point to `w*h` readable bytes.
unsafe fn frame_input(w: usize, h: usize, luma_ptr: *const u8) -> Option<GrayFrame<'static>> {
    let len = w.checked_mul(h)?;
    if len == 0 || luma_ptr.is_null() {
        return None;
    }
    GrayFrame::new(unsafe { input(luma_ptr, len) }, w, h).ok()
}

/// Serialize decoded symbols as the `[{"symbology":..,"text":..}]` JSON both decode
/// entry points return.
fn symbols_json(out: &[crate::Symbol]) -> Vec<u8> {
    let mut json = String::from("[");
    for (i, sym) in out.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"symbology\":");
        json_str(&mut json, &sym.symbology.to_string());
        json.push_str(",\"text\":");
        json_str(&mut json, &sym.text().unwrap_or_default());
        json.push('}');
    }
    json.push(']');
    json.into_bytes()
}

/// Decode only the **2D** codes (QR, Data Matrix, PDF417) in a `w`×`h` luminance frame,
/// returning JSON `[{"symbology":..,"text":..,"box":{"x0":..,..}|null}]`.
///
/// These samplers self-localize, so this is meant to run on the whole frame — a live
/// front-end can read 2D codes directly per frame without depending on a coarse locator,
/// and each result carries the code's bounding box (frame pixels) for overlay tracking.
/// 1D codes are intentionally excluded: a linear scan wants a located crop, not a full
/// cluttered frame (see [`crate::pipeline::scan_1d`]).
///
/// # Safety
/// `luma_ptr` must point to `w*h` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn decode2d(w: usize, h: usize, luma_ptr: *const u8) -> u64 {
    let Some(frame) = (unsafe { frame_input(w, h, luma_ptr) }) else {
        return export(b"[]".to_vec());
    };

    export(symbols_boxed_json(&crate::pipeline::scan_2d(&frame)))
}

/// Serialize decoded symbols with their bounding boxes, as `decode2d` returns them.
fn symbols_boxed_json(out: &[crate::Symbol]) -> Vec<u8> {
    let mut json = String::from("[");
    for (i, sym) in out.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"symbology\":");
        json_str(&mut json, &sym.symbology.to_string());
        json.push_str(",\"text\":");
        json_str(&mut json, &sym.text().unwrap_or_default());
        json.push_str(",\"box\":");
        match symbol_box(sym) {
            Some((x0, y0, x1, y1)) => {
                json.push_str("{\"x0\":");
                push_num(&mut json, x0);
                json.push_str(",\"y0\":");
                push_num(&mut json, y0);
                json.push_str(",\"x1\":");
                push_num(&mut json, x1);
                json.push_str(",\"y1\":");
                push_num(&mut json, y1);
                json.push('}');
            }
            None => json.push_str("null"),
        }
        json.push('}');
    }
    json.push(']');
    json.into_bytes()
}

/// Axis-aligned bounds `(x0, y0, x1, y1)` of a decoded symbol's outline, if it has one.
fn symbol_box(sym: &crate::Symbol) -> Option<(f32, f32, f32, f32)> {
    let cs = sym.location.as_ref()?.outline.corners;
    let x0 = cs.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    let y0 = cs.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let x1 = cs.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    let y1 = cs.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    Some((x0, y0, x1, y1))
}

/// Locate (position-only, no decode) every candidate code in a `w`×`h` luminance
/// frame; returns JSON `[{"family":..,"axis":radians|null,"corners":[[x,y],..]}]`.
/// A linear candidate's corners are a box aligned with the code and `axis` is its
/// reading axis — hand it to `decode1d` with a crop of the corners' bounds.
///
/// # Safety
/// `luma_ptr` must point to `w*h` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn locate(w: usize, h: usize, luma_ptr: *const u8) -> u64 {
    let Some(frame) = (unsafe { frame_input(w, h, luma_ptr) }) else {
        return export(b"[]".to_vec());
    };
    // The locator's default downscale=2 is tuned for a full-resolution camera frame.
    // The demo feeds an already GPU-downscaled half-res grab (cheaper to extract on the
    // main thread), so pick the factor from the actual size: keep the reduced working
    // image near the same ~500×500+ scale either way.
    let opts = crate::detect::LocateOptions {
        downscale: if w.min(h) >= 800 { 2 } else { 1 },
        ..Default::default()
    };
    export(candidates_json(&crate::detect::locate(&frame, &opts)))
}

/// Serialize located candidates, as `locate` returns them.
fn candidates_json(candidates: &[crate::pipeline::Candidate]) -> Vec<u8> {
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
        json.push_str("\",\"axis\":");
        match c.location.rotation {
            Some(a) if a.is_finite() => json.push_str(&a.to_string()),
            _ => json.push_str("null"),
        }
        json.push_str(",\"corners\":[");
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
    json.into_bytes()
}

fn push_num(s: &mut String, v: f32) {
    // Integer-ish pixel coordinates; one decimal is plenty for the overlay. JSON has
    // no NaN/Infinity, and one unparseable number would cost the page the whole reply.
    let v = if v.is_finite() { v } else { 0.0 };
    // A finite value above `f32::MAX / 10` overflows the scaling to infinity; it has no
    // fractional part to round anyway.
    let scaled = v * 10.0;
    let r = if scaled.is_finite() {
        scaled.round() / 10.0
    } else {
        v
    };
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
            let s = e.build_text(data, micro_ec(ec)?).map_err(es)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Location, Point, Quad};
    use crate::pipeline::Candidate;
    use crate::symbol::{Symbol, SymbolMeta};
    use crate::symbology::Symbology;

    /// A parsed JSON value — just enough structure to check what the page will see.
    #[derive(Debug, Clone, PartialEq)]
    enum Json {
        Null,
        Num(f64),
        Str(String),
        Arr(Vec<Json>),
        Obj(Vec<(String, Json)>),
    }

    impl Json {
        fn field(&self, key: &str) -> &Json {
            match self {
                Json::Obj(fields) => &fields.iter().find(|(k, _)| k == key).expect(key).1,
                other => panic!("not an object: {other:?}"),
            }
        }
        fn items(&self) -> &[Json] {
            match self {
                Json::Arr(items) => items,
                other => panic!("not an array: {other:?}"),
            }
        }
    }

    /// A strict RFC 8259 parser for the subset the shim emits (no booleans, no
    /// exponents): anything `JSON.parse` would reject — a raw control character, a bad
    /// escape, `NaN`, a trailing comma — is an `Err`.
    struct Parser<'a> {
        s: &'a [u8],
        i: usize,
    }

    impl Parser<'_> {
        fn parse(bytes: &[u8]) -> Result<Json, String> {
            core::str::from_utf8(bytes).map_err(|e| e.to_string())?;
            let mut p = Parser { s: bytes, i: 0 };
            let v = p.value()?;
            if p.i != bytes.len() {
                return Err(format!("trailing bytes at {}", p.i));
            }
            Ok(v)
        }

        fn eat(&mut self, c: u8) -> Result<(), String> {
            if self.s.get(self.i) == Some(&c) {
                self.i += 1;
                Ok(())
            } else {
                Err(format!("expected '{}' at {}", c as char, self.i))
            }
        }

        fn value(&mut self) -> Result<Json, String> {
            match self.s.get(self.i).copied() {
                Some(b'[') => {
                    self.i += 1;
                    let mut items = Vec::new();
                    if self.eat(b']').is_ok() {
                        return Ok(Json::Arr(items));
                    }
                    loop {
                        items.push(self.value()?);
                        if self.eat(b',').is_err() {
                            self.eat(b']')?;
                            return Ok(Json::Arr(items));
                        }
                    }
                }
                Some(b'{') => {
                    self.i += 1;
                    let mut fields = Vec::new();
                    loop {
                        let Json::Str(key) = self.value()? else {
                            return Err("object key must be a string".into());
                        };
                        self.eat(b':')?;
                        fields.push((key, self.value()?));
                        if self.eat(b',').is_err() {
                            self.eat(b'}')?;
                            return Ok(Json::Obj(fields));
                        }
                    }
                }
                Some(b'"') => {
                    self.i += 1;
                    self.string().map(Json::Str)
                }
                Some(b'n') => {
                    if self.s[self.i..].starts_with(b"null") {
                        self.i += 4;
                        Ok(Json::Null)
                    } else {
                        Err("bad literal".into())
                    }
                }
                Some(b'-' | b'0'..=b'9') => {
                    let start = self.i;
                    while matches!(self.s.get(self.i), Some(b'-' | b'.' | b'0'..=b'9')) {
                        self.i += 1;
                    }
                    let text = core::str::from_utf8(&self.s[start..self.i]).unwrap();
                    let digits = text.strip_prefix('-').unwrap_or(text);
                    let ok = !digits.is_empty()
                        && !digits.starts_with('.')
                        && !digits.ends_with('.')
                        && !digits.contains('-')
                        && digits.matches('.').count() <= 1
                        && (digits == "0" || digits.starts_with("0.") || !digits.starts_with('0'));
                    match text.parse() {
                        Ok(v) if ok => Ok(Json::Num(v)),
                        _ => Err(format!("bad number '{text}'")),
                    }
                }
                other => Err(format!("unexpected {other:?} at {}", self.i)),
            }
        }

        /// The rest of a string whose opening quote is already consumed.
        fn string(&mut self) -> Result<String, String> {
            let mut out = Vec::new();
            loop {
                let c = *self.s.get(self.i).ok_or("unterminated string")?;
                self.i += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        let e = *self.s.get(self.i).ok_or("dangling escape")?;
                        self.i += 1;
                        match e {
                            b'"' | b'\\' | b'/' => out.push(e),
                            b'n' => out.push(b'\n'),
                            b'r' => out.push(b'\r'),
                            b't' => out.push(b'\t'),
                            b'b' => out.push(8),
                            b'f' => out.push(12),
                            b'u' => {
                                let hex = self.s.get(self.i..self.i + 4).ok_or("short \\u")?;
                                let hex = core::str::from_utf8(hex).map_err(|e| e.to_string())?;
                                let cp = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
                                self.i += 4;
                                let ch = char::from_u32(cp).ok_or("surrogate escape")?;
                                out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                            }
                            other => return Err(format!("bad escape \\{}", other as char)),
                        }
                    }
                    c if c < 0x20 => return Err(format!("raw control byte {c:#04x}")),
                    c => out.push(c),
                }
            }
            String::from_utf8(out).map_err(|e| e.to_string())
        }
    }

    fn symbol(payload: &[u8]) -> Symbol {
        Symbol::new(
            Symbology::Code128,
            vec![Segment::byte(payload.to_vec())],
            SymbolMeta::Generic,
        )
    }

    fn quad(corners: [(f32, f32); 4]) -> Quad {
        Quad::new(corners.map(|(x, y)| Point::new(x, y)))
    }

    fn located(mut sym: Symbol, corners: [(f32, f32); 4]) -> Symbol {
        sym.location = Some(Location {
            outline: quad(corners),
            rotation: None,
            module_size: None,
        });
        sym
    }

    #[test]
    fn the_test_parser_is_strict() {
        for bad in [
            "",
            "[",
            "[1,]",
            "[NaN]",
            "[inf]",
            "[01]",
            "[1.]",
            "[.5]",
            "[+1]",
            "\"\n\"",
            "\"\\x\"",
            "{\"a\"}",
            "[1] ",
            "\"\\ud800\"",
            "[1e5]",
        ] {
            assert!(Parser::parse(bad.as_bytes()).is_err(), "{bad:?} parsed");
        }
        assert_eq!(
            Parser::parse(b"[{\"a\":-0.5,\"b\":null},\"\\u0001\"]").unwrap(),
            Json::Arr(vec![
                Json::Obj(vec![
                    ("a".into(), Json::Num(-0.5)),
                    ("b".into(), Json::Null)
                ]),
                Json::Str("\u{1}".into()),
            ])
        );
    }

    /// Decoded text is attacker-controlled: whatever a symbol carries, the reply must
    /// parse, and parse back to the same text.
    #[test]
    fn decoded_text_survives_json_escaping() {
        let every_control: String = (0u8..0x20).map(char::from).collect();
        let texts = [
            "",
            "plain",
            "quote \" backslash \\ slash / end",
            "\\u0041 is not an escape",
            "\"},{\"symbology\":\"injected\"",
            "line\r\nbreak\ttab\u{8}\u{c}\0nul",
            every_control.as_str(),
            "del \u{7f} c1 \u{80}\u{9f} seps \u{2028}\u{2029} bom \u{feff}",
            "caf\u{e9} \u{65e5}\u{672c}\u{8a9e} \u{1f600} \u{10ffff} \u{fffd}",
            "</script><!--",
        ];
        let symbols: Vec<Symbol> = texts.iter().map(|t| symbol(t.as_bytes())).collect();
        for bytes in [symbols_json(&symbols), symbols_boxed_json(&symbols)] {
            let json = Parser::parse(&bytes).expect("valid JSON");
            assert_eq!(json.items().len(), texts.len());
            for (item, text) in json.items().iter().zip(texts) {
                assert_eq!(item.field("text"), &Json::Str(text.into()));
                assert_eq!(item.field("symbology"), &Json::Str("Code 128".into()));
            }
        }
        // Every symbology name is itself a clean JSON string.
        let all: Vec<Symbol> = Symbology::ALL
            .iter()
            .map(|&s| Symbol::new(s, Vec::new(), SymbolMeta::Generic))
            .collect();
        let json = Parser::parse(&symbols_json(&all)).unwrap();
        for (item, s) in json.items().iter().zip(Symbology::ALL) {
            assert_eq!(item.field("symbology"), &Json::Str(s.name().into()));
        }
        assert_eq!(symbols_json(&[]), b"[]");
        assert_eq!(symbols_boxed_json(&[]), b"[]");
    }

    /// A payload that is not UTF-8 (raw bytes, Latin-1, a lone surrogate's encoding, a
    /// truncated or overlong sequence) reports empty text — never invalid output.
    #[test]
    fn non_utf8_payloads_still_emit_valid_json() {
        let payloads: [&[u8]; 6] = [
            b"\xff\xfe",
            b"caf\xe9",
            b"\xed\xa0\x80",
            b"\xf0\x9f\x98",
            b"\xc0\xaf",
            b"ok\x00\x80",
        ];
        let symbols: Vec<Symbol> = payloads.iter().map(|p| symbol(p)).collect();
        for bytes in [symbols_json(&symbols), symbols_boxed_json(&symbols)] {
            let json = Parser::parse(&bytes).expect("valid JSON");
            assert_eq!(json.items().len(), payloads.len());
            for item in json.items() {
                assert_eq!(item.field("text"), &Json::Str(String::new()));
            }
        }
    }

    #[test]
    fn boxes_and_corners_are_always_json_numbers() {
        let nan = f32::NAN;
        let inf = f32::INFINITY;
        let symbols = [
            symbol(b"no location"),
            located(
                symbol(b"a"),
                [(1.0, 2.0), (30.26, 2.0), (30.0, 41.04), (-0.04, 40.0)],
            ),
            located(symbol(b"b"), [(nan, nan); 4]),
            located(
                symbol(b"c"),
                [
                    (inf, -inf),
                    (nan, 5.0),
                    (f32::MAX, f32::MIN),
                    (1e-30, -1e-30),
                ],
            ),
        ];
        let json = Parser::parse(&symbols_boxed_json(&symbols)).expect("valid JSON");
        let items = json.items();
        assert_eq!(items[0].field("box"), &Json::Null);
        let b = items[1].field("box");
        for (key, want) in [("x0", 0.0), ("y0", 2.0), ("x1", 30.3), ("y1", 41.0)] {
            let Json::Num(got) = b.field(key) else {
                panic!("{key} is not a number")
            };
            assert!((got - want).abs() < 1e-4, "{key}: {got} vs {want}");
        }
        for item in &items[2..] {
            for key in ["x0", "y0", "x1", "y1"] {
                assert!(matches!(item.field("box").field(key), Json::Num(v) if v.is_finite()));
            }
        }

        let candidate = |outline, rotation, symbology| Candidate {
            symbology,
            ..Candidate::at(Location {
                outline,
                rotation,
                module_size: None,
            })
        };
        let square = quad([(0.0, 0.0), (9.96, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let wild = quad([(nan, inf), (-inf, nan), (f32::MAX, f32::MIN), (-0.0, 1e-40)]);
        let candidates = [
            candidate(square, Some(0.5), Some(Symbology::QrCode)),
            candidate(square, None, Some(Symbology::Pdf417)),
            candidate(wild, Some(nan), Some(Symbology::Postnet)),
            candidate(wild, Some(inf), None),
            candidate(square, Some(-1e-7), Some(Symbology::Ean13)),
            candidate(square, Some(f32::MAX), None),
        ];
        let json = Parser::parse(&candidates_json(&candidates)).expect("valid JSON");
        let items = json.items();
        let expect = ["matrix", "stacked", "postal", "linear", "linear", "linear"];
        for (c, family) in items.iter().zip(expect) {
            assert_eq!(c.field("family"), &Json::Str(family.into()));
            let corners = c.field("corners").items();
            assert_eq!(corners.len(), 4);
            for p in corners {
                assert_eq!(p.items().len(), 2);
                assert!(
                    p.items()
                        .iter()
                        .all(|v| matches!(v, Json::Num(n) if n.is_finite()))
                );
            }
        }
        assert_eq!(items[0].field("axis"), &Json::Num(0.5));
        for item in &items[1..=3] {
            assert_eq!(item.field("axis"), &Json::Null);
        }
        assert!(matches!(items[4].field("axis"), Json::Num(v) if *v < 0.0 && *v > -1e-6));
        assert!(matches!(items[5].field("axis"), Json::Num(v) if *v > 1e38));
        assert_eq!(
            items[0].field("corners").items()[1].items()[0],
            Json::Num(10.0)
        );
        assert_eq!(candidates_json(&[]), b"[]");
    }

    /// `slice::from_raw_parts(null, 0)` is undefined behaviour (and aborts a debug
    /// build); a host passing `0` for an empty string must simply get the empty slice.
    #[test]
    fn null_and_empty_inputs_are_empty_slices() {
        assert!(unsafe { input(core::ptr::null(), 0) }.is_empty());
        assert!(unsafe { input(core::ptr::null(), 7) }.is_empty());
        let bytes = [1u8, 2, 3];
        assert!(unsafe { input(bytes.as_ptr(), 0) }.is_empty());
        assert_eq!(unsafe { input(bytes.as_ptr(), 3) }, &bytes);
    }

    #[test]
    fn frames_that_are_not_frames_are_refused() {
        let luma = [0u8; 12];
        let p = luma.as_ptr();
        let side = 1usize << (usize::BITS / 2);
        for (w, h, ptr) in [
            (0, 0, p),
            (0, 3, p),
            (4, 0, p),
            (4, 3, core::ptr::null()),
            (side, side, p),
            (usize::MAX, 2, p),
            (usize::MAX, usize::MAX, p),
        ] {
            assert!(unsafe { frame_input(w, h, ptr) }.is_none(), "{w}x{h}");
        }
        let frame = unsafe { frame_input(4, 3, p) }.unwrap();
        assert_eq!((frame.width(), frame.height()), (4, 3));
    }

    #[test]
    fn alloc_and_dealloc_tolerate_empty_buffers() {
        let empty = alloc(0);
        assert!(!empty.is_null());
        unsafe { dealloc(empty, 0) };
        unsafe { dealloc(core::ptr::null_mut(), 0) };
        unsafe { dealloc(core::ptr::null_mut(), 16) };
        let p = alloc(64);
        assert!(
            unsafe { core::slice::from_raw_parts(p, 64) }
                .iter()
                .all(|&b| b == 0)
        );
        unsafe { dealloc(p, 64) };
    }

    #[test]
    fn options_string_parsing() {
        assert_eq!(opt("ec=H;size=min", "ec"), Some("H"));
        assert_eq!(opt("ec=H&size=min", "size"), Some("min"));
        assert_eq!(opt(" ec = H ; size= min ", "ec"), Some("H"));
        assert_eq!(opt("ec=H;ec=L", "ec"), Some("H"), "first wins");
        assert_eq!(opt("url=a=b", "url"), Some("a=b"));
        assert_eq!(opt("ec", "ec"), None);
        assert_eq!(opt("", "ec"), None);
        assert_eq!(opt("ec=", "ec"), Some(""));
        assert_eq!(opt("spec=H", "ec"), None);
        assert!(opt_bool("check=on", "check", false));
        assert!(opt_bool("check=1;x=y", "check", false));
        assert!(!opt_bool("check=off", "check", true));
        assert!(!opt_bool("check=", "check", true));
        assert!(opt_bool("", "check", true));
        assert!(is_auto(None) && is_auto(Some("")) && is_auto(Some("auto")));
        assert!(!is_auto(Some("12")));
    }

    #[test]
    fn encoding_wire_format() {
        let mut m = crate::output::BitMatrix::new(3, 2, 4);
        m.set(0, 0, true);
        m.set(2, 1, true);
        assert_eq!(
            serialize_encoding(&Encoding::Matrix(m)),
            [0, 3, 0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 1]
        );
        let p = crate::output::LinearPattern {
            modules: vec![true, false, true],
            quiet_zone: 10,
        };
        assert_eq!(
            serialize_encoding(&Encoding::Linear(p)),
            [1, 3, 0, 0, 0, 1, 0, 0, 0, 10, 0, 0, 0, 1, 0, 1]
        );
    }

    /// Every symbology name the demo page can send, with data it accepts.
    const NAMES: &[(&str, &str)] = &[
        ("qr", "HELLO"),
        ("microqr", "12345"),
        ("rmqr", "HELLO"),
        ("aztec", "HELLO"),
        ("aztecrunes", "42"),
        ("datamatrix", "HELLO"),
        ("maxicode", "HELLO"),
        ("hanxin", "HELLO"),
        ("dotcode", "HELLO"),
        ("gridmatrix", "HELLO"),
        ("pdf417", "HELLO"),
        ("micropdf417", "HELLO"),
        ("code16k", "HELLO"),
        ("code49", "HELLO"),
        ("codablockf", "HELLO"),
        ("ean13", "5901234123457"),
        ("ean8", "96385074"),
        ("upca", "036000291452"),
        ("upce", "01234565"),
        ("ean2", "12"),
        ("ean5", "12345"),
        ("code128", "HELLO"),
        ("gs1_128", "0109501101530003"),
        ("code39", "HELLO"),
        ("code93", "HELLO"),
        ("code11", "1234"),
        ("itf", "1234"),
        ("std2of5", "1234"),
        ("iata2of5", "1234"),
        ("matrix2of5", "1234"),
        ("codabar", "1234"),
        ("msi", "1234"),
        ("plessey", "1234"),
        ("telepen", "HELLO"),
        ("pharmacode", "1234"),
        ("pharmacode2", "1234"),
        ("dxfilm", "115-10"),
        ("databar_omni", "0950110153000"),
        ("databar_limited", "0950110153000"),
        ("databar_stacked", "0950110153000"),
        ("databar_stacked_omni", "0950110153000"),
        ("databar_expanded", "0109501101530003"),
        ("databar_expanded_stacked", "0109501101530003"),
        ("postnet", "12345"),
        ("planet", "12345678901"),
        ("imb", "01234567094987654321 01234567891"),
        ("rm4scc", "SN34RD1A"),
        ("kix", "1231FZ13XHS"),
        ("japanpost", "1234567"),
        ("mailmark", "41038422416563762EF61AH8T"),
        ("auspost", "12345678"),
    ];

    #[test]
    fn every_symbology_name_builds() {
        for &(name, data) in NAMES {
            let enc = build(name, data, "").unwrap_or_else(|e| panic!("{name} {data:?}: {e}"));
            let bytes = serialize_encoding(&enc);
            let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let (w, h) = (word(1) as usize, word(5) as usize);
            assert!(bytes[0] <= 1 && w > 0 && h > 0, "{name}");
            assert_eq!(bytes.len(), 13 + w * h, "{name}");
            assert!(bytes[13..].iter().all(|&b| b <= 1), "{name}");
            assert!(bytes[13..].contains(&1), "{name} has no dark module");
        }
        assert_eq!(
            build("nope", "x", "").unwrap_err(),
            "unknown symbology 'nope'"
        );
        assert!(build("", "", "").is_err());
    }

    /// A panic is a wasm trap that takes the whole page down, so whatever the form
    /// fields hold — empty, huge, non-ASCII, nonsense options — `build` must return.
    #[test]
    fn hostile_form_input_never_panics() {
        let long_digits = "7".repeat(5000);
        let long_text = "Az9 ".repeat(2000);
        let datas = [
            "",
            " ",
            "0",
            "-1",
            "99999999999999999999999999",
            "|||",
            "a|b|c|d",
            "12345|840|001|x",
            "\u{0}\u{1d}\u{7f}",
            "caf\u{e9} \u{65e5}\u{672c} \u{1f600}",
            long_digits.as_str(),
            long_text.as_str(),
        ];
        let options = [
            "",
            "ec=;size=;cols=;rows=;mode=;version=;check=;start=;stop=",
            "ec=Z;size=zz;cols=x;rows=x;mode=x;version=x;check=x;start=E;stop=E",
            "ec=H;size=0;cols=0;rows=0;mode=0;version=0;check=0",
            "ec=8;size=1;cols=1;rows=1;mode=2;version=1;check=2",
            "ec=L5;size=max;cols=99;rows=99;mode=6;version=13;check=mod1110",
            "ec=L1;size=4294967295;cols=18446744073709551615;rows=18446744073709551615;\
             mode=255;version=255;check=255",
            "mode=gs1;fullascii=off;check=on;start=D;stop=B",
            "mode=3;size=144;cols=30;rows=16",
            "=;;==;&&;ec",
        ];
        for &(name, _) in NAMES {
            for data in datas {
                for opts in options {
                    // Ok or Err are both fine; the point is that it returns.
                    if let Ok(enc) = build(name, data, opts) {
                        assert!(serialize_encoding(&enc).len() > 13, "{name}");
                    }
                }
            }
        }
    }

    #[cfg(feature = "appclip")]
    #[test]
    fn appclip_options() {
        let url = "https://example.com";
        let svg = appclip_svg(url, "").unwrap();
        assert!(svg.contains("<svg") && svg.trim_end().ends_with("</svg>"));
        assert_eq!(appclip_svg(url, "template=auto").unwrap(), svg);
        assert_eq!(appclip_svg(url, "template=1").unwrap(), svg);
        assert_ne!(appclip_svg(url, "template=0;logo=nfc").unwrap(), svg);
        assert!(appclip_svg(url, "fg=000000;bg=ffffff").is_ok());
        for bad in [
            "template=18",
            "template=-1",
            "template=x",
            "fg=zzzzzz;bg=ffffff",
            "fg=000000;bg=12",
            "fg=\u{e9}\u{e9}\u{e9};bg=\u{1f600}",
        ] {
            assert!(appclip_svg(url, bad).is_err(), "{bad}");
        }
        for bad_url in ["", "ftp://x", "not a url", "https://"] {
            let _ = appclip_svg(bad_url, "");
        }
    }
}
