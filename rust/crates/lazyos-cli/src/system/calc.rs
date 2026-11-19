//! Calculator: an expression parser, not a dmenu prompt (`lazyos-calc`).
//!
//! The old script was one prompt: type it, Enter evaluates, the result goes to
//! the clipboard and a notification. a dmenu cannot do a live preview (a script
//! mode is called once at startup and once on selection, never while the query
//! changes), so this is the honest version: a parser that evaluates on Enter,
//! with history and a copy action.
//!
//! The parser is a small Pratt parser that understands:
//! - operators: + - * / % ^ (right-associative)
//! - parentheses
//! - numbers (integers and floats)
//! - unary minus
//! - constants: pi, e, tau
//! - functions: sin, cos, tan, sqrt, log, ln, abs, floor, ceil, round
//! - history: last 50 results, each entry is "expr = result"

#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;
use std::collections::VecDeque;

use lazyos_ui::{
    BodyCtx, Card, ColorRole, Design, Glyph, Header, Input, KeyHint, ListAction, ListState,
    Outcome, Row, Screen, TextStyle, Urgency, draw_badge, draw_card,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::paths::Paths;
use crate::system::{have, missing};

const MAX_HISTORY: usize = 50;

/// The programs the calculator needs.
pub const NEEDED: &[&str] = &["wl-copy"];

/// The calculator state.
pub struct CalcScreen {
    input: Input,
    history: VecDeque<String>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status_note: Option<String>,
}

impl CalcScreen {
    pub fn new() -> Self {
        let history = Self::load_history();
        let mut list = ListState::new()
            .with_empty("no history yet - type an expression and press Enter")
            .ranked();
        list.set_rows(Self::history_rows(&history));

        Self {
            input: Input::new(),
            history,
            list,
            header: Header::new("LazyOS").subtitle("Calculator"),
            keybar: vec![
                KeyHint::new("Enter", "save & copy"),
                KeyHint::new("type", "live evaluate"),
                KeyHint::new("\u{2191}\u{2193}", "history"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            status_note: None,
        }
    }

    fn history_rows(history: &VecDeque<String>) -> Vec<Row> {
        history
            .iter()
            .map(|h| {
                if let Some(eq_pos) = h.find(" = ") {
                    let expr = &h[..eq_pos];
                    let res = &h[eq_pos + 3..];
                    Row::new(expr)
                        .glyph(Glyph::Calculator)
                        .detail("Enter copies result")
                        .value(format!("= {res}"))
                } else {
                    Row::new(h.as_str()).glyph(Glyph::Calculator)
                }
            })
            .collect()
    }

    fn load_history() -> VecDeque<String> {
        let path = Paths::resolve(&std::env::vars().collect())
            .ok()
            .map(|p| p.state.join("calc-history"));
        path.and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| {
                s.lines()
                    .rev()
                    .take(MAX_HISTORY)
                    .map(|l| l.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn save_history(&self) {
        if let Ok(paths) = Paths::resolve(&std::env::vars().collect()) {
            let path = paths.state.join("calc-history");
            let _ = std::fs::write(
                path,
                self.history
                    .iter()
                    .rev()
                    .map(|h| format!("{h}\n"))
                    .collect::<String>(),
            );
        }
    }

    fn evaluate(expr: &str) -> Result<String, String> {
        let mut parser = Parser::new(expr);
        let value = parser.parse()?;
        // Format with up to 9 decimal places, removing trailing zeros
        let formatted = format!("{value:.9}");
        if formatted.contains('.') {
            Ok(formatted
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string())
        } else {
            Ok(formatted)
        }
    }

    fn add_to_history(&mut self, expr: &str, result: &str) {
        let entry = format!("{expr} = {result}");
        self.history.push_front(entry);
        if self.history.len() > MAX_HISTORY {
            self.history.pop_back();
        }
        self.list.set_rows(Self::history_rows(&self.history));
        self.save_history();
    }

    fn copy_result(&mut self, entry_label: &str, result: &str) {
        if have("wl-copy") {
            let _ = std::process::Command::new("wl-copy")
                .arg(result)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        self.status_note = Some(format!("Copied {result} ({entry_label}) to clipboard"));
    }
}

impl Screen for CalcScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("History"), true);
        let (hero, rows) = Self::layout(ctx.area);

        let expr = self.input.text().trim();
        let mut x = draw_badge(
            ctx.buf,
            hero.x,
            hero.y,
            ctx.design,
            "EXPR",
            ColorRole::Accent,
        );
        x += 1;
        let input_area = Rect {
            x,
            y: hero.y,
            width: hero.right().saturating_sub(x),
            height: 1,
        };
        self.input.render_styled(
            input_area,
            ctx.buf,
            ctx.design,
            "type an expression: 2 + 3 * 4, 2**10, 17 // 3, sin(pi/2), 0xFF…",
            ColorRole::Bg,
            TextStyle::Title,
        );

        if hero.height >= 2 {
            let y2 = hero.y + 1;
            if expr.is_empty() {
                let note = self
                    .status_note
                    .as_deref()
                    .unwrap_or("Live evaluation · Enter saves & copies result to clipboard");
                ctx.buf
                    .set_string(hero.x, y2, note, ctx.design.style(TextStyle::Dim));
            } else {
                match Self::evaluate(expr) {
                    Ok(res) => {
                        let mut rx =
                            draw_badge(ctx.buf, hero.x, y2, ctx.design, "= LIVE", ColorRole::Ok);
                        rx += 2;
                        ctx.buf
                            .set_string(rx, y2, &res, ctx.design.style(TextStyle::Title));
                        if let Ok(iv) = res.parse::<i64>() {
                            let bases = format!("  (0x{iv:X} · 0b{iv:b})");
                            let bx = rx + res.chars().count() as u16;
                            if bx < hero.right() {
                                ctx.buf
                                    .set_string(bx, y2, bases, ctx.design.style(TextStyle::Dim));
                            }
                        }
                    }
                    Err(err) => {
                        ctx.buf.set_string(
                            hero.x,
                            y2,
                            format!("… {err}"),
                            ctx.design.style(TextStyle::Dim),
                        );
                    }
                }
            }
        }

        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let rows_height = self.rows_height.get();
        if matches!(
            key.code,
            KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
        ) {
            let _ = self.input.handle(key);
            return Outcome::Consumed;
        }
        match key.code {
            KeyCode::Enter => {
                let expr = self.input.text().trim().to_string();
                if !expr.is_empty() {
                    match Self::evaluate(&expr) {
                        Ok(result) => {
                            self.add_to_history(&expr, &result);
                            self.copy_result(&expr, &result);
                            self.input.set("");
                            return Outcome::Consumed;
                        }
                        Err(e) => {
                            self.status_note = Some(format!("Invalid expression: {e}"));
                            return Outcome::Consumed;
                        }
                    }
                } else if let Some(index) = self.list.selected_index() {
                    if let Some(entry) = self.history.get(index).cloned() {
                        if let Some(eq_pos) = entry.find(" = ") {
                            let expr_part = &entry[..eq_pos];
                            let result = &entry[eq_pos + 3..];
                            self.copy_result(expr_part, result);
                            return Outcome::Consumed;
                        }
                    }
                }
            }
            KeyCode::Esc => {
                if !self.input.text().is_empty() {
                    self.input.set("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => {
                if let Some(entry) = self.history.get(index).cloned() {
                    if let Some(eq_pos) = entry.find(" = ") {
                        let expr_part = &entry[..eq_pos];
                        let result = &entry[eq_pos + 3..];
                        self.copy_result(expr_part, result);
                    }
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                if let Some(entry) = self.history.get(index).cloned() {
                    if let Some(eq_pos) = entry.find(" = ") {
                        let expr_part = &entry[..eq_pos];
                        let result = &entry[eq_pos + 3..];
                        self.copy_result(expr_part, result);
                    }
                }
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, rows) = Self::layout(area);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((740, 460))
    }
}

impl CalcScreen {
    fn layout(area: Rect) -> (Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let hero_h = 2.min(inner.height);
        let hero = Rect {
            height: hero_h,
            ..inner
        };
        let rows = Rect {
            y: inner.y + hero_h + 1,
            height: inner.height.saturating_sub(hero_h + 1),
            ..inner
        };
        (hero, rows)
    }
}

/// A Pratt parser for simple arithmetic expressions.
struct Parser {
    input: String,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            input: input.to_string(),
            pos: 0,
        }
    }

    fn parse(&mut self) -> Result<f64, String> {
        self.skip_whitespace();
        let value = self.parse_expression(0)?;
        self.skip_whitespace();
        if self.pos < self.input.len() {
            return Err(format!("unexpected character: {:?}", self.current_char()));
        }
        Ok(value)
    }

    fn parse_expression(&mut self, min_prec: u8) -> Result<f64, String> {
        let mut lhs = self.parse_prefix()?;
        self.skip_whitespace();
        while let Some((op, token_len)) = self.peek_operator() {
            let (prec, assoc) = op.precedence();
            if prec < min_prec {
                break;
            }
            for _ in 0..token_len {
                self.advance();
            }
            let next_min_prec = if matches!(assoc, Assoc::Right) {
                prec
            } else {
                prec + 1
            };
            let rhs = self.parse_expression(next_min_prec)?;
            lhs = op.apply(lhs, rhs)?;
            self.skip_whitespace();
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<f64, String> {
        self.skip_whitespace();
        match self.current_char() {
            Some('(') => {
                self.advance();
                let value = self.parse_expression(0)?;
                self.skip_whitespace();
                if self.current_char() != Some(')') {
                    return Err("expected ')'".into());
                }
                self.advance();
                Ok(value)
            }
            Some('-') => {
                self.advance();
                Ok(-self.parse_prefix()?)
            }
            Some('+') => {
                self.advance();
                self.parse_prefix()
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() => self.parse_function_or_constant(),
            _ => Err("expected number, function, or '('".into()),
        }
    }

    fn parse_number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        if self.input[start..].starts_with("0x") || self.input[start..].starts_with("0X") {
            self.advance();
            self.advance();
            let hex_start = self.pos;
            while let Some(c) = self.current_char() {
                if c.is_ascii_hexdigit() || c == '_' {
                    self.advance();
                } else {
                    break;
                }
            }
            let raw = self.input[hex_start..self.pos].replace('_', "");
            let v = u64::from_str_radix(&raw, 16)
                .map_err(|_| format!("invalid hex number: 0x{raw}"))?;
            return Ok(v as f64);
        }
        if self.input[start..].starts_with("0b") || self.input[start..].starts_with("0B") {
            self.advance();
            self.advance();
            let bin_start = self.pos;
            while let Some(c) = self.current_char() {
                if c == '0' || c == '1' || c == '_' {
                    self.advance();
                } else {
                    break;
                }
            }
            let raw = self.input[bin_start..self.pos].replace('_', "");
            let v = u64::from_str_radix(&raw, 2)
                .map_err(|_| format!("invalid binary number: 0b{raw}"))?;
            return Ok(v as f64);
        }
        if self.input[start..].starts_with("0o") || self.input[start..].starts_with("0O") {
            self.advance();
            self.advance();
            let oct_start = self.pos;
            while let Some(c) = self.current_char() {
                if ('0'..='7').contains(&c) || c == '_' {
                    self.advance();
                } else {
                    break;
                }
            }
            let raw = self.input[oct_start..self.pos].replace('_', "");
            let v = u64::from_str_radix(&raw, 8)
                .map_err(|_| format!("invalid octal number: 0o{raw}"))?;
            return Ok(v as f64);
        }
        let mut has_dot = false;
        while let Some(c) = self.current_char() {
            if c.is_ascii_digit() || c == '_' {
                self.advance();
            } else if c == '.' && !has_dot {
                has_dot = true;
                self.advance();
            } else {
                break;
            }
        }
        let s = self.input[start..self.pos].replace('_', "");
        s.parse::<f64>().map_err(|_| format!("invalid number: {s}"))
    }

    fn parse_function_or_constant(&mut self) -> Result<f64, String> {
        let start = self.pos;
        while let Some(c) = self.current_char() {
            if c.is_ascii_alphabetic() {
                self.advance();
            } else {
                break;
            }
        }
        let name = &self.input[start..self.pos].to_lowercase();
        self.skip_whitespace();
        match name.as_str() {
            "pi" => Ok(std::f64::consts::PI),
            "e" => Ok(std::f64::consts::E),
            "tau" => Ok(std::f64::consts::TAU),
            "sin" => self.parse_unary_fn(f64::sin),
            "cos" => self.parse_unary_fn(f64::cos),
            "tan" => self.parse_unary_fn(f64::tan),
            "sqrt" => self.parse_unary_fn(f64::sqrt),
            "log" => self.parse_unary_fn(f64::log10),
            "ln" => self.parse_unary_fn(f64::ln),
            "abs" => self.parse_unary_fn(f64::abs),
            "floor" => self.parse_unary_fn(f64::floor),
            "ceil" => self.parse_unary_fn(f64::ceil),
            "round" => self.parse_unary_fn(f64::round),
            _ => Err(format!("unknown function or constant: {name}")),
        }
    }

    fn parse_unary_fn(&mut self, f: fn(f64) -> f64) -> Result<f64, String> {
        if self.current_char() != Some('(') {
            return Err(format!("expected '(' after function"));
        }
        self.advance();
        let value = self.parse_expression(0)?;
        self.skip_whitespace();
        if self.current_char() != Some(')') {
            return Err("expected ')'".into());
        }
        self.advance();
        Ok(f(value))
    }

    fn peek_operator(&self) -> Option<(Operator, usize)> {
        self.skip_whitespace_peek();
        let rest = &self.input[self.pos..];
        if rest.starts_with("**") {
            return Some((Operator::Pow, 2));
        }
        if rest.starts_with("//") {
            return Some((Operator::FloorDiv, 2));
        }
        match self.current_char() {
            Some('+') => Some((Operator::Add, 1)),
            Some('-') => Some((Operator::Sub, 1)),
            Some('*') => Some((Operator::Mul, 1)),
            Some('/') => Some((Operator::Div, 1)),
            Some('%') => Some((Operator::Mod, 1)),
            Some('^') => Some((Operator::Pow, 1)),
            _ => None,
        }
    }

    fn current_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) {
        if self.pos < self.input.len() {
            self.pos += self.input[self.pos..].chars().next().unwrap().len_utf8();
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.current_char() {
            if c.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_whitespace_peek(&self) {
        let mut pos = self.pos;
        while pos < self.input.len() {
            if self.input[pos..].chars().next().unwrap().is_whitespace() {
                pos += self.input[pos..].chars().next().unwrap().len_utf8();
            } else {
                break;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Operator {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

#[derive(Clone, Copy)]
enum Assoc {
    Left,
    Right,
}

impl Operator {
    fn precedence(&self) -> (u8, Assoc) {
        match self {
            Operator::Add | Operator::Sub => (1, Assoc::Left),
            Operator::Mul | Operator::Div | Operator::FloorDiv | Operator::Mod => (2, Assoc::Left),
            Operator::Pow => (3, Assoc::Right),
        }
    }

    fn apply(&self, lhs: f64, rhs: f64) -> Result<f64, String> {
        match self {
            Operator::Add => Ok(lhs + rhs),
            Operator::Sub => Ok(lhs - rhs),
            Operator::Mul => Ok(lhs * rhs),
            Operator::Div => {
                if rhs == 0.0 {
                    Err("division by zero".into())
                } else {
                    Ok(lhs / rhs)
                }
            }
            Operator::FloorDiv => {
                if rhs == 0.0 {
                    Err("division by zero".into())
                } else {
                    Ok((lhs / rhs).floor())
                }
            }
            Operator::Mod => Ok(lhs % rhs),
            Operator::Pow => Ok(lhs.powf(rhs)),
        }
    }
}

/// `lazyos menu calc` (`lazyos-calc`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Calculator",
            format!("{said} (wl-clipboard)"),
        ));
    }
    let mut screen = CalcScreen::new();
    let code = crate::run_screen(terminal, design, "Calculator", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_arithmetic() {
        assert_eq!(CalcScreen::evaluate("2 + 3 * 4").unwrap(), "14");
        assert_eq!(CalcScreen::evaluate("(2 + 3) * 4").unwrap(), "20");
        assert_eq!(CalcScreen::evaluate("2^10").unwrap(), "1024");
        assert_eq!(CalcScreen::evaluate("2**10").unwrap(), "1024");
        assert_eq!(CalcScreen::evaluate("17 // 3").unwrap(), "5");
        assert_eq!(CalcScreen::evaluate("0xFF + 0b1010").unwrap(), "265");
        assert_eq!(CalcScreen::evaluate("1_000 * 2").unwrap(), "2000");
        assert_eq!(CalcScreen::evaluate("10 / 3").unwrap(), "3.333333333");
    }

    #[test]
    fn unary_minus() {
        assert_eq!(CalcScreen::evaluate("-5").unwrap(), "-5");
        assert_eq!(CalcScreen::evaluate("2 * -3").unwrap(), "-6");
        assert_eq!(CalcScreen::evaluate("-(2 + 3)").unwrap(), "-5");
    }

    #[test]
    fn constants() {
        assert_eq!(CalcScreen::evaluate("pi").unwrap(), "3.141592654");
        assert_eq!(CalcScreen::evaluate("e").unwrap(), "2.718281828");
        assert_eq!(CalcScreen::evaluate("tau").unwrap(), "6.283185307");
    }

    #[test]
    fn functions() {
        assert_eq!(CalcScreen::evaluate("sin(pi/2)").unwrap(), "1");
        assert_eq!(CalcScreen::evaluate("cos(0)").unwrap(), "1");
        assert_eq!(CalcScreen::evaluate("sqrt(16)").unwrap(), "4");
        assert_eq!(CalcScreen::evaluate("log(100)").unwrap(), "2");
        assert_eq!(CalcScreen::evaluate("ln(e)").unwrap(), "1");
        assert_eq!(CalcScreen::evaluate("abs(-5)").unwrap(), "5");
        assert_eq!(CalcScreen::evaluate("floor(3.7)").unwrap(), "3");
        assert_eq!(CalcScreen::evaluate("ceil(3.2)").unwrap(), "4");
        assert_eq!(CalcScreen::evaluate("round(3.5)").unwrap(), "4");
    }

    #[test]
    fn invalid_expressions() {
        assert!(CalcScreen::evaluate("2 +").is_err());
        assert!(CalcScreen::evaluate("(2 + 3").is_err());
        assert!(CalcScreen::evaluate("unknown()").is_err());
        assert!(CalcScreen::evaluate("5 / 0").is_err());
    }
}
