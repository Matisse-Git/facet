//! Windows integration: Mica backdrop, custom title bar with native caption buttons, window
//! placement, native folder picker, shell hand-offs.
//!
//! Everything OS-specific lives behind this module so the rest of the app stays plain egui.

mod windows;

pub use self::windows::*;
