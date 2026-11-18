//! The game scan, model and launch rules, with no UI toolkit in the dependency
//! tree.
//!
//! The library is a Ratatui screen in `lazyos-cli`; the GTK + WebKit binary
//! `lazyos-games` is only the host that plays one web game fullscreen. Both need
//! the same scan, the same ids and the same "return to the library" rule, so
//! those live here and neither UI owns them.

pub mod launch;
pub mod library;
pub mod paths;
pub mod platform;
