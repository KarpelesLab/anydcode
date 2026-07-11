//! Abstract encoder output: the geometry of a symbol independent of pixels.
//!
//! Encoders produce *modules* (the smallest bar or cell), not rendered images.
//! Rendering modules to PNG/SVG/pixels is a separate, caller-owned concern; keeping
//! the encoder output abstract means the same result can be rasterized at any scale
//! or fed straight back into a decoder for round-trip verification.

use core::fmt;

/// A 2D grid of dark/light modules for matrix and stacked symbologies.
///
/// `true` means a dark module. The grid excludes the quiet zone; [`BitMatrix::quiet_zone`]
/// records how many light modules of margin the symbology requires around it.
#[derive(Clone, PartialEq, Eq)]
pub struct BitMatrix {
    width: usize,
    height: usize,
    bits: Vec<bool>,
    /// Required light-module margin on every side.
    pub quiet_zone: usize,
}

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

/// A linear (1D) module pattern: a single row of dark/light modules.
///
/// Each element is one narrow module; `true` is a bar, `false` a space. Wider bars
/// are runs of consecutive `true`s. The quiet zone (in modules) frames both ends.
#[derive(Clone, PartialEq, Eq)]
pub struct LinearPattern {
    /// One entry per narrow module across the code, `true` = bar.
    pub modules: Vec<bool>,
    /// Required light-module margin on the left and right.
    pub quiet_zone: usize,
}

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

/// The output of encoding a [`crate::Symbol`]: matrix geometry for 2D codes, a linear
/// pattern for 1D codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoding {
    /// A 2D module grid.
    Matrix(BitMatrix),
    /// A 1D module row.
    Linear(LinearPattern),
}
