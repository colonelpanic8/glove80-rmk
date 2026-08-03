//! The pure host-side model of managed Rynk runtime state: the TOML schema, its
//! validation, the conversions to and from the protocol's wire types, and the
//! difference report a host uses to decide what to write.
//!
//! Both the `glove80-control` CLI and the browser configurator work against the
//! same model, so it deliberately performs no I/O of any kind — no filesystem,
//! no transport, no `rynk::Client`. Everything here is a function of values the
//! caller already holds, which is what lets the crate compile to
//! `wasm32-unknown-unknown`.

pub mod color;
mod config;
pub mod keycodes;
mod moergo;
pub mod rynk_keycode;

pub use color::parse_color;
pub use config::*;
pub use moergo::*;
