//! Capture: screenshots, annotate, OCR, QR, recording (`lazyos menu capture`,
//! also reachable as `lazyos-capture`).
//!
//! The old script was a old list over tools the image already has. Every row is
//! one command that ends before the list closes: grimshot takes the picture
//! (screen, area, window, saved under ~/Pictures/Screenshots/<date> and named
//! in a notification), swappy annotates one, tesseract reads the text out of a
//! region and zbarimg reads a QR or barcode out of one. Both copy the result
//! to the clipboard and say so.
//!
//! Recording is the exception: wf-recorder keeps running after the menu closes,
//! so it is started here, announced on the bar and in a mako card, and stopped
//! by opening this same menu again. Its pid and file name live in
//! $XDG_RUNTIME_DIR/lazyos-capture, so a session that dies takes the state with
//! it and a killed recorder is not remembered; the file it was writing stays.

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use lazyos_ui::{
    BodyCtx, Card, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row, Screen,
    TextStyle, Urgency, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::{missing, out};

const NEEDED: &[&str] = &[
    "grimshot",
    "swappy",
    "tesseract",
    "zbarimg",
    "wf-recorder",
    "slurp",
    "grim",
];

/// The capture screen.
pub struct CaptureScreen {
    recorder: lazyos_ui::recording::Recorder,
    list: ListState,
    keys: Vec<String>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    open_qr: bool,
}

impl CaptureScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            recorder: lazyos_ui::recording::Recorder::Idle,
            list: ListState::new().with_empty("no matching capture actions"),
            keys: Vec::new(),
            header: Header::new("LazyOS").subtitle("Capture"),
            keybar: vec![
                KeyHint::new("Enter", "run"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            open_qr: false,
        };
        screen.check_recording();
        screen.build_rows();
        screen
    }

    fn check_recording(&mut self) {
        let state = lazyos_ui::recording::state_dir();
        self.recorder = lazyos_ui::recording::read_in(&state);
        if !self.recorder.is_recording() {
            // A pid whose comm is not wf-recorder is a session that died or a
            // number that was reused: forget it rather than offer a Stop that
            // signals somebody else. A webcam overlay from that recording goes
            // with it.
            stop_camera(&state);
            lazyos_ui::recording::clear_in(&state);
        }
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        if let lazyos_ui::recording::Recorder::Recording { file, .. } = &self.recorder {
            rows.push(
                Row::new("Stop recording")
                    .glyph(Glyph::Stop)
                    .detail(
                        file.as_deref()
                            .and_then(|path| path.file_name())
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                    )
                    .value("● recording"),
            );
            keys.push("stop".to_string());
        } else {
            let shotdir = dirs::picture_dir()
                .map(|p| p.join("Screenshots"))
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Pictures"));
            let day = chrono::Local::now().format("%Y-%m-%d").to_string();
            rows.push(
                Row::new("Screenshot · the whole screen")
                    .glyph(Glyph::Camera)
                    .detail(format!("saved to {}/{}", shotdir.display(), day)),
            );
            keys.push("shot:save:screen".to_string());
            rows.push(
                Row::new("Screenshot · an area")
                    .glyph(Glyph::Crop)
                    .detail(format!("saved to {}/{}", shotdir.display(), day)),
            );
            keys.push("shot:save:area".to_string());
            rows.push(
                Row::new("Screenshot · an area, to the clipboard")
                    .glyph(Glyph::Crop)
                    .detail("paste it anywhere"),
            );
            keys.push("shot:copy:area".to_string());
            rows.push(
                Row::new("Screenshot · a window")
                    .glyph(Glyph::Image)
                    .detail("pick one with the pointer"),
            );
            keys.push("shot:save:window".to_string());
            rows.push(
                Row::new("Annotate an area")
                    .glyph(Glyph::Camera)
                    .detail("arrows, text, crop (swappy)"),
            );
            keys.push("annotate".to_string());
            let options = lazyos_ui::recording::load_options();
            rows.push(
                Row::new("Recording audio")
                    .glyph(Glyph::Volume)
                    .detail(options.audio.label())
                    .value("Enter cycles"),
            );
            keys.push("record:audio".to_string());
            rows.push(
                Row::new("Webcam overlay")
                    .glyph(Glyph::Camera)
                    .detail(if options.webcam {
                        "on · bottom-right corner"
                    } else {
                        "off"
                    })
                    .value("Enter toggles"),
            );
            keys.push("record:webcam".to_string());
            rows.push(
                Row::new("Record · the whole screen")
                    .glyph(Glyph::Video)
                    .detail(format!("until you stop it · {}", options.summary())),
            );
            keys.push("rec:none".to_string());
            rows.push(
                Row::new("Record · an area")
                    .glyph(Glyph::Video)
                    .detail(format!("pick it with the pointer · {}", options.summary())),
            );
            keys.push("rec:area".to_string());
        }
        rows.push(
            Row::new("Text from an area")
                .glyph(Glyph::File)
                .detail("OCR (tesseract) → clipboard"),
        );
        keys.push("ocr".to_string());
        rows.push(
            Row::new("QR code from an area")
                .glyph(Glyph::Qr)
                .detail("zbarimg → clipboard"),
        );
        keys.push("qr".to_string());
        rows.push(
            Row::new("Make a QR code")
                .glyph(Glyph::Qr)
                .detail("the Wi-Fi password, or any text"),
        );
        keys.push("make-qr".to_string());
        let shotdir = dirs::picture_dir()
            .map(|p| p.join("Screenshots"))
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Pictures"));
        rows.push(
            Row::new("Open the pictures folder")
                .glyph(Glyph::Folder)
                .detail(shotdir.display().to_string()),
        );
        keys.push("folder".to_string());
        self.list.set_rows(rows);
        self.keys = keys;
    }
}

impl Screen for CaptureScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Actions"), true);
        let (filter, rows) = super::pick::list_layout(ctx.area);
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            "type to filter · Enter runs · Escape closes",
            self.list.len(),
            self.keys.len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let rows_height = self.rows_height.get();
        match key.code {
            KeyCode::Esc => return Outcome::Done(lazyos_ui::app::exit::ESCAPE),
            KeyCode::Enter => {
                if let Some(index) = self.list.selected_index() {
                    return self.activate(index);
                }
            }
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL) && self.list.type_filter(key) =>
            {
                return Outcome::Continue;
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, rows) = super::pick::list_layout(area);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.recorder {
            lazyos_ui::recording::Recorder::Idle => "idle",
            lazyos_ui::recording::Recorder::Recording { .. } => "recording",
        })
    }
}

impl CaptureScreen {
    fn activate(&mut self, index: usize) -> Outcome {
        let Some(key) = self.keys.get(index).cloned() else {
            return Outcome::Continue;
        };
        let key = key.as_str();
        if key == "record:audio" {
            let mut options = lazyos_ui::recording::load_options();
            options.audio = options.audio.cycle();
            let _ = lazyos_ui::recording::save_options(options);
            self.build_rows();
            Outcome::Continue
        } else if key == "record:webcam" {
            let mut options = lazyos_ui::recording::load_options();
            options.webcam = !options.webcam;
            let _ = lazyos_ui::recording::save_options(options);
            self.build_rows();
            Outcome::Continue
        } else if key.starts_with("shot:") {
            let action = &key[5..];
            let target = action.split(':').nth(1).unwrap_or("");
            let act = action.split(':').next().unwrap_or("");
            Self::screenshot(act, target);
            Outcome::Done(lazyos_ui::app::exit::OK)
        } else {
            match key {
                "annotate" => {
                    Self::annotate();
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                "ocr" => {
                    Self::ocr();
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                "qr" => {
                    Self::scan_qr();
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                "rec:none" => {
                    Self::start_recording(false);
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                "rec:area" => {
                    Self::start_recording(true);
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                "stop" => {
                    Self::stop_recording();
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                "make-qr" => {
                    self.open_qr = true;
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                "folder" => {
                    let shotdir = dirs::picture_dir()
                        .map(|p| p.join("Screenshots"))
                        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Pictures"));
                    crate::system::spawn_detached(&format!(
                        "exec xdg-open '{}'",
                        shotdir.display()
                    ));
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                _ => Outcome::Continue,
            }
        }
    }

    /// Build the argv to invoke `~/.config/bin/screenshot` so saved screenshots
    /// land in `~/Pictures/Screenshots/<date>/` instead of `XDG_PICTURES_DIR`.
    pub fn screenshot_argv(home: &str, action: &str, target: &str) -> Vec<String> {
        let mut argv = vec![
            format!("{home}/.config/bin/screenshot"),
            "-n".to_string(),
            action.to_string(),
        ];
        if !target.is_empty() {
            argv.push(target.to_string());
        }
        argv
    }

    fn screenshot(action: &str, target: &str) {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let argv = Self::screenshot_argv(&home, action, target);
        let cmd = format!("sleep 0.2; exec {}", argv.join(" "));
        crate::system::spawn_detached(&cmd);
    }

    fn annotate() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let script = format!(
            r#"sleep 0.2
_path=$("{home}/.config/bin/screenshot" save area) || exit 0
[ -n "$_path" ] || exit 0
swappy -f "$_path" -o "$_path" || true
if [ -s "$_path" ]; then
    notify-send -a LazyOS -i camera-web -h "string:x-lazyos-open-path:$_path" "Capture" "Saved $(basename "$_path") · click to show"
else
    notify-send -a LazyOS -i camera-web "Capture" "The annotation was not saved"
fi"#
        );
        crate::system::spawn_detached(&script);
    }

    fn ocr() {
        let script = r#"sleep 0.2
_geom=$(slurp -d) || exit 0
[ -n "$_geom" ] || exit 0
_tmp=$(mktemp "${TMPDIR:-/tmp}/ocr.XXXXXX.png") || exit 1
grim -g "$_geom" "$_tmp" 2>/dev/null || { rm -f "$_tmp"; exit 0; }
_text=$(tesseract "$_tmp" stdout -l eng 2>/dev/null | sed -e 's/[[:space:]]*$//' -e '/^$/d') || true
rm -f "$_tmp"
if [ -n "$_text" ]; then
    printf '%s' "$_text" | wl-copy
    _preview=$(printf '%s' "$_text" | head -1 | cut -c1-60)
    notify-send -a LazyOS -i text-x-generic "Text from the screen" "Text copied · $_preview"
else
    notify-send -a LazyOS -i text-x-generic "Text from the screen" "No text was found in that region"
fi"#;
        crate::system::spawn_detached(script);
    }

    fn scan_qr() {
        let script = r#"sleep 0.2
_geom=$(slurp -d) || exit 0
[ -n "$_geom" ] || exit 0
_tmp=$(mktemp "${TMPDIR:-/tmp}/qr.XXXXXX.png") || exit 1
grim -g "$_geom" "$_tmp" 2>/dev/null || { rm -f "$_tmp"; exit 0; }
_code=$(zbarimg -q --raw "$_tmp" 2>/dev/null | tr -d '\r\n') || true
rm -f "$_tmp"
if [ -n "$_code" ]; then
    printf '%s' "$_code" | wl-copy
    _preview=$(printf '%s' "$_code" | head -1 | cut -c1-60)
    notify-send -a LazyOS -i qr "QR code from the screen" "Copied · $_preview"
else
    notify-send -a LazyOS -i qr "QR code from the screen" "No code was found in that region"
fi"#;
        crate::system::spawn_detached(script);
    }

    fn start_recording(area: bool) {
        // The menu closes first; a detached copy of this binary does the work,
        // so the recorder never inherits the window's terminal or process
        // group and its own pid is the one saved.
        let mut args = vec![
            "menu".to_string(),
            "capture".to_string(),
            "--start".to_string(),
        ];
        if area {
            args.push("area".to_string());
        }
        spawn_self(&args);
    }

    fn stop_recording() {
        let state = lazyos_ui::recording::state_dir();
        let Some(pid) = lazyos_ui::recording::pid_in(&state) else {
            return;
        };
        // Only the recorder itself may be signalled: a stale file can name a
        // number that now belongs to something else.
        if !lazyos_ui::recording::is_recorder(pid) {
            lazyos_ui::recording::clear_in(&state);
            return;
        }
        let file = lazyos_ui::recording::file_in(&state)
            .map(|file| file.to_string_lossy().to_string())
            .unwrap_or_default();
        // The waiter outlives this window: it finalises the file, clears the
        // bar and only then says the recording is saved.
        spawn_self(&[
            "menu".to_string(),
            "capture".to_string(),
            "--stop-wait".to_string(),
            pid.to_string(),
            file,
        ]);
    }
}

/// Re-run this binary detached. `current_exe` is the real file, so a
/// multi-call symlink (`lazyos-capture`, `lazyos`) still reaches the dispatcher.
fn spawn_self(args: &[String]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::system::spawn_detached_argv(exe, &args);
}

/// The recorder's argv: one program and its arguments, never a shell line.
///
/// `-g` sits before `-f` exactly as `wf-recorder` expects it. The caller owns
/// every word, so a path with a space stays one argument.
pub fn recorder_argv(file: &Path, geometry: Option<&str>, audio: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "wf-recorder".to_string(),
        "-c".to_string(),
        "libx264".to_string(),
        "-p".to_string(),
        "preset=veryfast".to_string(),
        "-p".to_string(),
        "crf=28".to_string(),
        "-r".to_string(),
        "30".to_string(),
    ];
    if let Some(audio) = audio {
        argv.push(format!("--audio={audio}"));
    }
    if let Some(geometry) = geometry {
        argv.push("-g".to_string());
        argv.push(geometry.to_string());
    }
    argv.push("-f".to_string());
    argv.push(file.to_string_lossy().to_string());
    argv
}

/// `$HOME/Videos/lazyos-<date>-<time>.mp4`, the file the recorder writes.
pub fn videos_file() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    home.join("Videos").join(format!("lazyos-{stamp}.mp4"))
}

fn slurp_geometry() -> Option<String> {
    let output = Command::new("slurp")
        .arg("-d")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let geometry = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!geometry.is_empty()).then_some(geometry)
}

/// Write the recorder's pid and file. Called only after `/proc/<pid>/comm`
/// proved the pid is `wf-recorder`, so Stop can trust what it reads.
fn write_state(state: &Path, pid: u32, file: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(state)?;
    std::fs::write(state.join("recording.pid"), format!("{pid}\n"))?;
    std::fs::write(
        state.join("recording.file"),
        format!("{}\n", file.display()),
    )?;
    Ok(())
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn last_line(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_string())
        })
        .unwrap_or_default()
}

fn announce_recording(file: &Path, options: lazyos_ui::recording::Options) {
    let body = format!(
        "Capturing to {} · {} · Alt+Ctrl+C stops it",
        basename(file),
        options.summary()
    );
    if crate::system::have_busy() {
        crate::system::busy("record", "Recording", &body, "media-record");
    } else {
        crate::system::say("Recording", body, Urgency::Normal, Glyph::Video);
    }
}

fn announce_start_failure(why: &str) {
    let body = if why.is_empty() {
        "Recording did not start".to_string()
    } else {
        format!("Recording did not start · {why}")
    };
    if crate::system::have_busy() {
        crate::system::busy_fail("record", "Capture", &body, "dialog-error", None);
    } else {
        crate::system::say("Capture", body, Urgency::Critical, Glyph::Warning);
    }
}

fn announce_stopped(file: &Path, forced: bool) {
    let base = basename(file);
    let nonempty = file.metadata().map(|meta| meta.len() > 0).unwrap_or(false);
    let (body, critical) = if forced {
        (
            format!("Recording was killed · {base} may be incomplete"),
            true,
        )
    } else if nonempty {
        (format!("Recording saved · {base}"), false)
    } else {
        (format!("Recording failed · {base} is empty"), true)
    };
    if crate::system::have_busy() {
        if critical {
            crate::system::busy_fail("record", "Capture", &body, "dialog-error", Some(file));
        } else {
            crate::system::busy_end("record", "Recording", &body, Some(file));
        }
    } else {
        let glyph = if critical {
            Glyph::Warning
        } else {
            Glyph::Video
        };
        let urgency = if critical {
            Urgency::Critical
        } else {
            Urgency::Normal
        };
        crate::system::say_open("Recording", body, urgency, glyph, file);
    }
}

fn signal(pid: u32, signal: libc::c_int) {
    // SAFETY: `kill(2)` takes a pid and a signal number; the pid came from the
    // state file and was verified to be `wf-recorder` before this process ran.
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

/// Wait up to `tenths` × 100 ms for the recorder to stop being the recorder.
fn wait_gone(pid: u32, tenths: u32) -> bool {
    for _ in 0..tenths {
        if !lazyos_ui::recording::is_recorder(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !lazyos_ui::recording::is_recorder(pid)
}

/// The PulseAudio/PipeWire source for the chosen mode.
fn audio_source(mode: lazyos_ui::recording::AudioMode) -> Result<Option<String>, String> {
    use lazyos_ui::recording::AudioMode;
    match mode {
        AudioMode::Off => Ok(None),
        AudioMode::Mic => {
            if let Some(source) = out(&["pactl", "get-default-source"])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                return Ok(Some(source));
            }
            let source = out(&["pactl", "list", "short", "sources"]).and_then(|text| {
                text.lines().find_map(|line| {
                    let name = line.split_whitespace().nth(1)?;
                    (!name.ends_with(".monitor")).then(|| name.to_string())
                })
            });
            source
                .map(Some)
                .ok_or_else(|| "No microphone source is available (pactl found none)".to_string())
        }
        AudioMode::System => {
            if let Some(sink) = out(&["pactl", "get-default-sink"])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                return Ok(Some(format!("{sink}.monitor")));
            }
            let source = out(&["pactl", "list", "short", "sources"]).and_then(|text| {
                text.lines().find_map(|line| {
                    let name = line.split_whitespace().nth(1)?;
                    name.ends_with(".monitor").then(|| name.to_string())
                })
            });
            source.map(Some).ok_or_else(|| {
                "No system-audio monitor source is available (pactl found none)".to_string()
            })
        }
    }
}

fn webcam_window_exists() -> bool {
    out(&["swaymsg", "-t", "get_tree", "-r"])
        .map(|tree| tree.contains("\"lazyos-webcam\""))
        .unwrap_or(false)
}

fn geometry_rect(geometry: Option<&str>) -> (i64, i64, i64, i64) {
    if let Some(geometry) = geometry
        && let Some((pos, size)) = geometry.split_once(' ')
        && let (Some(x), Some(y)) = (
            pos.split(',').next().and_then(|v| v.trim().parse().ok()),
            pos.split(',').nth(1).and_then(|v| v.trim().parse().ok()),
        )
        && let (Some(w), Some(h)) = (
            size.split('x').next().and_then(|v| v.trim().parse().ok()),
            size.split('x').nth(1).and_then(|v| v.trim().parse().ok()),
        )
    {
        return (x, y, w, h);
    }
    let fallback = (0, 0, 1366, 768);
    let Some(text) = out(&["swaymsg", "-t", "get_outputs", "-r"]) else {
        return fallback;
    };
    let Ok(outputs) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return fallback;
    };
    let Some(output) = outputs
        .iter()
        .find(|output| output["focused"].as_bool().unwrap_or(false))
    else {
        return fallback;
    };
    let rect = &output["rect"];
    (
        rect["x"].as_i64().unwrap_or(0),
        rect["y"].as_i64().unwrap_or(0),
        rect["width"].as_i64().unwrap_or(1366),
        rect["height"].as_i64().unwrap_or(768),
    )
}

fn workspace_origin() -> (i64, i64) {
    let Some(text) = out(&["swaymsg", "-t", "get_workspaces", "-r"]) else {
        return (0, 0);
    };
    let Ok(workspaces) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return (0, 0);
    };
    workspaces
        .iter()
        .find(|workspace| workspace["focused"].as_bool().unwrap_or(false))
        .map(|workspace| {
            (
                workspace["rect"]["x"].as_i64().unwrap_or(0),
                workspace["rect"]["y"].as_i64().unwrap_or(0),
            )
        })
        .unwrap_or((0, 0))
}

fn place_webcam(geometry: Option<&str>) {
    let (x, y, w, h) = geometry_rect(geometry);
    let (wx, wy) = workspace_origin();
    let px = (x + w - 320 - 20 - wx).max(10);
    let py = (y + h - 240 - 20 - wy).max(10);
    let command = format!(
        "[app_id=\"lazyos-webcam\"] floating enable, border none, resize set 320 240, move position {px} {py}"
    );
    let _ = Command::new("swaymsg").arg(command).status();
}

/// Start the webcam preview window. It is a normal mpv window, placed as a
/// floating bottom-right overlay; wf-recorder records it as part of the screen.
fn start_webcam(geometry: Option<&str>) -> Result<u32, String> {
    if !crate::system::have("mpv") {
        return Err("mpv is not installed, so the webcam overlay cannot start".to_string());
    }
    let state = lazyos_ui::recording::state_dir();
    let _ = std::fs::create_dir_all(&state);
    let log = state.join("camera.log");
    let log_out = File::create(&log).map_err(|error| format!("camera log: {error}"))?;
    let log_err = log_out
        .try_clone()
        .map_err(|error| format!("camera log: {error}"))?;
    let mut command = Command::new("mpv");
    command
        .args([
            "--no-config",
            "--really-quiet",
            "--vo=wlshm",
            "--no-osc",
            "--no-input-default-bindings",
            "--no-border",
            "--ontop",
            "--wayland-app-id=lazyos-webcam",
            "--title=LazyOS Webcam",
            "--geometry=320x240",
            "av://v4l2:/dev/video0",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start mpv: {error}"))?;
    let pid = child.id();
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(100));
        if webcam_window_exists() {
            break;
        }
    }
    place_webcam(geometry);
    // mpv finishes sizing the surface just after the app id appears; a second
    // placement wins over that resize and leaves the 320x240 overlay.
    std::thread::sleep(Duration::from_millis(300));
    place_webcam(geometry);
    if let Ok(Some(status)) = child.try_wait() {
        let detail = last_line(&log);
        return Err(if detail.is_empty() {
            format!("the webcam overlay exited ({status})")
        } else {
            format!("the webcam overlay exited ({status}): {detail}")
        });
    }
    Ok(pid)
}

fn stop_camera_pid(pid: Option<u32>) {
    if let Some(pid) = pid {
        signal(pid, libc::SIGTERM);
    }
}

fn stop_camera(state: &Path) {
    let path = state.join("camera.pid");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let Ok(pid) = text.trim().parse::<u32>() else {
        return;
    };
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|value| value.trim().to_string())
        .ok();
    if comm.as_deref() == Some("mpv") {
        signal(pid, libc::SIGTERM);
    }
}

/// `lazyos menu capture --start [area]`: run detached by the menu.
///
/// The recorder is spawned as its own argv, so the pid saved here is the
/// recorder's and not a wrapper shell's. Nothing reaches the state directory
/// until `/proc/<pid>/comm` says `wf-recorder`, which is what makes Stop's
/// signal trustworthy.
pub fn start_headless(area: bool) -> i32 {
    // Let the menu close before the screen is captured.
    std::thread::sleep(Duration::from_millis(200));
    let geometry = if area {
        match slurp_geometry() {
            Some(geometry) => Some(geometry),
            None => return 0,
        }
    } else {
        None
    };
    let options = lazyos_ui::recording::load_options();
    let audio = match audio_source(options.audio) {
        Ok(source) => source,
        Err(error) => {
            announce_start_failure(&error);
            return 1;
        }
    };
    let camera = if options.webcam {
        match start_webcam(geometry.as_deref()) {
            Ok(pid) => Some(pid),
            Err(error) => {
                announce_start_failure(&error);
                return 1;
            }
        }
    } else {
        None
    };
    let state = lazyos_ui::recording::state_dir();
    let file = videos_file();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::create_dir_all(&state);
    let log = state.join("recorder.log");
    let Ok(log_out) = File::create(&log) else {
        stop_camera_pid(camera);
        announce_start_failure("the recorder log could not be opened");
        return 1;
    };
    let Ok(log_err) = log_out.try_clone() else {
        stop_camera_pid(camera);
        announce_start_failure("the recorder log could not be opened");
        return 1;
    };
    let argv = recorder_argv(&file, geometry.as_deref(), audio.as_deref());
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));
    // The recorder gets its own session: the window may close, and so may the
    // process that started it, without the capture taking the hangup.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            stop_camera_pid(camera);
            lazyos_ui::recording::clear_in(&state);
            announce_start_failure(&error.to_string());
            return 1;
        }
    };
    let pid = child.id();
    std::thread::sleep(Duration::from_millis(300));
    if lazyos_ui::recording::is_recorder(pid) {
        match write_state(&state, pid, &file) {
            Ok(()) => {
                if let Some(camera) = camera {
                    let _ = std::fs::write(state.join("camera.pid"), format!("{camera}\n"));
                }
                announce_recording(&file, options);
                0
            }
            Err(error) => {
                // A recorder with no saved pid cannot be stopped from the menu,
                // so end it now instead of leaving it running invisibly.
                signal(pid, libc::SIGINT);
                stop_camera_pid(camera);
                lazyos_ui::recording::clear_in(&state);
                announce_start_failure(&format!("the recording state could not be saved: {error}"));
                1
            }
        }
    } else {
        stop_camera_pid(camera);
        lazyos_ui::recording::clear_in(&state);
        announce_start_failure(&last_line(&log));
        1
    }
}

/// `lazyos menu capture --stop-wait PID FILE`: run detached by Stop.
///
/// SIGINT is what finalises an `.mp4`; a recorder that ignores it gets SIGTERM,
/// and one that ignores both is killed and reported as incomplete rather than
/// leaving the session believing a capture is still running. The state is
/// cleared only after the recorder is gone, so the card that says the file was
/// saved is never itself recorded.
pub fn stop_wait(pid: u32, file: &str) -> i32 {
    let path = PathBuf::from(file);
    signal(pid, libc::SIGINT);
    let mut gone = wait_gone(pid, 50);
    if !gone {
        signal(pid, libc::SIGTERM);
        gone = wait_gone(pid, 20);
    }
    let forced = if gone {
        false
    } else {
        signal(pid, libc::SIGKILL);
        true
    };
    let state = lazyos_ui::recording::state_dir();
    stop_camera(&state);
    lazyos_ui::recording::clear_in(&state);
    announce_stopped(&path, forced);
    0
}

/// `lazyos menu capture --status`: `idle`, or `recording<TAB><file>`.
///
/// Read-only, and it trusts the same `/proc/<pid>/comm` check as the menu, so a
/// stale state file never reports a capture.
pub fn status() -> String {
    let state = lazyos_ui::recording::state_dir();
    if !lazyos_ui::recording::is_recording_in(&state) {
        return "idle".to_string();
    }
    match lazyos_ui::recording::file_in(&state) {
        Some(file) => format!("recording\t{}", file.display()),
        None => "recording".to_string(),
    }
}

/// `lazyos menu capture` (`lazyos-capture`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Capture",
            format!("{said} (grimshot, swappy, tesseract, zbarimg, wf-recorder, slurp, grim)"),
        ));
    }
    let mut screen = CaptureScreen::new();
    let code = crate::run_screen(terminal, design, "Capture", &mut screen)?;
    if screen.open_qr {
        return super::qr::menu(terminal, design);
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_needed_tools() {
        assert!(NEEDED.contains(&"grimshot"));
        assert!(NEEDED.contains(&"wf-recorder"));
    }

    #[test]
    fn screenshot_uses_wrapper_in_config_bin() {
        let argv = CaptureScreen::screenshot_argv("/home/brunux", "save", "area");
        assert_eq!(
            argv,
            vec!["/home/brunux/.config/bin/screenshot", "-n", "save", "area"]
        );
    }

    #[test]
    fn the_recorder_is_started_as_its_own_argv() {
        let argv = recorder_argv(Path::new("/home/brunux/Videos/a.mp4"), None, None);
        assert_eq!(argv[0], "wf-recorder");
        assert!(
            !argv.iter().any(|arg| arg == "sh" || arg == "eval"),
            "the recorder is never wrapped in a shell"
        );
        assert!(argv.windows(2).any(|pair| pair == ["-c", "libx264"]));
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["-f", "/home/brunux/Videos/a.mp4"]),
            "the output file is its own argv word"
        );
        assert!(!argv.iter().any(|arg| arg == "-g"));
    }

    #[test]
    fn an_area_recording_passes_the_slurp_geometry_directly() {
        let argv = recorder_argv(Path::new("/tmp/a.mp4"), Some("10,20 300x200"), None);
        let geometry = argv.iter().position(|arg| arg == "-g").unwrap();
        assert_eq!(argv[geometry + 1], "10,20 300x200");
        let file = argv.iter().position(|arg| arg == "-f").unwrap();
        assert!(
            geometry < file,
            "-g is a recorder option, so it comes before the output file"
        );
    }

    #[test]
    fn an_audio_recording_names_the_source_as_one_argument() {
        let argv = recorder_argv(
            Path::new("/tmp/a.mp4"),
            None,
            Some("alsa_input.platform-sound.HiFi__Mic__source"),
        );
        assert!(
            argv.iter()
                .any(|arg| arg == "--audio=alsa_input.platform-sound.HiFi__Mic__source")
        );
        let audio = argv
            .iter()
            .position(|arg| arg.starts_with("--audio="))
            .unwrap();
        let file = argv.iter().position(|arg| arg == "-f").unwrap();
        assert!(audio < file, "audio is a recorder option");
    }

    #[test]
    fn a_path_with_a_space_or_quote_stays_one_argument() {
        let path = Path::new("/home/brunux/My Videos/it's.mp4");
        let argv = recorder_argv(path, None, None);
        assert!(argv.iter().any(|arg| arg == &path.to_string_lossy()));
        assert_eq!(argv.iter().filter(|arg| *arg == "-f").count(), 1);
    }

    #[test]
    fn a_videos_file_is_a_timestamped_mp4_under_home() {
        let file = videos_file();
        assert_eq!(
            file.parent().and_then(Path::file_name),
            Some(std::ffi::OsStr::new("Videos"))
        );
        let name = file.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("lazyos-"));
        assert!(name.ends_with(".mp4"));
    }
}
