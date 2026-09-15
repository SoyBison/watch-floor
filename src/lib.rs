//! watch-floor — live surveillance of a Python package, comparing the working
//! tree against the baseline committed at git HEAD.
//!
//! The binary is a thin shell around this library: [`app::App`] holds the two
//! snapshots and the compiled views, [`floor`] draws them, and [`report`]
//! renders the same picture as plain text.

pub mod app;
pub mod collection;
pub mod dossier;
pub mod floor;
pub mod intercept;
pub mod manual;
pub mod network;
pub mod report;
pub mod sitrep;
pub mod structure;
pub mod tripwire;
