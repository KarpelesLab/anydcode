//! Error and result types shared across the crate.

use core::fmt;

/// The human-readable detail carried by most [`Error`] variants: an owned
/// `String` when the `alloc` feature is enabled, a `&'static str` otherwise (the
/// heap-free encoders only ever report fixed messages).
#[cfg(feature = "alloc")]
pub type Detail = alloc::string::String;
/// The human-readable detail carried by most [`Error`] variants: an owned
/// `String` when the `alloc` feature is enabled, a `&'static str` otherwise (the
/// heap-free encoders only ever report fixed messages).
#[cfg(not(feature = "alloc"))]
pub type Detail = &'static str;

/// Convenience alias for results produced by this crate.
pub type Result<T> = core::result::Result<T, Error>;

/// The unified error type for encoding, decoding and detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The requested symbology exists in the model but has no implementation yet.
    Unsupported {
        /// Human-readable description of what is not supported.
        what: &'static str,
    },
    /// Input data does not fit the constraints of the target symbology
    /// (too long, wrong character set, out-of-range value, ...).
    Capacity {
        /// Description of the limit that was exceeded.
        detail: Detail,
    },
    /// The data contains characters/bytes the requested encoding mode cannot represent.
    InvalidData {
        /// Description of what was invalid.
        detail: Detail,
    },
    /// A symbol was located but could not be decoded (corrupt, low quality, ...).
    Undecodable {
        /// Description of the failure.
        detail: Detail,
    },
    /// Error correction detected more errors than it could repair.
    ErrorCorrectionFailed,
    /// The supplied image/frame buffer is malformed (bad stride, zero size, ...).
    InvalidFrame {
        /// Description of the problem.
        detail: Detail,
    },
    /// A caller passed parameters that are internally inconsistent.
    InvalidParameter {
        /// Description of the problem.
        detail: Detail,
    },
}

#[cfg(feature = "alloc")]
impl Error {
    /// Build an [`Error::Capacity`] from anything displayable.
    pub fn capacity(detail: impl fmt::Display) -> Self {
        use alloc::string::ToString;
        Error::Capacity {
            detail: detail.to_string(),
        }
    }

    /// Build an [`Error::InvalidData`] from anything displayable.
    pub fn invalid_data(detail: impl fmt::Display) -> Self {
        use alloc::string::ToString;
        Error::InvalidData {
            detail: detail.to_string(),
        }
    }

    /// Build an [`Error::Undecodable`] from anything displayable.
    pub fn undecodable(detail: impl fmt::Display) -> Self {
        use alloc::string::ToString;
        Error::Undecodable {
            detail: detail.to_string(),
        }
    }

    /// Build an [`Error::InvalidParameter`] from anything displayable.
    pub fn invalid_parameter(detail: impl fmt::Display) -> Self {
        use alloc::string::ToString;
        Error::InvalidParameter {
            detail: detail.to_string(),
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl Error {
    /// Build an [`Error::Capacity`] from a fixed message.
    pub const fn capacity(detail: &'static str) -> Self {
        Error::Capacity { detail }
    }

    /// Build an [`Error::InvalidData`] from a fixed message.
    pub const fn invalid_data(detail: &'static str) -> Self {
        Error::InvalidData { detail }
    }

    /// Build an [`Error::Undecodable`] from a fixed message.
    pub const fn undecodable(detail: &'static str) -> Self {
        Error::Undecodable { detail }
    }

    /// Build an [`Error::InvalidParameter`] from a fixed message.
    pub const fn invalid_parameter(detail: &'static str) -> Self {
        Error::InvalidParameter { detail }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported { what } => write!(f, "unsupported: {what}"),
            Error::Capacity { detail } => write!(f, "capacity exceeded: {detail}"),
            Error::InvalidData { detail } => write!(f, "invalid data: {detail}"),
            Error::Undecodable { detail } => write!(f, "undecodable symbol: {detail}"),
            Error::ErrorCorrectionFailed => write!(f, "error correction failed"),
            Error::InvalidFrame { detail } => write!(f, "invalid frame: {detail}"),
            Error::InvalidParameter { detail } => write!(f, "invalid parameter: {detail}"),
        }
    }
}

impl core::error::Error for Error {}
