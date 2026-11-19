//! Controller (`lazyos menu controller`, Settings → Input): what each pad
//! button does, and how a PlayStation pad pairs, in one line.
//!
//! The table is [`crate::padmap`], the same one `lazyos gamepad` reads, and
//! every change is written to `~/.config/lazyos/pad.json` immediately. The
//! mapper reloads the file within two seconds, so a remap applies while the
//! pad is connected, and the screen works with **no pad at all**: the actions
//! are chosen with ←/→ rather than by pressing the button, which is what makes
//! a preconfigured pad editable before it is paired.
//!
//! The defaults are the console layout, and a DualShock 4 is already covered by
//! them: Cross confirms, Circle cancels, Square opens the Games options,
//! Triangle the manual, L1 the game's folder. A DS4 pairs over Bluetooth (hold
//! **PS + Share** until the light bar flashes); a DualShock 3 is the exception,
//! pairing once over USB because BlueZ's sixaxis plugin writes this machine's
//! address into it. The screen says which one is waiting.

use std::path::PathBuf;

use lazyos_ui::widgets::put_trunc;
use lazyos_ui::{
    BodyCtx, ColorRole, Design, Header, KeyHint, Outcome, Screen, TextStyle, draw_card,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::padmap::{Bindings, Button, PadInfo, pads};

/// The buttons plus the reset row.
const RESET_ROW: usize = Button::ALL.len();
const ROWS: usize = RESET_ROW + 1;
const SIZE: (u16, u16) = (880, 620);

/// The Controller screen.
pub struct ControllerScreen {
    bindings: Bindings,
    path: Option<PathBuf>,
    pads: Vec<PadInfo>,
    row: usize,
    note: String,
    header: Header,
    keybar: Vec<KeyHint>,
}

impl ControllerScreen {
    pub fn new(bindings: Bindings, path: Option<PathBuf>, pads: Vec<PadInfo>) -> Self {
        Self {
            bindings,
            path,
            pads,
            row: 0,
            note: "Changes apply to every LazyOS window, without a restart".to_string(),
            header: Header::new("LazyOS").subtitle("Controller"),
            keybar: vec![
                KeyHint::new("←/→", "change"),
                KeyHint::new("Enter", "next"),
                KeyHint::new("r", "reset"),
                KeyHint::new("Esc", "back"),
            ],
        }
    }

    fn save(&mut self) {
        match self.bindings.save_to(self.path.as_deref()) {
            Ok(()) => self.note = "Saved — the mapper picks it up in two seconds".to_string(),
            Err(error) => self.note = error,
        }
    }

    fn change(&mut self, step: isize) {
        if self.row == RESET_ROW {
            return;
        }
        self.bindings.cycle(Button::ALL[self.row], step);
        self.save();
    }

    fn reset(&mut self) {
        self.bindings.reset();
        self.save();
    }

    fn activate(&mut self) {
        if self.row == RESET_ROW {
            self.reset();
        } else {
            self.change(1);
        }
    }

    fn move_row(&mut self, step: isize) {
        self.row = (self.row as isize + step).rem_euclid(ROWS as isize) as usize;
    }

    /// The line under the title: the pad, or the fact that there is none.
    fn pad_line(&self) -> (String, TextStyle) {
        match self.pads.as_slice() {
            [] => (
                "No controller connected · the defaults below still apply".to_string(),
                TextStyle::Body,
            ),
            [pad] => (pad.summary(), TextStyle::Title),
            many => (
                format!("{} controllers connected", many.len()),
                TextStyle::Title,
            ),
        }
    }
}

impl Screen for ControllerScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some(SIZE)
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Controller"), true);
        let inner = Rect {
            x: ctx.area.x + 2,
            y: ctx.area.y + 1,
            width: ctx.area.width.saturating_sub(4),
            height: ctx.area.height.saturating_sub(2),
        };
        if inner.width < 30 || inner.height < 8 {
            return;
        }
        let mut y = inner.y;

        let (line, style) = self.pad_line();
        put_trunc(
            ctx.buf,
            inner.x,
            y,
            &line,
            inner.width,
            ctx.design.style(style),
        );
        y += 1;

        if let Some(pad) = self.pads.iter().find(|pad| pad.is_playstation_on_usb()) {
            let hint = format!(
                "{} on USB · press the PS button, then unplug and press PS again",
                pad.name
            );
            put_trunc(
                ctx.buf,
                inner.x,
                y,
                &hint,
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
            y += 1;
        }

        let rule_style = Style::default().fg(ctx.design.color(ColorRole::SurfaceAlt));
        for col in inner.x..inner.right() {
            if let Some(cell) = ctx.buf.cell_mut((col, y)) {
                cell.set_symbol("\u{2500}").set_style(rule_style);
            }
        }
        y += 1;

        let bottom = inner.bottom().saturating_sub(3);
        for index in 0..ROWS {
            if y >= bottom {
                break;
            }
            let selected = index == self.row;
            let row = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            };
            let background = if selected {
                ctx.buf.set_style(
                    row,
                    Style::default().bg(ctx.design.color(ColorRole::Selection)),
                );
                ctx.design.color(ColorRole::Selection)
            } else {
                ctx.design.color(ColorRole::Bg)
            };
            let label_style = Style::default()
                .fg(ctx.design.color(if selected {
                    ColorRole::Accent
                } else {
                    ColorRole::Text
                }))
                .bg(background)
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            let value_style = Style::default()
                .fg(ctx.design.color(ColorRole::AccentAlt))
                .bg(background);
            let arrow_style = Style::default()
                .fg(ctx.design.color(ColorRole::Dim))
                .bg(background);

            if index == RESET_ROW {
                ctx.buf
                    .set_string(inner.x, y, "Reset to defaults", label_style);
                ctx.buf
                    .set_string(inner.right().saturating_sub(6), y, "r", value_style);
                y += 1;
                continue;
            }

            let button = Button::ALL[index];
            let action = self.bindings.get(button);
            ctx.buf.set_string(inner.x, y, button.label(), label_style);
            let arrows = "‹  ›";
            let arrows_x = inner
                .right()
                .saturating_sub(unicode_width::UnicodeWidthStr::width(arrows) as u16);
            let value = action.name();
            let value_x =
                arrows_x.saturating_sub(unicode_width::UnicodeWidthStr::width(value) as u16 + 2);
            ctx.buf.set_string(value_x, y, value, value_style);
            ctx.buf.set_string(arrows_x, y, arrows, arrow_style);
            y += 1;
        }

        // The pairing step, shown until a Sony pad is seen at all, so a person
        // who has just taken the pad out of the drawer knows what to do before
        // pairing it.
        if let Some(text) = pairing_note(&self.pads) {
            for (offset, line) in crate::system::manual::wrap(text, usize::from(inner.width))
                .into_iter()
                .take(3)
                .enumerate()
            {
                let note_y = y + 1 + offset as u16;
                if note_y >= inner.bottom().saturating_sub(2) {
                    break;
                }
                put_trunc(
                    ctx.buf,
                    inner.x,
                    note_y,
                    &line,
                    inner.width,
                    ctx.design.style(TextStyle::Dim),
                );
            }
        }

        let hint = "↑/↓ row · ←/→ change · Enter next · r reset · Esc back";
        put_trunc(
            ctx.buf,
            inner.x,
            inner.bottom().saturating_sub(2),
            hint,
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
        let note = if self.note.is_empty() {
            "Stick = arrows · Select+Start = Escape".to_string()
        } else {
            self.note.clone()
        };
        put_trunc(
            ctx.buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            &note,
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
    }

    fn key(&mut self, key: KeyEvent, _design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Done(lazyos_ui::app::exit::ESCAPE),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_row(-1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_row(1);
                Outcome::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.change(-1);
                Outcome::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate();
                Outcome::Consumed
            }
            KeyCode::Char('r') => {
                self.reset();
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some("controller")
    }
}

/// The pairing line, or nothing once a Sony pad is known: a pad that is
/// already connected does not need the instruction. A DualShock 4 is the
/// Bluetooth case (PS + Share); a DualShock 3 is the USB exception.
fn pairing_note(pads: &[PadInfo]) -> Option<&'static str> {
    let playstation = pads.iter().any(|pad| pad.vendor == 0x054c);
    (!playstation).then_some(
        "PS4 pad: hold PS + Share until the light bar flashes, then pair it in Settings → Bluetooth. \
         A PS3 pad pairs once over USB: plug it in, press PS, unplug it and press PS again.",
    )
}

/// `lazyos menu controller --rows`: the effective table, one button per line,
/// for `check-desktop.sh` and for reading the file without opening a window.
pub fn rows() -> Vec<String> {
    let bindings = Bindings::load();
    Button::ALL
        .into_iter()
        .map(|button| {
            format!(
                "{}\t{}\t{}",
                button.name(),
                button.label(),
                bindings.get(button).name()
            )
        })
        .collect()
}

/// Open the screen. The pad list is read once, when the window opens; the
/// mapper watches the file, so the screen does not need to watch it too.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut screen = ControllerScreen::new(Bindings::load(), Bindings::path(), pads());
    crate::run_screen(terminal, design, "Controller", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::padmap::Action;

    fn screen(path: Option<PathBuf>) -> ControllerScreen {
        ControllerScreen::new(Bindings::default(), path, Vec::new())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, ratatui::crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn every_button_is_a_row_and_the_last_row_resets() {
        assert_eq!(ROWS, Button::ALL.len() + 1);
        let screen = screen(None);
        assert_eq!(screen.row, 0);
        assert_eq!(screen.bindings.get(Button::ALL[0]), Action::Up);
    }

    #[test]
    fn the_arrows_change_the_action_without_a_pad() {
        let mut screen = screen(None);
        let design = Design::for_session(None);
        // Cross / A starts as Enter; one step right is Escape.
        let south = Button::ALL
            .iter()
            .position(|button| *button == Button::South)
            .expect("south is in the table");
        screen.row = south;
        screen.key(key(KeyCode::Right), &design);
        assert_eq!(screen.bindings.get(Button::South), Action::Escape);
        screen.key(key(KeyCode::Left), &design);
        assert_eq!(screen.bindings.get(Button::South), Action::Enter);
    }

    #[test]
    fn reset_is_one_key_and_the_save_is_attempted() {
        let path = std::env::temp_dir().join(format!(
            "lazyos-controller-{}-reset.json",
            std::process::id()
        ));
        let mut screen = screen(Some(path.clone()));
        let design = Design::for_session(None);
        screen.bindings.set(Button::South, Action::Options);
        screen.row = RESET_ROW;
        screen.key(key(KeyCode::Enter), &design);
        assert_eq!(screen.bindings.get(Button::South), Action::Enter);
        assert!(path.is_file(), "the reset was not written");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_pairing_note_names_the_playstation_cases() {
        let note = pairing_note(&[]).expect("a note when no pad is connected");
        assert!(note.contains("PS4"), "the Bluetooth case must be first");
        assert!(note.contains("PS + Share"));
        assert!(note.contains("PS3"));
        assert!(note.contains("USB"));
        let xbox = PadInfo {
            name: "Xbox Wireless Controller".to_string(),
            bus: evdev::BusType::BUS_BLUETOOTH,
            vendor: 0x045e,
            product: 0x0b13,
        };
        assert!(pairing_note(&[xbox]).is_some());
        let ps4 = PadInfo {
            name: "Wireless Controller".to_string(),
            bus: evdev::BusType::BUS_BLUETOOTH,
            vendor: 0x054c,
            product: 0x09cc,
        };
        assert!(pairing_note(&[ps4]).is_none());
    }

    #[test]
    fn a_pad_line_names_the_absence_or_the_pad() {
        let screen = screen(None);
        let (line, _) = screen.pad_line();
        assert!(line.contains("No controller connected"));
        assert!(line.contains("defaults below"));
    }
}
