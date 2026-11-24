#![allow(dead_code)]
//! Verifies that Hayato Cassette renders cleanly at both full Cyberdeck
//! (`120x36`) and Mini HUD (`60x16`) sizes across light and dark themes.

use std::path::PathBuf;

use hayato_music_core::{CodecInfo, SourceKind, Track, TrackId};
use hayato_ui::{App, Design, Header};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[path = "../src/art.rs"]
mod art;
#[path = "../src/screen.rs"]
mod screen;
#[path = "../src/sway_ipc.rs"]
mod sway_ipc;

fn sample_tracks() -> Vec<Track> {
    vec![
        Track {
            id: TrackId::Local(1),
            title: "Cyberdeck".into(),
            artist: "Midnight Signal".into(),
            album: "Neon Architecture".into(),
            track_number: Some(1),
            year: Some(2025),
            duration_secs: 245,
            source: SourceKind::Local {
                path: PathBuf::from("/music/cyberdeck.flac"),
                codec: CodecInfo {
                    format: "FLAC".into(),
                    sample_rate_hz: 48000,
                    bit_depth: Some(24),
                    bitrate_kbps: None,
                },
            },
            art_path: None,
        },
        Track {
            id: TrackId::Local(2),
            title: "Ghost in the Bus".into(),
            artist: "Midnight Signal".into(),
            album: "Neon Architecture".into(),
            track_number: Some(2),
            year: Some(2025),
            duration_secs: 222,
            source: SourceKind::Local {
                path: PathBuf::from("/music/ghost.flac"),
                codec: CodecInfo::default(),
            },
            art_path: None,
        },
    ]
}

#[test]
fn renders_full_120x36_and_mini_60x16_cleanly() {
    for (w, h, mini) in [(120, 36, false), (60, 16, true)] {
        let design = Design::solarized().with_reduced_motion(true);
        let mut s = screen::CassetteScreen::for_test(sample_tracks(), mini, &design);
        let mut app = App::new(Header::new("Hayato").subtitle("Cassette"))
            .design(design)
            .screen(&mut s);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let text = hayato_ui::screen(term.backend().buffer());
        assert_eq!(text.lines().count(), h as usize, "{w}x{h}");
        assert!(text.contains("Midnight Signal"), "{w}x{h}:\n{text}");
        assert!(text.contains("02:14 / 04:05"), "{w}x{h}:\n{text}");
        if !mini {
            assert!(text.contains(" All "), "missing All filter tab:\n{text}");
            assert!(text.contains("Spotify"), "missing Spotify filter tab:\n{text}");
            assert!(text.contains("Local"), "missing Local filter tab:\n{text}");
            assert!(text.contains("VOL 80%"), "missing labeled volume:\n{text}");
            assert!(!text.contains("[⌂ Home]"), "Home tab should be removed:\n{text}");
        }
    }
}
