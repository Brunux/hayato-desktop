//! Hayato Drop (`hayato-drop`, `hayato menu drop [FILE]`): compatibility facade
//! delegating to [`super::share`].
//!
//! - `hayato-drop <file>` opens Hayato Share directly in LAN QR send mode.
//! - `hayato-drop` (no args) opens Hayato Share directly in Receive mode (`~/Downloads`).

use std::path::PathBuf;

use hayato_ui::Design;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub use super::share::server::{detect_lan_ip, generate_token, sanitize_filename};
use super::share::{SharePayload, SharePreset, open_with_preset, server::default_downloads_dir};

/// `hayato menu drop [FILE]` (`hayato-drop`): Send a file or Receive uploads over LAN.
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(arg) = args.first() {
        let path = PathBuf::from(arg);
        if !path.exists() {
            return Err(crate::Failure::new(
                "Hayato Drop",
                format!("Cannot read {}: path does not exist", path.display()),
            ));
        }
        open_with_preset(
            SharePayload::from_path(path),
            SharePreset::LanQr,
            terminal,
            design,
        )
    } else {
        open_with_preset(
            SharePayload::Receive {
                dest_dir: default_downloads_dir(),
            },
            SharePreset::Receive,
            terminal,
            design,
        )
    }
}
