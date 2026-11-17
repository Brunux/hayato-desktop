//! One window for a question with answers: the shape every system picker has.
//!
//! The nine dialogs were nine `a dmenu` rounds with nine ways of saying
//! "that did not work". They are one screen here: a card, a note line, the
//! rows, and - when the next answer writes a disk - a [`Confirm`] whose first
//! answer is Cancel. A picker is a short queue of questions, so the format
//! flow (disk, filesystem, are you sure) is three steps with no nested state,
//! and Escape steps back one question at a time, exactly like the menu's
//! confirm does.
//!
//! Two rules make it safe rather than pretty:
//!
//! - **A destructive answer is a [`Confirm`]**, never a row: a row is one
//!   Enter away, and the shell's answer was to put Cancel first in a list
//!   whose first row is selected - the same idea with a title on it.
//! - **The answers are strings from the questions, never a shell line.** The
//!   caller builds [`Step`](super::step::Step)s from them, so a device name or
//!   a filesystem is one argument.

use std::cell::Cell;

use lazyos_ui::widgets::put_trunc;
use lazyos_ui::{
    BodyCtx, Confirm, ConfirmAction, Design, Glyph, Header, Input, InputAction, KeyHint,
    ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;

/// The inside of a card split into its note line and the rows under it: the
/// one layout every list in a card uses, so a new picker cannot draw its rows
/// a row higher than the menu's.
pub fn list_layout(area: Rect) -> (Rect, Rect) {
    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let filter = Rect {
        height: 1.min(inner.height),
        ..inner
    };
    let rows = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    (filter, rows)
}

/// One question in a picker.
pub enum Question {
    /// Pick a row; `keys[i]` is the answer `rows[i]` gives.
    Choose {
        title: String,
        note: String,
        rows: Vec<Row>,
        keys: Vec<String>,
    },
    /// Rows built dynamically from the previous answer (e.g., the size
    /// question for swap depends on the chosen disk's capacity). The builder
    /// receives the answers so far and returns (rows, keys).
    #[allow(clippy::type_complexity)]
    Rows {
        title: String,
        note: String,
        build: fn(&[String]) -> (Vec<Row>, Vec<String>),
    },
    /// The answer that writes something: Cancel is selected, and the verb is
    /// only reachable by moving to it.
    Ask {
        title: String,
        note: String,
        message: String,
        verb: String,
        /// The value this question answers with once the verb is chosen.
        answer: String,
    },
    /// A single-line text input question (e.g. Wi-Fi password or download URL).
    Input {
        title: String,
        note: String,
        placeholder: String,
        masked: bool,
    },
}

impl Question {
    pub fn choose(
        title: impl Into<String>,
        note: impl Into<String>,
        rows: Vec<Row>,
        keys: Vec<String>,
    ) -> Self {
        Question::Choose {
            title: title.into(),
            note: note.into(),
            rows,
            keys,
        }
    }

    pub fn input(
        title: impl Into<String>,
        note: impl Into<String>,
        placeholder: impl Into<String>,
        masked: bool,
    ) -> Self {
        Question::Input {
            title: title.into(),
            note: note.into(),
            placeholder: placeholder.into(),
            masked,
        }
    }

    /// The last question of a destructive picker. `answer` is what the caller
    /// reads back to know the person said yes.
    #[allow(clippy::type_complexity)]
    pub fn rows(
        title: impl Into<String>,
        note: impl Into<String>,
        build: fn(&[String]) -> (Vec<Row>, Vec<String>),
    ) -> Self {
        Question::Rows {
            title: title.into(),
            note: note.into(),
            build,
        }
    }

    pub fn ask(
        title: impl Into<String>,
        note: impl Into<String>,
        message: impl Into<String>,
        verb: impl Into<String>,
        answer: impl Into<String>,
    ) -> Self {
        Question::Ask {
            title: title.into(),
            note: note.into(),
            message: message.into(),
            verb: verb.into(),
            answer: answer.into(),
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Question::Choose { title, .. }
            | Question::Ask { title, .. }
            | Question::Rows { title, .. }
            | Question::Input { title, .. } => title,
        }
    }

    pub fn note(&self) -> &str {
        match self {
            Question::Choose { note, .. }
            | Question::Ask { note, .. }
            | Question::Rows { note, .. }
            | Question::Input { note, .. } => note,
        }
    }
}

/// The one question the window is on, with the widget that answers it.
///
/// The current question used to be spread across `at`, `list`, `confirm` and
/// `input`: two widgets could exist at once, and either could belong to a
/// different question than `at` named. A picker now holds one variant, so
/// "which question" and "what answers it" cannot disagree.
enum Current {
    Choose {
        at: usize,
        list: ListState,
    },
    Rows {
        at: usize,
        list: ListState,
    },
    Ask {
        at: usize,
        confirm: Confirm,
    },
    Input {
        at: usize,
        input: Input,
    },
    /// No question is loaded: an empty picker, or the last answer was given.
    Empty,
}

impl Current {
    fn at(&self) -> Option<usize> {
        match self {
            Self::Choose { at, .. }
            | Self::Rows { at, .. }
            | Self::Ask { at, .. }
            | Self::Input { at, .. } => Some(*at),
            Self::Empty => None,
        }
    }
}

/// A queue of questions in one window. The window ends with
/// [`Outcome::Done(0)`](Outcome) and [`Picker::answers`] when every question
/// answered, or `Done(1)` and nothing when the person backed out of the first
/// one.
pub struct Picker {
    name: String,
    questions: Vec<Question>,
    current: Current,
    answers: Vec<String>,
    header: Header,
    /// The current question's title and note with the answers already given
    /// filled in, so `Erase {0}` becomes `Erase /dev/sda` before it is drawn.
    subtitle: String,
    note: String,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

/// What a person answered, once the window has closed.
pub type Answers = Vec<String>;

impl Picker {
    pub fn new(title: &str, questions: Vec<Question>) -> Self {
        let mut picker = Self {
            name: title.to_string(),
            questions,
            current: Current::Empty,
            answers: Vec::new(),
            header: Header::new(title.to_string()),
            subtitle: String::new(),
            note: String::new(),
            keybar: Vec::new(),
            rows_height: Cell::new(20),
        };
        picker.load(0);
        picker
    }

    /// The answers in the order the questions asked them, only after the
    /// window ended with `Done(0)`.
    pub fn answers(&self) -> &Answers {
        &self.answers
    }

    /// Set the answers directly (used when reconstructing a picker with
    /// pre-filled answers, e.g., after the first question is answered).
    pub fn set_answers(&mut self, answers: Vec<String>) {
        self.answers = answers;
    }

    fn load(&mut self, at: usize) {
        let answers = self.answers.clone();
        let (current, subtitle, status, keys): (
            Current,
            String,
            String,
            Vec<(&'static str, &'static str)>,
        ) = match self.questions.get(at) {
            Some(Question::Choose { title, rows, .. }) => {
                let mut list = ListState::new();
                list.set_rows(rows.clone());
                // The list itself is the safety, so the status says so
                // once instead of repeating the keybar's two words: after
                // this row the picker asks again, and only the last
                // question writes anything.
                let status = if matches!(
                    self.questions.last(),
                    Some(Question::Ask { .. }) | Some(Question::Rows { .. })
                ) {
                    "nothing is written until the last question".to_string()
                } else {
                    "Enter picks · Escape goes back".to_string()
                };
                (
                    Current::Choose { at, list },
                    fill(title, &answers),
                    status,
                    vec![("Enter", "pick"), ("Esc", "back")],
                )
            }
            Some(Question::Rows { title, build, .. }) => {
                let (rows, _keys) = build(&answers);
                let mut list = ListState::new();
                list.set_rows(rows);
                let status = if matches!(self.questions.last(), Some(Question::Ask { .. })) {
                    "nothing is written until the last question".to_string()
                } else {
                    "Enter picks · Escape goes back".to_string()
                };
                (
                    Current::Rows { at, list },
                    fill(title, &answers),
                    status,
                    vec![("Enter", "pick"), ("Esc", "back")],
                )
            }
            Some(Question::Ask {
                title,
                message,
                verb,
                ..
            }) => {
                let confirm = Confirm::new(fill(title, &answers), fill(message, &answers))
                    .with_options(&["Cancel", fill(verb, &answers).as_str()]);
                (
                    Current::Ask { at, confirm },
                    fill(title, &answers),
                    "Cancel is selected".to_string(),
                    vec![
                        ("\u{2190}\u{2192}", "choose"),
                        ("Enter", "answer"),
                        ("Esc", "cancel"),
                    ],
                )
            }
            Some(Question::Input { title, masked, .. }) => {
                let input = Input::new().with_masked(*masked);
                (
                    Current::Input { at, input },
                    fill(title, &answers),
                    "Type and press Enter · Escape goes back".to_string(),
                    vec![("Enter", "confirm"), ("Esc", "back")],
                )
            }
            None => (Current::Empty, String::new(), String::new(), Vec::new()),
        };
        self.current = current;
        self.subtitle = subtitle;
        self.note = self
            .questions
            .get(at)
            .map(|q| fill(q.note(), &answers))
            .unwrap_or_default();
        self.header = Header::new(self.name.clone())
            .subtitle(self.subtitle.clone())
            .status(status);
        self.keybar = keys
            .into_iter()
            .map(|(key, label)| KeyHint::new(key, label))
            .collect();
    }

    /// Record the answer to the current question and move on, or finish.
    fn advance(&mut self, answer: String) -> Outcome {
        let Some(at) = self.current.at() else {
            return Outcome::Continue;
        };
        self.answers.truncate(at);
        self.answers.push(answer);
        let next = at + 1;
        if next >= self.questions.len() {
            self.current = Current::Empty;
            return Outcome::Done(0);
        }
        self.load(next);
        Outcome::Consumed
    }

    /// One question back; at the first one it is the end, with nothing done.
    fn back(&mut self) -> Outcome {
        let Some(at) = self.current.at() else {
            return Outcome::Done(1);
        };
        if at == 0 {
            return Outcome::Done(1);
        }
        self.answers.truncate(at - 1);
        self.load(at - 1);
        Outcome::Consumed
    }

    fn list(&self) -> Option<&ListState> {
        match &self.current {
            Current::Choose { list, .. } | Current::Rows { list, .. } => Some(list),
            _ => None,
        }
    }

    fn list_mut(&mut self) -> Option<&mut ListState> {
        match &mut self.current {
            Current::Choose { list, .. } | Current::Rows { list, .. } => Some(list),
            _ => None,
        }
    }
}

impl Screen for Picker {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        if matches!(self.current, Current::Empty) {
            return;
        }
        draw_card(ctx.buf, ctx.area, ctx.design, Some(&self.name), true);
        let (filter, rows) = list_layout(ctx.area);
        match &mut self.current {
            Current::Choose { list, .. } | Current::Rows { list, .. } => {
                if list.filter().is_empty() {
                    put_trunc(
                        ctx.buf,
                        filter.x,
                        filter.y,
                        &self.note,
                        filter.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                } else {
                    draw_filter_bar(
                        ctx.buf,
                        filter,
                        ctx.design,
                        list.filter(),
                        &self.note,
                        list.visible_len(),
                        list.len(),
                    );
                }
                self.rows_height.set(rows.height);
                list.render(rows, ctx.buf, ctx.design, true);
            }
            Current::Ask { confirm, .. } => {
                put_trunc(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    &self.note,
                    filter.width,
                    ctx.design.style(TextStyle::Dim),
                );
                confirm.render(ctx.area, ctx.buf, ctx.design);
            }
            Current::Input { at, input } => {
                put_trunc(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    &self.note,
                    filter.width,
                    ctx.design.style(TextStyle::Dim),
                );
                let placeholder = match self.questions.get(*at) {
                    Some(Question::Input { placeholder, .. }) => placeholder.as_str(),
                    _ => "",
                };
                let input_rect = Rect {
                    x: filter.x,
                    y: filter.y + 2,
                    width: filter.width,
                    height: 1,
                };
                input.render(input_rect, ctx.buf, ctx.design, placeholder);
            }
            Current::Empty => {}
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if matches!(self.current, Current::Empty) {
            return Outcome::Done(1);
        }

        if matches!(self.current, Current::Choose { .. } | Current::Rows { .. }) {
            if key.code == KeyCode::Esc {
                let filtered = self.list().is_some_and(|list| !list.filter().is_empty());
                if filtered {
                    if let Some(list) = self.list_mut() {
                        list.set_filter("");
                    }
                    return Outcome::Consumed;
                }
                return self.back();
            }
            let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                && !key
                    .modifiers
                    .contains(ratatui::crossterm::event::KeyModifiers::CONTROL);
            if typed {
                if let Some(list) = self.list_mut() {
                    list.type_filter(key);
                }
                return Outcome::Continue;
            }
            let rows_height = self.rows_height.get();
            let activated =
                self.list_mut()
                    .and_then(|list| match list.handle(key, rows_height, design) {
                        ListAction::Activated(index) => Some(index),
                        _ => None,
                    });
            if let Some(index) = activated
                && let Some(answer) = self.answers_for(index)
            {
                return self.advance(answer);
            }
            return Outcome::Continue;
        }

        match &mut self.current {
            Current::Ask { at, confirm } => {
                let answer = match self.questions.get(*at) {
                    Some(Question::Ask { answer, .. }) => answer.clone(),
                    _ => return Outcome::Continue,
                };
                let action = confirm.handle(key);
                match action {
                    ConfirmAction::Chosen(1) => self.advance(answer),
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => self.back(),
                    ConfirmAction::None => Outcome::Continue,
                }
            }
            Current::Input { input, .. } => {
                let action = input.handle(key);
                match action {
                    InputAction::Submitted => {
                        let text = input.text().to_string();
                        self.advance(text)
                    }
                    InputAction::Cancelled => self.back(),
                    _ => Outcome::Continue,
                }
            }
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(row) = row else {
            return Outcome::Continue;
        };
        let Some(list) = self.list_mut() else {
            return Outcome::Continue;
        };
        list.set_cursor(row);
        let index = list.selected_index();
        match index.and_then(|index| self.answers_for(index)) {
            Some(answer) => self.advance(answer),
            None => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if !matches!(self.current, Current::Choose { .. }) {
            return None;
        }
        self.list()
            .and_then(|list| list.hit_row(list_layout(area).1, x, y, design))
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl Picker {
    /// The answer the row at `index` (an index into the unfiltered rows, which
    /// is what [`ListAction::Activated`] carries) gives.
    fn answers_for(&self, index: usize) -> Option<String> {
        let at = self.current.at()?;
        let Question::Choose { keys, .. } = self.questions.get(at)? else {
            return None;
        };
        keys.get(index).cloned()
    }
}

/// A row with a glyph and its dim detail, the shape every picker uses.
pub fn row(label: impl Into<String>, detail: impl Into<String>, glyph: Glyph) -> Row {
    Row::new(label).glyph(glyph).detail(detail)
}

/// `Erase {0}` with the answers already given: `{0}` is the first one, `{1}`
/// the second. A placeholder with no answer behind it stays as it was written,
/// which is how a question about the first answer reads before it exists.
pub fn fill(text: &str, answers: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}')
            && let Ok(index) = after[..close].parse::<usize>()
            && let Some(answer) = answers.get(index)
        {
            out.push_str(answer);
            rest = &after[close + 1..];
            continue;
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazyos_ui::{App, Design};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn drive(picker: &mut Picker, keys: &[KeyEvent]) -> Outcome {
        let mut app = App::new(Header::new("LazyOS"))
            .design(Design::solarized().with_reduced_motion(true))
            .screen(picker);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut outcome = Outcome::Continue;
        for key in keys {
            terminal.draw(|frame| app.render(frame)).unwrap();
            outcome = app.handle(ratatui::crossterm::event::Event::Key(*key));
            if let Outcome::Done(_) = outcome {
                break;
            }
        }
        outcome
    }

    fn two() -> Picker {
        Picker::new(
            "Test",
            vec![
                Question::choose(
                    "Disk",
                    "pick one",
                    vec![row("/dev/sda", "29.5G", Glyph::Disk)],
                    vec!["sda".to_string()],
                ),
                Question::ask(
                    "Erase?",
                    "nothing is written until the second answer",
                    "every file on it is lost",
                    "Erase",
                    "go",
                ),
            ],
        )
    }

    #[test]
    fn a_picker_answers_every_question_in_order() {
        let mut picker = two();
        // Enter on the row, then Right and Enter on the confirm.
        let keys = [
            press(KeyCode::Enter),
            press(KeyCode::Right),
            press(KeyCode::Enter),
        ];
        assert_eq!(drive(&mut picker, &keys), Outcome::Done(0));
        assert_eq!(picker.answers(), &["sda".to_string(), "go".to_string()]);
    }

    #[test]
    fn a_stray_enter_at_the_confirm_cancels_and_steps_back() {
        let mut picker = two();
        let keys = [press(KeyCode::Enter), press(KeyCode::Enter)];
        assert_eq!(drive(&mut picker, &keys), Outcome::Continue);
        assert!(picker.answers().is_empty());
    }

    #[test]
    fn escape_steps_back_one_question_and_then_closes_with_nothing() {
        let mut picker = two();
        let keys = [press(KeyCode::Enter), press(KeyCode::Esc)];
        assert_eq!(drive(&mut picker, &keys), Outcome::Continue);
        let keys = [press(KeyCode::Esc)];
        assert_eq!(drive(&mut picker, &keys), Outcome::Done(1));
        assert!(picker.answers().is_empty());
    }

    #[test]
    fn the_filter_narrows_the_rows_and_the_answer_follows_the_row() {
        let mut picker = Picker::new(
            "Test",
            vec![Question::choose(
                "Disk",
                "pick one",
                vec![
                    row("/dev/sda", "a stick", Glyph::Disk),
                    row("/dev/sdb", "another", Glyph::Disk),
                ],
                vec!["sda".to_string(), "sdb".to_string()],
            )],
        );
        let keys = [
            press(KeyCode::Char('s')),
            press(KeyCode::Char('d')),
            press(KeyCode::Char('b')),
            press(KeyCode::Enter),
        ];
        assert_eq!(drive(&mut picker, &keys), Outcome::Done(0));
        assert_eq!(picker.answers(), &["sdb".to_string()]);
    }

    #[test]
    fn a_confirm_only_picker_cancels_straight_out() {
        let mut picker = Picker::new(
            "Test",
            vec![Question::ask(
                "Turn it off?",
                "the card is not touched",
                "no swap from that card at the next boot",
                "Turn it off",
                "go",
            )],
        );
        assert_eq!(
            drive(&mut picker, &[press(KeyCode::Enter)]),
            Outcome::Done(1)
        );
        assert!(picker.answers().is_empty());
    }

    #[test]
    fn the_picker_holds_one_widget_for_the_current_question() {
        let mut picker = two();
        assert!(matches!(picker.current, Current::Choose { at: 0, .. }));
        let _ = drive(&mut picker, &[press(KeyCode::Enter)]);
        assert!(matches!(picker.current, Current::Ask { at: 1, .. }));
    }

    #[test]
    fn a_question_fills_in_the_answers_before_it() {
        // The confirm names the device and the filesystem the person chose,
        // so it reads as one sentence and not as three questions in a row.
        assert_eq!(
            fill("Erase {0} as {1}?", &["/dev/sda".into(), "exfat".into()]),
            "Erase /dev/sda as exfat?"
        );
        // Before the second answer exists the placeholder is still literal.
        assert_eq!(
            fill("Erase {0} as {1}?", &["/dev/sda".into()]),
            "Erase /dev/sda as {1}?"
        );
        // A brace that is not a placeholder is text.
        assert_eq!(fill("a { b } 100%", &[]), "a { b } 100%");
    }
}
