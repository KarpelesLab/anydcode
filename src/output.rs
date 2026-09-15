//! Abstract encoder output: the geometry of a symbol independent of pixels.
//!
//! Encoders produce *modules* (the smallest bar or cell), not rendered images.
//! Rendering modules to PNG/SVG/pixels is a separate, caller-owned concern; keeping
//! the encoder output abstract means the same result can be rasterized at any scale
//! or fed straight back into a decoder for round-trip verification.
//!
//! Two families of output types exist:
//!
//! - **Owned** ([`BitMatrix`], [`LinearPattern`], [`Encoding`]; `alloc` feature) —
//!   what the [`Encode`](crate::traits::Encode) trait returns.
//! - **Heap-free** ([`MatrixBuf`], [`LinearBuf`], [`LinearSink`]; always available) —
//!   what the `encode_into` encoders write, over storage the caller provides. A
//!   [`LinearSink`] can also stream modules somewhere without buffering them at all.

use core::fmt;

#[cfg(feature = "alloc")]
use alloc::{vec, vec::Vec};

use crate::error::{Error, Result};

/// A 2D grid of dark/light modules for matrix and stacked symbologies.
///
/// `true` means a dark module. The grid excludes the quiet zone; [`BitMatrix::quiet_zone`]
/// records how many light modules of margin the symbology requires around it.
#[cfg(feature = "alloc")]
#[derive(Clone, PartialEq, Eq)]
pub struct BitMatrix {
    width: usize,
    height: usize,
    bits: Vec<bool>,
    /// Required light-module margin on every side.
    pub quiet_zone: usize,
}

#[cfg(feature = "alloc")]
impl BitMatrix {
    /// A new all-light matrix of the given size.
    pub fn new(width: usize, height: usize, quiet_zone: usize) -> Self {
        BitMatrix {
            width,
            height,
            bits: vec![false; width * height],
            quiet_zone,
        }
    }

    /// Grid width in modules (excluding quiet zone).
    pub fn width(&self) -> usize {
        self.width
    }

    /// Grid height in modules (excluding quiet zone).
    pub fn height(&self) -> usize {
        self.height
    }

    /// Module at `(x, y)`; `false` (light) if out of bounds.
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.bits[y * self.width + x]
    }

    /// Set the module at `(x, y)`.
    ///
    /// # Panics
    /// Panics if the coordinates are out of bounds.
    pub fn set(&mut self, x: usize, y: usize, dark: bool) {
        assert!(
            x < self.width && y < self.height,
            "BitMatrix::set out of bounds"
        );
        self.bits[y * self.width + x] = dark;
    }
}

#[cfg(feature = "alloc")]
impl fmt::Debug for BitMatrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "BitMatrix {}x{} (quiet_zone {}):",
            self.width, self.height, self.quiet_zone
        )?;
        for y in 0..self.height {
            for x in 0..self.width {
                f.write_str(if self.get(x, y) { "██" } else { "  " })?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

#[cfg(feature = "alloc")]
impl From<&MatrixBuf<'_>> for BitMatrix {
    fn from(buf: &MatrixBuf<'_>) -> Self {
        let mut m = BitMatrix::new(buf.width, buf.height, buf.quiet_zone);
        for y in 0..buf.height {
            for x in 0..buf.width {
                if buf.get(x, y) {
                    m.set(x, y, true);
                }
            }
        }
        m
    }
}

/// A linear (1D) module pattern: a single row of dark/light modules.
///
/// Each element is one narrow module; `true` is a bar, `false` a space. Wider bars
/// are runs of consecutive `true`s. The quiet zone (in modules) frames both ends.
#[cfg(feature = "alloc")]
#[derive(Clone, PartialEq, Eq)]
pub struct LinearPattern {
    /// One entry per narrow module across the code, `true` = bar.
    pub modules: Vec<bool>,
    /// Required light-module margin on the left and right.
    pub quiet_zone: usize,
}

#[cfg(feature = "alloc")]
impl LinearPattern {
    /// An empty pattern, ready to be filled through [`LinearSink`].
    pub fn new() -> Self {
        LinearPattern {
            modules: Vec::new(),
            quiet_zone: 0,
        }
    }
}

#[cfg(feature = "alloc")]
impl Default for LinearPattern {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "alloc")]
impl fmt::Debug for LinearPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "LinearPattern[{} modules, quiet_zone {}] ",
            self.modules.len(),
            self.quiet_zone
        )?;
        for &b in &self.modules {
            f.write_str(if b { "█" } else { " " })?;
        }
        Ok(())
    }
}

#[cfg(feature = "alloc")]
impl LinearSink for LinearPattern {
    fn begin(&mut self, quiet_zone: usize) -> Result<()> {
        self.modules.clear();
        self.quiet_zone = quiet_zone;
        Ok(())
    }

    fn push_run(&mut self, bar: bool, count: usize) -> Result<()> {
        self.modules.extend(core::iter::repeat_n(bar, count));
        Ok(())
    }
}

#[cfg(feature = "alloc")]
impl From<&LinearBuf<'_>> for LinearPattern {
    fn from(buf: &LinearBuf<'_>) -> Self {
        LinearPattern {
            modules: buf.iter().collect(),
            quiet_zone: buf.quiet_zone,
        }
    }
}

/// The output of encoding a [`crate::Symbol`]: matrix geometry for 2D codes, a linear
/// pattern for 1D codes.
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoding {
    /// A 2D module grid.
    Matrix(BitMatrix),
    /// A 1D module row.
    Linear(LinearPattern),
}

/// Destination for the modules of a linear (1D) symbol, written left to right.
///
/// The heap-free linear encoders (`encode_into`) are generic over this trait, so the
/// same encoder can fill a caller-provided [`LinearBuf`], an owned [`LinearPattern`]
/// (with `alloc`), or stream runs straight to a printer or display driver.
pub trait LinearSink {
    /// Called once before the first module with the light margin, in modules, the
    /// symbology requires on both sides. Implementations should reset any previous
    /// content here.
    fn begin(&mut self, quiet_zone: usize) -> Result<()> {
        let _ = quiet_zone;
        Ok(())
    }

    /// Append `count` modules of one color (`true` = bar).
    fn push_run(&mut self, bar: bool, count: usize) -> Result<()>;

    /// Append a single module (`true` = bar).
    fn push(&mut self, bar: bool) -> Result<()> {
        self.push_run(bar, 1)
    }
}

impl<S: LinearSink + ?Sized> LinearSink for &mut S {
    fn begin(&mut self, quiet_zone: usize) -> Result<()> {
        (**self).begin(quiet_zone)
    }

    fn push_run(&mut self, bar: bool, count: usize) -> Result<()> {
        (**self).push_run(bar, count)
    }
}

/// A linear module row stored in caller-provided memory, one bit per module
/// (LSB-first within each byte, `1` = bar). The heap-free counterpart of
/// [`LinearPattern`].
///
/// Pushing past the end of the storage fails with [`Error::Capacity`].
///
/// ```
/// use anyd::output::{LinearBuf, LinearSink};
///
/// let mut storage = [0u8; 2];
/// let mut row = LinearBuf::new(&mut storage);
/// row.begin(10).unwrap();
/// row.push_run(true, 3).unwrap();
/// row.push(false).unwrap();
/// assert_eq!(row.len(), 4);
/// assert!(row.get(2) && !row.get(3));
/// assert_eq!(row.quiet_zone, 10);
/// ```
pub struct LinearBuf<'a> {
    bits: &'a mut [u8],
    len: usize,
    /// Required light-module margin on the left and right (set by the encoder).
    pub quiet_zone: usize,
}

impl<'a> LinearBuf<'a> {
    /// An empty row over `storage`, which holds up to `storage.len() * 8` modules.
    pub fn new(storage: &'a mut [u8]) -> Self {
        LinearBuf {
            bits: storage,
            len: 0,
            quiet_zone: 0,
        }
    }

    /// Bytes of storage needed for `modules` modules.
    pub const fn bytes_for(modules: usize) -> usize {
        modules.div_ceil(8)
    }

    /// Number of modules written.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no module has been written.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Maximum number of modules the storage can hold.
    pub fn capacity(&self) -> usize {
        self.bits.len() * 8
    }

    /// Module `i` (`true` = bar); `false` past the end.
    pub fn get(&self, i: usize) -> bool {
        i < self.len && self.bits[i / 8] & (1 << (i % 8)) != 0
    }

    /// Iterate the written modules left to right.
    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.len).map(|i| self.get(i))
    }

    /// Forget all modules (the storage is reused).
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl fmt::Debug for LinearBuf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "LinearBuf[{} modules, quiet_zone {}] ",
            self.len, self.quiet_zone
        )?;
        for b in self.iter() {
            f.write_str(if b { "█" } else { " " })?;
        }
        Ok(())
    }
}

impl LinearSink for LinearBuf<'_> {
    fn begin(&mut self, quiet_zone: usize) -> Result<()> {
        self.len = 0;
        self.quiet_zone = quiet_zone;
        Ok(())
    }

    fn push_run(&mut self, bar: bool, count: usize) -> Result<()> {
        if count > self.capacity() - self.len {
            return Err(Error::capacity("LinearBuf storage too small"));
        }
        for i in self.len..self.len + count {
            let (byte, mask) = (i / 8, 1u8 << (i % 8));
            if bar {
                self.bits[byte] |= mask;
            } else {
                self.bits[byte] &= !mask;
            }
        }
        self.len += count;
        Ok(())
    }
}

/// A 2D module grid stored in caller-provided memory, one bit per module
/// (row-major, LSB-first within each byte, `1` = dark). The heap-free counterpart
/// of [`BitMatrix`].
///
/// ```
/// use anyd::output::MatrixBuf;
///
/// let mut storage = [0u8; MatrixBuf::bytes_for(21, 21)];
/// let mut grid = MatrixBuf::new(&mut storage, 21, 21, 4).unwrap();
/// grid.set(3, 4, true);
/// assert!(grid.get(3, 4) && !grid.get(4, 3));
/// ```
pub struct MatrixBuf<'a> {
    width: usize,
    height: usize,
    bits: &'a mut [u8],
    /// Required light-module margin on every side.
    pub quiet_zone: usize,
}

impl<'a> MatrixBuf<'a> {
    /// Bytes of storage needed for a `width × height` grid.
    pub const fn bytes_for(width: usize, height: usize) -> usize {
        (width * height).div_ceil(8)
    }

    /// An all-light `width × height` grid over `storage`.
    ///
    /// # Errors
    /// [`Error::Capacity`] if `storage` is shorter than [`MatrixBuf::bytes_for`].
    pub fn new(
        storage: &'a mut [u8],
        width: usize,
        height: usize,
        quiet_zone: usize,
    ) -> Result<Self> {
        let needed = Self::bytes_for(width, height);
        if storage.len() < needed {
            return Err(Error::capacity("MatrixBuf storage too small"));
        }
        let bits = &mut storage[..needed];
        bits.fill(0);
        Ok(MatrixBuf {
            width,
            height,
            bits,
            quiet_zone,
        })
    }

    /// Grid width in modules (excluding quiet zone).
    pub fn width(&self) -> usize {
        self.width
    }

    /// Grid height in modules (excluding quiet zone).
    pub fn height(&self) -> usize {
        self.height
    }

    /// Module at `(x, y)`; `false` (light) if out of bounds.
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let i = y * self.width + x;
        self.bits[i / 8] & (1 << (i % 8)) != 0
    }

    /// Set the module at `(x, y)`.
    ///
    /// # Panics
    /// Panics if the coordinates are out of bounds.
    pub fn set(&mut self, x: usize, y: usize, dark: bool) {
        assert!(
            x < self.width && y < self.height,
            "MatrixBuf::set out of bounds"
        );
        let i = y * self.width + x;
        let mask = 1u8 << (i % 8);
        if dark {
            self.bits[i / 8] |= mask;
        } else {
            self.bits[i / 8] &= !mask;
        }
    }

    /// Flip the module at `(x, y)`.
    ///
    /// # Panics
    /// Panics if the coordinates are out of bounds.
    pub fn toggle(&mut self, x: usize, y: usize) {
        assert!(
            x < self.width && y < self.height,
            "MatrixBuf::toggle out of bounds"
        );
        let i = y * self.width + x;
        self.bits[i / 8] ^= 1 << (i % 8);
    }

    /// Number of dark modules.
    pub fn count_dark(&self) -> usize {
        // Bits past `width * height` in the last byte are never set.
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }

    /// The packed module bits (`bytes_for(width, height)` bytes).
    pub fn as_bytes(&self) -> &[u8] {
        self.bits
    }
}

impl fmt::Debug for MatrixBuf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "MatrixBuf {}x{} (quiet_zone {}):",
            self.width, self.height, self.quiet_zone
        )?;
        for y in 0..self.height {
            for x in 0..self.width {
                f.write_str(if self.get(x, y) { "██" } else { "  " })?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

#[cfg(feature = "alloc")]
impl PartialEq<BitMatrix> for MatrixBuf<'_> {
    fn eq(&self, other: &BitMatrix) -> bool {
        self.width == other.width()
            && self.height == other.height()
            && self.quiet_zone == other.quiet_zone
            && (0..self.height).all(|y| (0..self.width).all(|x| self.get(x, y) == other.get(x, y)))
    }
}

#[cfg(feature = "alloc")]
impl PartialEq<LinearPattern> for LinearBuf<'_> {
    fn eq(&self, other: &LinearPattern) -> bool {
        self.quiet_zone == other.quiet_zone && self.iter().eq(other.modules.iter().copied())
    }
}
