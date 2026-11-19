//! The Notes screen at both session grids, as golden snapshots.
//!
//! The list is a shelf beside a page on a wide window and a shelf with a
//! two-line page strip on a small one; these snapshots are the witness that the
//! page does not eat the list, that the filter counter counts matches, and that
//! the action rows keep their help beside them.
//!
//! `UPDATE_SNAPSHOTS=1 cargo test -p lazyos-cli --test notes` regenerates them;
//! read the diff before committing it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use lazyos_cli::system::notes::NotesScreen;
use lazyos_ui::{App, Design, Header};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

const SIZES: [(u16, u16); 2] = [(80, 24), (152, 45)];

fn design() -> Design {
    Design::solarized().with_reduced_motion(true)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lazyos-notes-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("Documents/Notes")).unwrap();
    fs::create_dir_all(dir.join(".config")).unwrap();
    dir
}

/// The screen asks `have("xournalpp")` to choose between the action and the
/// install hint, so a runner without the tool would render different rows. A
/// fake executable first on PATH pins the image state instead.
fn fake_bin(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lazyos-notes-bin-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let tool = dir.join("xournalpp");
    fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn note(dir: &Path, name: &str, body: &str, modified: u64) {
    let path = dir.join("Documents/Notes").join(name);
    fs::write(&path, body).unwrap();
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(modified))
        .unwrap();
}

fn press(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn typed(text: &str) -> Vec<Event> {
    text.chars().map(|ch| press(KeyCode::Char(ch))).collect()
}

fn render(screen: &mut NotesScreen, events: &[Event], width: u16, height: u16) -> String {
    let mut app = App::new(Header::new("LazyOS"))
        .design(design())
        .screen(screen);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    for event in events {
        terminal.draw(|frame| app.render(frame)).unwrap();
        let _ = app.handle(event.clone());
    }
    terminal.draw(|frame| app.render(frame)).unwrap();
    text(terminal.backend().buffer())
}

fn text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf.cell((x, y)).map(|cell| cell.symbol()).unwrap_or(" "));
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
    }
    out
}

fn compare(name: &str, rendered: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let path = dir.join(format!("{name}.txt"));
    if std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, rendered).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}: run UPDATE_SNAPSHOTS=1 cargo test -p lazyos-cli --test notes",
            path.display()
        )
    });
    assert_eq!(rendered, expected, "snapshot {} changed", path.display());
}

fn snapshot(name: &str, events: &[Event], make: impl Fn() -> NotesScreen) {
    for (width, height) in SIZES {
        let mut screen = make();
        compare(
            &format!("{name}-{width}x{height}"),
            &render(&mut screen, events, width, height),
        );
    }
}

/// One test drives every state: the screen reads `HOME`, and a second test in
/// this binary would race the first on it.
#[test]
fn the_notes_screen_at_both_grids() {
    // Times are formatted in local time; UTC keeps the snapshots portable.
    unsafe {
        std::env::set_var("TZ", "UTC");
    }
    let bin = fake_bin("snap");
    unsafe {
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    }
    let home = scratch("snap");
    note(
        &home,
        "2026-11-18-reading-list.md",
        "# Reading list\n\nSome **bold** notes about the books I want to read this winter, with a line long enough that it has to wrap across the page margin twice before it is done.\n\n- The Left Hand of Darkness\n- Piranesi\n- A Wizard of Earthsea\n\n> Ursula K. Le Guin wrote more than she is given credit for.\n\n## Next\n\nFinish the essay on the train and add the citations.\n",
        1_795_000_300,
    );
    note(
        &home,
        "2026-11-17-morning-pages.md",
        "# Morning pages\n\nA short note.\n",
        1_795_000_100,
    );
    note(
        &home,
        "2026-11-16-sketch.xopp",
        "not really xopp\n",
        1_795_000_200,
    );
    note(&home, "README.md", "# Notes\n", 1_795_000_000);
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    }

    // The newest note is selected, so the page opens on the reading list.
    snapshot("notes-text", &[], NotesScreen::new);
    // The ink note is the second row.
    snapshot("notes-ink", &[press(KeyCode::Down)], NotesScreen::new);
    // The actions follow the three notes; "New quick note" is the fourth row.
    snapshot(
        "notes-action",
        &[
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Down),
        ],
        NotesScreen::new,
    );
    // Typing narrows the shelf and the counter follows; Escape clears it.
    snapshot("notes-filter", &typed("read"), NotesScreen::new);
    // Enter on the action opens the title prompt in place of the shelf.
    snapshot(
        "notes-title",
        &[
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Enter),
        ],
        NotesScreen::new,
    );

    let _ = fs::remove_dir_all(&home);

    let empty = scratch("empty");
    unsafe {
        std::env::set_var("HOME", &empty);
        std::env::set_var("XDG_CONFIG_HOME", empty.join(".config"));
    }
    snapshot("notes-empty", &[], NotesScreen::new);
    let _ = fs::remove_dir_all(&empty);
    let _ = fs::remove_dir_all(&bin);
}
