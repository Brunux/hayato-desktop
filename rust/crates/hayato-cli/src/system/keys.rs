// Keys viewer: parsed from sway configuration with comments as descriptions (`hayato-keys`).
//
// The old keys viewer was an awk script that parsed `/etc/sway/config`,
// `/etc/sway/config.d/*.conf`, and `~/.config/sway/config`. This version does the same
// parsing in pure Rust with the same rules: comments above bindings become descriptions,
// `set $var` expansion, `$mod` → `Alt`, mode blocks, and section icons from the theme.

#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]

use std::cell::Cell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, Input, InputAction, KeyHint, ListAction, ListState,
    Outcome, Row, Screen, TextStyle, draw_card,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::system::{have, missing, print, say};

/// A key binding from the sway config.
#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub keys: String,
    pub command: String,
    pub description: String,
    pub mode: Option<String>,
    pub icon: String,
    pub category: String,
}

/// The keys viewer state.
pub struct KeysScreen {
    input: Input,
    bindings: Vec<KeyBinding>,
    filtered: Vec<KeyBinding>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl KeysScreen {
    pub fn new() -> Self {
        let bindings = Self::load_bindings();
        let mut list = ListState::new().with_empty("no bindings found").ranked();
        list.set_rows(bindings.iter().map(Self::binding_row).collect());

        Self {
            input: Input::new(),
            bindings: bindings.clone(),
            filtered: bindings,
            list,
            header: Header::new("Hayato").subtitle("Keys"),
            keybar: vec![
                KeyHint::new("Enter", "copy command"),
                KeyHint::new("type", "filter"),
                KeyHint::new("\u{2191}\u{2193}", "move"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
        }
    }

    fn load_bindings() -> Vec<KeyBinding> {
        let mut bindings = Vec::new();
        let mut vars = HashMap::new();

        // Config files to parse
        let mut files = vec!["/etc/sway/config".to_string()];

        // Add config.d files
        if let Ok(entries) = fs::read_dir("/etc/sway/config.d") {
            let mut conf_files: Vec<_> = entries
                .flatten()
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("conf"))
                .map(|e| e.path().to_string_lossy().to_string())
                .collect();
            conf_files.sort();
            files.extend(conf_files);
        }

        // Add user config
        if let Ok(home) = env::var("HOME") {
            files.push(format!("{home}/.config/sway/config"));
        }

        // Pass 1: collect variables
        for file in &files {
            if let Ok(content) = fs::read_to_string(file) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with("set $") {
                        let parts: Vec<&str> = line.splitn(3, ' ').collect();
                        if parts.len() == 3 {
                            let var = parts[1].to_string();
                            let val = parts[2].trim_matches(|c| c == '"' || c == '\'').to_string();
                            vars.insert(var, val);
                        }
                    }
                }
            }
        }

        // Pass 2: parse bindings
        let tools: HashMap<String, ()> = [
            "bar-audio",
            "bar-bt",
            "bar-busy",
            "bar-key",
            "bar-menu",
            "bar-net",
            "bar-power",
            "bar-updates",
            "busy",
            "dictate",
            "dictate-model",
            "hayato-screensaver",
            "hayato-about",
            "hayato-agents",
            "hayato-calc",
            "hayato-capture",
            "hayato-clean",
            "hayato-clip",
            "hayato-color",
            "hayato-dev",
            "hayato-dns",
            "hayato-emoji",
            "hayato-files",
            "hayato-keys",
            "hayato-kill",
            "hayato-learn",
            "hayato-media",
            "hayato-notify-history",
            "hayato-pkg",
            "hayato-places",
            "hayato-power",
            "hayato-qr",
            "hayato-settings",
            "hayato-remind",
            "hayato-tz",
            "sway-hibernate",
            "sway-lock",
            "sway-suspend",
        ]
        .into_iter()
        .map(|s| (s.to_string(), ()))
        .collect();

        for file in &files {
            if let Ok(content) = fs::read_to_string(file) {
                let mut comment = String::new();
                let mut mode = None::<String>;
                let mut category = String::new();

                for line in content.lines() {
                    let trimmed = line.trim();

                    // Comments become descriptions
                    if trimmed.starts_with('#') {
                        comment = trimmed[1..].trim().to_string();
                        continue;
                    }

                    // Blank line clears comment
                    if trimmed.is_empty() {
                        comment.clear();
                        continue;
                    }

                    // Mode blocks
                    if trimmed.starts_with("mode \"") {
                        if let Some(first_quote) = trimmed.find('\"') {
                            if let Some(second_quote) = trimmed[first_quote + 1..].find('\"') {
                                mode = Some(
                                    trimmed[first_quote + 1..first_quote + 1 + second_quote]
                                        .to_string(),
                                );
                            }
                        }
                        continue;
                    }
                    if trimmed == "}" {
                        mode = None;
                        continue;
                    }

                    // Bindings
                    let binding_type = if trimmed.starts_with("bindsym ") {
                        "bindsym"
                    } else if trimmed.starts_with("bindcode ") {
                        "bindcode"
                    } else if trimmed.starts_with("bindswitch ") {
                        "bindswitch"
                    } else if trimmed.starts_with("bindgesture ") {
                        "bindgesture"
                    } else {
                        continue;
                    };

                    let rest = &trimmed[binding_type.len()..].trim_start();
                    let is_release = rest.starts_with("--release ");
                    let rest = rest.strip_prefix("--release ").unwrap_or(rest);
                    let rest = rest.strip_prefix("--locked ").unwrap_or(rest);
                    let rest = rest.strip_prefix("--no-repeat ").unwrap_or(rest);
                    let rest = rest.strip_prefix("--no-warns ").unwrap_or(rest);
                    let rest = rest.strip_prefix("--to-code ").unwrap_or(rest);

                    let (keys, cmd) = match rest.split_once(' ') {
                        Some((k, c)) => (k, c.trim()),
                        None => continue,
                    };

                    if cmd.is_empty() || (is_release && cmd.contains("dictate --release")) {
                        continue;
                    }

                    // Expand variables
                    let keys = Self::expand_vars(keys, &vars);
                    let cmd = Self::expand_vars(cmd, &vars);

                    // Pretty print keys
                    let keys = Self::pretty_keys(&keys);

                    // Determine category from file
                    let cat = Self::category(file);
                    if cat != category {
                        category = cat;
                    }

                    // Determine icon
                    let icon = Self::binding_icon(&cmd, &category, &tools);

                    // Description: comment or command
                    let desc = if comment.is_empty() {
                        Self::expand_vars(&cmd, &vars)
                    } else {
                        Self::expand_vars(&comment, &vars)
                    };

                    bindings.push(KeyBinding {
                        keys,
                        command: cmd.to_string(),
                        description: desc,
                        mode: mode.clone(),
                        icon,
                        category: category.clone(),
                    });
                }
            }
        }

        bindings
    }

    fn expand_vars(s: &str, vars: &HashMap<String, String>) -> String {
        let mut result = s.to_string();
        for (k, v) in vars {
            // k is like "$term", we want to replace $term with the value
            result = result.replace(k, v);
        }
        result
    }

    fn pretty_keys(keys: &str) -> String {
        let mut k = keys.to_string();
        k = k.replace("$mod", "Alt");
        k = k.replace("Mod1", "Alt");
        k = k.replace("Mod4", "Super");
        k = k.replace("Super_L", "Super");
        // Remove flags
        k = k.replace("--release ", "");
        k = k.replace("--locked ", "");
        k = k.replace("--no-repeat ", "");
        k = k.replace("--no-warns ", "");
        k = k.replace("--to-code ", "");
        k
    }

    fn category(file: &str) -> String {
        if file.contains("config.d/19-launcher") {
            "Launcher".to_string()
        } else if file.contains("config.d/24-keys") {
            "Menu, panels and session".to_string()
        } else if file.contains("config.d/21-lock-screen") {
            "Lock and system".to_string()
        } else if file.contains("config.d/20-brightness") {
            "Brightness".to_string()
        } else if file.contains("config.d/21-volume") {
            "Volume".to_string()
        } else if file.contains("config.d/22-workspace-gestures")
            || file.contains("config.d/90-touchpad")
        {
            "Touchpad and gestures".to_string()
        } else if file.contains("config.d/90-screenshot") {
            "Screenshots".to_string()
        } else if file.contains("config.d/90-desktop") {
            "Theme".to_string()
        } else if file.contains("config.d/26-swayfx") {
            "Effects".to_string()
        } else if file.contains("config.d/20-waybar")
            || file.contains("config.d/01-status-bar")
            || file.contains("config.d/25-bar-panels")
        {
            "Bar".to_string()
        } else if file.contains("config.d/00-logo-key") {
            "Keyboard".to_string()
        } else if file.contains("config.d/") {
            "Session".to_string()
        } else {
            "Windows and workspaces".to_string()
        }
    }

    fn binding_icon(cmd: &str, category: &str, tools: &HashMap<String, ()>) -> String {
        // Check if command starts with a known tool
        let words: Vec<&str> = cmd.split_whitespace().collect();
        for word in words {
            let tool = word.trim_start_matches(|c: char| {
                !c.is_ascii_alphanumeric() && c != '.' && c != '_' && c != '+' && c != '-'
            });
            let tool = tool.trim_end_matches(|c: char| {
                !c.is_ascii_alphanumeric() && c != '.' && c != '_' && c != '+' && c != '-'
            });
            let tool = tool.split('/').next_back().unwrap_or(tool);
            if tools.contains_key(tool) {
                return tool.to_string();
            }
        }

        // Check for sway actions
        if cmd.contains("kill") && !cmd.contains("sway-lock") {
            return "window-close".to_string();
        }
        if cmd.contains("reload") {
            return "view-refresh".to_string();
        }
        if cmd.contains("exit") {
            return "application-exit".to_string();
        }
        if cmd.contains("focus left") || cmd.contains("move left") {
            return "go-previous".to_string();
        }
        if cmd.contains("focus right") || cmd.contains("move right") {
            return "go-next".to_string();
        }
        if cmd.contains("focus up") || cmd.contains("move up") {
            return "go-up".to_string();
        }
        if cmd.contains("focus down") || cmd.contains("move down") {
            return "go-down".to_string();
        }

        // Category icon
        match category {
            "Launcher" => "hayato".to_string(),
            "Menu, panels and session" => "preferences-system".to_string(),
            "Lock and system" => "system-lock-screen".to_string(),
            "Brightness" => "display-brightness".to_string(),
            "Volume" => "audio-speakers".to_string(),
            "Lock and sleep" => "system-suspend".to_string(),
            "Touchpad and gestures" => "input-touchpad".to_string(),
            "Screenshots" => "applets-screenshooter".to_string(),
            "Theme" => "preferences-desktop-theme".to_string(),
            "Effects" => "preferences-desktop-effects".to_string(),
            "Bar" => "utilities-system-monitor".to_string(),
            "Keyboard" => "preferences-desktop-keyboard".to_string(),
            "Session" => "utilities-terminal".to_string(),
            _ => "preferences-system-windows".to_string(),
        }
    }

    fn binding_row(b: &KeyBinding) -> Row {
        let desc = if b.description.trim().is_empty() {
            b.command.trim()
        } else {
            b.description.trim()
        };
        let mut row = Row::new(&format!("{}  {}", b.keys, desc)).glyph(Glyph::Key);
        if let Some(mode) = &b.mode {
            row = row.detail(&format!("mode: {}", mode));
        }
        row
    }

    fn filter_bindings(&mut self) {
        let query = self.input.text().trim().to_lowercase();
        if query.is_empty() {
            self.filtered = self.bindings.clone();
        } else {
            self.filtered = self
                .bindings
                .iter()
                .filter(|b| {
                    b.keys.to_lowercase().contains(&query)
                        || b.description.to_lowercase().contains(&query)
                        || b.command.to_lowercase().contains(&query)
                        || b.category.to_lowercase().contains(&query)
                        || b.mode
                            .as_ref()
                            .is_some_and(|m| m.to_lowercase().contains(&query))
                })
                .cloned()
                .collect();
        }
        self.list
            .set_rows(self.filtered.iter().map(Self::binding_row).collect());
    }

    fn copy_command(&self, binding: &KeyBinding) {
        let _ = std::process::Command::new("wl-copy")
            .arg(&binding.command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        print(&format!("Copied: {}", binding.command));
        say(
            "Keys",
            format!("Copied command · {}", binding.command),
            hayato_ui::Urgency::Normal,
            Glyph::Key,
        );
    }
}

impl Screen for KeysScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Bindings"), true);
        let (filter, rows) = Self::list_layout(left);
        hayato_ui::widgets::draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.input.text(),
            "type to filter bindings...",
            self.filtered.len(),
            self.bindings.len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
        if let Some(right_area) = right {
            self.draw_inspector(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let rows_height = self.rows_height.get();
        if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            let _ = self.input.handle(key);
            self.filter_bindings();
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Enter => {
                if let Some(selected) = self.list.selected_index() {
                    if let Some(binding) = self.filtered.get(selected) {
                        self.copy_command(binding);
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                }
            }
            KeyCode::Esc => {
                if !self.input.text().is_empty() {
                    self.input.clear();
                    self.filter_bindings();
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => {
                if let Some(binding) = self.filtered.get(index) {
                    self.copy_command(binding);
                    return Outcome::Done(hayato_ui::app::exit::OK);
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
                if let Some(binding) = self.filtered.get(index) {
                    self.copy_command(binding);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let (_, rows) = Self::list_layout(left);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl KeysScreen {
    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 56, 52)
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Shortcut Inspector"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 8 {
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let ok = Style::default().fg(design.color(ColorRole::Ok));

        let Some(idx) = self.list.selected_index() else {
            return;
        };
        let Some(b) = self.filtered.get(idx) else {
            return;
        };
        let mut y = inner.y;
        let cat = if b.category.is_empty() {
            "SWAY BINDING"
        } else {
            b.category.as_str()
        };
        put_trunc(buf, inner.x, y, cat, inner.width, accent);
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!("[ {} ]", b.keys),
            inner.width,
            design.style(TextStyle::Title).add_modifier(Modifier::BOLD),
        );
        y += 2;
        if let Some(mode) = &b.mode {
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Mode      {mode}"),
                inner.width,
                dim,
            );
            y += 1;
        }
        put_trunc(buf, inner.x, y, "Description", inner.width, accent);
        y += 1;
        put_trunc(buf, inner.x, y, &b.description, inner.width, body);
        y += 2;
        put_trunc(buf, inner.x, y, "Sway Command", inner.width, accent);
        y += 1;
        let w = usize::from(inner.width.max(1));
        let chars: Vec<char> = b.command.chars().collect();
        for chunk in chars.chunks(w) {
            if y >= inner.bottom().saturating_sub(2) {
                break;
            }
            let s: String = chunk.iter().collect();
            put_trunc(buf, inner.x, y, &s, inner.width, ok);
            y += 1;
        }
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "Enter copies command to clipboard",
            inner.width,
            dim,
        );
    }

    fn list_layout(area: Rect) -> (Rect, Rect) {
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
}

/// All parsed sway bindings for `hayato menu keys --rows`.
pub fn rows() -> Vec<KeyBinding> {
    KeysScreen::load_bindings()
}

/// `hayato menu keys` (`hayato-keys`): the keys viewer.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(&["wl-copy"]) {
        say(
            "Keys",
            format!("{said} (wl-clipboard)"),
            hayato_ui::Urgency::Critical,
            Glyph::Key,
        );
        return Ok(3);
    }
    let mut screen = KeysScreen::new();
    let code = crate::run_screen(terminal, design, "Keys", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_expansion() {
        let mut vars = HashMap::new();
        vars.insert("$term".to_string(), "foot".to_string());
        vars.insert("$mod".to_string(), "Alt".to_string());
        let result = KeysScreen::expand_vars("$term $mod+Return", &vars);
        assert_eq!(result, "foot Alt+Return");
    }

    #[test]
    fn pretty_keys() {
        assert_eq!(KeysScreen::pretty_keys("$mod+Return"), "Alt+Return");
        assert_eq!(KeysScreen::pretty_keys("Mod1+Tab"), "Alt+Tab");
        assert_eq!(KeysScreen::pretty_keys("Mod4+d"), "Super+d");
        assert_eq!(KeysScreen::pretty_keys("--release $mod+d"), "Alt+d");
    }

    #[test]
    fn category_detection() {
        assert_eq!(
            KeysScreen::category("/etc/sway/config.d/19-launcher.conf"),
            "Launcher"
        );
        assert_eq!(
            KeysScreen::category("/etc/sway/config.d/24-keys.conf"),
            "Menu, panels and session"
        );
        assert_eq!(
            KeysScreen::category("/etc/sway/config.d/21-lock-screen.conf"),
            "Lock and system"
        );
    }

    #[test]
    fn icon_for_tool() {
        let mut tools = HashMap::new();
        tools.insert("hayato-files".to_string(), ());
        let icon = KeysScreen::binding_icon("hayato-files --search", "Launcher", &tools);
        assert_eq!(icon, "hayato-files");
    }

    #[test]
    fn uncommented_binding_falls_back_to_command() {
        let b = KeyBinding {
            keys: "Alt+Return".to_string(),
            command: "exec foot".to_string(),
            description: String::new(),
            mode: None,
            icon: "utilities-terminal".to_string(),
            category: "Session".to_string(),
        };
        let row = KeysScreen::binding_row(&b);
        assert_eq!(row.label, "Alt+Return  exec foot");
    }
}
