//! `prowl` library surface.
//!
//! Exists so integration tests, benchmarks, and `examples/` can exercise
//! the same modules the binary uses (`process::collect_tree`, etc.) without
//! duplicating code.  The binary entry point in `src/main.rs` consumes
//! these modules via `use prowl::*;`.

pub mod app;
pub mod collector;
pub mod format;
pub mod picker;
pub mod process;
pub mod tree;
pub mod ui;

#[cfg(test)]
pub mod test_support;
