//! Capture: screenshots, annotate, OCR, QR, recording (`rofi-capture`).
//!
//! The old script was a rofi list over tools the image already has. Every row is
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
use std::path::Path;

use lazyos_ui::{
    BodyCtx, Card, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row, Screen,
    TextStyle, Urgency, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::missing;

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
    recording: bool,
    rec_file: String,
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
            recording: false,
            rec_file: String::new(),
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
        if lazyos_ui::recording::is_recording_in(&state) {
            self.recording = true;
            self.rec_file = lazyos_ui::recording::file_in(&state)
                .map(|file| file.display().to_string())
                .unwrap_or_default();
        } else {
            // A pid whose comm is not wf-recorder is a session that died or a
            // number that was reused: forget it rather than offer a Stop that
            // signals somebody else.
            lazyos_ui::recording::clear_in(&state);
        }
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        if self.recording {
            rows.push(
                Row::new("Stop recording")
                    .glyph(Glyph::Stop)
                    .detail(
                        Path::new(&self.rec_file)
                            .file_name()
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
            rows.push(
                Row::new("Record · the whole screen")
                    .glyph(Glyph::Video)
                    .detail("until you stop it"),
            );
            keys.push("rec:none".to_string());
            rows.push(
                Row::new("Record · an area")
                    .glyph(Glyph::Video)
                    .detail("pick it with the pointer"),
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
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Capture"), true);
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
}

impl CaptureScreen {
    fn activate(&mut self, index: usize) -> Outcome {
        let Some(key) = self.keys.get(index).cloned() else {
            return Outcome::Continue;
        };
        let key = key.as_str();
        if key.starts_with("shot:") {
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
    notify-send -a LazyOS -i camera-web "Capture" "Saved $(basename "$_path")"
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
        crate::system::spawn_detached(&start_script(area));
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
        crate::system::spawn_detached(&stop_script(pid, &file));
    }
}

/// The shell line that starts a recorder.
///
/// There is deliberately no `eval`: `$!` has to be `wf-recorder` itself,
/// because that is the pid the state file saves and Stop signals. The old
/// `eval "wf-recorder …" &` saved the shell that wrapped the recorder, so Stop
/// sent SIGINT to the wrapper and the capture kept running. The
/// `/proc/<pid>/comm` check is what makes the saved pid trustworthy; state is
/// written only after it passes.
pub fn start_script(area: bool) -> String {
    let mut script = String::from("sleep 0.2\n");
    if area {
        script.push_str("_geom=$(slurp -d) || exit 0\n[ -n \"$_geom\" ] || exit 0\n");
    } else {
        script.push_str("_geom=\n");
    }
    script.push_str(
        r#"state="${XDG_RUNTIME_DIR:-/tmp}/lazyos-capture"
mkdir -p "$state" "$HOME/Videos" 2>/dev/null || true
_file="$HOME/Videos/lazyos-$(date +%Y%m%d-%H%M%S).mp4"
set -- -c libx264 -p preset=veryfast -p crf=28 -r 30
[ -z "$_geom" ] || set -- "$@" -g "$_geom"
wf-recorder "$@" -f "$_file" >"$state/recorder.log" 2>&1 &
_pid=$!
sleep 0.3
if [ "$(cat "/proc/$_pid/comm" 2>/dev/null)" = wf-recorder ] && kill -0 "$_pid" 2>/dev/null; then
    printf '%s\n' "$_pid" >"$state/recording.pid"
    printf '%s\n' "$_file" >"$state/recording.file"
    if [ -x "$HOME/.config/bin/busy" ]; then
        "$HOME/.config/bin/busy" start record Recording "Capturing to $(basename "$_file") · Alt+Ctrl+C stops it" media-record
    else
        notify-send -a LazyOS -i media-record "Recording" "Capturing to $(basename "$_file") · Alt+Ctrl+C stops it"
    fi
else
    rm -f "$state/recording.pid" "$state/recording.file"
    _why=$(tail -n 1 "$state/recorder.log" 2>/dev/null)
    notify-send -a LazyOS -u critical -i dialog-error "Capture" "Recording did not start${_why:+ · $_why}"
fi"#,
    );
    script
}

/// The shell line that stops the recorder whose pid and file were saved.
///
/// SIGINT is what finalises an `.mp4`; a recorder that ignores it gets SIGTERM,
/// and one that ignores both is killed and reported as incomplete instead of
/// leaving the session believing a capture is still running.
pub fn stop_script(pid: u32, file: &str) -> String {
    format!(
        r#"state="${{XDG_RUNTIME_DIR:-/tmp}}/lazyos-capture"
_pid={pid}
_file={file}
kill -INT "$_pid" 2>/dev/null || true
_n=0
while [ "$_n" -lt 50 ] && kill -0 "$_pid" 2>/dev/null; do
    sleep 0.1
    _n=$((_n + 1))
done
_forced=0
if kill -0 "$_pid" 2>/dev/null; then
    kill -TERM "$_pid" 2>/dev/null || true
    _n=0
    while [ "$_n" -lt 20 ] && kill -0 "$_pid" 2>/dev/null; do
        sleep 0.1
        _n=$((_n + 1))
    done
fi
if kill -0 "$_pid" 2>/dev/null; then
    _forced=1
    kill -KILL "$_pid" 2>/dev/null || true
fi
rm -f "$state/recording.pid" "$state/recording.file"
_base=$(basename "$_file")
if [ "$_forced" -eq 1 ]; then
    if [ -x "$HOME/.config/bin/busy" ]; then
        "$HOME/.config/bin/busy" fail record "Recording was killed · $_base may be incomplete" dialog-error
    else
        notify-send -a LazyOS -u critical -i dialog-error "Capture" "Recording was killed · $_base may be incomplete"
    fi
elif [ -s "$_file" ]; then
    if [ -x "$HOME/.config/bin/busy" ]; then
        "$HOME/.config/bin/busy" end record "Recording saved · $_base"
    else
        notify-send -a LazyOS -i media-record "Recording saved · $_base"
    fi
else
    if [ -x "$HOME/.config/bin/busy" ]; then
        "$HOME/.config/bin/busy" fail record "Recording failed · $_base is empty" dialog-error
    else
        notify-send -a LazyOS -u critical -i dialog-error "Capture" "Recording failed · $_base is empty"
    fi
fi"#,
        file = shell_quote(file),
    )
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

/// One shell word. A path from the session may contain spaces or quotes, and
/// the stop script assigns it before using it as an argument.
pub fn shell_quote(text: &str) -> String {
    let plain = !text.is_empty()
        && text.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'+' | b':')
        });
    if plain {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `lazyos menu capture` (`rofi-capture`): the window.
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
        let script = start_script(false);
        assert!(
            !script.contains("eval "),
            "eval makes $! the wrapper shell, not wf-recorder"
        );
        assert!(script.contains("wf-recorder \"$@\" -f \"$_file\""));
        assert!(script.contains("/proc/$_pid/comm"));
        assert!(script.contains("= wf-recorder"));
        // State is written only after the comm check, so Stop can trust it.
        let check = script.find("/proc/$_pid/comm").unwrap();
        let pid_file = script.find(">\"$state/recording.pid\"").unwrap();
        assert!(check < pid_file, "the pid is saved before it is verified");
    }

    #[test]
    fn an_area_recording_passes_the_slurp_geometry_directly() {
        let script = start_script(true);
        assert!(script.contains("slurp -d"));
        assert!(script.contains("set -- \"$@\" -g \"$_geom\""));
        assert!(!script.contains("_extra"));
        assert!(!script.contains("eval "));
    }

    #[test]
    fn stop_signals_the_recorder_and_waits_before_clearing_the_bar() {
        let script = stop_script(4242, "/home/brunux/Videos/lazyos-test.mp4");
        assert!(script.contains("_pid=4242"));
        assert!(script.contains("kill -INT \"$_pid\""));
        assert!(script.contains("kill -TERM \"$_pid\""));
        assert!(script.contains("kill -KILL \"$_pid\""));
        let wait = script.find("kill -0 \"$_pid\"").unwrap();
        let clear = script.find("rm -f \"$state/recording.pid\"").unwrap();
        assert!(
            wait < clear,
            "the state is cleared before the recorder is gone"
        );
        assert!(script.contains("busy\" end record"));
    }

    #[test]
    fn a_path_with_a_space_or_quote_stays_one_shell_word() {
        assert_eq!(
            shell_quote("/home/brunux/Videos/a.mp4"),
            "/home/brunux/Videos/a.mp4"
        );
        assert_eq!(
            shell_quote("/home/brunux/My Videos/a.mp4"),
            "'/home/brunux/My Videos/a.mp4'"
        );
        assert_eq!(shell_quote("it's.mp4"), "'it'\\''s.mp4'");
        assert_eq!(shell_quote(""), "''");
    }
}
