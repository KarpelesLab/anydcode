//! Han Xin Code (ISO/IEC 20830) encoder and structural decoder.
//!
//! Layout mirrors the QR module ([`crate::codes::qr`]):
//! - [`gf`]      — GF(2^8)/GF(2^4) arithmetic and Reed–Solomon (own field, **not**
//!   shared with QR; Han Xin uses primitive `0x163` and first root `α^1`).
//! - `tables`    — per-version capacity and EC-block tables, finder patterns.
//! - `matrix`    — finder/function placement, the data path, masking and the
//!   function-information codeword with its GF(2^4) RS.
//! - `encode`    — [`Symbol`] → [`BitMatrix`].
//! - `decode`    — [`BitMatrix`] → [`Symbol`].
//!
//! ## Scope
//!
//! - **Versions 1–3** (23×23, 25×25, 27×27). These have no alignment/assistant
//!   patterns, so the layout is exactly four corner finders + function information.
//!   Versions 4–84 (which add the Annex A alignment grid) are **not** implemented.
//! - **Modes:** Numeric, Text (Han Xin sub-modes Text 1/Text 2, mapped to the
//!   crate's [`crate::Mode::Alphanumeric`]) and Binary/Byte. The GB 18030 /
//!   Chinese-region modes (0100/0101 region, 0110/0111 GB 18030) are **not**
//!   implemented — their tables are large; the four-mode bit framework is in place.
//! - **EC levels** L1–L4 and the four data masks (0–3), with the 12-bit function
//!   information protected by GF(2^4) Reed–Solomon, per the standard.
//!
//! The RS field, generator convention, finder patterns, masking, picket-fence
//! interleave and function-information encoding are all mirrored from zint's
//! `backend/hanxin.c`; see the individual submodules for citations.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
pub mod gf;
mod matrix;
mod tables;

pub use decode::HanXinDecoder;
pub use encode::HanXinEncoder;

/// Han Xin error-correction level, L1 (lowest) to L4 (highest recovery capacity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EcLevel {
    /// Level 1 (~8% recovery).
    L1,
    /// Level 2 (~15% recovery).
    L2,
    /// Level 3 (~23% recovery).
    L3,
    /// Level 4 (~30% recovery).
    L4,
}

impl EcLevel {
    /// The 2-bit field value stored in the function information (`level − 1`).
    pub fn field_value(self) -> u8 {
        match self {
            EcLevel::L1 => 0,
            EcLevel::L2 => 1,
            EcLevel::L3 => 2,
            EcLevel::L4 => 3,
        }
    }

    /// Recover an EC level from its 2-bit field value.
    pub fn from_field_value(v: u8) -> Option<Self> {
        match v & 0x03 {
            0 => Some(EcLevel::L1),
            1 => Some(EcLevel::L2),
            2 => Some(EcLevel::L3),
            3 => Some(EcLevel::L4),
            _ => None,
        }
    }
}

/// A Han Xin symbol version. Supported range is **1–3** (see module docs); the module
/// grid is `23 + 2 * (version − 1)` on a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version(u8);

impl Version {
    /// Construct a version, validating the supported `1..=3` range.
    pub fn new(v: u8) -> Option<Self> {
        (1..=tables::MAX_VERSION).contains(&v).then_some(Version(v))
    }

    /// The version number.
    pub fn number(self) -> u8 {
        self.0
    }

    /// Side length of the module grid.
    pub fn size(self) -> usize {
        23 + 2 * (self.0 as usize - 1)
    }
}

/// The data mask pattern applied to the encoding region, 0–3 (0 is the null mask).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mask(u8);

impl Mask {
    /// Construct a mask, validating the `0..=3` range.
    pub fn new(m: u8) -> Option<Self> {
        (m <= 3).then_some(Mask(m))
    }

    /// The mask pattern index, 0–3.
    pub fn index(self) -> u8 {
        self.0
    }
}

/// Han Xin-specific parameters needed to re-encode a symbol identically.
///
/// The mode sequence itself lives in [`crate::Symbol::segments`] (as for QR); this
/// struct pins the geometry and coding choices. Together they reproduce the exact
/// module bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HanXinMeta {
    /// Symbol version (size), 1–3.
    pub version: Version,
    /// Error-correction level.
    pub ec_level: EcLevel,
    /// Applied data mask pattern.
    pub mask: Mask,
}

impl Default for HanXinMeta {
    fn default() -> Self {
        HanXinMeta {
            version: Version(1),
            ec_level: EcLevel::L1,
            mask: Mask(0),
        }
    }
}
