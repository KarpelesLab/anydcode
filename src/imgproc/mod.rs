//! Symbology-agnostic image-processing toolkit shared by 2D barcode samplers.
//!
//! A per-symbology *sampler* turns a [`GrayFrame`] containing a matrix code into a
//! clean [`BitMatrix`] for the symbology's decoder. This module provides the shared
//! primitives every such sampler needs, none of them tied to a particular code:
//!
//! - [`binary`] — an owned [`BinaryImage`] (dark = `true`) with accessors.
//! - [`integral`] — an [`IntegralImage`] for O(1) box sums (used by adaptive
//!   thresholding and blob analysis).
//! - [`threshold`] — global [`otsu_threshold`] and adaptive local binarization
//!   ([`adaptive_binarize_bradley`], [`adaptive_binarize_sauvola`]) robust to
//!   uneven lighting.
//! - [`components`] — [`connected_components`] labeling (4/8-connectivity) returning
//!   per-blob bounding boxes, centroids and areas.
//! - [`homography`] — a 3×3 [`Homography`] solved from four point correspondences,
//!   with forward mapping and inversion.
//! - [`line`] — least-squares line fitting and a seeded [`ransac_line`] fitter for
//!   locating code borders.
//! - [`sample`] — [`sample_grid`], the sub-pixel bilinear grid reader that turns a
//!   warped frame plus a homography into a [`BitMatrix`].
//! - [`edges`] — Sobel gradient magnitude and binary morphology (erode/dilate).
//! - [`rng`] — a small explicit seeded [`Prng`] so any randomness is reproducible.
//!
//! Everything here is deterministic and dependency-free.
//!
//! [`GrayFrame`]: crate::image::GrayFrame
//! [`BitMatrix`]: crate::output::BitMatrix

pub mod binary;
pub mod components;
pub mod edges;
pub mod homography;
pub mod integral;
pub mod line;
pub mod rng;
pub mod sample;
pub mod threshold;

pub use binary::BinaryImage;
pub use components::{BoundingBox, Component, Connectivity, connected_components};
pub use edges::{dilate, erode, sobel_magnitude};
pub use homography::Homography;
pub use integral::IntegralImage;
pub use line::{Line, fit_line_least_squares, ransac_line};
pub use rng::Prng;
pub use sample::{sample_bilinear, sample_grid, sample_grid_binary};
pub use threshold::{
    adaptive_binarize_bradley, adaptive_binarize_sauvola, otsu_binarize, otsu_threshold,
};
