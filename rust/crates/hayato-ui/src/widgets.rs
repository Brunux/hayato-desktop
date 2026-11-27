//! The widgets the shell is made of.
//!
//! Every one of them draws into a `Buffer` and allocates nothing: the text a row
//! shows is built once, when the command's state changes, and a frame only
//! copies cells (`optimization.md` §13 - rebuilding 40 formatted rows per frame
//! costs 445 µs against 231 µs for one changing cell, and the difference is
//! allocations, not ratatui).
//!
//! The contract each widget keeps with the rest of the system:
//!
//! - colours and text styles come from `Design` (spec 002's tokens);
//! - the density decides the row stride, so a hit region and the row it names
//!   are the same arithmetic - `ListState::hit_row` is the one that answers, and
//!   a click test drives it at both densities;
//! - the key bar carries the way out, because a window with no visible escape
//!   is the failure the session's manifests keep hitting.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::design::{Border, ColorRole, Design, TextStyle};
use crate::glyphs::Glyph;

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

/// Write `text` at `(x, y)` and return the column after it.
pub fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, max: u16, style: Style) -> u16 {
    if max == 0 || y >= buf.area.bottom() || x >= buf.area.right() {
        return x;
    }
    buf.set_stringn(x, y, text, max as usize, style).0
}

/// Write `text`, replacing its tail with `…` when it does not fit.
///
/// Two writes instead of a `format!`: an ellipsis built with `format!` allocates
/// once per truncated row per frame, which is the cost this crate exists to
/// avoid.
pub fn put_trunc(buf: &mut Buffer, x: u16, y: u16, text: &str, max: u16, style: Style) -> u16 {
    if max == 0 {
        return x;
    }
    let width = UnicodeWidthStr::width(text) as u16;
    if width <= max {
        return put(buf, x, y, text, max, style);
    }
    let budget = max.saturating_sub(1);
    let mut used = 0u16;
    let mut cut = text.len();
    for (index, ch) in text.char_indices() {
        let cell = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if used + cell > budget {
            cut = index;
            break;
        }
        used += cell;
    }
    let x = put(buf, x, y, &text[..cut], budget, style);
    put(buf, x, y, "\u{2026}", 1, style)
}

/// The same truncation rule, but the text is right-aligned inside `area`.
fn put_right(buf: &mut Buffer, area: Rect, y: u16, text: &str, style: Style, floor: u16) {
    let width = UnicodeWidthStr::width(text) as u16;
    let x = area.right().saturating_sub(width + 1);
    if x > floor && x >= area.left() {
        put(buf, x, y, text, area.right() - x, style);
    }
}

/// Fill a rectangle with one style, keeping the cells' characters.
fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    buf.set_style(area, style);
}

/// `width` spaces, without formatting a string per row per frame.
const PADDING: &str = "                ";

fn spaces(width: u16) -> &'static str {
    &PADDING[..(width as usize).min(PADDING.len())]
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// The band at the top of every window: what this is, and the one status worth
/// showing while it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub title: String,
    pub subtitle: Option<String>,
    pub status: Option<String>,
}

impl Header {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            status: None,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }
}

pub fn draw_header(buf: &mut Buffer, area: Rect, design: &Design, header: &Header) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    fill(buf, area, design.style(TextStyle::Body));
    if header.title.is_empty() && header.subtitle.is_none() && header.status.is_none() {
        return;
    }
    let mut x = put(
        buf,
        area.x,
        area.y,
        &header.title,
        area.width,
        design.style(TextStyle::Title),
    );
    match (&header.subtitle, area.height) {
        (Some(subtitle), height) if height >= 3 => {
            put(
                buf,
                area.x,
                area.y + 1,
                subtitle,
                area.width,
                design.style(TextStyle::Dim),
            );
        }
        (Some(subtitle), _) => {
            x = put(
                buf,
                x,
                area.y,
                " \u{00b7} ",
                area.right() - x,
                design.style(TextStyle::Dim),
            );
            put(
                buf,
                x,
                area.y,
                subtitle,
                area.right() - x,
                design.style(TextStyle::Dim),
            );
        }
        (None, _) => {}
    }
    if let Some(status) = &header.status {
        put_right(buf, area, area.y, status, design.style(TextStyle::Dim), x);
    }
    // The rule that separates the header from the body is what makes the band
    // read as a band rather than as a title that happens to be at the top.
    if area.height >= 2 {
        let y = area.bottom() - 1;
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol("\u{2500}");
                cell.set_style(Style::default().fg(design.color(ColorRole::SurfaceAlt)));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key bar
// ---------------------------------------------------------------------------

/// One `[key] what it does` pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyHint {
    pub key: String,
    pub label: String,
}

impl KeyHint {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

/// The bottom band: the keys that are not obvious.
///
/// Letters in brackets rather than Alt chords, because sway sees every Alt chord
/// before the focused surface does (spec 002 §Design) - a hint the session would
/// swallow is worse than no hint.
pub fn draw_keybar(buf: &mut Buffer, area: Rect, design: &Design, hints: &[KeyHint]) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    fill(buf, area, design.style(TextStyle::Body));
    // The width a hint occupies, without the separator that follows it.
    fn width(hint: &KeyHint) -> u16 {
        2 + UnicodeWidthStr::width(hint.key.as_str()) as u16
            + 2
            + UnicodeWidthStr::width(hint.label.as_str()) as u16
    }
    // Draw one hint whole; the caller has already proved that it fits.
    fn draw(buf: &mut Buffer, right: u16, x: u16, y: u16, hint: &KeyHint, design: &Design) -> u16 {
        let mut x = put(buf, x, y, "[", right - x, design.style(TextStyle::Key));
        x = put(
            buf,
            x,
            y,
            &hint.key,
            right - x,
            design.style(TextStyle::Key),
        );
        x = put(buf, x, y, "]", right - x, design.style(TextStyle::Key));
        x = put(buf, x, y, "  ", right - x, design.style(TextStyle::Dim));
        x = put(
            buf,
            x,
            y,
            &hint.label,
            right - x,
            design.style(TextStyle::Dim),
        );
        put(buf, x, y, "   ", right - x, design.style(TextStyle::Dim))
    }
    let total = hints
        .iter()
        .fold(0u16, |sum, hint| sum.saturating_add(width(hint)))
        .saturating_add(3 * hints.len().saturating_sub(1) as u16);
    // When the bar overflows, the last hint is the way out and keeps its place
    // on the right; the middle falls to an ellipsis. A label is never cut.
    let reserve = hints
        .last()
        .filter(|_| hints.len() > 1 && area.x + 1 + total > area.right())
        .map(width)
        .filter(|width| width + 2 <= area.width);
    let limit = reserve.map_or(area.right(), |width| area.right().saturating_sub(width + 1));
    let mut x = area.x + 1;
    for (index, hint) in hints.iter().enumerate() {
        if reserve.is_some() && index == hints.len() - 1 {
            break;
        }
        let hint_width = width(hint);
        if x + hint_width > limit {
            if limit.saturating_sub(x) >= 2 {
                put(buf, x, area.y, "\u{2026}", 1, design.style(TextStyle::Dim));
            }
            break;
        }
        x = draw(buf, area.right(), x, area.y, hint, design);
    }
    if let (Some(last), Some(width)) = (hints.last(), reserve) {
        draw(
            buf,
            area.right(),
            area.right().saturating_sub(width + 1),
            area.y,
            last,
            design,
        );
    }
}

// ---------------------------------------------------------------------------
// Card
// ---------------------------------------------------------------------------

/// A framed surface, and the area inside it for its content.
///
/// The caller passes the rect; the frame is the design's (`rounded`, the surface
/// colour, the title in the accent). A focused card is drawn in the accent so
/// two cards on one screen say which one has the keys.
pub fn draw_card(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    title: Option<&str>,
    focused: bool,
) -> Rect {
    if focused {
        draw_card_with(
            buf,
            area,
            design,
            title,
            ColorRole::Accent,
            design.style(TextStyle::Title),
        )
    } else {
        draw_card_with(
            buf,
            area,
            design,
            title,
            ColorRole::Border,
            design.style(TextStyle::Dim),
        )
    }
}

/// The card frame with its border and title chosen by the caller: a
/// notification's card is drawn in its urgency's role (spec 006), a focused
/// list card in the accent. The frame, the title placement and the truncation
/// are the same in both, which is why this is the one function that draws them.
pub fn draw_card_with(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    title: Option<&str>,
    border: ColorRole,
    title_style: Style,
) -> Rect {
    let block = design
        .block(Border::Rounded)
        .border_style(Style::default().fg(design.color(border)));
    let inner = block.inner(area);
    block.render(area, buf);
    // The title is written over the top border by hand instead of through
    // `Block::title`: a padded title built with `format!` allocates once per
    // card per frame, and a card is drawn on every tick of every screen that
    // has one.
    if let Some(title) = title {
        let x = put(buf, area.x + 1, area.y, " ", 1, title_style);
        let x = put_trunc(
            buf,
            x,
            area.y,
            title,
            area.width.saturating_sub(4),
            title_style,
        );
        put(buf, x, area.y, " ", 1, title_style);
    }
    inner
}

// ---------------------------------------------------------------------------
// Row and list
// ---------------------------------------------------------------------------

/// One line of a list: an icon, a label, a dim detail and a value.
///
/// The four fields are what the session's `ui_row` drew with the old picker, and keeping
/// them means a ported menu looks like the menu it replaces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub glyph: Option<String>,
    pub label: String,
    pub detail: Option<String>,
    pub value: Option<String>,
    /// How strongly a [ranked](ListState::ranked) list prefers this row among
    /// rows that match a needle equally well (spec 007: frecency). A plain list
    /// ignores it, and so does a ranked list with an empty needle: the order a
    /// command gave its rows is the order a person reads them in.
    pub boost: u32,
    /// A section label, not a row: it is drawn, never selected, and it leaves
    /// the list as soon as the person types a filter.
    pub heading: bool,
    /// Optional semantic foreground color override (e.g. `ColorRole::Accent`
    /// for an actively playing track or playlist).
    pub tone: Option<ColorRole>,
}

impl Row {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            glyph: None,
            label: label.into(),
            detail: None,
            value: None,
            boost: 0,
            heading: false,
            tone: None,
        }
    }

    pub fn tone(mut self, role: ColorRole) -> Self {
        self.tone = Some(role);
        self
    }

    /// A section label above a group of rows.
    pub fn heading(label: impl Into<String>) -> Self {
        Self {
            heading: true,
            ..Self::new(label)
        }
    }

    pub fn boost(mut self, boost: u32) -> Self {
        self.boost = boost;
        self
    }

    /// The row's picture, from the TUI alphabet ([`Glyph`]).
    ///
    /// The glyph is resolved once, here, not per frame: a name draws either its
    /// codepoint or its text token (`Glyph::cell`), and `ListState` reserves the
    /// widest cell among the rows so the labels line up whether a row has a
    /// picture, a token or nothing.
    pub fn glyph(mut self, glyph: Glyph) -> Self {
        self.glyph = Some(glyph.cell().to_string());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// `needle` arrives already lowercased (`ListState` does that once per
    /// keystroke, not once per row).
    fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        // A section label is not an answer: it leaves as soon as the person
        // types, and comes back when the filter is cleared.
        if self.heading {
            return false;
        }
        let ascii = needle.is_ascii();
        contains_fold(&self.label, needle, ascii)
            || self
                .detail
                .as_deref()
                .is_some_and(|d| contains_fold(d, needle, ascii))
            || self
                .value
                .as_deref()
                .is_some_and(|v| contains_fold(v, needle, ascii))
    }
}

/// Does `haystack` contain `needle`, ignoring case?
///
/// An **ASCII needle takes the byte path** and allocates nothing: UTF-8 never
/// encodes an ASCII byte inside a multi-byte character, so folding `a-z`/`A-Z`
/// is the whole of case-insensitivity for it - and it is what makes typing in
/// a 5,000-row list a scan instead of a reallocation of every row
/// (`tests/filter.rs` counts both). Any other needle falls back to
/// `to_lowercase`, the only form that folds `Ü` to `ü`; a row pays one
/// allocation, and only while the needle itself carries an accent.
fn contains_fold(haystack: &str, needle_lower: &str, needle_is_ascii: bool) -> bool {
    if needle_is_ascii {
        contains_ascii_fold(haystack.as_bytes(), needle_lower.as_bytes())
    } else {
        haystack.to_lowercase().contains(needle_lower)
    }
}

fn contains_ascii_fold(haystack: &[u8], needle: &[u8]) -> bool {
    match needle {
        [] => true,
        // One character is what the list looks like after the first key.
        [only] => {
            let only = only.to_ascii_lowercase();
            haystack.iter().any(|b| b.to_ascii_lowercase() == only)
        }
        _ => {
            if needle.len() > haystack.len() {
                return false;
            }
            let first = needle[0].to_ascii_lowercase();
            haystack
                .windows(needle.len())
                .any(|w| w[0].to_ascii_lowercase() == first && w.eq_ignore_ascii_case(needle))
        }
    }
}

/// How well a row matches a needle in a [ranked](ListState::ranked) list,
/// best first. `None` is no match; the variants are ordered so a larger value
/// is a better match (spec 007: "exact prefix > word prefix > subsequence").
///
/// The tiers only ever compare the **label**: a person types the name of the
/// thing, and a row whose description happens to contain the letters ranks
/// below every row whose name does - which is `Detail`, the floor, kept so a
/// ranked list still finds what a plain one finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// The needle is in the detail or the value, not in the label.
    Detail = 1,
    /// The label holds the needle's characters in order (`tst` → `theme set`).
    Subsequence = 2,
    /// The label contains the needle somewhere else.
    Substring = 3,
    /// A word of the label starts with the needle (`set` → `theme set`).
    WordPrefix = 4,
    /// The label starts with the needle.
    Prefix = 5,
    /// The label is the needle.
    Exact = 6,
}

impl Tier {
    /// The tier `row` reaches for `needle` (already lowercased), or `None`.
    ///
    /// Same two paths as the plain filter: an ASCII needle compares bytes and
    /// allocates nothing, any other needle folds the label once.
    pub fn of(row: &Row, needle_lower: &str) -> Option<Tier> {
        if needle_lower.is_empty() {
            return Some(Tier::Exact);
        }
        if row.heading {
            return None;
        }
        let ascii = needle_lower.is_ascii();
        let label = if ascii {
            label_tier_ascii(row.label.as_bytes(), needle_lower.as_bytes())
        } else {
            label_tier_unicode(&row.label.to_lowercase(), needle_lower)
        };
        label.or_else(|| {
            let found = |text: Option<&str>| {
                text.is_some_and(|text| contains_fold(text, needle_lower, ascii))
            };
            (found(row.detail.as_deref()) || found(row.value.as_deref())).then_some(Tier::Detail)
        })
    }
}

fn label_tier_ascii(label: &[u8], needle: &[u8]) -> Option<Tier> {
    if label.len() == needle.len() && label.eq_ignore_ascii_case(needle) {
        return Some(Tier::Exact);
    }
    if label.len() >= needle.len() && label[..needle.len()].eq_ignore_ascii_case(needle) {
        return Some(Tier::Prefix);
    }
    if label.len() > needle.len() {
        let first = needle[0].to_ascii_lowercase();
        let mut substring = false;
        for start in 1..=label.len() - needle.len() {
            if label[start].to_ascii_lowercase() != first
                || !label[start..start + needle.len()].eq_ignore_ascii_case(needle)
            {
                continue;
            }
            if !label[start - 1].is_ascii_alphanumeric() {
                return Some(Tier::WordPrefix);
            }
            substring = true;
        }
        if substring {
            return Some(Tier::Substring);
        }
    }
    let mut want = needle.iter().map(u8::to_ascii_lowercase).peekable();
    for byte in label {
        if want.peek() == Some(&byte.to_ascii_lowercase()) {
            want.next();
        }
    }
    want.peek().is_none().then_some(Tier::Subsequence)
}

fn label_tier_unicode(label: &str, needle: &str) -> Option<Tier> {
    if label == needle {
        return Some(Tier::Exact);
    }
    if label.starts_with(needle) {
        return Some(Tier::Prefix);
    }
    let mut substring = false;
    for (start, _) in label.match_indices(needle) {
        let after_boundary = label[..start]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric());
        if after_boundary {
            return Some(Tier::WordPrefix);
        }
        substring = true;
    }
    if substring {
        return Some(Tier::Substring);
    }
    let mut want = needle.chars().peekable();
    for ch in label.chars() {
        if want.peek() == Some(&ch) {
            want.next();
        }
    }
    want.peek().is_none().then_some(Tier::Subsequence)
}

/// What the list did with a key, so the command decides what a choice means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListAction {
    /// The list did not use this key.
    Ignored,
    Moved,
    Activated(usize),
}

/// The state a list keeps between frames: its rows, the cursor and the window
/// onto them.
///
/// Rows are owned and filtered once per state change, so drawing 40 rows costs
/// copies, not formatting (`tests/no_alloc.rs` watches it).
pub struct ListState {
    rows: Vec<Row>,
    visible: Vec<usize>,
    cursor: usize,
    offset: usize,
    filter: String,
    /// `filter`, lowercased once per change; `Row::matches` takes the folded
    /// form so no row allocates to compare.
    needle: String,
    /// The needle `visible` was computed for. When the next one extends it,
    /// the visible set can only shrink, so the pass is a `retain`.
    applied: String,
    empty: String,
    /// The widest glyph cell among the rows, reserved for every row so a text
    /// token (`bt`) does not shift the labels under it.  Computed when the rows
    /// change, from *all* of them: filtering a row out must not move the column.
    glyph_width: u16,
    /// Order the matches by how well they match (spec 007) instead of keeping
    /// the rows' own order. Off by default: a file list is in the order the
    /// directory gave it, a menu or a palette is in the order of the best match.
    ranked: bool,
    /// The tier each row reached for the current needle, indexed like `rows`
    /// (0 = no match). One buffer for the list's life, so ranking a keystroke
    /// costs no allocation per row - the rule `tests/filter.rs` enforces for
    /// the plain filter holds for the ranked one too.
    tiers: Vec<u8>,
}

impl ListState {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            visible: Vec::new(),
            cursor: 0,
            offset: 0,
            filter: String::new(),
            needle: String::new(),
            applied: String::new(),
            empty: "nothing here yet".to_string(),
            glyph_width: 0,
            ranked: false,
            tiers: Vec::new(),
        }
    }

    pub fn with_empty(mut self, text: impl Into<String>) -> Self {
        self.empty = text.into();
        self
    }

    /// A list that orders its matches: the best [`Tier`] first, then the
    /// larger [`Row::boost`], then the shorter label, then the rows' own order.
    ///
    /// With an empty needle nothing is reordered, so a menu still reads in the
    /// order its groups were written. This is the one ranking the session has:
    /// the root menu and the palette both use it (spec 007), and a surface that
    /// wants another order sets boosts instead of writing a second matcher.
    pub fn ranked(mut self) -> Self {
        self.ranked = true;
        self.tiers.clear();
        self.tiers.resize(self.rows.len(), 0);
        self.applied.clear();
        self.refilter();
        self
    }

    pub fn is_ranked(&self) -> bool {
        self.ranked
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.glyph_width = rows
            .iter()
            .filter_map(|row| row.glyph.as_deref())
            .map(|glyph| UnicodeWidthStr::width(glyph) as u16)
            .max()
            .unwrap_or(0);
        self.rows = rows;
        if self.ranked {
            self.tiers.clear();
            self.tiers.resize(self.rows.len(), 0);
        }
        // The visible set belonged to the old rows. Every index is a match
        // again until `refilter` says otherwise, which is what lets its
        // narrowing pass (a `retain`) walk the new list exactly once.
        self.visible.clear();
        self.visible.extend(0..self.rows.len());
        self.applied.clear();
        self.refilter();
    }

    /// Replace the rows in-place while preserving the active filter, the
    /// selected row's identity (`value` or `label`), and the scroll offset.
    /// Used when a background scan (`nmcli`, `bluetoothctl`, `du`, `ps`)
    /// refreshes a live list while the person is navigating or typing.
    pub fn update_rows(&mut self, rows: Vec<Row>) {
        let selected_value = self.selected().and_then(|r| r.value.clone());
        let selected_label = self.selected().map(|r| r.label.clone());
        let prev_cursor = self.cursor;
        let prev_offset = self.offset;

        self.glyph_width = rows
            .iter()
            .filter_map(|row| row.glyph.as_deref())
            .map(|glyph| UnicodeWidthStr::width(glyph) as u16)
            .max()
            .unwrap_or(0);
        self.rows = rows;
        if self.ranked {
            self.tiers.clear();
            self.tiers.resize(self.rows.len(), 0);
        }
        self.visible.clear();
        self.visible.extend(0..self.rows.len());
        self.applied.clear();
        self.refilter();

        if self.visible.is_empty() {
            self.cursor = 0;
            self.offset = 0;
            return;
        }

        let matched_pos = selected_value
            .as_deref()
            .and_then(|val| {
                self.visible
                    .iter()
                    .position(|&idx| self.rows[idx].value.as_deref() == Some(val))
            })
            .or_else(|| {
                selected_label.as_deref().and_then(|lbl| {
                    self.visible
                        .iter()
                        .position(|&idx| self.rows[idx].label == lbl)
                })
            });

        self.cursor = matched_pos.unwrap_or_else(|| prev_cursor.min(self.visible.len() - 1));
        self.offset = prev_offset.min(self.visible.len().saturating_sub(1));
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
    }

    /// Append rows while the list is open, for a producer that streams them (a
    /// walk of the disk, spec 009). Only the new rows are tested against the
    /// needle, and the cursor stays on the row it was on: a batch arriving
    /// while a person moves down the list must not move the row under them,
    /// which is what `set_rows` (a new list) would do.
    pub fn extend_rows(&mut self, rows: impl IntoIterator<Item = Row>) {
        let start = self.rows.len();
        for row in rows {
            if let Some(glyph) = row.glyph.as_deref() {
                self.glyph_width = self.glyph_width.max(UnicodeWidthStr::width(glyph) as u16);
            }
            self.rows.push(row);
        }
        if self.rows.len() == start {
            return;
        }
        let selected = self.selected_index();
        let needle = &self.needle;
        let rows = &self.rows;
        if self.ranked && !needle.is_empty() {
            self.tiers.resize(rows.len(), 0);
            for (i, row) in rows.iter().enumerate().skip(start) {
                let tier = Tier::of(row, needle).map_or(0, |t| t as u8);
                self.tiers[i] = tier;
                if tier > 0 {
                    self.visible.push(i);
                }
            }
            self.sort_ranked();
            if let Some(selected) = selected
                && let Some(position) = self.visible.iter().position(|&i| i == selected)
            {
                self.cursor = position;
            }
        } else {
            if self.ranked {
                self.tiers.resize(rows.len(), 0);
            }
            self.visible
                .extend((start..rows.len()).filter(|&i| rows[i].matches(needle)));
        }
    }

    /// The ranked order: the best tier, then the boost, then the shorter label,
    /// then the rows' own order - one comparator for a new needle and for rows
    /// that arrive later.
    fn sort_ranked(&mut self) {
        let rows = &self.rows;
        let tiers = &self.tiers;
        // Unstable, because the last key is the index: a total order sorts
        // the same way every time, and `sort_unstable` allocates nothing.
        self.visible.sort_unstable_by(|&a, &b| {
            tiers[b]
                .cmp(&tiers[a])
                .then(rows[b].boost.cmp(&rows[a].boost))
                .then(
                    UnicodeWidthStr::width(rows[a].label.as_str())
                        .cmp(&UnicodeWidthStr::width(rows[b].label.as_str())),
                )
                .then(a.cmp(&b))
        });
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The row index behind a visible position. The cursor counts visible rows,
    /// so a screen that draws its own cards needs this to find the row.
    pub fn visible_index(&self, position: usize) -> Option<usize> {
        self.visible.get(position).copied()
    }

    /// The visible position of a row index, for a click that arrives as a row.
    pub fn position_of(&self, row: usize) -> Option<usize> {
        self.visible.iter().position(|visible| *visible == row)
    }

    /// Whether the visible position is a row the cursor may stand on.
    fn selectable(&self, position: usize) -> bool {
        self.visible
            .get(position)
            .is_some_and(|row| !self.rows[*row].heading)
    }

    /// The nearest selectable position from `target`, walking in `direction`
    /// (`1` down, `-1` up). A section heading is a label, never a cursor stop.
    fn snap(&self, target: usize, direction: isize) -> Option<usize> {
        let len = self.visible.len();
        if len == 0 {
            return None;
        }
        let mut position = target.min(len - 1) as isize;
        while position >= 0 && (position as usize) < len {
            if self.selectable(position as usize) {
                return Some(position as usize);
            }
            position += direction.signum();
        }
        None
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        if self.visible.is_empty() {
            return;
        }
        let cursor = cursor.min(self.visible.len() - 1);
        if let Some(position) = self.snap(cursor, 1).or_else(|| self.snap(cursor, -1)) {
            self.cursor = position;
        }
    }

    /// Move the cursor to the visible row matching unfiltered `index`, if visible.
    pub fn select_index(&mut self, index: usize) {
        if let Some(pos) = self.visible.iter().position(|&i| i == index) {
            self.set_cursor(pos);
        }
    }

    pub fn selected(&self) -> Option<&Row> {
        if !self.selectable(self.cursor) {
            return None;
        }
        self.visible.get(self.cursor).map(|i| &self.rows[*i])
    }

    /// The selected row's index in the unfiltered set (what a command acts on:
    /// a filter must not change which file it opens).
    pub fn selected_index(&self) -> Option<usize> {
        if !self.selectable(self.cursor) {
            return None;
        }
        self.visible.get(self.cursor).copied()
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
        self.needle = self.filter.to_lowercase();
        self.refilter();
    }

    /// Recompute the visible set for the current needle.
    ///
    /// Typing appends to the needle, and a longer needle can only match a
    /// subset of a shorter one's matches (`"repo"` matches nothing `"rep"`
    /// did not), so the common case - one more character - tests the rows that
    /// are already visible and keeps the vector in place: no scan of the full
    /// list and no allocation. Anything else (a backspace, a pasted word, a
    /// cleared filter, new rows) rebuilds from scratch.
    fn refilter(&mut self) {
        let narrowing =
            self.needle.len() > self.applied.len() && self.needle.starts_with(&self.applied);
        let rows = &self.rows;
        let needle = &self.needle;
        if self.ranked && !needle.is_empty() {
            // Every tier is monotonic in the needle (a row that holds "repo"
            // as a subsequence holds "rep" too), so narrowing may keep the
            // visible set here as well; the order is rebuilt either way.
            let tiers = &mut self.tiers;
            let mut score = |i: usize| {
                let tier = Tier::of(&rows[i], needle).map_or(0, |t| t as u8);
                tiers[i] = tier;
                tier > 0
            };
            if narrowing {
                self.visible.retain(|&i| score(i));
            } else {
                self.visible.clear();
                self.visible.extend((0..rows.len()).filter(|&i| score(i)));
            }
            self.sort_ranked();
        } else if narrowing {
            self.visible.retain(|&i| rows[i].matches(needle));
        } else {
            self.visible.clear();
            self.visible
                .extend((0..rows.len()).filter(|&i| rows[i].matches(needle)));
        }
        self.applied.clear();
        self.applied.push_str(&self.needle);
        if self.ranked {
            // The best match is the first row, and Enter must take it: a cursor
            // left on the third row by the previous needle would open the
            // third-best answer to this one. A heading is not an answer.
            self.cursor = self.snap(0, 1).unwrap_or(0);
        } else if !self.visible.is_empty() {
            self.cursor = self.cursor.min(self.visible.len() - 1);
            self.cursor = self
                .snap(self.cursor, -1)
                .or_else(|| self.snap(self.cursor, 1))
                .unwrap_or(0);
        }
        self.offset = 0;
    }

    /// How many rows fit in `height` lines at this density.
    pub fn capacity(&self, height: u16, design: &Design) -> u16 {
        height / design.row_stride().max(1)
    }

    pub fn handle(&mut self, key: KeyEvent, height: u16, design: &Design) -> ListAction {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return ListAction::Ignored;
        }
        let capacity = self.capacity(height, design).max(1);
        let len = self.visible.len();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if len == 0 {
                    return ListAction::Ignored;
                }
                if let Some(position) = self.snap(self.cursor.saturating_sub(1), -1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::Char('p') if ctrl => {
                if len == 0 {
                    return ListAction::Ignored;
                }
                if let Some(position) = self.snap(self.cursor.saturating_sub(1), -1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len == 0 {
                    return ListAction::Ignored;
                }
                if let Some(position) = self.snap((self.cursor + 1).min(len - 1), 1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::Char('n') if ctrl => {
                if len == 0 {
                    return ListAction::Ignored;
                }
                if let Some(position) = self.snap((self.cursor + 1).min(len - 1), 1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::PageUp => {
                let target = self.cursor.saturating_sub(capacity as usize);
                if let Some(position) = self.snap(target, 1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::PageDown => {
                let target = (self.cursor + capacity as usize).min(len.saturating_sub(1));
                if let Some(position) = self.snap(target, -1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::Home => {
                if let Some(position) = self.snap(0, 1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::End => {
                if let Some(position) = self.snap(len.saturating_sub(1), -1) {
                    self.cursor = position;
                    self.scroll_into_view(capacity);
                }
                ListAction::Moved
            }
            KeyCode::Enter => match self.selected_index() {
                Some(index) => ListAction::Activated(index),
                None => ListAction::Ignored,
            },
            _ => ListAction::Ignored,
        }
    }

    /// Backspace removes one character of the filter; a printable key adds it.
    /// Returns true when the filter changed.
    pub fn type_filter(&mut self, key: KeyEvent) -> bool {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return false;
        }
        match key.code {
            KeyCode::Backspace => {
                let mut filter = self.filter.clone();
                filter.pop();
                self.set_filter(filter);
                true
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let mut filter = self.filter.clone();
                filter.push(ch);
                self.set_filter(filter);
                true
            }
            _ => false,
        }
    }

    fn scroll_into_view(&mut self, capacity: u16) {
        let capacity = capacity as usize;
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if capacity > 0 && self.cursor >= self.offset + capacity {
            self.offset = self.cursor + 1 - capacity;
        }
    }

    /// The row under `(x, y)`, or `None` for a blank line (tablet density leaves
    /// one between rows, and a click there must not choose the row above it).
    pub fn hit_row(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if x < area.left() || x >= area.right() || y < area.y || y >= area.bottom() {
            return None;
        }
        let stride = design.row_stride().max(1);
        let line = (y - area.y) / stride;
        if !(y - area.y).is_multiple_of(stride) {
            return None;
        }
        let index = self.offset + line as usize;
        if index < self.visible.len() && !self.rows[self.visible[index]].heading {
            Some(index)
        } else {
            None
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, design: &Design, focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        fill(
            buf,
            area,
            Style::default()
                .bg(design.color(ColorRole::Bg))
                .fg(design.color(ColorRole::Text)),
        );
        if self.visible.is_empty() {
            put(
                buf,
                area.x + 1,
                area.y,
                &self.empty,
                area.width.saturating_sub(1),
                design.style(TextStyle::Dim),
            );
            return;
        }
        let stride = design.row_stride().max(1);
        let capacity = self.capacity(area.height, design) as usize;
        for line in 0..capacity {
            let index = self.offset + line;
            let Some(row_index) = self.visible.get(index) else {
                break;
            };
            let row = &self.rows[*row_index];
            let y = area.y + line as u16 * stride;
            if row.heading {
                // A section label: drawn like a title with a rule to the right,
                // never filled and never reached by the cursor or a click.
                let mut x = put(
                    buf,
                    area.x + 1,
                    y,
                    &row.label,
                    area.width.saturating_sub(1),
                    design.style(TextStyle::Title),
                );
                x = put(
                    buf,
                    x,
                    y,
                    "  ",
                    area.right().saturating_sub(x),
                    design.style(TextStyle::Dim),
                );
                for rule_x in x..area.right().saturating_sub(2) {
                    if let Some(cell) = buf.cell_mut((rule_x, y)) {
                        cell.set_symbol("\u{2500}");
                        cell.set_style(Style::default().fg(design.color(ColorRole::Border)));
                    }
                }
                continue;
            }
            let selected = focused && index == self.cursor;
            let row_bg = if selected {
                design.color(ColorRole::Selection)
            } else {
                design.color(ColorRole::Bg)
            };
            let mut x = area.x;
            if selected {
                let rect = Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                };
                fill(buf, rect, Style::default().bg(row_bg));
                x = put(
                    buf,
                    x,
                    y,
                    "\u{258d} ",
                    area.width,
                    design.style(TextStyle::Title).bg(row_bg),
                );
            } else {
                x = put(
                    buf,
                    x,
                    y,
                    "  ",
                    area.width,
                    design.style(TextStyle::Dim).bg(row_bg),
                );
            }
            if self.glyph_width > 0 {
                let mut glyph_style = design
                    .style(if selected {
                        TextStyle::Selected
                    } else {
                        TextStyle::Body
                    })
                    .bg(row_bg);
                if let Some(role) = row.tone {
                    glyph_style = glyph_style.fg(design.color(role));
                }
                let mut room = self.glyph_width;
                if let Some(glyph) = &row.glyph {
                    let width = UnicodeWidthStr::width(glyph.as_str()) as u16;
                    x = put(buf, x, y, glyph, area.right() - x, glyph_style);
                    room = room.saturating_sub(width);
                }
                // A row with no picture of its own still spends the column, or
                // its label starts further left than every other row's.
                x = put(buf, x, y, spaces(room), area.right() - x, glyph_style);
                // Nerd Font / Font Awesome glyphs are slightly wider than one
                // monospace cell in foot, so single-cell icons get 2 spaces to
                // avoid visually crowding the label (keeping label col == 5).
                let gap = if self.glyph_width <= 1 { "  " } else { " " };
                x = put(buf, x, y, gap, area.right() - x, glyph_style);
            }
            let value_width = row
                .value
                .as_deref()
                .map(|v| UnicodeWidthStr::width(v) as u16 + 2)
                .unwrap_or(0);
            let label_room = area.right().saturating_sub(x + value_width + 1);
            let mut style = if selected {
                design.style(TextStyle::Selected).bg(row_bg)
            } else {
                design.style(TextStyle::Body).bg(row_bg)
            };
            if let Some(role) = row.tone {
                style = style
                    .fg(design.color(role))
                    .add_modifier(ratatui::style::Modifier::BOLD);
            }
            let detail_style = if selected {
                design.style(TextStyle::SelectedDim).bg(row_bg)
            } else {
                design.style(TextStyle::Dim).bg(row_bg)
            };
            let mut label_end = put_trunc(buf, x, y, &row.label, label_room, style);
            if let Some(detail) = &row.detail {
                label_end = put(buf, label_end, y, "  ", area.right() - label_end, style);
                label_end = put_trunc(
                    buf,
                    label_end,
                    y,
                    detail,
                    area.right().saturating_sub(label_end + value_width),
                    detail_style,
                );
            }
            if let Some(value) = &row.value {
                let value_style = detail_style;
                put_right(
                    buf,
                    Rect {
                        x: label_end,
                        y,
                        width: area.right().saturating_sub(label_end + 1),
                        height: 1,
                    },
                    y,
                    value,
                    value_style,
                    label_end,
                );
            }
        }
        draw_scrollbar(
            buf,
            area,
            design,
            self.offset,
            capacity,
            self.visible.len(),
            focused,
        );
    }
}

/// Standard card layout for a filter/hint row and a scrollable list body
/// inside `draw_card(buf, area, design, ...)`.
pub fn list_layout(area: Rect, _design: &Design) -> (Rect, Rect) {
    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let filter_row = Rect {
        height: 1.min(inner.height),
        ..inner
    };
    let rows = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    (filter_row, rows)
}

/// A 1-column scrollbar on the right edge of `area` (`│` track, `█` thumb)
/// drawn only when `total > capacity`, with zero heap allocations.
pub fn draw_scrollbar(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    offset: usize,
    capacity: usize,
    total: usize,
    focused: bool,
) {
    if total <= capacity || capacity == 0 || area.height < 3 || area.width < 12 {
        return;
    }
    let x = area.right() - 1;
    let h = area.height as usize;
    let thumb_h = ((capacity * h) / total).max(1).min(h);
    let max_offset = total.saturating_sub(capacity).max(1);
    let thumb_top = (offset.min(max_offset) * (h - thumb_h)) / max_offset;
    let track_style = Style::default().fg(design.color(ColorRole::Border));
    let thumb_style = Style::default().fg(design.color(if focused {
        ColorRole::Accent
    } else {
        ColorRole::Muted
    }));
    for row in 0..h {
        let y = area.y + row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            if row >= thumb_top && row < thumb_top + thumb_h {
                cell.set_symbol("\u{2588}").set_style(thumb_style);
            } else {
                cell.set_symbol("\u{2502}").set_style(track_style);
            }
        }
    }
}

fn write_usize_digits(mut n: usize, out: &mut [u8], pos: &mut usize) {
    let start = *pos;
    if n == 0 {
        if *pos < out.len() {
            out[*pos] = b'0';
            *pos += 1;
        }
        return;
    }
    while n > 0 && *pos < out.len() {
        out[*pos] = b'0' + (n % 10) as u8;
        *pos += 1;
        n /= 10;
    }
    out[start..*pos].reverse();
}

/// Zero-allocation search/filter bar for standard cards: draws the search glyph,
/// either the dim hint or the active filter with a `█` cursor, and a right-aligned
/// `visible / total` match counter.
pub fn draw_filter_bar(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    filter: &str,
    hint: &str,
    visible: usize,
    total: usize,
) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let active = !filter.is_empty();
    let icon_style = if active {
        design.style(TextStyle::Title)
    } else {
        design.style(TextStyle::Dim)
    };
    let mut x = put(
        buf,
        area.x,
        area.y,
        Glyph::Search.cell(),
        area.width,
        icon_style,
    );
    x = put(
        buf,
        x,
        area.y,
        " ",
        area.right().saturating_sub(x),
        design.style(TextStyle::Dim),
    );
    let mut scratch = [0u8; 24];
    let mut counter_len = 0usize;
    let show_counter = total > 0 && area.width >= 24 && (active || total > 5);
    if show_counter {
        write_usize_digits(visible, &mut scratch, &mut counter_len);
        if counter_len + 3 < scratch.len() {
            scratch[counter_len] = b' ';
            scratch[counter_len + 1] = b'/';
            scratch[counter_len + 2] = b' ';
            counter_len += 3;
        }
        write_usize_digits(total, &mut scratch, &mut counter_len);
    }
    let reserve = if show_counter {
        (counter_len as u16) + 2
    } else {
        0
    };
    if active {
        x = put_trunc(
            buf,
            x,
            area.y,
            filter,
            area.right().saturating_sub(x + reserve + 1),
            design.style(TextStyle::Title),
        );
        x = put(
            buf,
            x,
            area.y,
            "\u{2588}",
            area.right().saturating_sub(x),
            Style::default().fg(design.color(ColorRole::Accent)),
        );
    } else {
        x = put_trunc(
            buf,
            x,
            area.y,
            hint,
            area.right().saturating_sub(x + reserve),
            design.style(TextStyle::Dim),
        );
    }
    if show_counter && let Ok(counter) = std::str::from_utf8(&scratch[..counter_len]) {
        let count_style = if active && visible == 0 {
            design.style(TextStyle::Danger)
        } else if active {
            design.style(TextStyle::Code)
        } else {
            design.style(TextStyle::Dim)
        };
        put_right(buf, area, area.y, counter, count_style, x);
    }
}

impl Default for ListState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Empty state, failure card
// ---------------------------------------------------------------------------

/// A rectangle centred in `area`, `percent_x` wide and `height` lines tall.
pub fn centered(area: Rect, percent_x: u16, height: u16) -> Rect {
    let width = area
        .width
        .saturating_mul(percent_x)
        .saturating_div(100)
        .max(24)
        .min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn centred_line(buf: &mut Buffer, area: Rect, y: u16, text: &str, style: Style) {
    let width = UnicodeWidthStr::width(text) as u16;
    let x = if width >= area.width {
        area.x
    } else {
        area.x + (area.width - width) / 2
    };
    put_trunc(buf, x, y, text, area.right().saturating_sub(x), style);
}

/// The card a command shows when it has nothing to show yet.
pub fn draw_empty(buf: &mut Buffer, area: Rect, design: &Design, title: &str, hint: &str) {
    let card = centered(area, 76, 9);
    let inner = draw_card(buf, card, design, Some("Hayato"), true);
    centred_line(
        buf,
        inner,
        inner.y + 1,
        title,
        design.style(TextStyle::Title),
    );
    centred_line(buf, inner, inner.y + 3, hint, design.style(TextStyle::Dim));
}

/// The card a command shows when it failed: what failed, and the way out.
pub fn draw_failure(buf: &mut Buffer, area: Rect, design: &Design, summary: &str, detail: &str) {
    let card = centered(area, 76, 11);
    let inner = draw_card(
        buf,
        card,
        design,
        Some("Hayato: something went wrong"),
        true,
    );
    centred_line(
        buf,
        inner,
        inner.y + 1,
        summary,
        design.style(TextStyle::Danger),
    );
    centred_line(
        buf,
        inner,
        inner.y + 3,
        detail,
        design.style(TextStyle::Body),
    );
    centred_line(
        buf,
        inner,
        inner.bottom().saturating_sub(1),
        "press any key to close",
        design.style(TextStyle::Dim),
    );
}

// ---------------------------------------------------------------------------
// Badge, gauge, progress
// ---------------------------------------------------------------------------

/// A one-line pill: ` text ` on the role's colour. Returns the column after it.
pub fn draw_badge(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    design: &Design,
    text: &str,
    role: ColorRole,
) -> u16 {
    let style = Style::default()
        .fg(design.color(ColorRole::Bg))
        .bg(design.color(role));
    let mut x = put(buf, x, y, " ", 1, style);
    x = put(buf, x, y, text, u16::MAX, style);
    put(buf, x, y, " ", 1, style)
}

/// The width `put_percent` writes: three digits and the sign, never more.
pub const PERCENT_WIDTH: u16 = 4;

const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Write ` 42%` (four cells, right-aligned) without formatting a string.
///
/// `format!` here would be one allocation per gauge per frame, which is the cost
/// `optimization.md` §13 names; a percentage is an integer from 0 to 100 and
/// needs no formatter.
pub fn put_percent(buf: &mut Buffer, x: u16, y: u16, value: f64, style: Style) -> u16 {
    let percent = (value.clamp(0.0, 1.0) * 100.0).round() as u16;
    let (hundreds, tens, units) = if percent >= 100 {
        ("1", "0", "0")
    } else if percent >= 10 {
        (
            " ",
            DIGITS[(percent / 10) as usize],
            DIGITS[(percent % 10) as usize],
        )
    } else {
        (" ", " ", DIGITS[percent as usize])
    };
    let mut x = put(buf, x, y, hundreds, 1, style);
    x = put(buf, x, y, tens, 1, style);
    x = put(buf, x, y, units, 1, style);
    put(buf, x, y, "%", 1, style)
}

/// A bar with end caps: `▕████░░░░▕  42% battery` colored by utilization
/// (`Ok` < 70%, `Warn` 70–89%, `Danger` >= 90%).
///
/// The label is reserved before the bar is measured, or a wide gauge eats the
/// word that says what the number means (80 columns of bar and `b…`).
pub fn draw_gauge(buf: &mut Buffer, area: Rect, design: &Design, value: f64, label: &str) {
    let value = value.clamp(0.0, 1.0);
    let role = if value >= 0.9 {
        ColorRole::Danger
    } else if value >= 0.7 {
        ColorRole::Warn
    } else {
        ColorRole::Ok
    };
    draw_gauge_with(buf, area, design, value, label, role);
}

/// A bar with end caps drawn in a caller-specified [`ColorRole`].
///
/// Use this for completion progress bars (where 70%+ is normal progress, not a
/// capacity warning) while keeping [`draw_gauge`] for resource utilization.
pub fn draw_gauge_with(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    value: f64,
    label: &str,
    role: ColorRole,
) {
    if area.width < 12 || area.height == 0 {
        return;
    }
    let value = value.clamp(0.0, 1.0);
    let style = Style::default().fg(design.color(role));
    let empty = Style::default().fg(design.color(ColorRole::SurfaceAlt));
    let label_room = (UnicodeWidthStr::width(label) as u16).min(area.width / 3);
    // "100%" / " 42%" / "  7%": four cells, built from digits rather than
    // `format!` so a gauge allocates nothing per frame.
    let fixed = 4 + 1 + PERCENT_WIDTH + if label_room > 0 { 1 + label_room } else { 0 };
    let bars = area.width.saturating_sub(fixed);
    let filled = (bars as f64 * value).round() as u16;
    let mut x = put(buf, area.x, area.y, "\u{2595}", 1, empty);
    for i in 0..bars {
        let (symbol, style) = if i < filled {
            ("\u{2588}", style)
        } else {
            ("\u{2591}", empty)
        };
        x = put(buf, x, area.y, symbol, 1, style);
    }
    x = put(buf, x, area.y, "\u{2595}", 1, empty);
    x = put(buf, x, area.y, " ", 1, empty);
    x = put_percent(buf, x, area.y, value, style);
    if label_room > 0 {
        x = put(buf, x, area.y, " ", 1, empty);
        put_trunc(
            buf,
            x,
            area.y,
            label,
            area.right() - x,
            design.style(TextStyle::Dim),
        );
    }
}

/// A thin progress bar for a task whose total is unknown: the bar carries the
/// fraction, the label says what is running.
pub fn draw_progress(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    value: f64,
    label: &str,
    role: ColorRole,
) {
    if area.width < 8 || area.height == 0 {
        return;
    }
    let value = value.clamp(0.0, 1.0);
    let style = Style::default().fg(design.color(role));
    let empty = Style::default().fg(design.color(ColorRole::SurfaceAlt));
    let label_width = UnicodeWidthStr::width(label) as u16;
    let bar_width = if label_width == 0 {
        area.width
    } else {
        area.width.saturating_sub(label_width + 1)
    };
    let filled = (bar_width as f64 * value).round() as u16;
    for x in area.left()..area.left() + bar_width {
        let (symbol, style) = if x - area.left() < filled {
            ("\u{2588}", style)
        } else {
            ("\u{2591}", empty)
        };
        put(buf, x, area.y, symbol, 1, style);
    }
    if label_width > 0 {
        let x = area.left() + bar_width + 1;
        put_trunc(
            buf,
            x,
            area.y,
            label,
            area.right().saturating_sub(x),
            design.style(TextStyle::Dim),
        );
    }
}

// ---------------------------------------------------------------------------
// Log pane
// ---------------------------------------------------------------------------

/// The tail of a log, following it unless the person scrolled up.
///
/// Following is a state, not a timer: the pane jumps to the end when a line is
/// pushed while following, and stops following the moment a scroll key arrives -
/// which is how "the log stays where I put it" works without a flag the command
/// has to remember.
pub struct LogPane {
    lines: Vec<String>,
    follow: bool,
    offset: usize,
}

impl LogPane {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            follow: true,
            offset: 0,
        }
    }

    pub fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
        if self.lines.len() > 5_000 {
            self.lines.drain(..1_000);
        }
        if self.follow {
            self.offset = 0;
        }
    }

    pub fn set_lines(&mut self, lines: Vec<String>) {
        self.lines = lines;
        if self.follow {
            self.offset = 0;
        }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn following(&self) -> bool {
        self.follow
    }

    pub fn follow(&mut self, follow: bool) {
        self.follow = follow;
        if follow {
            self.offset = 0;
        }
    }

    /// Scroll keys; returns true when the pane used the key.
    pub fn handle(&mut self, key: KeyEvent, height: u16) -> bool {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return false;
        }
        let page = height.saturating_sub(1).max(1) as usize;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.follow = false;
                self.offset = (self.offset + 1).min(self.lines.len().saturating_sub(1));
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.offset = self.offset.saturating_sub(1);
                if self.offset == 0 {
                    self.follow = true;
                }
                true
            }
            KeyCode::PageUp => {
                self.follow = false;
                self.offset = (self.offset + page).min(self.lines.len().saturating_sub(1));
                true
            }
            KeyCode::PageDown => {
                self.offset = self.offset.saturating_sub(page);
                if self.offset == 0 {
                    self.follow = true;
                }
                true
            }
            KeyCode::End => {
                self.follow = true;
                self.offset = 0;
                true
            }
            _ => false,
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, design: &Design) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        fill(buf, area, design.style(TextStyle::Body));
        let height = area.height as usize;
        let end = self.lines.len().saturating_sub(self.offset);
        let start = end.saturating_sub(height);
        let scrollable = self.lines.len() > height;
        let width = if scrollable {
            area.width.saturating_sub(1)
        } else {
            area.width
        };
        for (line_no, line) in self.lines[start..end].iter().enumerate() {
            let y = area.y + line_no as u16;
            put_trunc(buf, area.x, y, line, width, design.style(TextStyle::Dim));
        }
        if !self.follow && !self.lines.is_empty() {
            put_right(
                buf,
                Rect { width, ..area },
                area.bottom().saturating_sub(1),
                "[paused]",
                design.style(TextStyle::Danger),
                area.left(),
            );
        }
        if scrollable {
            draw_scrollbar(buf, area, design, start, height, self.lines.len(), false);
        }
    }
}

impl Default for LogPane {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Confirm
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    None,
    Cancelled,
    Chosen(usize),
}

/// Zero-allocation line wrapper over `&str` that splits on explicit newlines
/// and wraps at whitespace boundaries to `max_width` display columns.
struct WrappedLines<'a> {
    rest: &'a str,
    max_width: usize,
}

impl<'a> WrappedLines<'a> {
    fn new(text: &'a str, max_width: u16) -> Self {
        Self {
            rest: text,
            max_width: max_width.max(1) as usize,
        }
    }
}

impl<'a> Iterator for WrappedLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        // Take up to the next explicit newline first.
        let (segment, after_nl) = match self.rest.find('\n') {
            Some(pos) => (&self.rest[..pos], Some(&self.rest[pos + 1..])),
            None => (self.rest, None),
        };
        let trimmed = segment.trim_end();
        if UnicodeWidthStr::width(trimmed) <= self.max_width {
            self.rest = after_nl.unwrap_or("");
            return Some(trimmed);
        }
        // Walk characters to find the last whitespace within `max_width`,
        // falling back to a hard break at `max_width` for long unspaced tokens.
        let mut width = 0usize;
        let mut last_space: Option<usize> = None;
        let mut hard_break = segment.len();
        for (idx, ch) in segment.char_indices() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + cw > self.max_width {
                hard_break = idx;
                break;
            }
            if ch.is_whitespace() {
                last_space = Some(idx);
            }
            width += cw;
        }
        let split_at = last_space.filter(|&p| p > 0).unwrap_or_else(|| {
            if hard_break == 0 {
                segment
                    .char_indices()
                    .nth(1)
                    .map_or(segment.len(), |(i, _)| i)
            } else {
                hard_break
            }
        });
        let line = segment[..split_at].trim_end();
        let remainder = self.rest[split_at..].trim_start_matches(' ');
        self.rest = remainder;
        Some(line)
    }
}

/// A confirmation dialog whose first answer is Cancel and whose first option is
/// selected by default: a stray Enter never triggers a destructive action.
pub struct Confirm {
    pub title: String,
    pub message: String,
    pub detail: Option<String>,
    options: Vec<String>,
    selected: usize,
    destructive: bool,
}

const CONFIRM_HINT: &str = "\u{2190}/\u{2192} choose \u{00b7} Enter confirm \u{00b7} Esc back";

impl Confirm {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            detail: None,
            options: vec!["Cancel".to_string(), "Continue".to_string()],
            selected: 0,
            destructive: false,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let d = detail.into();
        self.detail = if d.is_empty() { None } else { Some(d) };
        self
    }

    pub fn with_options(mut self, options: &[&str]) -> Self {
        self.options = options.iter().map(|o| o.to_string()).collect();
        self.selected = 0;
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    pub fn is_destructive(&self) -> bool {
        self.destructive
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_label(&self) -> &str {
        self.options
            .get(self.selected)
            .map(String::as_str)
            .unwrap_or("Cancel")
    }

    pub fn handle(&mut self, key: KeyEvent) -> ConfirmAction {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return ConfirmAction::None;
        }
        let last = self.options.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => ConfirmAction::Cancelled,
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                ConfirmAction::None
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(last);
                ConfirmAction::None
            }
            KeyCode::Tab => {
                if !self.options.is_empty() {
                    self.selected = (self.selected + 1) % self.options.len();
                }
                ConfirmAction::None
            }
            KeyCode::BackTab => {
                if !self.options.is_empty() {
                    self.selected = if self.selected == 0 {
                        last
                    } else {
                        self.selected - 1
                    };
                }
                ConfirmAction::None
            }
            KeyCode::Enter => ConfirmAction::Chosen(self.selected),
            // Typing the first letter of an answer is how a person says "no"
            // quickly; it never activates the destructive row by accident,
            // because Cancel starts with C and is the first answer.
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let wanted = ch.to_lowercase().to_string();
                match self
                    .options
                    .iter()
                    .position(|o| o.to_lowercase().starts_with(&wanted))
                {
                    Some(index) => ConfirmAction::Chosen(index),
                    None => ConfirmAction::None,
                }
            }
            _ => ConfirmAction::None,
        }
    }

    /// Activate the option clicked by a pointer event.
    pub fn click(&mut self, index: usize) -> ConfirmAction {
        if index < self.options.len() {
            self.selected = index;
            ConfirmAction::Chosen(index)
        } else {
            ConfirmAction::None
        }
    }

    fn options_total_width(&self) -> u16 {
        let buttons: u16 = self
            .options
            .iter()
            .map(|o| UnicodeWidthStr::width(o.as_str()) as u16 + 4)
            .sum();
        let gaps = self.options.len().saturating_sub(1) as u16 * 2;
        buttons.saturating_add(gaps)
    }

    fn card_rect(&self, area: Rect) -> Rect {
        if area.width == 0 || area.height == 0 {
            return area;
        }
        let max_w = area.width.saturating_sub(4).min(64).max(area.width.min(28));
        let min_w = 46u16.min(max_w);
        let title_w = UnicodeWidthStr::width(self.title.as_str()) as u16 + 6;
        let opts_w = self.options_total_width() + 6;
        let hint_w = UnicodeWidthStr::width(CONFIRM_HINT) as u16 + 6;
        let msg_w = (UnicodeWidthStr::width(self.message.as_str()) as u16 + 6).min(max_w);
        let detail_w = self
            .detail
            .as_deref()
            .map_or(0, |d| (UnicodeWidthStr::width(d) as u16 + 6).min(max_w));
        let width = title_w
            .max(opts_w)
            .max(hint_w)
            .max(msg_w)
            .max(detail_w)
            .clamp(min_w, max_w);

        let text_w = width.saturating_sub(6).max(1);
        let msg_lines = if self.message.is_empty() {
            0u16
        } else {
            WrappedLines::new(&self.message, text_w).take(6).count() as u16
        };
        let detail_lines = self.detail.as_deref().map_or(0u16, |d| {
            WrappedLines::new(d, text_w).take(4).count() as u16
        });
        let mid_gap = u16::from(msg_lines > 0 && detail_lines > 0);
        let body_lines = (msg_lines + mid_gap + detail_lines).max(1);
        // 2 borders + 1 top pad + body + 1 gap + 1 divider + 1 buttons + 1 hint
        let ideal_h = 2 + 1 + body_lines + 1 + 1 + 1 + 1;
        let height = ideal_h.min(area.height).max(area.height.min(7));
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        Rect::new(x, y, width, height)
    }

    fn button_row(&self, inner: Rect) -> u16 {
        if inner.height >= 4 {
            inner.bottom().saturating_sub(2)
        } else {
            inner.bottom().saturating_sub(1)
        }
    }

    /// Return the option index under `(x, y)`, if the pointer is over a button.
    pub fn hit(&self, area: Rect, design: &Design, x: u16, y: u16) -> Option<usize> {
        let card = self.card_rect(area);
        let inner = design.block(Border::Rounded).inner(card);
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        let row = self.button_row(inner);
        if y != row {
            return None;
        }
        let total = self.options_total_width();
        let mut cursor_x = inner.x + inner.width.saturating_sub(total) / 2;
        for (index, option) in self.options.iter().enumerate() {
            let btn_w = UnicodeWidthStr::width(option.as_str()) as u16 + 4;
            if x >= cursor_x && x < cursor_x.saturating_add(btn_w) {
                return Some(index);
            }
            cursor_x = cursor_x.saturating_add(btn_w + 2);
        }
        None
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, design: &Design) {
        let card = self.card_rect(area);
        let (border_role, title_style) = if self.destructive {
            (ColorRole::Danger, design.style(TextStyle::Danger))
        } else {
            (ColorRole::Accent, design.style(TextStyle::Title))
        };
        let inner = draw_card_with(
            buf,
            card,
            design,
            Some(&self.title),
            border_role,
            title_style,
        );
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let button_row = self.button_row(inner);
        let show_hint = inner.height >= 4;
        let divider_row = if inner.height >= 5 {
            Some(button_row.saturating_sub(1))
        } else {
            None
        };
        let content_limit = divider_row.unwrap_or(button_row);
        let text_w = inner.width.saturating_sub(2).max(1);
        let mut y = if inner.height >= 6 {
            inner.y + 1
        } else {
            inner.y
        };

        if !self.message.is_empty() {
            for line in WrappedLines::new(&self.message, text_w).take(6) {
                if y >= content_limit {
                    break;
                }
                centred_line(buf, inner, y, line, design.style(TextStyle::Body));
                y += 1;
            }
        }

        if let Some(detail) = &self.detail {
            if !self.message.is_empty() && y + 1 < content_limit {
                y += 1;
            }
            for line in WrappedLines::new(detail, text_w).take(4) {
                if y >= content_limit {
                    break;
                }
                centred_line(buf, inner, y, line, design.style(TextStyle::Dim));
                y += 1;
            }
        }

        if let Some(div_y) = divider_row {
            let rule_style = Style::default()
                .fg(design.color(ColorRole::SurfaceAlt))
                .bg(design.color(ColorRole::Bg));
            for col in inner.left()..inner.right() {
                if let Some(cell) = buf.cell_mut((col, div_y)) {
                    cell.set_symbol("\u{2500}");
                    cell.set_style(rule_style);
                }
            }
        }

        let total = self.options_total_width();
        let mut x = inner.x + inner.width.saturating_sub(total) / 2;
        for (index, option) in self.options.iter().enumerate() {
            let selected = index == self.selected;
            let is_danger_btn = self.destructive && index > 0;
            let style = if selected {
                if is_danger_btn {
                    Style::default()
                        .fg(design.color(ColorRole::Bg))
                        .bg(design.color(ColorRole::Danger))
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    design.style(TextStyle::Key)
                }
            } else if is_danger_btn {
                Style::default()
                    .fg(design.color(ColorRole::Danger))
                    .bg(design.color(ColorRole::Bg))
            } else {
                design.style(TextStyle::Dim)
            };
            let (left_pad, right_pad) = if selected { ("[ ", " ]") } else { ("  ", "  ") };
            let avail = inner.right().saturating_sub(x);
            x = put(buf, x, button_row, left_pad, avail, style);
            let avail = inner.right().saturating_sub(x);
            x = put(buf, x, button_row, option, avail, style);
            let avail = inner.right().saturating_sub(x);
            x = put(buf, x, button_row, right_pad, avail, style);
            if index + 1 < self.options.len() {
                let avail = inner.right().saturating_sub(x);
                x = put(
                    buf,
                    x,
                    button_row,
                    "  ",
                    avail,
                    design.style(TextStyle::Dim),
                );
            }
        }

        if show_hint {
            centred_line(
                buf,
                inner,
                inner.bottom().saturating_sub(1),
                CONFIRM_HINT,
                design.style(TextStyle::Dim),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// What a key did to an [`Input`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAction {
    /// Not a key the input uses (the screen may give it to the shell).
    Ignored,
    Changed,
    /// Enter, with text in the field.
    Submitted,
    /// Escape: the person did not want to ask after all.
    Cancelled,
}

/// One line of text the person types: the Ask strip's question (spec 007), and
/// later the run prompt (spec 012).
///
/// It edits at the end only - the plan's keymap has no Alt chords inside a
/// window, and a one-line question is typed, not edited in the middle - with
/// Backspace, `Ctrl+U` (clear) and `Ctrl+W` (the last word). Enter on an empty
/// field does nothing: a stray Enter must not send an empty question.
#[derive(Clone, Debug, Default)]
pub struct Input {
    text: String,
    cursor: usize,
    masked: bool,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self {
            text,
            cursor,
            masked: false,
        }
    }

    pub fn with_masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn set_masked(&mut self, masked: bool) {
        self.masked = masked;
    }

    pub fn is_masked(&self) -> bool {
        self.masked
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor.min(self.text.len())
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    fn prev_boundary(&self, pos: usize) -> usize {
        let pos = pos.min(self.text.len());
        self.text[..pos]
            .char_indices()
            .next_back()
            .map_or(0, |(idx, _)| idx)
    }

    fn next_boundary(&self, pos: usize) -> usize {
        let pos = pos.min(self.text.len());
        let mut iter = self.text[pos..].char_indices();
        let _ = iter.next();
        iter.next().map_or(self.text.len(), |(off, _)| pos + off)
    }

    /// True when `key` edits text or moves the caret inside this input
    /// (`Char`, `Backspace`, `Delete`, `Left`, `Right`, `Ctrl+U`, `Ctrl+W`, `Ctrl+A`, `Ctrl+E`),
    /// leaving `Enter`, `Esc`, `Up`, `Down`, `Home`, `End`, and app shortcuts untouched.
    pub fn handles_edit_key(key: KeyEvent) -> bool {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return false;
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if alt {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return matches!(
                key.code,
                KeyCode::Char('a' | 'e' | 'u' | 'w' | 'A' | 'E' | 'U' | 'W')
            );
        }
        matches!(
            key.code,
            KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
        )
    }

    pub fn handle(&mut self, key: KeyEvent) -> InputAction {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return InputAction::Ignored;
        }
        self.cursor = self.cursor.min(self.text.len());
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => InputAction::Cancelled,
            KeyCode::Enter if self.text.trim().is_empty() => InputAction::Ignored,
            KeyCode::Enter => InputAction::Submitted,
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor = self.prev_boundary(self.cursor);
                    InputAction::Changed
                } else {
                    InputAction::Ignored
                }
            }
            KeyCode::Right => {
                if self.cursor < self.text.len() {
                    self.cursor = self.next_boundary(self.cursor);
                    InputAction::Changed
                } else {
                    InputAction::Ignored
                }
            }
            KeyCode::Home => {
                if self.cursor > 0 {
                    self.cursor = 0;
                    InputAction::Changed
                } else {
                    InputAction::Ignored
                }
            }
            KeyCode::End => {
                if self.cursor < self.text.len() {
                    self.cursor = self.text.len();
                    InputAction::Changed
                } else {
                    InputAction::Ignored
                }
            }
            KeyCode::Char('a' | 'A') if control => {
                self.cursor = 0;
                InputAction::Changed
            }
            KeyCode::Char('e' | 'E') if control => {
                self.cursor = self.text.len();
                InputAction::Changed
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.prev_boundary(self.cursor);
                    self.text.drain(prev..self.cursor);
                    self.cursor = prev;
                    InputAction::Changed
                } else {
                    InputAction::Ignored
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.text.len() {
                    let next = self.next_boundary(self.cursor);
                    self.text.drain(self.cursor..next);
                    InputAction::Changed
                } else {
                    InputAction::Ignored
                }
            }
            KeyCode::Char('u' | 'U') if control => {
                self.text.clear();
                self.cursor = 0;
                InputAction::Changed
            }
            KeyCode::Char('w' | 'W') if control => {
                let before = &self.text[..self.cursor];
                let kept = before.trim_end().len();
                let start = before[..kept]
                    .rfind(char::is_whitespace)
                    .map_or(0, |space| space + 1);
                self.text.drain(start..self.cursor);
                self.cursor = start;
                InputAction::Changed
            }
            KeyCode::Char(ch) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.text.insert(self.cursor, ch);
                self.cursor += ch.len_utf8();
                InputAction::Changed
            }
            _ => InputAction::Ignored,
        }
    }

    /// One line: the text, or the dim placeholder, and a block cursor. A text
    /// wider than the field shows its tail, because the end is where the
    /// person is typing.
    pub fn render(&self, area: Rect, buf: &mut Buffer, design: &Design, placeholder: &str) {
        self.render_field(
            area,
            buf,
            design,
            placeholder,
            true,
            true,
            ColorRole::Surface,
            TextStyle::Body,
        );
    }

    /// Render the input field with a custom background role and text style,
    /// drawing the active cursor at `self.cursor` with zero heap allocations.
    pub fn render_styled(
        &self,
        area: Rect,
        buf: &mut Buffer,
        design: &Design,
        placeholder: &str,
        bg_role: ColorRole,
        text_style: TextStyle,
    ) {
        self.render_field(
            area,
            buf,
            design,
            placeholder,
            true,
            true,
            bg_role,
            text_style,
        );
    }

    /// Render the input field with explicit focus and blink state, a custom
    /// background role and text style, and zero heap allocations.
    ///
    /// When `focused` is true, column `area.x` carries an accent focus bar
    /// (`\u{258d}`) and the cursor highlights at `self.cursor` using
    /// `ColorRole::Accent` when `blink_on || design.reduced_motion()`. When
    /// `focused` is false, no cursor or focus bar is drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn render_field(
        &self,
        area: Rect,
        buf: &mut Buffer,
        design: &Design,
        placeholder: &str,
        focused: bool,
        blink_on: bool,
        bg_role: ColorRole,
        text_style: TextStyle,
    ) {
        if area.width < 2 || area.height == 0 {
            return;
        }
        let bg = design.color(bg_role);
        let accent = design.color(ColorRole::Accent);
        fill(
            buf,
            Rect { height: 1, ..area },
            Style::default().bg(bg).fg(design.color(ColorRole::Text)),
        );
        if focused {
            put(
                buf,
                area.x,
                area.y,
                "\u{258d}",
                1,
                Style::default().fg(accent).bg(bg),
            );
        }
        let room = area.width.saturating_sub(2);
        let x = area.x + 1;
        let normal_style = design.style(text_style).bg(bg);
        let cursor_visible = focused && (blink_on || design.reduced_motion());
        let cursor_style = if cursor_visible {
            Style::default()
                .fg(design.color(ColorRole::Bg))
                .bg(accent)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            normal_style
        };

        if self.text.is_empty() {
            let ph_style = design.style(TextStyle::Dim).bg(bg);
            if focused {
                let mut ph = placeholder.chars();
                if let Some(first_ch) = ph.next() {
                    let mut tmp = [0u8; 4];
                    let first_str = first_ch.encode_utf8(&mut tmp);
                    let first_style = if cursor_visible {
                        cursor_style
                    } else {
                        ph_style
                    };
                    put(buf, x, area.y, first_str, 1, first_style);
                    put_trunc(
                        buf,
                        x + 1,
                        area.y,
                        ph.as_str(),
                        room.saturating_sub(1),
                        ph_style,
                    );
                } else {
                    put(buf, x, area.y, " ", 1, cursor_style);
                }
            } else {
                put_trunc(buf, x, area.y, placeholder, room, ph_style);
            }
            return;
        }

        if self.masked {
            const STARS: &str = "****************************************************************************************************";
            let char_count = self.text.chars().count();
            let cur_chars = self.text[..self.cursor.min(self.text.len())]
                .chars()
                .count();
            let tail_reserve = if focused && cur_chars >= char_count {
                1
            } else {
                0
            };
            let max_vis = room.saturating_sub(tail_reserve) as usize;
            let mut start_char = char_count.saturating_sub(max_vis);
            if cur_chars < start_char {
                start_char = cur_chars;
            }
            let vis_chars = char_count.saturating_sub(start_char).min(room as usize);
            let cur_offset = cur_chars.saturating_sub(start_char).min(vis_chars);
            let before_len = cur_offset.min(STARS.len());
            let c1 = put(buf, x, area.y, &STARS[..before_len], room, normal_style);
            if focused {
                if cur_offset < vis_chars {
                    let used = c1.saturating_sub(x);
                    let c2 = put(
                        buf,
                        c1,
                        area.y,
                        "*",
                        room.saturating_sub(used),
                        cursor_style,
                    );
                    let used2 = c2.saturating_sub(x);
                    let after_len = vis_chars.saturating_sub(cur_offset + 1).min(STARS.len());
                    put(
                        buf,
                        c2,
                        area.y,
                        &STARS[..after_len],
                        room.saturating_sub(used2),
                        normal_style,
                    );
                } else if c1 < x + room {
                    put(buf, c1, area.y, " ", 1, cursor_style);
                }
            } else {
                let rest = vis_chars.saturating_sub(before_len).min(STARS.len());
                let used = c1.saturating_sub(x);
                put(
                    buf,
                    c1,
                    area.y,
                    &STARS[..rest],
                    room.saturating_sub(used),
                    normal_style,
                );
            }
            return;
        }

        let cur = self.cursor.min(self.text.len());
        // The tail that fits, measured in cells from the end.
        let mut start = self.text.len();
        let mut width = 0u16;
        let tail_reserve = if focused && cur >= self.text.len() {
            1
        } else {
            0
        };
        for (index, ch) in self.text.char_indices().rev() {
            let w = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]) as &str) as u16;
            if width + w + tail_reserve > room {
                break;
            }
            width += w;
            start = index;
        }
        if cur < start {
            start = cur;
        }
        if !focused {
            put(buf, x, area.y, &self.text[start..], room, normal_style);
        } else if cur >= self.text.len() {
            let end = put(buf, x, area.y, &self.text[start..], room, normal_style);
            if end < x + room {
                put(buf, end, area.y, " ", 1, cursor_style);
            }
        } else {
            let next = self.next_boundary(cur);
            let before = &self.text[start..cur];
            let cur_str = &self.text[cur..next];
            let after = &self.text[next..];
            let c1 = put(buf, x, area.y, before, room, normal_style);
            let used1 = c1.saturating_sub(x);
            let c2 = put(
                buf,
                c1,
                area.y,
                cur_str,
                room.saturating_sub(used1),
                cursor_style,
            );
            let used2 = c2.saturating_sub(x);
            put(
                buf,
                c2,
                area.y,
                after,
                room.saturating_sub(used2),
                normal_style,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ActionStrip — Separating Tool Actions from Results
// ---------------------------------------------------------------------------

/// A single action pill inside an [`ActionStrip`].
#[derive(Clone, Debug)]
pub struct ActionItem {
    pub id: &'static str,
    pub key: &'static str,
    pub label: String,
    pub glyph: Option<Glyph>,
}

impl ActionItem {
    pub fn new(id: &'static str, key: &'static str, label: impl Into<String>) -> Self {
        Self {
            id,
            key,
            label: label.into(),
            glyph: None,
        }
    }

    pub fn glyph(mut self, glyph: Glyph) -> Self {
        self.glyph = Some(glyph);
        self
    }

    fn pill_width(&self) -> u16 {
        let l_w = UnicodeWidthStr::width(self.label.as_str()) as u16;
        let g_w = self
            .glyph
            .map(|g| {
                let w = UnicodeWidthStr::width(g.cell()) as u16;
                if l_w == 0 {
                    w
                } else {
                    w + if w <= 1 { 2 } else { 1 }
                }
            })
            .unwrap_or(0);
        let k_w = UnicodeWidthStr::width(self.key) as u16;
        // " [key] [glyph[  ]]label "
        1 + k_w + 1 + g_w + l_w + 1
    }
}

/// What happened when an [`ActionStrip`] handled a key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionStripEvent {
    Ignored,
    Consumed,
    Activated(&'static str),
}

/// Horizontal bar of action pills separating tool-level operations (Rescan,
/// Toggle Radio, Clean All, Pick Pixel, etc.) from the scrollable domain
/// results list.
///
/// Users can either:
/// - Press `Tab` to focus the strip, use `Left`/`Right` to select a pill, and
///   press `Enter`/`Space` to activate it (`Down`/`Tab`/`Esc` returns focus to
///   the results list),
/// - Click any pill directly with the mouse, or
/// - Trigger actions anytime via their direct shortcut (`Ctrl+R`, `Ctrl+T`, etc.).
#[derive(Clone, Debug, Default)]
pub struct ActionStrip {
    items: Vec<ActionItem>,
    cursor: usize,
    active: Option<usize>,
    focused: bool,
}

impl ActionStrip {
    pub fn new(items: Vec<ActionItem>) -> Self {
        Self {
            items,
            cursor: 0,
            active: None,
            focused: false,
        }
    }

    pub fn with_active(mut self, active: Option<usize>) -> Self {
        self.active = active.filter(|&i| i < self.items.len());
        if let Some(i) = self.active {
            self.cursor = i;
        }
        self
    }

    pub fn set_active(&mut self, active: Option<usize>) {
        self.active = active.filter(|&i| i < self.items.len());
        if let Some(i) = self.active {
            self.cursor = i;
        }
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        if cursor < self.items.len() {
            self.cursor = cursor;
        }
    }

    pub fn set_items(&mut self, items: Vec<ActionItem>) {
        self.items = items;
        if self.cursor >= self.items.len() && !self.items.is_empty() {
            self.cursor = self.items.len() - 1;
        }
        if let Some(a) = self.active
            && a >= self.items.len()
        {
            self.active = None;
        }
    }

    pub fn items(&self) -> &[ActionItem] {
        &self.items
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Handle keyboard navigation when `Tab` toggles focus or when the strip is focused.
    pub fn handle(&mut self, key: KeyEvent) -> ActionStripEvent {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press || self.items.is_empty() {
            return ActionStripEvent::Ignored;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.focused = !self.focused;
            return ActionStripEvent::Consumed;
        }
        if !self.focused {
            return ActionStripEvent::Ignored;
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                } else {
                    self.cursor = self.items.len() - 1;
                }
                ActionStripEvent::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.cursor = (self.cursor + 1) % self.items.len();
                ActionStripEvent::Consumed
            }
            KeyCode::Down | KeyCode::Up | KeyCode::Esc => {
                self.focused = false;
                ActionStripEvent::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let id = self.items[self.cursor].id;
                self.focused = false;
                ActionStripEvent::Activated(id)
            }
            _ => ActionStripEvent::Ignored,
        }
    }

    /// Map a mouse click at `(col, row)` inside `area` to an action ID.
    pub fn hit(&self, area: Rect, col: u16, row: u16) -> Option<&'static str> {
        self.hit_index(area, col, row).map(|idx| self.items[idx].id)
    }

    /// Map a mouse click at `(col, row)` inside `area` to an action index.
    pub fn hit_index(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        if area.height == 0 || row != area.y || col < area.x || col >= area.right() {
            return None;
        }
        let mut x = area.x;
        for (idx, item) in self.items.iter().enumerate() {
            let w = item.pill_width();
            if x + w > area.right() {
                break;
            }
            if col >= x && col < x + w {
                return Some(idx);
            }
            x += w + 1;
        }
        None
    }

    /// Render the action strip on a single row with zero heap allocations.
    pub fn render(&self, area: Rect, buf: &mut Buffer, design: &Design) {
        self.render_on(area, buf, design, ColorRole::Surface);
    }

    /// Render the action strip on a specific container background role
    /// (`ColorRole::Surface` on `Bg` screens; `ColorRole::Bg` inside `Surface`
    /// cards so unselected pills keep distinct contrast) with zero allocations.
    pub fn render_on(
        &self,
        area: Rect,
        buf: &mut Buffer,
        design: &Design,
        unselected_bg: ColorRole,
    ) {
        if area.width < 8 || area.height == 0 || self.items.is_empty() {
            return;
        }
        let y = area.y;
        let mut x = area.x;
        for (idx, item) in self.items.iter().enumerate() {
            let w = item.pill_width();
            if x + w > area.right() {
                break;
            }
            let selected = self.focused && idx == self.cursor;
            let active = !selected && self.active == Some(idx);
            let pill_bg = if selected {
                design.color(ColorRole::Accent)
            } else if active {
                let alt = if unselected_bg == ColorRole::SurfaceAlt {
                    ColorRole::Surface
                } else {
                    ColorRole::SurfaceAlt
                };
                design.color(alt)
            } else {
                design.color(unselected_bg)
            };
            let key_style = if selected {
                Style::default()
                    .fg(design.color(ColorRole::Bg))
                    .bg(pill_bg)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .bg(pill_bg)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            };
            let label_style = if selected {
                Style::default()
                    .fg(design.color(ColorRole::Bg))
                    .bg(pill_bg)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else if active {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .bg(pill_bg)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
                    .fg(design.color(ColorRole::Text))
                    .bg(pill_bg)
            };
            fill(
                buf,
                Rect {
                    x,
                    y,
                    width: w,
                    height: 1,
                },
                label_style,
            );
            let mut col = put(buf, x, y, " ", 1, label_style);
            col = put(buf, col, y, item.key, w, key_style);
            col = put(buf, col, y, " ", 1, label_style);
            if let Some(g) = item.glyph {
                let gw = UnicodeWidthStr::width(g.cell()) as u16;
                col = put(buf, col, y, g.cell(), 4, label_style);
                if !item.label.is_empty() {
                    let gap = if gw <= 1 { "  " } else { " " };
                    col = put(buf, col, y, gap, gap.len() as u16, label_style);
                }
            }
            if !item.label.is_empty() {
                put(
                    buf,
                    col,
                    y,
                    &item.label,
                    (x + w).saturating_sub(col + 1),
                    label_style,
                );
            }
            x += w + 1;
        }
        let hint = if self.focused {
            "[\u{2190}/\u{2192}] move  [Enter] run  [Esc] list"
        } else {
            "[Tab] actions"
        };
        let hint_w = UnicodeWidthStr::width(hint) as u16;
        if x + hint_w < area.right() {
            put(
                buf,
                area.right() - hint_w,
                y,
                hint,
                hint_w,
                design.style(TextStyle::Dim),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Signal bars, half-block QR renderer, and non-blocking ActionRunner
// ---------------------------------------------------------------------------

/// Four-step Wi-Fi/radio signal bar string (`▂▄▆█`) using font-pinned block glyphs.
pub fn signal_bars(signal: u8) -> &'static str {
    match signal {
        0..=9 => "    ",
        10..=34 => "\u{2582}   ",
        35..=59 => "\u{2582}\u{2584}  ",
        60..=79 => "\u{2582}\u{2584}\u{2586} ",
        _ => "\u{2582}\u{2584}\u{2586}\u{2588}",
    }
}

/// Draw a 4-bar signal strength indicator (`▂▄▆█`) colored by signal quality,
/// returning the column after the 4 bars.
pub fn draw_signal_bars(buf: &mut Buffer, x: u16, y: u16, design: &Design, signal: u8) -> u16 {
    const BARS: [&str; 4] = ["\u{2582}", "\u{2584}", "\u{2586}", "\u{2588}"];
    let active_count = match signal {
        0..=9 => 0,
        10..=34 => 1,
        35..=59 => 2,
        60..=79 => 3,
        _ => 4,
    };
    let active_role = if signal >= 65 {
        ColorRole::Ok
    } else if signal >= 35 {
        ColorRole::Warn
    } else {
        ColorRole::Danger
    };
    let on_style = Style::default().fg(design.color(active_role));
    let off_style = Style::default().fg(design.color(ColorRole::SurfaceAlt));
    let mut col = x;
    for (i, sym) in BARS.iter().enumerate() {
        let style = if i < active_count {
            on_style
        } else {
            off_style
        };
        col = put(buf, col, y, sym, 1, style);
    }
    col
}

/// A parsed QR module grid (`true` = dark module, `false` = light module / quiet zone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCode {
    pub width: u16,
    pub height: u16,
    pub modules: Vec<bool>,
}

impl QrCode {
    /// Parse `qrencode -t ASCII -m 2` output into a module grid.
    /// In `qrencode -t ASCII`, each dark module is `"##"` and each light module is `"  "`.
    pub fn from_ascii(ascii: &str) -> Option<Self> {
        let lines: Vec<&str> = ascii.lines().filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            return None;
        }
        let max_chars = lines.iter().map(|l| l.len()).max().unwrap_or(0);
        if max_chars < 2 {
            return None;
        }
        let width = (max_chars / 2) as u16;
        let height = lines.len() as u16;
        let mut modules = Vec::with_capacity(width as usize * height as usize);
        for line in &lines {
            let bytes = line.as_bytes();
            for col in 0..(width as usize) {
                let idx = col * 2;
                let dark = bytes.get(idx).copied() == Some(b'#');
                modules.push(dark);
            }
        }
        Some(Self {
            width,
            height,
            modules,
        })
    }

    /// Encode `payload` in-memory via `qrencode -t ASCII -m 2 -o -` without
    /// creating temporary files or invoking a shell.
    pub fn encode(payload: &str) -> Option<Self> {
        if payload.is_empty() {
            return None;
        }
        let output = std::process::Command::new("qrencode")
            .args(["-t", "ASCII", "-m", "2", "-o", "-", "--", payload])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let ascii = String::from_utf8_lossy(&output.stdout);
        Self::from_ascii(&ascii)
    }

    /// Terminal cell dimensions `(cols, rows)` required by `draw_qr`:
    /// 2 horizontal cells per module (or 1 cell per module with 2 vertical modules per cell).
    /// Using 2 horizontal chars × 1 vertical cell per 2×2 module block keeps the QR square
    /// even when a font renders half-blocks 1 cell wide, or 1 cell wide × 1/2 cell tall when
    /// `cell_aspect` is 1:2 (standard monospace). Here each cell is 1 module wide by 2 modules tall.
    pub fn cell_size(&self) -> (u16, u16) {
        (self.width, self.height.div_ceil(2))
    }

    /// Recommended floating window pixel dimensions `(width_px, height_px)` so
    /// the QR code, quiet zone, card frame, header, and subtitle fit cleanly.
    pub fn recommended_window_size(&self) -> (u16, u16) {
        let (cols, rows) = self.cell_size();
        let w = (cols.saturating_mul(11).saturating_add(240)).clamp(680, 1040);
        let h = (rows.saturating_mul(22).saturating_add(260)).clamp(560, 860);
        (w, h)
    }

    #[inline]
    pub fn get(&self, col: u16, row: u16) -> bool {
        if col >= self.width || row >= self.height {
            return false;
        }
        self.modules
            .get(row as usize * self.width as usize + col as usize)
            .copied()
            .unwrap_or(false)
    }
}

/// Render a [`QrCode`] centered inside `area` using Unicode half-blocks
/// (`█` `\u{2588}`, `▀` `\u{2580}`, `▄` `\u{2584}`, and `" "`) with zero heap allocations.
pub fn draw_qr(buf: &mut Buffer, area: Rect, design: &Design, qr: &QrCode) {
    let (qr_cols, qr_rows) = qr.cell_size();
    if area.width < qr_cols || area.height < qr_rows || qr_cols == 0 || qr_rows == 0 {
        return;
    }
    let start_x = area.x + (area.width - qr_cols) / 2;
    let start_y = area.y + (area.height - qr_rows) / 2;
    // Standard black-on-white (or inverted Text-on-Bg) quiet zone + modules:
    // Using `Text` for light modules/quiet zone and `Bg` for dark modules (or vice-versa)
    // ensures high contrast on both dark and light themes.
    let style = Style::default()
        .fg(design.color(ColorRole::Text))
        .bg(design.color(ColorRole::Bg));

    for r in 0..qr_rows {
        let y = start_y + r;
        let mod_y_top = r * 2;
        let mod_y_bot = r * 2 + 1;
        for c in 0..qr_cols {
            let x = start_x + c;
            // Invert so the quiet zone (`false`) is drawn as solid light (`Text`)
            // and dark QR modules (`true`) are `Bg`, which phone cameras scan reliably
            // on dark terminal themes!
            let top_light = !qr.get(c, mod_y_top);
            let bot_light = if mod_y_bot < qr.height {
                !qr.get(c, mod_y_bot)
            } else {
                false
            };
            let sym = match (top_light, bot_light) {
                (true, true) => "\u{2588}",
                (true, false) => "\u{2580}",
                (false, true) => "\u{2584}",
                (false, false) => " ",
            };
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(sym).set_style(style);
            }
        }
    }
}

/// Ten-frame braille spinner sequence shared across tasks and in-window actions.
pub const BRAILLE_SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

/// Post-action status notice with an explicit transient vs. persistent lifecycle.
///
/// - **Routine discovery** (Wi-Fi/Bluetooth/Clean rescan) clears status immediately
///   upon completion (`clear_status()`), so `"Scanning..."` disappears as soon as
///   fresh results arrive.
/// - **Positive state transitions** (connected, toggled radio, paired) show a
///   **transient** notice (`set_transient`) that automatically disappears after 3s.
/// - **Action failures or critical summaries** where the user *must* know the last
///   triggered outcome override auto-dismissal (`set_persistent_error` /
///   `set_persistent_info`) and stay visible until the next action is triggered.
#[derive(Clone, Debug)]
pub struct ActionNotice {
    pub text: String,
    pub is_error: bool,
    pub expires_at: Option<std::time::Instant>,
}

/// What an [`ActionRunner`] is doing: idle, a background closure in flight,
/// or a post-action notice. The receiver and its label live in the running
/// variant, so "running" cannot exist without the thing it is running.
enum RunnerState<T> {
    Idle,
    Running {
        rx: std::sync::mpsc::Receiver<T>,
        label: String,
        /// A notice set while the action is still running. Screens currently
        /// set notices only after `poll` returns, but the old API allowed it
        /// and `poll` must not lose the result when it happens.
        notice: Option<ActionNotice>,
    },
    Notice(ActionNotice),
}

/// Non-blocking background action runner for interactive `Screen`s (1–8s operations
/// like Wi-Fi rescan/connect, Bluetooth toggle/pair, DNS switch, cache cleanup).
pub struct ActionRunner<T> {
    state: RunnerState<T>,
    frame: usize,
}

impl<T: Send + 'static> ActionRunner<T> {
    pub fn new() -> Self {
        Self {
            state: RunnerState::Idle,
            frame: 0,
        }
    }

    /// Spawn a background closure, clear any previous notice, and enter the spinning state.
    pub fn spawn(&mut self, label: impl Into<String>, f: impl FnOnce() -> T + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.state = RunnerState::Running {
            rx,
            label: label.into(),
            notice: None,
        };
        self.frame = 0;
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, RunnerState::Running { .. })
    }

    pub fn label(&self) -> Option<&str> {
        match &self.state {
            RunnerState::Running { label, .. } => Some(label),
            RunnerState::Idle | RunnerState::Notice(_) => None,
        }
    }

    pub fn spinner(&self) -> &'static str {
        BRAILLE_SPINNER[self.frame % BRAILLE_SPINNER.len()]
    }

    /// Clear any post-action notice immediately (used when routine scans finish).
    pub fn clear_status(&mut self) {
        if let RunnerState::Running { notice, .. } = &mut self.state {
            *notice = None;
        } else {
            self.state = RunnerState::Idle;
        }
    }

    fn set_notice(&mut self, notice: ActionNotice) {
        if let RunnerState::Running {
            notice: current, ..
        } = &mut self.state
        {
            *current = Some(notice);
        } else {
            self.state = RunnerState::Notice(notice);
        }
    }

    /// Show a transient confirmation notice that automatically disappears after 3 seconds.
    pub fn set_transient(&mut self, text: impl Into<String>) {
        self.set_notice(ActionNotice {
            text: text.into(),
            is_error: false,
            expires_at: Some(std::time::Instant::now() + std::time::Duration::from_secs(3)),
        });
    }

    /// Show a persistent error notice (overrides auto-hide because the user must know why
    /// the last triggered action failed).
    pub fn set_persistent_error(&mut self, text: impl Into<String>) {
        self.set_notice(ActionNotice {
            text: text.into(),
            is_error: true,
            expires_at: None,
        });
    }

    /// Show a persistent informational summary (used when the user must read a final
    /// measurement such as total freed disk space).
    pub fn set_persistent_info(&mut self, text: impl Into<String>) {
        self.set_notice(ActionNotice {
            text: text.into(),
            is_error: false,
            expires_at: None,
        });
    }

    /// Active post-action notice (if not running and not expired).
    pub fn notice(&self) -> Option<&ActionNotice> {
        match &self.state {
            RunnerState::Running { notice, .. } => notice.as_ref(),
            RunnerState::Notice(notice) => Some(notice),
            RunnerState::Idle => None,
        }
    }

    pub fn tick_rate(&self) -> Option<std::time::Duration> {
        if self.is_running() {
            Some(std::time::Duration::from_millis(100))
        } else if let RunnerState::Notice(notice) = &self.state
            && notice.expires_at.is_some()
        {
            Some(std::time::Duration::from_millis(250))
        } else {
            None
        }
    }

    fn expire_notice(&mut self) {
        let expired = match &self.state {
            RunnerState::Notice(notice) => notice
                .expires_at
                .is_some_and(|exp| std::time::Instant::now() >= exp),
            RunnerState::Running {
                notice: Some(notice),
                ..
            } => notice
                .expires_at
                .is_some_and(|exp| std::time::Instant::now() >= exp),
            _ => false,
        };
        if expired {
            if let RunnerState::Running { notice, .. } = &mut self.state {
                *notice = None;
            } else {
                self.state = RunnerState::Idle;
            }
        }
    }

    /// Advance spinner, expire transient notices, and poll for completion;
    /// returns `Some(result)` once the background closure finishes.
    pub fn poll(&mut self) -> Option<T> {
        self.expire_notice();
        let finished = {
            let RunnerState::Running { rx, notice, .. } = &mut self.state else {
                return None;
            };
            self.frame = (self.frame + 1) % BRAILLE_SPINNER.len();
            match rx.try_recv() {
                Ok(value) => Some((Some(value), notice.take())),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((None, notice.take())),
            }
        };
        let (value, notice) = finished?;
        self.state = match notice {
            Some(notice) => RunnerState::Notice(notice),
            None => RunnerState::Idle,
        };
        value
    }
}

impl<T: Send + 'static> Default for ActionRunner<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests that do not need a snapshot
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn rows(n: usize) -> ListState {
        let mut list = ListState::new();
        list.set_rows(
            (0..n)
                .map(|i| Row::new(format!("row {i:02}")).detail("detail"))
                .collect(),
        );
        list
    }

    #[test]
    fn a_heading_is_drawn_but_never_selected() {
        let design = Design::solarized();
        let mut list = ListState::new();
        list.set_rows(vec![
            Row::heading("Open"),
            Row::new("Terminal"),
            Row::heading("AI"),
            Row::new("Ask"),
        ]);
        list.set_cursor(0);
        assert_eq!(list.selected().unwrap().label, "Terminal");
        list.handle(key(KeyCode::Down), 10, &design);
        assert_eq!(list.selected().unwrap().label, "Ask");
        // A heading is not a row a click or Enter can land on.
        let area = Rect::new(0, 0, 30, 4);
        assert_eq!(list.hit_row(area, 2, 0, &design), None);
        assert_eq!(list.hit_row(area, 2, 1, &design), Some(1));
        // Typing hides the labels: a section heading is not an answer.
        list.set_filter("ask");
        assert_eq!(list.visible_len(), 1);
        assert_eq!(list.selected().unwrap().label, "Ask");
    }

    #[test]
    fn the_keybar_drops_a_hint_rather_than_cutting_its_label() {
        let design = Design::solarized();
        let area = Rect::new(0, 0, 34, 1);
        let mut buf = Buffer::empty(area);
        draw_keybar(
            &mut buf,
            area,
            &design,
            &[
                KeyHint::new("Enter", "open"),
                KeyHint::new("Ctrl+H", "dotfiles"),
                KeyHint::new("Esc", "close"),
            ],
        );
        let line: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol())
            .collect();
        assert!(line.contains('\u{2026}'), "no ellipsis: {line:?}");
        assert!(!line.contains("dotf"), "a label was cut: {line:?}");
        assert!(line.contains("[Enter]"), "{line:?}");
        assert!(line.contains("open"), "{line:?}");
        // The way out is the one hint the bar may not drop.
        assert!(line.contains("[Esc]"), "{line:?}");
        assert!(line.contains("close"), "{line:?}");
    }

    #[test]
    fn action_runner_moves_from_running_to_notice_to_idle() {
        let mut runner = ActionRunner::new();
        assert!(!runner.is_running());
        runner.spawn("Working", || 7);
        assert!(runner.is_running());
        assert_eq!(runner.label(), Some("Working"));

        let value = (0..100)
            .find_map(|_| {
                let value = runner.poll();
                if value.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                value
            })
            .expect("the runner finished");
        assert_eq!(value, 7);
        assert!(!runner.is_running());
        assert!(runner.notice().is_none());

        runner.set_transient("done");
        assert!(runner.notice().is_some());
        assert!(runner.tick_rate().is_some());
        runner.clear_status();
        assert!(runner.notice().is_none());
        assert!(runner.tick_rate().is_none());
    }

    #[test]
    fn a_notice_set_while_running_survives_the_result() {
        let mut runner = ActionRunner::new();
        runner.spawn("Slow", || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            9
        });
        runner.set_transient("still working");
        assert!(runner.is_running());
        assert!(runner.notice().is_some());

        let value = (0..100)
            .find_map(|_| {
                let value = runner.poll();
                if value.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                value
            })
            .expect("the runner finished");
        assert_eq!(value, 9);
        assert!(runner.notice().is_some(), "the notice survives the result");
    }

    #[test]
    fn the_cursor_scrolls_the_window_and_stops_at_the_ends() {
        let design = Design::solarized();
        let mut list = rows(40);
        for _ in 0..39 {
            assert_eq!(
                list.handle(key(KeyCode::Down), 10, &design),
                ListAction::Moved
            );
        }
        assert_eq!(list.cursor(), 39);
        assert_eq!(
            list.handle(key(KeyCode::Down), 10, &design),
            ListAction::Moved
        );
        assert_eq!(list.cursor(), 39, "the last row is the end");
        assert_eq!(
            list.handle(key(KeyCode::Home), 10, &design),
            ListAction::Moved
        );
        assert_eq!(list.cursor(), 0);
    }

    #[test]
    fn the_window_follows_the_cursor() {
        let design = Design::solarized();
        let mut list = rows(40);
        for _ in 0..25 {
            list.handle(key(KeyCode::Down), 10, &design);
        }
        // The cursor must be inside the drawn window.
        assert!(list.offset <= list.cursor);
        assert!(list.cursor < list.offset + 10);
    }

    #[test]
    fn a_click_maps_to_the_row_it_was_drawn_on() {
        let normal = Design::solarized();
        let tablet = Design::solarized().with_density(crate::design::Density::Tablet);
        let list = rows(40);
        let area = Rect::new(0, 3, 80, 20);
        for (design, step) in [(normal, 1u16), (tablet, 2u16)] {
            for row in 0..5u16 {
                let y = area.y + row * step;
                assert_eq!(
                    list.hit_row(area, 10, y, &design),
                    Some(row as usize),
                    "{design:?} row {row}"
                );
                if step == 2 {
                    // The blank line between two touch rows belongs to nobody.
                    assert_eq!(list.hit_row(area, 10, y + 1, &design), None);
                }
            }
            assert_eq!(list.hit_row(area, 10, area.bottom(), &design), None);
            assert_eq!(list.hit_row(area, 10, area.y + 200, &design), None);
        }
    }

    #[test]
    fn the_filter_narrows_the_rows_and_keeps_the_index() {
        let mut list = ListState::new();
        list.set_rows(vec![
            Row::new("alpha").detail("first"),
            Row::new("beta").detail("second"),
            Row::new("gamma").detail("third"),
        ]);
        list.set_filter("amm");
        assert_eq!(list.visible_len(), 1);
        assert_eq!(list.selected().unwrap().label, "gamma");
        // The index is the unfiltered one: a command acts on the row, not on
        // its position in the filtered view.
        assert_eq!(list.selected_index(), Some(2));
    }

    #[test]
    fn typing_builds_the_filter_and_backspace_removes_it() {
        let mut list = rows(5);
        assert!(list.type_filter(key(KeyCode::Char('r'))));
        assert_eq!(list.filter(), "r");
        assert!(!list.type_filter(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)));
        assert!(list.type_filter(key(KeyCode::Backspace)));
        assert_eq!(list.filter(), "");
    }

    #[test]
    fn a_row_without_a_glyph_lines_up_with_one_that_has_it() {
        let design = Design::solarized();
        let mut list = ListState::new();
        list.set_rows(vec![
            Row::new("pictured").glyph(Glyph::Folder),
            Row::new("bare"),
            // A text token is wider than a codepoint; the column takes the
            // widest row, so every label still starts at the same place.
            Row::new("tokenised").glyph(Glyph::Bluetooth),
        ]);
        let area = Rect::new(0, 0, 60, 3);
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf, &design, false);
        let line = |y: u16| -> String {
            (0..60)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect()
        };
        let columns: Vec<usize> = ["pictured", "bare", "tokenised"]
            .iter()
            .enumerate()
            .map(|(row, label)| {
                let text = line(row as u16);
                let byte = text
                    .find(label)
                    .unwrap_or_else(|| panic!("{label} is not on row {row}"));
                // A glyph is three bytes and one cell, so the column is the
                // number of characters before the label, not of bytes.
                text[..byte].chars().count()
            })
            .collect();
        assert_eq!(columns[0], columns[1], "a row with no glyph drifts left");
        assert_eq!(columns[0], columns[2], "a text token widens the column");
        assert_eq!(columns[0], 5, "two of margin, the token, one of gap");
    }

    #[test]
    fn a_list_without_glyphs_does_not_reserve_the_column() {
        let design = Design::solarized();
        let mut list = ListState::new();
        list.set_rows(vec![Row::new("plain")]);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf, &design, false);
        let text: String = (0..40)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(text.starts_with("  plain"), "{text:?}");
    }

    #[test]
    fn the_fallback_token_stays_ascii_when_icon_exists() {
        assert_eq!(Glyph::Bluetooth.codepoint(), Some('\u{f293}'));
        assert_eq!(Glyph::Bluetooth.cell(), "\u{f293}");
        assert_eq!(Glyph::Bluetooth.token(), "bt");
        assert_eq!(Glyph::Folder.cell(), "\u{f413}");
        let row = Row::new("x").glyph(Glyph::Bluetooth);
        assert_eq!(row.glyph.as_deref(), Some("\u{f293}"));
    }

    #[test]
    fn confirm_starts_on_cancel_and_needs_a_second_key_for_continue() {
        let mut confirm = Confirm::new("Format the card?", "This erases it");
        assert_eq!(confirm.selected_label(), "Cancel");
        assert_eq!(confirm.handle(key(KeyCode::Right)), ConfirmAction::None);
        assert_eq!(confirm.selected_label(), "Continue");
        assert_eq!(confirm.handle(key(KeyCode::Right)), ConfirmAction::None);
        assert_eq!(
            confirm.selected_label(),
            "Continue",
            "the last answer is the end"
        );
        assert_eq!(
            confirm.handle(key(KeyCode::Enter)),
            ConfirmAction::Chosen(1)
        );
        let mut confirm = Confirm::new("Format the card?", "This erases it");
        assert_eq!(confirm.handle(key(KeyCode::Esc)), ConfirmAction::Cancelled);
    }

    #[test]
    fn a_destructive_answer_is_not_the_default() {
        let mut confirm = Confirm::new("Shut down?", "").destructive(true);
        assert!(confirm.is_destructive());
        // Enter without moving chooses the first row, which is Cancel.
        assert_eq!(
            confirm.handle(key(KeyCode::Enter)),
            ConfirmAction::Chosen(0)
        );
        assert_eq!(confirm.selected_label(), "Cancel");
    }

    #[test]
    fn confirm_tab_cycles_and_hit_click_targets_each_button() {
        let design = Design::solarized();
        let mut confirm = Confirm::new(
            "Erase /dev/sda?",
            "ALL DATA on /dev/sda (SanDisk Ultra 32G) WILL BE LOST.",
        )
        .with_detail("Formatted as exfat (label HAYATO)")
        .with_options(&["Cancel", "Erase & Format"])
        .destructive(true);

        // Tab toggles forward and wraps back to Cancel; BackTab wraps reverse.
        assert_eq!(confirm.selected(), 0);
        assert_eq!(confirm.handle(key(KeyCode::Tab)), ConfirmAction::None);
        assert_eq!(confirm.selected(), 1);
        assert_eq!(confirm.handle(key(KeyCode::Tab)), ConfirmAction::None);
        assert_eq!(confirm.selected(), 0);
        assert_eq!(confirm.handle(key(KeyCode::BackTab)), ConfirmAction::None);
        assert_eq!(confirm.selected(), 1);

        // Render and verify hit-testing locates both buttons and avoids the rest.
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        confirm.render(area, &mut buf, &design);

        let mut hit_cancel = None;
        let mut hit_erase = None;
        for y in 0..area.height {
            for x in 0..area.width {
                match confirm.hit(area, &design, x, y) {
                    Some(0) if hit_cancel.is_none() => hit_cancel = Some((x, y)),
                    Some(1) if hit_erase.is_none() => hit_erase = Some((x, y)),
                    _ => {}
                }
            }
        }
        assert!(hit_cancel.is_some(), "Cancel button should be clickable");
        assert!(hit_erase.is_some(), "Erase button should be clickable");
        assert_eq!(confirm.hit(area, &design, 0, 0), None);
        assert_eq!(confirm.click(0), ConfirmAction::Chosen(0));
        assert_eq!(confirm.click(1), ConfirmAction::Chosen(1));
        assert_eq!(confirm.click(99), ConfirmAction::None);

        // Destructive card uses the Danger colour on its border.
        let card = confirm.card_rect(area);
        let corner = buf.cell((card.x, card.y)).unwrap();
        assert_eq!(corner.fg, design.color(ColorRole::Danger));
    }

    #[test]
    fn confirm_wraps_long_messages_without_truncating() {
        let lines: Vec<&str> = WrappedLines::new(
            "ALL DATA on /dev/sda (SanDisk Ultra, 32G) WILL BE LOST permanently.",
            30,
        )
        .collect();
        assert!(lines.len() >= 2, "expected wrapping, got {lines:?}");
        for line in &lines {
            assert!(
                UnicodeWidthStr::width(*line) <= 30,
                "wrapped line exceeded width: {line:?}"
            );
        }
    }

    #[test]
    fn the_log_follows_until_scrolled_and_comes_back() {
        let mut log = LogPane::new();
        for i in 0..100 {
            log.push(format!("line {i}"));
        }
        assert!(log.following());
        assert!(log.handle(key(KeyCode::PageUp), 10));
        assert!(!log.following());
        assert!(log.handle(key(KeyCode::End), 10));
        assert!(log.following());
    }

    #[test]
    fn the_selected_row_is_filled_and_bars_the_margin() {
        let design = Design::solarized();
        let mut list = rows(3);
        list.set_cursor(1);
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf, &design, true);
        let bar = buf.cell((0, 1)).unwrap();
        assert_eq!(bar.symbol(), "\u{258d}", "the selected row has the bar");
        assert_eq!(bar.fg, design.color(ColorRole::Accent));
        assert_eq!(
            buf.cell((8, 1)).unwrap().bg,
            design.color(ColorRole::Selection),
            "and its whole width is filled"
        );
        assert_eq!(
            buf.cell((8, 0)).unwrap().bg,
            design.color(ColorRole::Bg),
            "the other rows are not"
        );
        // Without focus nothing is highlighted: two lists on one screen must
        // not both claim the keys.
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf, &design, false);
        assert_ne!(buf.cell((0, 1)).unwrap().symbol(), "\u{258d}");
    }

    #[test]
    fn truncation_keeps_the_ellipsis_inside_the_width() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let design = Design::solarized();
        let used = put_trunc(
            &mut buf,
            0,
            0,
            "a very long label indeed",
            10,
            design.style(TextStyle::Body),
        );
        assert!(used <= 10, "the truncation respected the width: {used}");
        let line: String = (0..10)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(line.ends_with('\u{2026}'), "{line:?}");
    }

    #[test]
    fn masked_input_renders_asterisks() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let design = Design::solarized();
        let input = Input::with_text("secret").with_masked(true);
        input.render(area, &mut buf, &design, "password");
        let rendered: String = (1..7)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(rendered, "******");
    }

    #[test]
    fn update_rows_preserves_selected_value_and_filter() {
        let mut list = ListState::new();
        list.set_rows(vec![
            Row::new("Alpha").value("ap:alpha"),
            Row::new("HomeNet").value("ap:home"),
            Row::new("OfficeNet").value("ap:office"),
        ]);
        list.set_filter("net");
        list.set_cursor(1); // OfficeNet
        assert_eq!(
            list.selected().and_then(|r| r.value.as_deref()),
            Some("ap:office")
        );

        // Background rescan inserts a stronger AP at the top and keeps OfficeNet
        list.update_rows(vec![
            Row::new("NewStrongNet").value("ap:strong"),
            Row::new("HomeNet").value("ap:home"),
            Row::new("OfficeNet (updated)").value("ap:office"),
        ]);
        assert_eq!(list.filter(), "net");
        assert_eq!(list.visible_len(), 3);
        assert_eq!(
            list.selected().and_then(|r| r.value.as_deref()),
            Some("ap:office"),
            "cursor followed ap:office across row refresh"
        );
    }

    #[test]
    fn scrollbar_draws_only_when_rows_exceed_capacity() {
        let design = Design::solarized();
        let list = rows(25);
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf, &design, true);
        let right_col: String = (0..8)
            .map(|y| buf.cell((39, y)).unwrap().symbol().to_string())
            .collect();
        assert!(
            right_col.contains('\u{2588}') && right_col.contains('\u{2502}'),
            "scrollbar draws thumb and track when 25 > 8: {right_col:?}"
        );
    }

    #[test]
    fn qr_from_ascii_and_draw_qr_render_half_blocks() {
        let ascii = "        \n  ####  \n  ##    \n        \n";
        let qr = QrCode::from_ascii(ascii).expect("parsed ascii qr");
        assert_eq!(qr.width, 4);
        assert_eq!(qr.height, 4);
        assert!(!qr.get(0, 0));
        assert!(qr.get(1, 1));
        assert!(qr.get(2, 1));
        assert!(qr.get(1, 2));
        assert!(!qr.get(2, 2));

        let design = Design::solarized();
        let area = Rect::new(0, 0, 10, 4);
        let mut buf = Buffer::empty(area);
        draw_qr(&mut buf, area, &design, &qr);
        let text = crate::screen(&buf);
        assert!(
            text.contains('\u{2588}') || text.contains('\u{2580}') || text.contains('\u{2584}'),
            "draw_qr rendered half-block symbols: {text:?}"
        );
    }

    #[test]
    fn render_field_focus_and_blink_toggle() {
        let design = Design::solarized();
        let area = Rect::new(0, 0, 20, 1);
        let input = Input::with_text("abc");
        let accent = design.color(ColorRole::Accent);
        let bg = design.color(ColorRole::Bg);

        // Focused + blink on -> left focus bar + Accent block cursor at end ("abc" -> x=1..4, cursor at x=4)
        let mut buf = Buffer::empty(area);
        input.render_field(
            area,
            &mut buf,
            &design,
            "type here",
            true,
            true,
            ColorRole::Bg,
            TextStyle::Body,
        );
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "\u{258d}");
        assert_eq!(buf.cell((0, 0)).unwrap().fg, accent);
        assert_eq!(buf.cell((4, 0)).unwrap().bg, accent);

        // Focused + blink off -> left focus bar stays, Accent cursor background disappears
        let mut buf_off = Buffer::empty(area);
        input.render_field(
            area,
            &mut buf_off,
            &design,
            "type here",
            true,
            false,
            ColorRole::Bg,
            TextStyle::Body,
        );
        assert_eq!(buf_off.cell((0, 0)).unwrap().symbol(), "\u{258d}");
        assert_eq!(buf_off.cell((4, 0)).unwrap().bg, bg);
        assert!(
            !buf_off
                .cell((4, 0))
                .unwrap()
                .modifier
                .contains(ratatui::style::Modifier::UNDERLINED),
            "blink-off cursor must not draw an underline fallback"
        );

        // Unfocused -> neither focus bar nor cursor block appears
        let mut buf_unfocused = Buffer::empty(area);
        input.render_field(
            area,
            &mut buf_unfocused,
            &design,
            "type here",
            false,
            true,
            ColorRole::Bg,
            TextStyle::Body,
        );
        assert_eq!(buf_unfocused.cell((0, 0)).unwrap().symbol(), " ");
        assert_eq!(buf_unfocused.cell((4, 0)).unwrap().bg, bg);

        // Reduced motion keeps cursor visible even when blink_on is false
        let reduced = design.with_reduced_motion(true);
        let mut buf_rm = Buffer::empty(area);
        input.render_field(
            area,
            &mut buf_rm,
            &reduced,
            "type here",
            true,
            false,
            ColorRole::Bg,
            TextStyle::Body,
        );
        assert_eq!(buf_rm.cell((4, 0)).unwrap().bg, accent);
    }

    #[test]
    fn selected_list_row_has_no_background_holes() {
        let design = Design::solarized();
        let selection = design.color(ColorRole::Selection);
        let mut list = ListState::new();
        list.set_rows(vec![
            Row::new("Wi-Fi")
                .glyph(Glyph::Wifi)
                .detail("wlan0")
                .value("connected"),
            Row::new("Bluetooth")
                .glyph(Glyph::Bluetooth)
                .detail("bluez")
                .value("off"),
        ]);
        list.set_cursor(1);
        let area = Rect::new(0, 0, 50, 2);
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf, &design, true);
        for x in 0..50 {
            assert_eq!(
                buf.cell((x, 1)).unwrap().bg,
                selection,
                "cell at col {x} on selected row dropped selection background"
            );
        }
    }

    #[test]
    fn action_strip_pill_glyph_has_two_space_gap() {
        let design = Design::solarized();
        let strip = ActionStrip::new(vec![
            ActionItem::new("scan", "F5", "Rescan").glyph(Glyph::Rotate),
        ]);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        strip.render(area, &mut buf, &design);
        let line: String = (0..20)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        let expected = format!(" F5 {}  Rescan ", Glyph::Rotate.cell());
        assert!(
            line.starts_with(&expected),
            "expected pill {expected:?}, got {line:?}"
        );
        assert_eq!(
            strip.hit(area, expected.chars().count() as u16 - 1, 0),
            Some("scan")
        );
    }
}
