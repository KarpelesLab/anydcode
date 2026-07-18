//! URL compression: HTTPS URL ⇄ the 128-bit (16-byte) App Clip payload.
//!
//! The compressed stream is `[1 begin][template][subdomain][host format][host bits]
//! [path/query bits]`, left-padded with zeros to 128 bits. Hosts encode through one
//! of three formats (Huffman-coded common TLD, fixed-index TLD, or the whole host
//! through the trained host coder); paths and queries through whichever of Apple's
//! template, combined or segmented encodings yields the fewest bits, each component
//! choosing among context-Huffman text, unsigned LEB128 decimals, a fixed 6-bit
//! alphabet and (for paths) the shared wordbook.

use std::sync::OnceLock;

use super::huffman::{HuffmanCoder, MultiCoder, Trie};
use super::tables::{
    CPQ_SYMBOLS, FIXED6_ALPHABET, FIXED_TLDS, HOST_SYMBOLS, HUFFMAN_TLDS, KNOWN_WORDS,
    SPQ_SYMBOLS, fixed6_index, known_word_index,
};
use crate::error::{Error, Result};

static H_DATA: &[u8] = include_bytes!("data/h.data");
static SPQ_DATA: &[u8] = include_bytes!("data/spq.data");
static CPQ_DATA: &[u8] = include_bytes!("data/cpq.data");

fn host_coder() -> &'static MultiCoder {
    static C: OnceLock<MultiCoder> = OnceLock::new();
    C.get_or_init(|| MultiCoder::new(Trie::new(H_DATA, &HOST_SYMBOLS)))
}

fn spq_coder() -> &'static MultiCoder {
    static C: OnceLock<MultiCoder> = OnceLock::new();
    C.get_or_init(|| MultiCoder::new(Trie::new(SPQ_DATA, &SPQ_SYMBOLS)))
}

fn cpq_coder() -> &'static MultiCoder {
    static C: OnceLock<MultiCoder> = OnceLock::new();
    C.get_or_init(|| MultiCoder::new(Trie::new(CPQ_DATA, &CPQ_SYMBOLS)))
}

/// TLD symbol list (alphabetical) and its Huffman coder for host format 0.
fn tld_coder() -> &'static HuffmanCoder {
    static C: OnceLock<HuffmanCoder> = OnceLock::new();
    C.get_or_init(|| {
        let freqs: Vec<u16> = HUFFMAN_TLDS.iter().map(|&(_, f)| f).collect();
        let syms: Vec<&'static str> = HUFFMAN_TLDS.iter().map(|&(s, _)| s).collect();
        HuffmanCoder::new(&freqs, &syms)
    })
}

// ===================================================================================
// Compression
// ===================================================================================

/// Compress an HTTPS URL to the 16-byte payload.
pub fn compress_url(raw: &str) -> Result<[u8; 16]> {
    let u = parse_url(raw)?;

    let mut host = u.host.as_str();
    let subdomain = host.starts_with("appclip.");
    if subdomain {
        host = &host["appclip.".len()..];
    }

    let has_pq = !u.path.is_empty() || !u.query.is_empty() || !u.fragment.is_empty();
    let (pq_bits, template) = if has_pq {
        choose_path_query(&u.path, &u.query, &u.fragment)?
    } else {
        (String::new(), false)
    };

    let mut bits = String::from("1"); // begin marker
    bits.push(if template { '1' } else { '0' });
    bits.push(if subdomain { '1' } else { '0' });

    let (host_bits, host_fmt) = encode_host(host, has_pq)?;
    match host_fmt {
        0 => bits.push('0'),
        1 => bits.push_str("10"),
        _ => bits.push_str("11"),
    }
    bits.push_str(&host_bits);
    bits.push_str(&pq_bits);

    if bits.len() > 128 {
        return Err(Error::capacity(format!(
            "compressed URL needs {} bits (max 128)",
            bits.len()
        )));
    }
    let mut out = [0u8; 16];
    let pad = 128 - bits.len();
    for (i, c) in bits.bytes().enumerate() {
        if c == b'1' {
            let bit = pad + i;
            out[bit / 8] |= 1 << (7 - bit % 8);
        }
    }
    Ok(out)
}

/// Host → (bits, format): Huffman TLD (0), fixed-index TLD (1), or full host (2).
fn encode_host(host: &str, has_pq: bool) -> Result<(String, u8)> {
    let Some(last_dot) = host.rfind('.') else {
        return Err(Error::invalid_parameter("App Clip URL host has no TLD"));
    };
    let (domain, tld) = (&host[..last_dot], &host[last_dot..]);
    let mut domain_term = domain.to_string();
    if has_pq {
        domain_term.push('|');
    }

    if let Some(idx) = HUFFMAN_TLDS.iter().position(|&(t, _)| t == tld)
        && tld_coder().can_encode(idx)
        && let Some(domain_bits) = host_coder().encode(&domain_term, "")
    {
        return Ok((format!("{}{}", tld_coder().encode(idx), domain_bits), 0));
    }

    if let Some(&(_, idx)) = FIXED_TLDS.iter().find(|&&(t, _)| t == tld)
        && let Some(domain_bits) = host_coder().encode(&domain_term, "")
    {
        return Ok((format!("{}{}", int_bits(idx as usize, 8), domain_bits), 1));
    }

    let mut full = host.to_string();
    if has_pq {
        full.push('|');
    }
    match host_coder().encode(&full, "") {
        Some(bits) => Ok((bits, 2)),
        None => Err(Error::invalid_parameter("App Clip host not encodable")),
    }
}

/// Best path/query encoding and whether it used the auto-query template.
fn choose_path_query(path: &str, query: &str, fragment: &str) -> Result<(String, bool)> {
    let template = encode_template_pq(path, query, fragment);
    let plain = encode_non_template_pq(path, query, fragment);
    match (template, plain) {
        // Apple prefers the non-template encoding when lengths tie.
        (Some(t), Some(p)) if t.len() < p.len() => Ok((t, true)),
        (_, Some(p)) => Ok((p, false)),
        (Some(t), None) => Ok((t, true)),
        (None, None) => Err(Error::invalid_parameter("App Clip path/query not encodable")),
    }
}

// -------------------------------- template mode --------------------------------

fn encode_template_pq(path: &str, query: &str, fragment: &str) -> Option<String> {
    if !fragment.is_empty() {
        return None;
    }
    let (path_word, params) = match_auto_query_template(path, query)?;

    let mut bits = String::new();
    if let Some(word) = path_word {
        let idx = known_word_index(word)?;
        bits.push('0');
        bits.push_str(&int_bits(idx, 8));
    }
    if !params.is_empty() {
        bits.push('1');
        for (i, param) in params.iter().enumerate() {
            bits.push_str(&encode_template_query_component(
                param,
                i + 1 < params.len(),
            )?);
        }
    }
    if bits.is_empty() {
        return None;
    }
    Some(bits)
}

/// Apple's `PathWordBookAndAutoQueryTemplateFormat` matcher: at most one wordbook
/// path segment, and only the positional `p`, `p1`, `p2`, … query keys.
fn match_auto_query_template<'a>(
    path: &'a str,
    query: &'a str,
) -> Option<(Option<&'a str>, Vec<&'a str>)> {
    if path.len() >= 2 && path.ends_with('/') {
        return None;
    }
    if query.ends_with('&') {
        return None;
    }

    let path_parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if path_parts.len() > 1 {
        return None;
    }
    let path_word = match path_parts.first() {
        Some(&part) => {
            known_word_index(part)?;
            Some(part)
        }
        None => None,
    };

    let params: Vec<&str> = query.split('&').filter(|p| !p.is_empty()).collect();
    if params.is_empty() {
        return (path_word.is_some() || path == "/").then_some((path_word, params));
    }
    for (i, param) in params.iter().enumerate() {
        let (key, rest) = param.split_once('=')?;
        let _ = rest;
        let want = if i == 0 {
            "p".to_string()
        } else {
            format!("p{i}")
        };
        if key != want {
            return None;
        }
    }
    Some((path_word, params))
}

fn encode_template_query_component(param: &str, has_more: bool) -> Option<String> {
    let (_, value) = param.split_once('=')?;
    let mut best: Option<String> = None;
    if let Some(bits) = encode_spq_value("=", value, has_more) {
        best = Some(format!("00{bits}"));
    }
    if let Some(bits) = encode_uleb128(value) {
        best = shorter(best, format!("01{bits}"));
    }
    if let Some(bits) = encode_fixed6_value(value, has_more) {
        best = shorter(best, format!("10{bits}"));
    }
    best
}

// ------------------------------ non-template modes ------------------------------

fn encode_non_template_pq(path: &str, query: &str, fragment: &str) -> Option<String> {
    let combined = encode_combined_pq(path, query, fragment);
    let segmented = encode_segmented_pq(path, query, fragment);
    match (combined, segmented) {
        // Apple uses `0` for combined, `1` for segmented, preferring combined on ties.
        (Some(c), Some(s)) if s.len() < c.len() => Some(format!("1{s}")),
        (Some(c), _) => Some(format!("0{c}")),
        (None, Some(s)) => Some(format!("1{s}")),
        (None, None) => None,
    }
}

fn encode_combined_pq(path: &str, query: &str, fragment: &str) -> Option<String> {
    let mut combined = String::from(path);
    if !query.is_empty() {
        combined.push('?');
        combined.push_str(query);
    }
    if !fragment.is_empty() {
        combined.push('#');
        combined.push_str(fragment);
    }
    if combined.starts_with('/') && (combined.len() == 1 || combined.as_bytes()[1] != b'#') {
        combined.remove(0);
    }
    if combined.is_empty() {
        return None;
    }
    cpq_coder().encode(&combined, "")
}

fn encode_segmented_pq(path: &str, query: &str, fragment: &str) -> Option<String> {
    if !fragment.is_empty() {
        return None;
    }
    let items = segmented_path_items(path);
    let mut bits = String::new();
    for (i, item) in items.iter().enumerate() {
        if *item == "/" {
            bits.push_str("10");
            continue;
        }
        let has_more = i + 1 < items.len() || !query.is_empty();
        bits.push('0');
        bits.push_str(&encode_segmented_path_component(item, has_more)?);
    }
    if !query.is_empty() {
        let params: Vec<&str> = query.split('&').collect();
        bits.push_str("11");
        for (i, param) in params.iter().enumerate() {
            bits.push_str(&encode_segmented_query_component(
                param,
                i + 1 < params.len(),
            )?);
        }
    }
    if bits.is_empty() {
        return None;
    }
    Some(bits)
}

fn segmented_path_items(path: &str) -> Vec<&str> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut items: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    if items.is_empty() || path.ends_with('/') {
        items.push("/");
    }
    items
}

fn encode_segmented_path_component(component: &str, needs_term: bool) -> Option<String> {
    if component.is_empty() {
        return None;
    }
    let mut best: Option<String> = None;
    if let Some(bits) = encode_spq_value("", component, needs_term) {
        best = Some(format!("00{bits}"));
    }
    if let Some(bits) = encode_uleb128(component) {
        best = shorter(best, format!("01{bits}"));
    }
    if let Some(bits) = encode_fixed6_value(component, needs_term) {
        best = shorter(best, format!("10{bits}"));
    }
    if let Some(idx) = known_word_index(component) {
        best = shorter(best, format!("11{}", int_bits(idx, 8)));
    }
    best
}

fn encode_segmented_query_component(param: &str, has_more: bool) -> Option<String> {
    let (key, value) = param.split_once('=')?;
    let key_term = encode_spq_value("?", key, true)?;
    let key_bare = encode_spq_value("?", key, has_more)?;

    let mut best: Option<String> = None;
    if let Some(bits) = encode_spq_value("=", value, has_more) {
        best = Some(format!("00{key_term}{bits}"));
    }
    if let Some(bits) = encode_uleb128(value) {
        best = shorter(best, format!("01{bits}{key_bare}"));
    }
    if let Some(bits) = encode_fixed6_value(value, has_more) {
        best = shorter(best, format!("10{key_term}{bits}"));
    }
    best
}

fn encode_spq_value(start_ctx: &str, value: &str, needs_term: bool) -> Option<String> {
    let mut s = value.to_string();
    if needs_term {
        s.push('|');
    }
    spq_coder().encode(&s, start_ctx)
}

fn encode_fixed6_value(value: &str, needs_term: bool) -> Option<String> {
    let mut bits = String::new();
    for &c in value.as_bytes() {
        bits.push_str(&int_bits(fixed6_index(c)?, 6));
    }
    if needs_term {
        bits.push_str(&int_bits(fixed6_index(b'|')?, 6));
    }
    Some(bits)
}

fn shorter(current: Option<String>, candidate: String) -> Option<String> {
    match current {
        Some(cur) if cur.len() <= candidate.len() => Some(cur),
        _ => Some(candidate),
    }
}

fn int_bits(value: usize, n: usize) -> String {
    (0..n)
        .rev()
        .map(|i| if (value >> i) & 1 == 1 { '1' } else { '0' })
        .collect()
}

// --------------------------- decimal-string ULEB128 ---------------------------

/// Encode a decimal digit string as unsigned LEB128 bits (arbitrary precision).
fn encode_uleb128(value: &str) -> Option<String> {
    if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut digits: Vec<u8> = value.bytes().map(|c| c - b'0').collect();
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        // Long division of `digits` by 128, collecting the remainder.
        let mut quotient: Vec<u8> = Vec::with_capacity(digits.len());
        let mut carry = 0u32;
        for &d in &digits {
            carry = carry * 10 + u32::from(d);
            quotient.push((carry / 128) as u8);
            carry %= 128;
        }
        while quotient.len() > 1 && quotient[0] == 0 {
            quotient.remove(0);
        }
        let done = quotient == [0];
        bytes.push(carry as u8 | if done { 0 } else { 0x80 });
        if done {
            break;
        }
        digits = quotient;
    }
    Some(bytes.iter().map(|&b| int_bits(b as usize, 8)).collect())
}

/// Decode ULEB128 bytes from the bit stream to a canonical decimal string.
fn decode_uleb128(data: &[bool], pos: &mut usize) -> Result<String> {
    let mut acc = vec![0u8]; // decimal digits, most significant first
    let mut power = vec![1u8]; // 128^k as decimal digits
    loop {
        let b = read_int(data, pos, 8)?;
        let term = dec_mul_small(&power, (b & 0x7f) as u32);
        acc = dec_add(&acc, &term);
        if b & 0x80 == 0 {
            return Ok(acc.iter().map(|&d| (b'0' + d) as char).collect());
        }
        power = dec_mul_small(&power, 128);
    }
}

fn dec_mul_small(digits: &[u8], k: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(digits.len() + 3);
    let mut carry = 0u32;
    for &d in digits.iter().rev() {
        let v = u32::from(d) * k + carry;
        out.push((v % 10) as u8);
        carry = v / 10;
    }
    while carry > 0 {
        out.push((carry % 10) as u8);
        carry /= 10;
    }
    if out.is_empty() {
        out.push(0);
    }
    while out.len() > 1 && *out.last().unwrap() == 0 {
        out.pop();
    }
    out.reverse();
    out
}

fn dec_add(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
    let (mut ia, mut ib) = (a.len(), b.len());
    let mut carry = 0u8;
    while ia > 0 || ib > 0 || carry > 0 {
        let da = if ia > 0 { a[ia - 1] } else { 0 };
        let db = if ib > 0 { b[ib - 1] } else { 0 };
        ia = ia.saturating_sub(1);
        ib = ib.saturating_sub(1);
        let v = da + db + carry;
        out.push(v % 10);
        carry = v / 10;
    }
    out.reverse();
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    out
}

// ===================================================================================
// URL parsing / canonicalization
// ===================================================================================

struct ParsedUrl {
    host: String,
    path: String,
    query: String,
    fragment: String,
}

#[derive(PartialEq, Clone, Copy)]
enum Component {
    Path,
    Query,
    Fragment,
}

fn parse_url(raw: &str) -> Result<ParsedUrl> {
    let scheme = "https://";
    if raw.len() < scheme.len() || !raw[..scheme.len()].eq_ignore_ascii_case(scheme) {
        return Err(Error::invalid_parameter("App Clip URL scheme must be https"));
    }
    let rest = &raw[scheme.len()..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, mut suffix) = rest.split_at(authority_end);

    if authority.is_empty() {
        return Err(Error::invalid_parameter("App Clip URL must have a host"));
    }
    if authority.contains('@') || authority.contains(':') {
        return Err(Error::invalid_parameter(
            "App Clip URL must not carry user info or a port",
        ));
    }
    let host = canonical_host(authority)?;

    let mut out = ParsedUrl {
        host,
        path: String::new(),
        query: String::new(),
        fragment: String::new(),
    };
    if suffix.starts_with('/') {
        let end = suffix.find(['?', '#']).unwrap_or(suffix.len());
        out.path = canonical_component(&suffix[..end], Component::Path)?;
        suffix = &suffix[end..];
    }
    if let Some(stripped) = suffix.strip_prefix('?') {
        let end = stripped.find('#').unwrap_or(stripped.len());
        out.query = canonical_component(&stripped[..end], Component::Query)?;
        suffix = &stripped[end..];
    }
    if let Some(stripped) = suffix.strip_prefix('#') {
        out.fragment = canonical_component(stripped, Component::Fragment)?;
    }
    Ok(out)
}

fn canonical_host(authority: &str) -> Result<String> {
    let lower = authority.to_ascii_lowercase();
    for &c in lower.as_bytes() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-' {
            continue;
        }
        return Err(Error::invalid_parameter(
            "App Clip URL host must be ASCII letters, digits, '.', '-'",
        ));
    }
    if lower.split('.').any(|label| label.starts_with("xn--")) {
        return Err(Error::invalid_parameter(
            "App Clip URL host must not use punycode",
        ));
    }
    Ok(lower)
}

fn canonical_component(s: &str, kind: Component) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'%' {
            if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit()
            {
                out.push('%');
                out.push(bytes[i + 1] as char);
                out.push(bytes[i + 2] as char);
                i += 3;
                continue;
            }
            return Err(Error::invalid_parameter(
                "App Clip URL has an invalid percent escape",
            ));
        }
        if !(0x20..0x7f).contains(&c) {
            return Err(Error::invalid_parameter(
                "App Clip URL contains unsupported characters",
            ));
        }
        if rejects_raw(c, kind) {
            return Err(Error::invalid_parameter(
                "App Clip URL contains unsupported characters",
            ));
        }
        if allowed_raw(c, kind) {
            out.push(c as char);
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push('%');
            out.push(HEX[(c >> 4) as usize] as char);
            out.push(HEX[(c & 0x0f) as usize] as char);
        }
        i += 1;
    }
    Ok(out)
}

fn rejects_raw(c: u8, kind: Component) -> bool {
    match c {
        b' ' | b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}' => true,
        b'#' => kind == Component::Fragment,
        _ => false,
    }
}

fn allowed_raw(c: u8, kind: Component) -> bool {
    if c.is_ascii_alphanumeric() {
        return true;
    }
    match c {
        b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+'
        | b',' | b';' | b'=' | b':' | b'@' | b'/' => true,
        b'?' => kind != Component::Path,
        _ => false,
    }
}

// ===================================================================================
// Decompression
// ===================================================================================

fn read_bit(data: &[bool], pos: &mut usize) -> Result<bool> {
    if *pos >= data.len() {
        return Err(Error::undecodable("App Clip payload truncated"));
    }
    let b = data[*pos];
    *pos += 1;
    Ok(b)
}

fn read_int(data: &[bool], pos: &mut usize, n: usize) -> Result<usize> {
    let mut v = 0usize;
    for _ in 0..n {
        v = (v << 1) | usize::from(read_bit(data, pos)?);
    }
    Ok(v)
}

/// Decompress a 16-byte payload back to the URL string.
pub fn decompress_url(payload: &[u8; 16]) -> Result<String> {
    let mut bits = [false; 128];
    for (i, b) in bits.iter_mut().enumerate() {
        *b = (payload[i / 8] >> (7 - i % 8)) & 1 == 1;
    }
    let start = bits
        .iter()
        .position(|&b| b)
        .ok_or_else(|| Error::undecodable("App Clip payload has no begin marker"))?;
    let data = &bits[start + 1..];
    let mut pos = 0usize;

    let template = read_bit(data, &mut pos)?;
    let subdomain = read_bit(data, &mut pos)?;
    let host_fmt = if !read_bit(data, &mut pos)? {
        0u8
    } else if !read_bit(data, &mut pos)? {
        1
    } else {
        2
    };

    let (host, has_pq) = match host_fmt {
        0 => {
            let idx = tld_coder()
                .decode(data, &mut pos)
                .ok_or_else(|| Error::undecodable("App Clip TLD code invalid"))?;
            let tld = HUFFMAN_TLDS[idx].0;
            let (domain, has_pq) = decode_host_chars(data, &mut pos);
            (format!("{domain}{tld}"), has_pq)
        }
        1 => {
            let idx = read_int(data, &mut pos, 8)? as u8;
            let tld = FIXED_TLDS
                .iter()
                .find(|&&(_, i)| i == idx)
                .map(|&(t, _)| t)
                .ok_or_else(|| Error::undecodable("App Clip fixed TLD index unknown"))?;
            let (domain, has_pq) = decode_host_chars(data, &mut pos);
            (format!("{domain}{tld}"), has_pq)
        }
        _ => decode_host_chars(data, &mut pos),
    };

    let mut url = String::from("https://");
    if subdomain {
        url.push_str("appclip.");
    }
    url.push_str(&host);

    if has_pq {
        if template {
            url.push_str(&decode_template_rest(data, &mut pos)?);
        } else if !read_bit(data, &mut pos)? {
            // Combined CPQ string.
            let mut path = cpq_coder().decode(data, &mut pos, None, "");
            if !path.is_empty() && !path.starts_with('/') && !path.starts_with('#') {
                path.insert(0, '/');
            }
            url.push_str(&path);
        } else {
            url.push_str(&decode_segmented(data, &mut pos)?);
        }
    }
    Ok(url)
}

/// Host characters until the `|` terminator; returns (host, saw_terminator).
fn decode_host_chars(data: &[bool], pos: &mut usize) -> (String, bool) {
    let mut s = host_coder().decode(data, pos, Some('|'), "");
    let has_pq = s.ends_with('|');
    if has_pq {
        s.pop();
    }
    (s, has_pq)
}

fn decode_template_rest(data: &[bool], pos: &mut usize) -> Result<String> {
    if *pos >= data.len() {
        return Ok(String::new());
    }
    let mut out = String::new();
    let first = read_bit(data, pos)?;
    if !first {
        let idx = read_int(data, pos, 8)?;
        let word = KNOWN_WORDS
            .get(idx)
            .ok_or_else(|| Error::undecodable("App Clip template word index unknown"))?;
        out.push('/');
        out.push_str(word);
        if *pos >= data.len() {
            return Ok(out);
        }
        if !read_bit(data, pos)? {
            return Err(Error::undecodable(
                "App Clip template query marker expected",
            ));
        }
    }
    if *pos >= data.len() {
        return Ok(out);
    }
    out.push('?');
    let mut i = 0usize;
    while *pos < data.len() {
        if i > 0 {
            out.push('&');
        }
        out.push('p');
        if i > 0 {
            out.push_str(&i.to_string());
        }
        out.push('=');
        let value = match read_int(data, pos, 2)? {
            0 => spq_until_terminator(data, pos, "="),
            1 => decode_uleb128(data, pos)?,
            2 => decode_fixed6(data, pos)?,
            _ => {
                return Err(Error::undecodable(
                    "App Clip template component type invalid",
                ));
            }
        };
        out.push_str(&value);
        i += 1;
    }
    Ok(out)
}

fn spq_until_terminator(data: &[bool], pos: &mut usize, start_ctx: &str) -> String {
    let mut s = spq_coder().decode(data, pos, Some('|'), start_ctx);
    if s.ends_with('|') {
        s.pop();
    }
    s
}

fn decode_segmented(data: &[bool], pos: &mut usize) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut root_only = false;
    let mut trailing_slash = false;

    while *pos < data.len() {
        if read_bit(data, pos)? {
            if !read_bit(data, pos)? {
                if parts.is_empty() {
                    root_only = true;
                }
                trailing_slash = true;
                continue;
            }
            let query = decode_segmented_query(data, pos)?;
            let mut path = build_segmented_path(&parts, root_only, trailing_slash);
            if path.is_empty() {
                path.push('/');
            }
            return Ok(path + &query);
        }
        parts.push(decode_segmented_component(data, pos)?);
        root_only = false;
        trailing_slash = false;
    }
    Ok(build_segmented_path(&parts, root_only, trailing_slash))
}

fn build_segmented_path(parts: &[String], root_only: bool, trailing_slash: bool) -> String {
    if parts.is_empty() {
        return if root_only { "/".to_string() } else { String::new() };
    }
    let mut path = format!("/{}", parts.join("/"));
    if trailing_slash {
        path.push('/');
    }
    path
}

fn decode_segmented_component(data: &[bool], pos: &mut usize) -> Result<String> {
    match read_int(data, pos, 2)? {
        0 => Ok(spq_until_terminator(data, pos, "")),
        1 => decode_uleb128(data, pos),
        2 => decode_fixed6(data, pos),
        _ => {
            let idx = read_int(data, pos, 8)?;
            KNOWN_WORDS
                .get(idx)
                .map(|w| w.to_string())
                .ok_or_else(|| Error::undecodable("App Clip path word index unknown"))
        }
    }
}

fn decode_segmented_query(data: &[bool], pos: &mut usize) -> Result<String> {
    let mut out = String::from("?");
    let mut first = true;
    while *pos < data.len() {
        let (key, value) = match read_int(data, pos, 2)? {
            0 => {
                let key = spq_until_terminator(data, pos, "?");
                let value = spq_until_terminator(data, pos, "=");
                (key, value)
            }
            1 => {
                let value = decode_uleb128(data, pos)?;
                let key = spq_until_terminator(data, pos, "?");
                (key, value)
            }
            2 => {
                let key = spq_until_terminator(data, pos, "?");
                let value = decode_fixed6(data, pos)?;
                (key, value)
            }
            _ => {
                return Err(Error::undecodable(
                    "App Clip segmented query type invalid",
                ));
            }
        };
        if !first {
            out.push('&');
        }
        first = false;
        out.push_str(&key);
        out.push('=');
        out.push_str(&value);
    }
    Ok(out)
}

fn decode_fixed6(data: &[bool], pos: &mut usize) -> Result<String> {
    let mut out = String::new();
    while *pos + 6 <= data.len() {
        let idx = read_int(data, pos, 6)?;
        let c = FIXED6_ALPHABET[idx];
        if c == b'|' {
            break;
        }
        out.push(c as char);
    }
    Ok(out)
}
