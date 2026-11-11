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

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;

    #[test]
    fn constructors_build_the_matching_variant() {
        assert!(matches!(Error::capacity("a"), Error::Capacity { detail } if detail == "a"));
        assert!(matches!(Error::invalid_data("b"), Error::InvalidData { detail } if detail == "b"));
        assert!(matches!(Error::undecodable("c"), Error::Undecodable { detail } if detail == "c"));
        assert!(matches!(
            Error::invalid_parameter("d"),
            Error::InvalidParameter { detail } if detail == "d"
        ));
    }

    /// With `alloc`, the constructors take anything displayable, not just `&str`.
    #[cfg(feature = "alloc")]
    #[test]
    fn constructors_accept_any_display() {
        assert_eq!(
            Error::capacity(format_args!("{} > {}", 9, 7)),
            Error::Capacity {
                detail: "9 > 7".into()
            }
        );
        assert_eq!(Error::invalid_data(42), Error::invalid_data("42"));
        assert_ne!(Error::invalid_data("x"), Error::undecodable("x"));
    }

    #[test]
    fn display_names_the_failure_and_its_detail() {
        let cases = [
            (Error::Unsupported { what: "Code 1" }, "unsupported: Code 1"),
            (Error::capacity("too long"), "capacity exceeded: too long"),
            (Error::invalid_data("bad byte"), "invalid data: bad byte"),
            (
                Error::undecodable("no finder"),
                "undecodable symbol: no finder",
            ),
            (Error::ErrorCorrectionFailed, "error correction failed"),
            (Error::invalid_parameter("ec"), "invalid parameter: ec"),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
        // `InvalidFrame` has no constructor helper; `GrayFrame` builds it directly.
        let frame = crate::GrayFrame::new(&[], 0, 0).unwrap_err();
        assert_eq!(frame.to_string(), "invalid frame: zero-sized frame");
    }

    #[test]
    fn is_a_standard_error() {
        fn source_of(e: &dyn core::error::Error) -> bool {
            e.source().is_some()
        }
        assert!(!source_of(&Error::ErrorCorrectionFailed));
        let boxed: std::boxed::Box<dyn std::error::Error + Send + Sync> =
            std::boxed::Box::new(Error::capacity("x"));
        assert_eq!(boxed.to_string(), "capacity exceeded: x");
    }
}
