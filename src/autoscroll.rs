//! Middle-click scrolling.
//!
//! Views report their scroll areas; the controller applies changes after drawing.

#[path = "autoscroll/active.rs"]
mod platform;

pub use platform::{Autoscroll, Outcome, lyrics, playlist, row, show};
