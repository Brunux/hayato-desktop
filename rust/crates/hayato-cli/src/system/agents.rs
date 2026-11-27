//! AI Agent Selector (`hayato-agents`, `hayato menu agents`).
//!
//! Launch a coding agent CLI, manage accounts per agent, and pick a default.
//! Provides `--default`, `--env` and `--new` subcommands.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    BodyCtx, Card, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, Task, TaskState, TaskView, TextStyle, Urgency, draw_card, draw_filter_bar,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::pick::{Picker, Question, row};
use super::say;

#[derive(Debug)]
pub struct AgentDef {
    pub name: &'static str,
    pub binary: &'static str,
    pub npm: &'static str,
    pub envvar: &'static str,
}

pub const AGENTS: &[AgentDef] = &[
    AgentDef {
        name: "Claude Code",
        binary: "claude",
        npm: "@anthropic-ai/claude-code",
        envvar: "CLAUDE_CONFIG_DIR",
    },
    AgentDef {
        name: "Codex",
        binary: "codex",
        npm: "@openai/codex",
        envvar: "CODEX_HOME",
    },
    AgentDef {
        name: "OpenCode",
        binary: "opencode",
        npm: "@opencode/cli",
        envvar: "",
    },
    AgentDef {
        name: "Gemini CLI",
        binary: "gemini",
        npm: "@google/gemini-cli",
        envvar: "",
    },
    AgentDef {
        name: "GitHub Copilot",
        binary: "copilot",
        npm: "@github/copilot",
        envvar: "",
    },
];

fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home).join(".config")
        })
}

fn raw_data_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home).join(".local/share")
        })
}

fn state_dir() -> PathBuf {
    config_dir().join("hayato/agents")
}

fn data_dir() -> PathBuf {
    raw_data_dir().join("hayato/agents")
}

pub fn default_agent_file() -> PathBuf {
    state_dir().join("default")
}

/// Check whether `binary` exists on PATH or in `~/.opencode/bin` or `~/.local/bin`.
pub fn is_installed(binary: &str) -> bool {
    if let Ok(home) = std::env::var("HOME") {
        let opencode_bin = Path::new(&home).join(".opencode/bin").join(binary);
        if opencode_bin.exists() {
            return true;
        }
        let local_bin = Path::new(&home).join(".local/bin").join(binary);
        if local_bin.exists() {
            return true;
        }
    }
    super::have(binary)
}

/// Get the active default agent binary name.
pub fn get_default_agent() -> Option<&'static AgentDef> {
    if let Ok(content) = fs::read_to_string(default_agent_file()) {
        let want = content.lines().next().unwrap_or("").trim();
        if let Some(agent) = AGENTS
            .iter()
            .find(|a| a.binary == want && is_installed(a.binary))
        {
            return Some(agent);
        }
    }
    // Fallback: opencode if installed
    if let Some(agent) = AGENTS
        .iter()
        .find(|a| a.binary == "opencode" && is_installed(a.binary))
    {
        return Some(agent);
    }
    // Fallback: first installed
    AGENTS.iter().find(|a| is_installed(a.binary))
}

/// Set default agent.
pub fn set_default_agent(binary: &str) -> std::io::Result<()> {
    let dir = state_dir();
    fs::create_dir_all(&dir)?;
    fs::write(default_agent_file(), format!("{binary}\n"))
}

/// The account an agent starts with, when it keeps accounts under
/// `~/.local/share/hayato/agents/<binary>/`.
pub fn default_account(agent: &AgentDef) -> Option<String> {
    if agent.envvar.is_empty() {
        return None;
    }
    fs::read_to_string(data_dir().join(agent.binary).join("default"))
        .ok()
        .map(|content| content.lines().next().unwrap_or("").trim().to_string())
        .filter(|line| !line.is_empty())
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The window title and the `bash -lc` line that starts an agent session.
/// One builder for the Agents menu, `hayato-agents --default` and the Quake
/// console, so an account picked in one place is the account every surface
/// starts.
pub fn session(agent: &AgentDef, account: Option<&str>) -> (String, String) {
    let title = session_title(agent, account);
    (title, agent_line(agent, account, &[]))
}

fn session_title(agent: &AgentDef, account: Option<&str>) -> String {
    match account {
        Some(acc) => format!("{} · {}", agent.name, acc),
        None => agent.name.to_string(),
    }
}

fn agent_line(agent: &AgentDef, account: Option<&str>, extra: &[String]) -> String {
    let mut line = String::new();
    if !agent.envvar.is_empty() {
        if let Some(acc) = account {
            let p = data_dir().join(agent.binary).join(acc);
            line.push_str(&format!(
                "export {}={}; ",
                agent.envvar,
                shell_quote(&p.display().to_string())
            ));
        }
    }
    line.push_str(&format!("exec {}", agent.binary));
    for argument in extra {
        line.push(' ');
        line.push_str(&shell_quote(argument));
    }
    line
}

/// OpenCode's TUI reopens the most recently viewed session, which would put the
/// console on whatever session another window already has - two views of one
/// conversation. Its CLI has no "start empty" flag for the TUI, so the console
/// creates the session on the server first (the API's documented create) and
/// passes that id. Every other agent starts a new session on its own.
fn create_opencode_session(dir: &Path) -> Option<String> {
    let directory = serde_json::to_string(&dir.display().to_string()).ok()?;
    let body = format!(r#"{{"location":{{"directory":{directory}}}}}"#);
    let text = super::out(&["opencode", "api", "post", "/api/session", "--data", &body])?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value["data"]["id"].as_str().map(str::to_string)
}

/// The console's session: the default agent in `dir`, with a session of its own
/// so it never attaches to the window the person already has open. `None` when
/// OpenCode is the agent and its server cannot create one - the caller shows a
/// shell rather than hijacking another window's conversation.
pub fn console_session(
    agent: &AgentDef,
    account: Option<&str>,
    dir: &Path,
) -> Option<(String, String)> {
    let extra = if agent.binary == "opencode" {
        let id = create_opencode_session(dir)?;
        vec!["--session".to_string(), id]
    } else {
        Vec::new()
    };
    Some((
        session_title(agent, account),
        agent_line(agent, account, &extra),
    ))
}

/// Launch agent CLI in a terminal.
pub fn launch(agent: &AgentDef, account: Option<&str>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let workdir = Path::new(&home).join("Work");
    let cwd = if workdir.is_dir() {
        workdir
    } else {
        PathBuf::from(home)
    };

    let (title, line) = session(agent, account);

    let script = format!(
        "cd {} 2>/dev/null || cd ~; {line}",
        shell_quote(&cwd.display().to_string())
    );
    let escaped_title = title.replace('\'', "'\\''");
    let escaped_script = script.replace('\'', "'\\''");
    crate::system::spawn_detached(&format!(
        "exec foot -T '{escaped_title}' -e bash -lc '{escaped_script}'"
    ));
}

/// Execute CLI query modes.
pub fn cli_default() -> i32 {
    let Some(agent) = get_default_agent() else {
        say(
            "Agents",
            "No agent CLI is installed",
            Urgency::Critical,
            Glyph::Code,
        );
        return 1;
    };
    let mut acc = None;
    if !agent.envvar.is_empty() {
        let def_acc_file = data_dir().join(agent.binary).join("default");
        if let Ok(content) = fs::read_to_string(def_acc_file) {
            let line = content.trim().to_string();
            if !line.is_empty() {
                acc = Some(line);
            }
        }
    }
    launch(agent, acc.as_deref());
    0
}

pub fn cli_env(agent_name: &str, account: &str) -> i32 {
    let agent = AGENTS
        .iter()
        .find(|a| a.name == agent_name || a.binary == agent_name);
    let Some(agent) = agent else {
        eprintln!("unknown agent: {agent_name}");
        return 2;
    };
    if agent.envvar.is_empty() {
        eprintln!("no account support for {}", agent.name);
        return 2;
    }
    let p = data_dir().join(agent.binary).join(account);
    println!("export {}={:?}", agent.envvar, p.display().to_string());
    0
}

pub fn cli_new(agent_name: &str) -> i32 {
    let agent = AGENTS
        .iter()
        .find(|a| a.name == agent_name || a.binary == agent_name);
    let Some(agent) = agent else {
        eprintln!("unknown agent: {agent_name}");
        return 2;
    };
    if agent.envvar.is_empty() {
        eprintln!("no account support for {}", agent.name);
        return 2;
    }
    // Interactive name prompt can be handled or created
    0
}

/// The question of `hayato menu agents`, built from data so a snapshot can
/// draw it without an agent on `PATH` or an account file.
pub fn questions(
    installed: &[&str],
    accounts: &[(&str, String)],
    default: Option<&str>,
    note: &str,
) -> Vec<Question> {
    let mut rows = Vec::new();
    let mut keys = Vec::new();

    for agent in AGENTS {
        let is_installed = installed.contains(&agent.binary);
        let is_default = default == Some(agent.binary);

        if is_installed {
            let mut r = row(agent.name, "launch session", Glyph::Code);
            if is_default {
                r = r.value("\u{25cf} default");
            }
            rows.push(r);
            keys.push(format!("launch:{}", agent.binary));

            if !agent.envvar.is_empty() {
                let detail = match accounts.iter().find(|(bin, _)| *bin == agent.binary) {
                    Some((_, account)) => format!("active account: {account}"),
                    None => "switch or create accounts".to_string(),
                };
                rows.push(row(
                    format!("{} accounts\u{2026}", agent.name),
                    detail,
                    Glyph::People,
                ));
                keys.push(format!("accounts:{}", agent.binary));
            }
        } else {
            rows.push(row(
                agent.name,
                format!("install {}", agent.npm),
                Glyph::Download,
            ));
            keys.push(format!("install:{}", agent.binary));
        }
    }

    rows.push(row(
        "Set the default agent\u{2026}",
        "choose active default agent",
        Glyph::Gear,
    ));
    keys.push("default".to_string());

    rows.push(row(
        "Agent console",
        "drop down from the top \u{00b7} Alt+`",
        Glyph::Terminal,
    ));
    keys.push("console".to_string());

    rows.push(row("Open a terminal in ~/Work", "~/Work", Glyph::Terminal));
    keys.push("shell".to_string());

    vec![Question::choose("Agents", note, rows, keys)]
}

#[derive(Clone, Debug)]
struct AgentSnapshot {
    def: &'static AgentDef,
    installed: bool,
    is_default: bool,
    active_account: Option<String>,
    accounts: Vec<String>,
}

/// Interactive AI Agents & Accounts Studio.
pub struct AgentsScreen {
    snapshots: Vec<AgentSnapshot>,
    row_keys: Vec<String>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    banner: Option<String>,
    pub chosen: Option<String>,
}

impl Default for AgentsScreen {
    fn default() -> Self {
        Self::new(None)
    }
}

impl AgentsScreen {
    pub fn new(banner: Option<String>) -> Self {
        let default = get_default_agent().map(|d| d.binary);
        let snapshots: Vec<AgentSnapshot> = AGENTS
            .iter()
            .map(|def| {
                let installed = is_installed(def.binary);
                let active_account = default_account(def);
                let mut accounts = Vec::new();
                if !def.envvar.is_empty()
                    && let Ok(entries) = fs::read_dir(data_dir().join(def.binary))
                {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                            accounts.push(entry.file_name().to_string_lossy().to_string());
                        }
                    }
                    accounts.sort();
                }
                AgentSnapshot {
                    def,
                    installed,
                    is_default: default == Some(def.binary),
                    active_account,
                    accounts,
                }
            })
            .collect();
        Self::from_snapshots(snapshots, banner)
    }

    fn from_snapshots(snapshots: Vec<AgentSnapshot>, banner: Option<String>) -> Self {
        let mut rows = Vec::new();
        let mut row_keys = Vec::new();
        let mut installed_count = 0usize;

        for snap in &snapshots {
            if snap.installed {
                installed_count += 1;
                let detail = match &snap.active_account {
                    Some(acc) => format!("{} · account: {acc}", snap.def.binary),
                    None => format!("{} · launch in ~/Work", snap.def.binary),
                };
                let mut r = Row::new(snap.def.name).glyph(Glyph::Code).detail(detail);
                if snap.is_default {
                    r = r.value("● default");
                }
                rows.push(r);
                row_keys.push(format!("launch:{}", snap.def.binary));
            } else {
                rows.push(
                    Row::new(snap.def.name)
                        .glyph(Glyph::Download)
                        .detail(format!("install {}", snap.def.npm))
                        .value("not installed"),
                );
                row_keys.push(format!("install:{}", snap.def.binary));
            }
        }

        rows.push(
            Row::new("Agent console")
                .glyph(Glyph::Terminal)
                .detail("drop down from the top · Alt+`")
                .value("console"),
        );
        row_keys.push("console".to_string());

        rows.push(
            Row::new("Open a terminal in ~/Work")
                .glyph(Glyph::Terminal)
                .detail("~/Work")
                .value("shell"),
        );
        row_keys.push("shell".to_string());

        let mut list = ListState::new().with_empty("no matching agents");
        list.set_rows(rows);

        Self {
            snapshots,
            row_keys,
            list,
            header: Header::new("Hayato")
                .subtitle("Agents Studio")
                .status(format!("{installed_count}/{} installed", AGENTS.len())),
            keybar: vec![
                KeyHint::new("Enter", "launch / install"),
                KeyHint::new("Ctrl+D", "set default"),
                KeyHint::new("Ctrl+A", "accounts"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(18),
            banner,
            chosen: None,
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 52, 46)
    }

    fn left_layout(area: Rect) -> (Rect, Rect) {
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        let filter = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(2).min(inner.bottom()),
            inner.width,
            inner.height.saturating_sub(2),
        );
        (filter, rows)
    }

    fn selected_agent(&self) -> Option<&AgentSnapshot> {
        let idx = self.list.selected_index()?;
        self.snapshots.get(idx)
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Agent Deck"), false);
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
        let ok = Style::default()
            .fg(design.color(ColorRole::Ok))
            .add_modifier(Modifier::BOLD);
        let title_style = design.style(TextStyle::Title).add_modifier(Modifier::BOLD);

        if let Some(snap) = self.selected_agent() {
            let mut y = inner.y;
            let status_pill = if snap.installed {
                if snap.is_default {
                    "● INSTALLED · DEFAULT"
                } else {
                    "● INSTALLED"
                }
            } else {
                "○ NOT INSTALLED"
            };
            put_trunc(
                buf,
                inner.x,
                y,
                status_pill,
                inner.width,
                if snap.installed { ok } else { dim },
            );
            y += 1;
            put_trunc(buf, inner.x, y, snap.def.name, inner.width, title_style);
            y += 2;

            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Binary    {}", snap.def.binary),
                inner.width,
                body,
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Package   {}", snap.def.npm),
                inner.width,
                dim,
            );
            y += 2;

            if !snap.def.envvar.is_empty() {
                put_trunc(buf, inner.x, y, "Account Profiles", inner.width, accent);
                y += 1;
                put_trunc(
                    buf,
                    inner.x,
                    y,
                    &format!("Env var   {}", snap.def.envvar),
                    inner.width,
                    dim,
                );
                y += 1;
                if snap.accounts.is_empty() {
                    put_trunc(
                        buf,
                        inner.x,
                        y,
                        "Default profile · press Ctrl+A to add accounts",
                        inner.width,
                        dim,
                    );
                } else {
                    for acc in &snap.accounts {
                        if y >= inner.bottom().saturating_sub(3) {
                            break;
                        }
                        let active = snap.active_account.as_deref() == Some(acc.as_str());
                        let line = if active {
                            format!("● {acc} (active)")
                        } else {
                            format!("  {acc}")
                        };
                        put_trunc(
                            buf,
                            inner.x,
                            y,
                            &line,
                            inner.width,
                            if active { ok } else { body },
                        );
                        y += 1;
                    }
                }
            } else {
                put_trunc(buf, inner.x, y, "Session", inner.width, accent);
                y += 1;
                put_trunc(
                    buf,
                    inner.x,
                    y,
                    "Launches in ~/Work using its native config.",
                    inner.width,
                    dim,
                );
            }

            let footer = if snap.installed {
                if !snap.def.envvar.is_empty() {
                    "Enter launches · Ctrl+D sets default · Ctrl+A manages accounts"
                } else {
                    "Enter launches · Ctrl+D sets default"
                }
            } else {
                "Enter installs globally into ~/.local via npm TaskView"
            };
            put_trunc(
                buf,
                inner.x,
                inner.bottom().saturating_sub(1),
                footer,
                inner.width,
                dim,
            );
        } else {
            put_trunc(
                buf,
                inner.x,
                inner.y,
                "WORKSPACE & CONSOLE",
                inner.width,
                accent,
            );
            put_trunc(
                buf,
                inner.x,
                inner.y + 2,
                "Quick-launch the Quake drop-down agent console (Alt+`)",
                inner.width,
                body,
            );
            put_trunc(
                buf,
                inner.x,
                inner.y + 3,
                "or open a fresh terminal directly inside ~/Work.",
                inner.width,
                dim,
            );
        }
    }

    fn activate_selected(&mut self) -> Outcome {
        if let Some(idx) = self.list.selected_index()
            && let Some(key) = self.row_keys.get(idx)
        {
            self.chosen = Some(key.clone());
            return Outcome::Done(hayato_ui::app::exit::OK);
        }
        Outcome::Continue
    }
}

impl Screen for AgentsScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Agents"), true);
        let (filter, rows) = Self::left_layout(left);
        let msg = self
            .banner
            .as_deref()
            .unwrap_or("Enter launches/installs · Ctrl+D default · Ctrl+A accounts");
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            self.row_keys.len(),
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
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    if let Some(snap) = self.selected_agent()
                        && snap.installed
                    {
                        self.chosen = Some(format!("set-default:{}", snap.def.binary));
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                    self.chosen = Some("default".to_string());
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    if let Some(snap) = self.selected_agent()
                        && !snap.def.envvar.is_empty()
                    {
                        self.chosen = Some(format!("accounts:{}", snap.def.binary));
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                    self.banner = Some("Selected agent does not use multi-account profiles".into());
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => return self.activate_selected(),
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => self.activate_selected(),
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let (_, rows) = Self::left_layout(left);
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            return self.activate_selected();
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some("agents.studio")
    }
}

/// Interactive surface: `hayato menu agents`.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut status_note: Option<String> = None;

    loop {
        let mut screen = AgentsScreen::new(status_note.take());
        let code = crate::run_screen(terminal, design, "Agents", &mut screen)?;
        if code != 0 {
            return Ok(code);
        }

        let Some(action) = screen.chosen.take() else {
            return Ok(1);
        };

        match action.as_str() {
            act if act.starts_with("set-default:") => {
                let bin = &act["set-default:".len()..];
                let _ = set_default_agent(bin);
                status_note = Some(format!("Default agent set to {bin}"));
            }
            "console" => {
                let _ = super::quake::toggle();
                return Ok(0);
            }
            "shell" => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
                let work = Path::new(&home).join("Work");
                let cwd = if work.is_dir() {
                    work
                } else {
                    PathBuf::from(home)
                };
                crate::system::spawn_detached(&format!(
                    "exec foot -T Work --working-directory '{}'",
                    cwd.display()
                ));
                return Ok(0);
            }
            "default" => {
                let mut inst_rows = Vec::new();
                let mut inst_keys = Vec::new();
                for agent in AGENTS {
                    if is_installed(agent.binary) {
                        inst_rows.push(row(agent.name, agent.binary, Glyph::Code));
                        inst_keys.push(agent.binary.to_string());
                    }
                }
                if inst_rows.is_empty() {
                    status_note = Some("No agent CLI is installed yet".to_string());
                    continue;
                }
                let mut pick_def = Picker::new(
                    "Default agent",
                    vec![Question::choose(
                        "Default agent",
                        "Enter selects as default · Escape goes back",
                        inst_rows,
                        inst_keys,
                    )],
                );
                let c = crate::run_screen(terminal, design, "Default agent", &mut pick_def)?;
                if c == 0 {
                    if let Some(chosen) = pick_def.answers().first() {
                        let _ = set_default_agent(chosen);
                        status_note = Some(format!("Default agent set to {chosen}"));
                    }
                }
            }
            act if act.starts_with("launch:") => {
                let bin = &act[7..];
                if let Some(agent) = AGENTS.iter().find(|a| a.binary == bin) {
                    let mut acc = None;
                    if !agent.envvar.is_empty() {
                        let def_acc_file = data_dir().join(agent.binary).join("default");
                        if let Ok(content) = fs::read_to_string(def_acc_file) {
                            let l = content.trim().to_string();
                            if !l.is_empty() {
                                acc = Some(l);
                            }
                        }
                    }
                    launch(agent, acc.as_deref());
                }
                return Ok(0);
            }
            act if act.starts_with("install:") => {
                let bin = &act[8..];
                if let Some(agent) = AGENTS.iter().find(|a| a.binary == bin) {
                    let npm = agent.npm.to_string();
                    let name = agent.name.to_string();
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
                    let prefix = format!("{home}/.local");
                    let allow = format!("--allow-scripts={npm}");
                    let cmd = vec![
                        "npm".to_string(),
                        "install".to_string(),
                        "-g".to_string(),
                        "--prefix".to_string(),
                        prefix,
                        allow,
                        npm.clone(),
                    ];
                    let id = format!("agent-install-{}", std::process::id());
                    let task = Task::start_steps(
                        &id,
                        format!("Installing {name}"),
                        vec![format!("Install {npm} into ~/.local")],
                        false,
                        vec![cmd],
                    )
                    .map_err(|e| {
                        crate::Failure::new("Agents", format!("Could not start installer: {e}"))
                    })?;
                    let mut view = TaskView::new(task);
                    let _ = crate::run_screen(
                        terminal,
                        design,
                        &format!("Install {}", agent.name),
                        &mut view,
                    )?;
                    status_note = Some(if view.task().state == TaskState::Done {
                        format!("Installed {}", agent.name)
                    } else {
                        format!("Install of {} did not complete", agent.name)
                    });
                }
            }
            act if act.starts_with("accounts:") => {
                let bin = &act[9..];
                let Some(agent) = AGENTS.iter().find(|a| a.binary == bin) else {
                    continue;
                };
                let dir = data_dir().join(agent.binary);
                let _ = fs::create_dir_all(&dir);

                let mut acc_rows = Vec::new();
                let mut acc_keys = Vec::new();

                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        if let Ok(ft) = entry.file_type() {
                            if ft.is_dir() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                acc_rows.push(row(&name, "account profile", Glyph::People));
                                acc_keys.push(format!("use:{name}"));
                            }
                        }
                    }
                }

                acc_rows.push(row("New account…", "create profile", Glyph::People));
                acc_keys.push("new".to_string());

                let mut acc_picker = Picker::new(
                    &format!("Accounts · {}", agent.name),
                    vec![Question::choose(
                        format!("Accounts · {}", agent.name),
                        "Enter selects · Escape goes back",
                        acc_rows,
                        acc_keys,
                    )],
                );
                let c = crate::run_screen(
                    terminal,
                    design,
                    &format!("Accounts · {}", agent.name),
                    &mut acc_picker,
                )?;
                if c == 0 {
                    if let Some(chosen) = acc_picker.answers().first() {
                        if chosen == "new" {
                            let mut new_picker = Picker::new(
                                "New Account",
                                vec![Question::input(
                                    "Account name",
                                    "Enter account name (e.g. work, personal)",
                                    "account_name",
                                    false,
                                )],
                            );
                            let nc = crate::run_screen(
                                terminal,
                                design,
                                "New Account",
                                &mut new_picker,
                            )?;
                            if nc == 0 {
                                if let Some(name) = new_picker.answers().first() {
                                    let safe_name: String = name
                                        .chars()
                                        .map(|c| {
                                            if c.is_alphanumeric()
                                                || c == '.'
                                                || c == '_'
                                                || c == '-'
                                            {
                                                c
                                            } else {
                                                '_'
                                            }
                                        })
                                        .collect();
                                    if !safe_name.is_empty() {
                                        let new_dir = dir.join(&safe_name);
                                        let _ = fs::create_dir_all(&new_dir);
                                        let _ = fs::write(dir.join("default"), &safe_name);
                                        status_note = Some(format!(
                                            "{}: account {safe_name} created & selected",
                                            agent.name
                                        ));
                                    }
                                }
                            }
                        } else if let Some(acc) = chosen.strip_prefix("use:") {
                            let _ = fs::write(dir.join("default"), acc);
                            status_note =
                                Some(format!("{}: active account set to {acc}", agent.name));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_definitions_contain_expected_names() {
        assert!(
            AGENTS
                .iter()
                .any(|a| a.name == "Claude Code" && a.binary == "claude")
        );
        assert!(
            AGENTS
                .iter()
                .any(|a| a.name == "OpenCode" && a.binary == "opencode")
        );
    }

    #[test]
    fn accounts_env_var_is_set_for_claude_and_codex() {
        let claude = AGENTS.iter().find(|a| a.binary == "claude").unwrap();
        assert_eq!(claude.envvar, "CLAUDE_CONFIG_DIR");

        let codex = AGENTS.iter().find(|a| a.binary == "codex").unwrap();
        assert_eq!(codex.envvar, "CODEX_HOME");

        let opencode = AGENTS.iter().find(|a| a.binary == "opencode").unwrap();
        assert_eq!(opencode.envvar, "");
    }

    #[test]
    fn an_account_is_exported_before_the_binary_starts() {
        let claude = AGENTS.iter().find(|a| a.binary == "claude").unwrap();
        let line = agent_line(claude, Some("work"), &[]);
        assert!(line.starts_with("export CLAUDE_CONFIG_DIR="));
        assert!(line.ends_with("exec claude"));
        assert!(line.contains("/hayato/agents/claude/work'"));
    }

    #[test]
    fn extra_arguments_land_after_the_binary() {
        let opencode = AGENTS.iter().find(|a| a.binary == "opencode").unwrap();
        let extra = vec!["--session".to_string(), "ses_abc".to_string()];
        assert_eq!(
            agent_line(opencode, None, &extra),
            "exec opencode '--session' 'ses_abc'"
        );
    }

    #[test]
    fn an_agent_that_starts_fresh_needs_no_server_call() {
        let claude = AGENTS.iter().find(|a| a.binary == "claude").unwrap();
        let (title, line) = console_session(claude, None, Path::new("/tmp")).unwrap();
        assert_eq!(title, "Claude Code");
        assert_eq!(line, "exec claude");
    }
}
