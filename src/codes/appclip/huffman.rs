//! Huffman coding over Apple's trained frequency tries.
//!
//! The URL codec drives three *multi-context* Huffman coders (host, segmented
//! path/query, combined path/query): the code for each character depends on the two
//! characters before it, via a depth-2 k-ary trie of frequency tables extracted from
//! Apple's `URLCompression.framework`. Each trie node holds `k` big-endian `u16`
//! frequencies; the Huffman tree built from a node's table encodes the next symbol.
//!
//! Tree construction must match Apple's `UCHuffmanCoder` bit-for-bit, including its
//! tie-break: when two subtrees tie on frequency, the one whose *leftmost leaf* has
//! the lexicographically smaller symbol is popped first, and the first-popped subtree
//! becomes the left ('0') child.

use std::collections::HashMap;

/// One Huffman code table: symbol index → bit string of '0'/'1'.
pub(super) struct HuffmanCoder {
    codes: Vec<String>,
}

/// A subtree during construction, keeping only what ordering and code assignment
/// need: total frequency, the leftmost leaf's symbol, and the leaf indices in
/// left-to-right order paired with their depth increments.
struct Node {
    freq: u32,
    /// Symbol of the leftmost leaf (Go walks preferring left children, so a merged
    /// node inherits its left child's leftmost symbol).
    leftmost: &'static str,
    /// Leaves of this subtree as `(symbol_index, code_so_far)` — codes grow from the
    /// leaf side as subtrees merge, by prefixing '0' (left) or '1' (right).
    leaves: Vec<(usize, String)>,
}

impl HuffmanCoder {
    /// Build the coder for `freqs` with Apple's construction and tie-breaks.
    /// `symbols[i]` names symbol `i` (used only for tie-breaking).
    pub fn new(freqs: &[u16], symbols: &[&'static str]) -> HuffmanCoder {
        let mut codes = vec![String::new(); freqs.len()];

        let mut pool: Vec<Node> = freqs
            .iter()
            .enumerate()
            .filter(|&(_, &f)| f > 0)
            .map(|(i, &f)| Node {
                freq: u32::from(f),
                leftmost: symbols[i],
                leaves: vec![(i, String::new())],
            })
            .collect();

        if pool.is_empty() {
            return HuffmanCoder { codes };
        }
        if pool.len() == 1 {
            codes[pool[0].leaves[0].0] = "0".to_string();
            return HuffmanCoder { codes };
        }

        // Repeatedly merge the two minimal nodes. The pool stays small (≤ 75), so a
        // linear scan per merge is plenty and sidesteps heap-ordering subtleties.
        while pool.len() > 1 {
            let a = pop_min(&mut pool);
            let b = pop_min(&mut pool);
            let mut leaves = Vec::with_capacity(a.leaves.len() + b.leaves.len());
            for (i, code) in a.leaves {
                let mut c = String::with_capacity(code.len() + 1);
                c.push('0');
                c.push_str(&code);
                leaves.push((i, c));
            }
            for (i, code) in b.leaves {
                let mut c = String::with_capacity(code.len() + 1);
                c.push('1');
                c.push_str(&code);
                leaves.push((i, c));
            }
            pool.push(Node {
                freq: a.freq + b.freq,
                leftmost: a.leftmost,
                leaves,
            });
        }

        for (i, code) in pool.pop().unwrap().leaves {
            codes[i] = code;
        }
        HuffmanCoder { codes }
    }

    pub fn encode(&self, symbol_index: usize) -> &str {
        &self.codes[symbol_index]
    }

    pub fn can_encode(&self, symbol_index: usize) -> bool {
        symbol_index < self.codes.len() && !self.codes[symbol_index].is_empty()
    }

    /// Match the next code in `data` at `*pos`, returning the symbol index and
    /// advancing `*pos` past it.
    pub fn decode(&self, data: &[bool], pos: &mut usize) -> Option<usize> {
        for (i, code) in self.codes.iter().enumerate() {
            if code.is_empty() || *pos + code.len() > data.len() {
                continue;
            }
            if code
                .bytes()
                .zip(&data[*pos..])
                .all(|(c, &bit)| (c == b'1') == bit)
            {
                *pos += code.len();
                return Some(i);
            }
        }
        None
    }
}

/// Remove and return the minimal node: lowest frequency, ties broken by the
/// lexicographically smaller leftmost-leaf symbol.
fn pop_min(pool: &mut Vec<Node>) -> Node {
    let mut best = 0;
    for i in 1..pool.len() {
        let (a, b) = (&pool[i], &pool[best]);
        if a.freq < b.freq || (a.freq == b.freq && a.leftmost < b.leftmost) {
            best = i;
        }
    }
    pool.swap_remove(best)
}

/// A depth-2 k-ary frequency trie over `symbols`, stored as the raw big-endian `u16`
/// table dump from Apple's framework.
pub(super) struct Trie {
    data: &'static [u8],
    pub symbols: &'static [&'static str],
    k: usize,
    max_depth: usize,
}

impl Trie {
    /// Wrap a raw table. Panics if the byte length disagrees with the alphabet — the
    /// tables are compiled in, so a mismatch is a build error, not a runtime case.
    pub fn new(data: &'static [u8], symbols: &'static [&'static str]) -> Trie {
        let k = symbols.len();
        let expected = (1 + k + k * k) * k * 2;
        assert_eq!(
            data.len(),
            expected,
            "trie table size mismatch for k={k}: expected {expected}, got {}",
            data.len()
        );
        Trie {
            data,
            symbols,
            k,
            max_depth: 2,
        }
    }

    fn frequencies(&self, node: usize) -> Vec<u16> {
        let base = node * self.k * 2;
        (0..self.k)
            .map(|i| u16::from_be_bytes([self.data[base + i * 2], self.data[base + i * 2 + 1]]))
            .collect()
    }

    fn child(&self, parent: usize, symbol_index: usize) -> usize {
        self.k * parent + 1 + symbol_index
    }
}

/// A multi-context coder: the Huffman table for the next symbol is chosen by the trie
/// node reached through the previous (up to two) symbols.
pub(super) struct MultiCoder {
    trie: Trie,
    index: HashMap<char, usize>,
    cache: std::sync::Mutex<HashMap<usize, std::sync::Arc<HuffmanCoder>>>,
}

impl MultiCoder {
    pub fn new(trie: Trie) -> MultiCoder {
        let index = trie
            .symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (s.chars().next().unwrap(), i))
            .collect();
        MultiCoder {
            trie,
            index,
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn symbol_index(&self, c: char) -> Option<usize> {
        self.index.get(&c).copied()
    }

    fn coder(&self, node: usize) -> std::sync::Arc<HuffmanCoder> {
        let mut cache = self.cache.lock().expect("coder cache poisoned");
        cache
            .entry(node)
            .or_insert_with(|| {
                std::sync::Arc::new(HuffmanCoder::new(
                    &self.trie.frequencies(node),
                    self.trie.symbols,
                ))
            })
            .clone()
    }

    /// Sliding-window context advance: descend while shallower than the trie, then
    /// re-root on the depth-1 node of the previous symbol.
    fn advance(&self, node: usize, depth: usize, symbol_index: usize) -> (usize, usize) {
        if depth < self.trie.max_depth {
            (self.trie.child(node, symbol_index), depth + 1)
        } else {
            let prev = (node - 1) % self.trie.k;
            (self.trie.child(1 + prev, symbol_index), depth)
        }
    }

    /// Encode `text` (starting from the context primed with `start_ctx`) to a bit
    /// string, or `None` if any character is outside the alphabet or has no code in
    /// its context.
    pub fn encode(&self, text: &str, start_ctx: &str) -> Option<String> {
        let (mut node, mut depth) = (0usize, 0usize);
        for c in start_ctx.chars() {
            let idx = self.symbol_index(c)?;
            (node, depth) = self.advance(node, depth, idx);
        }
        let mut bits = String::new();
        for c in text.chars() {
            let idx = self.symbol_index(c)?;
            let hc = self.coder(node);
            if !hc.can_encode(idx) {
                return None;
            }
            bits.push_str(hc.encode(idx));
            (node, depth) = self.advance(node, depth, idx);
        }
        Some(bits)
    }

    /// Decode symbols from `data` at `*pos` until `stop` is produced (included in the
    /// result), the bits run out, or no code matches.
    pub fn decode(&self, data: &[bool], pos: &mut usize, stop: Option<char>, start_ctx: &str) -> String {
        let (mut node, mut depth) = (0usize, 0usize);
        for c in start_ctx.chars() {
            let Some(idx) = self.symbol_index(c) else {
                return String::new();
            };
            (node, depth) = self.advance(node, depth, idx);
        }
        let mut out = String::new();
        while *pos < data.len() {
            let hc = self.coder(node);
            let Some(idx) = hc.decode(data, pos) else {
                break;
            };
            let sym = self.trie.symbols[idx];
            out.push_str(sym);
            if stop.is_some_and(|s| sym.starts_with(s)) {
                break;
            }
            (node, depth) = self.advance(node, depth, idx);
        }
        out
    }
}
