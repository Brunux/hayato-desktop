//! Enforces the Hayato architecture invariant: `hayato-cassette` screens draw
//! into `hayato_ui::BodyCtx` and never touch `ratatui::Frame` directly.

use std::fs;
use std::path::{Path, PathBuf};

fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(sources(&p));
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn no_screen_mentions_ratatui_frame() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in sources(&src) {
        let text = fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("ratatui::Frame") && !text.contains("&mut Frame"),
            "{} must draw through hayato_ui::BodyCtx, not Frame",
            file.display()
        );
    }
}
