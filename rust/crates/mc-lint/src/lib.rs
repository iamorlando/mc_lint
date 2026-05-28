//! Marker attributes for Monte Carlo Linter.
//!
//! Downstream crates should depend on `mc-lint` directly and import marker
//! attributes from this crate:
//!
//! ```rust
//! use mc_lint::hot_path;
//!
//! #[hot_path]
//! fn evolve(input: &[f64]) -> f64 {
//!     input.iter().copied().sum()
//! }
//! ```

pub use mc_lint_attr::{hot_path, hot_path_boundary};
