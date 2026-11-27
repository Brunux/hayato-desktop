//! Auxiliary swap: a removable card or stick used as more memory (`hayato-swap`).
//!
//! The candidate list is `disk::candidates` — the same safety as Format a disk —
//! and the guard is asked again after the pick because setup rewrites the whole
//! disk. The steps are: a fresh GPT, one Linux-swap partition with the
//! `hayato_swap` label, a marked block in `/etc/fstab` (replaced, not appended),
//! `systemctl daemon-reload`, and `swapon -p 10` with a read-back that joins
//! the kernel's list onto one line (zram is always first).
//!
//! The priority is 10, below zram's 100 (`scripts/rootfs-setup.sh`), so the
//! compressed RAM is used first and the card is the spill.
//!
//! It is **not stoppable**, like Format a disk: a kill between the table and
//! `mkswap` leaves a card that is neither a filesystem nor swap.

use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    BodyCtx, ColorRole, Confirm, ConfirmAction, Design, Glyph, Header, KeyHint, ListAction,
    ListState, Outcome, Row, Screen, Task, TaskView, TextStyle, draw_card, draw_gauge,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::disk::{self, Disk, Facts};
use super::fstab;
use super::pick::{Picker, Question, row};
use super::step::{self, Failed, Step};
use super::{missing, out};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &[
    "parted",
    "mkswap",
    "swapon",
    "swapoff",
    "swaplabel",
    "blkid",
    "lsblk",
];

/// The state of the auxiliary swap, readable from the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The partition node carrying a swap signature, if any.
    pub part: Option<String>,
    /// The label the partition carries now. An older release wrote another
    /// name, and a person should not have to repartition the card to keep it.
    pub label: Option<String>,
    /// The size `lsblk` reports for the partition.
    pub size: Option<String>,
    /// The priority the kernel shows for the partition, if active.
    pub priority: Option<u8>,
    /// Whether `/etc/fstab` asks for it (the block or a hand-written line).
    pub configured: bool,
    /// Whether the partition is currently active in the kernel's list.
    pub active: bool,
}

impl Status {
    /// Read the current state from the machine.
    pub fn from_machine() -> Self {
        let (part, label) = detect_swap_partition();

        let size = part
            .as_ref()
            .and_then(|p| out(&["lsblk", "-dno", "SIZE", p]));
        let (priority, active) = if let Some(ref p) = part {
            let listing = out(&["swapon", "--show=NAME,PRIO", "--noheadings"]).unwrap_or_default();
            let mut prio = None;
            let mut active = false;
            for line in listing.lines() {
                let mut fields = line.split_whitespace();
                if fields.next() == Some(p.as_str()) {
                    active = true;
                    if let Some(pr) = fields.next()
                        && let Ok(pr) = pr.parse::<u8>()
                    {
                        prio = Some(pr);
                    }
                    break;
                }
            }
            (prio, active)
        } else {
            (None, false)
        };

        let configured =
            fstab::configured(&std::fs::read_to_string("/etc/fstab").unwrap_or_default());

        Self {
            part,
            label,
            size,
            priority,
            configured,
            active,
        }
    }

    /// A one-line summary for `--status`.
    pub fn line(&self) -> String {
        let mut out = String::new();
        if let Some(ref part) = self.part {
            out.push_str(part);
            out.push(' ');
            if let Some(ref size) = self.size {
                out.push_str(size);
                out.push(' ');
            }
            if let Some(prio) = self.priority {
                out.push_str(&format!("pri={prio} "));
            }
            if self.active {
                out.push_str("active ");
            }
        }
        if self.configured {
            out.push_str("configured ");
        }
        out.trim().to_string()
    }

    /// The detail lines the status row shows.
    pub fn detail(&self) -> String {
        if let Some(ref part) = self.part {
            let mut d = format!("{part} · {}", self.size.as_deref().unwrap_or("?"));
            if self.active {
                if let Some(prio) = self.priority {
                    d.push_str(&format!(" · priority {prio}, after the compressed RAM"));
                }
            } else if self.configured {
                d.push_str(" · not active now");
            } else {
                d.push_str(" · a swap area this build can adopt");
            }
            d
        } else {
            "no auxiliary swap card is inserted · a boot without it waits 15 s at most".into()
        }
    }
}

/// The disk a partition belongs to: `lsblk -s` walks up and the last line is
/// the disk (`mmcblk1p1` -> `mmcblk1`).
fn disk_of(node: &str) -> Option<String> {
    out(&["lsblk", "-snro", "NAME", node]).and_then(|chain| {
        chain
            .lines()
            .last()
            .map(|line| line.trim().to_string())
            .filter(|disk| !disk.is_empty())
    })
}

/// Whether the disk is the one this system runs from (`/` or `/boot`).
fn is_root_disk(disk: &str) -> bool {
    ["/", "/boot"].iter().any(|mount| {
        out(&["findmnt", "-no", "SOURCE", mount])
            .and_then(|source| disk_of(source.trim()))
            .is_some_and(|root| root == disk)
    })
}

/// The auxiliary swap area, whatever it is labelled.
///
/// `blkid -L hayato_swap` is the fast path, but a card prepared by an older
/// release carries another label, and the menu used to answer "not inserted"
/// while the partition was right there. Any swap partition on a disk the
/// system does not run from is the auxiliary swap; the compressed RAM and
/// loop devices are never it.
fn detect_swap_partition() -> (Option<String>, Option<String>) {
    let labelled = out(&["blkid", "-L", fstab::LABEL]).unwrap_or_default();
    if !labelled.is_empty() {
        return (Some(labelled), Some(fstab::LABEL.to_string()));
    }
    let listing = out(&["lsblk", "-nrpo", "NAME,FSTYPE,LABEL"]).unwrap_or_default();
    for line in listing.lines() {
        let mut fields = line.split_whitespace();
        let (Some(node), Some(fstype)) = (fields.next(), fields.next()) else {
            continue;
        };
        if fstype != "swap" || node.starts_with("/dev/zram") || node.starts_with("/dev/loop") {
            continue;
        }
        let Some(disk) = disk_of(node) else { continue };
        if is_root_disk(&disk) {
            continue;
        }
        let label = fields.next().map(str::to_string);
        return (Some(node.to_string()), label);
    }
    (None, None)
}

/// The whole-card size and the smaller fixed sizes (with 64 MiB margin).
fn sizes(total_mib: u64) -> Vec<(String, String)> {
    let mut out = Vec::new();
    out.push(("100%".to_string(), "The whole card".to_string()));
    for gib in [2, 4, 8] {
        let need = gib * 1024 + 64;
        if total_mib >= need {
            out.push((
                format!("{gib}GiB"),
                format!("{} GiB · the rest of the card stays unpartitioned", gib),
            ));
        }
    }
    out
}

/// The `parted` end argument for the chosen size.
fn extent(answer: &str, total_mib: u64) -> String {
    match answer {
        "100%" => "100%".to_string(),
        "2GiB" => "2049MiB".to_string(),
        "4GiB" => "4097MiB".to_string(),
        "8GiB" => "8193MiB".to_string(),
        _ => format!("{}MiB", total_mib.min(100)),
    }
}

/// Whether another disk already carries the `hayato_swap` label.
fn label_taken(existing: &str, dev: &str, parent: &str, name: &str) -> bool {
    if existing.is_empty() {
        return false;
    }
    existing != dev && parent != name
}

/// Read `/proc/swaps`-style listing and return it joined onto one line.
/// The newline→space fix is what makes `case " $swaps "` find the card.
/// Returns a string with leading and trailing spaces so patterns like
/// ` {part} ` match even the first and last entries.
fn joined(listing: &str) -> String {
    let mut s = String::new();
    s.push(' ');
    s.push_str(&listing.lines().collect::<Vec<_>>().join(" "));
    s.push(' ');
    s
}

/// Whether the partition is in the joined kernel swap list.
fn active_in(joined_listing: &str, part: &str) -> bool {
    let needle = format!(" {part} ");
    joined_listing.contains(&needle)
}

/// The priority from the joined listing, if present.
/// Finds the part name, then returns the next token that parses as u8.
#[allow(dead_code)]
fn priority_of(joined_listing: &str, part: &str) -> Option<u8> {
    let mut found_part = false;
    for token in joined_listing.split_whitespace() {
        if found_part && let Ok(prio) = token.parse::<u8>() {
            return Some(prio);
        }
        if token == part {
            found_part = true;
        }
    }
    None
}

/// The partition-table and partition steps.
fn partition_steps(dev: &str, end: &str) -> Vec<Step> {
    vec![
        Step::root("partition-table", &["parted", "-s", dev, "mktable", "gpt"]),
        Step::root(
            "partition",
            &[
                "parted",
                "-s",
                dev,
                "mkpart",
                "primary",
                "linux-swap",
                "1MiB",
                end,
            ],
        ),
    ]
}

/// The `mkswap` step (with `-f` because the new partition may carry an old signature).
fn swap_area_step(part: &str) -> Step {
    Step::root("swap-area", &["mkswap", "-f", "-L", fstab::LABEL, part])
}

/// The activation step with the priority.
fn activation_step(part: &str) -> Step {
    Step::root("activation", &["swapon", "-p", "10", part])
}

/// The systemd reload step.
fn reload_step() -> Step {
    Step::root("systemd", &["systemctl", "daemon-reload"])
}

/// Wait for a path to exist with a custom timeout (used by tests).
#[allow(dead_code)]
fn wait_for_in(node: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if Path::new(node).exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Path::new(node).exists()
}

/// Set up the auxiliary swap on `dev` with the chosen `end` and allocation `alloc`.
/// `fstab_text` is the current `/etc/fstab` so we can write the new one atomically.
fn apply(dev: &str, end: &str, alloc: &str, fstab_text: &str) -> Result<String, Failed> {
    let part = fstab::partition(dev);

    let steps = partition_steps(dev, end);
    step::run(&steps)?;

    if !super::wait_for(&part) {
        return Err(Failed {
            step: "partition",
            reason: format!("{part} did not appear: the kernel still has the old table"),
        });
    }

    step::run(&[swap_area_step(&part)])?;

    let new_fstab = fstab::with_swap(fstab_text);
    fstab::replace(&new_fstab)?;

    step::run(&[reload_step()])?;

    step::run(&[activation_step(&part)])?;

    let listing = out(&["swapon", "--show=NAME", "--noheadings"]).unwrap_or_default();
    let joined = joined(&listing);
    if !active_in(&joined, &part) {
        // Activation did not stick: turn it off and roll back fstab.
        let _ = out(&["swapoff", &part]);
        let stripped = fstab::strip(fstab_text);
        let _ = fstab::replace(&stripped);
        return Err(Failed {
            step: "activation",
            reason: format!("swapon did not report {part} as active"),
        });
    }

    Ok(format!(
        "{dev} is {alloc} of swap (label {}) · active now and on every boot with the card in",
        fstab::LABEL
    ))
}

/// Ensure `/etc/initramfs-tools/conf.d/resume` is set to `RESUME=none` so
/// `update-initramfs` never bakes a removable SD/USB auxiliary swap UUID into
/// initramfs (which would stall boot for 30s whenever the card is unplugged).
fn ensure_resume_none() {
    if std::fs::read_to_string("/etc/initramfs-tools/conf.d/resume")
        .map(|s| s.lines().any(|l| l.trim() == "RESUME=none"))
        .unwrap_or(false)
    {
        return;
    }
    let _ = crate::system::sudo(&[
        "sh",
        "-c",
        "mkdir -p /etc/initramfs-tools/conf.d && printf 'RESUME=none\\n' >/etc/initramfs-tools/conf.d/resume",
    ]);
}

/// Turn the auxiliary swap off, given its current `status` and the current fstab.
fn turn_off(status: &Status, fstab_text: &str) -> Result<String, Failed> {
    ensure_resume_none();
    let part = match status.part {
        Some(ref p) => p.clone(),
        None => {
            // No partition, just the fstab block to remove.
            let stripped = fstab::strip(fstab_text);
            fstab::replace(&stripped)?;
            step::run(&[reload_step()])?;
            return Ok("the /etc/fstab entry is gone; the card is no longer needed at boot".into());
        }
    };

    let listing = out(&["swapon", "--show=NAME", "--noheadings"]).unwrap_or_default();
    let joined = joined(&listing);
    let active = active_in(&joined, &part);

    let steps = if active {
        vec![Step::root("swapoff", &["swapoff", part.as_str()])]
    } else {
        Vec::new()
    };

    step::run(&steps)?;

    let stripped = fstab::strip(fstab_text);
    fstab::replace(&stripped)?;

    step::run(&[reload_step()])?;

    if active {
        Ok(format!(
            "{part} is no longer used; the partition is still there (Format a disk erases it)"
        ))
    } else {
        Ok("the /etc/fstab entry is gone; the card is no longer needed at boot".into())
    }
}

/// The questions a swap setup asks: the disk, the size, and the confirm.
fn questions(found: &[Disk]) -> Vec<Question> {
    let (rows, keys) = {
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        for disk in found {
            rows.push(row(
                disk.dev(),
                format!("{} · {}", disk.model, disk.size),
                Glyph::Disk,
            ));
            keys.push(disk.dev());
        }
        (rows, keys)
    };

    vec![
        Question::choose(
            "Which disk?",
            "Enter picks how much swap · only removable disks with nothing mounted are listed",
            rows,
            keys,
        ),
        Question::choose(
            "How much swap?",
            "{0} · more swap is slower than RAM",
            Vec::new(),
            Vec::new(),
        ),
        Question::ask(
            "Use this card for swap?",
            "{0} · every file on it is lost",
            "{0} · {1} of swap, label hayato_swap",
            "Erase it",
            "go",
        ),
    ]
}

enum SwapMode {
    Overview,
    ConfirmOff(Confirm),
}

/// Memory & Auxiliary Swap Studio screen (for active/configured swap management).
pub struct SwapScreen {
    status: Status,
    mem_used_pct: f64,
    mem_summary: String,
    swaps_lines: Vec<String>,
    list: ListState,
    mode: SwapMode,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    pub turn_off_confirmed: bool,
}

impl SwapScreen {
    pub fn new(status: Status) -> Self {
        let (mem_used_pct, mem_summary) = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|raw| super::kill::parse_meminfo(&raw))
            .map(|(used, total)| {
                let pct = if total > 0 {
                    (used as f64 / total as f64).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let s = format!(
                    "{} / {} ({:.0}%)",
                    super::kill::format_mem_kb(used),
                    super::kill::format_mem_kb(total),
                    pct * 100.0
                );
                (pct, s)
            })
            .unwrap_or((0.0, "unavailable".into()));

        let swaps_lines: Vec<String> =
            out(&["swapon", "--show=NAME,SIZE,USED,PRIO", "--noheadings"])
                .unwrap_or_default()
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();

        let mut list = ListState::new();
        list.set_rows(vec![
            Row::new("Auxiliary swap status")
                .glyph(if status.active {
                    Glyph::Chip
                } else {
                    Glyph::Warning
                })
                .detail(status.detail())
                .value(if status.active {
                    "● active"
                } else {
                    "○ inactive"
                }),
            Row::new("Turn off auxiliary swap")
                .glyph(Glyph::Undo)
                .detail("swapoff & remove /etc/fstab entry · card files untouched"),
        ]);

        Self {
            header: Header::new("Hayato")
                .subtitle("Memory & Swap Studio")
                .status(status.line()),
            status,
            mem_used_pct,
            mem_summary,
            swaps_lines,
            list,
            mode: SwapMode::Overview,
            keybar: vec![KeyHint::new("Enter", "act"), KeyHint::new("Esc", "close")],
            rows_height: Cell::new(12),
            turn_off_confirmed: false,
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 48, 46)
    }

    fn draw_telemetry(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Memory & Swap Telemetry"), false);
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

        let mut y = inner.y;
        put_trunc(buf, inner.x, y, "SYSTEM RAM", inner.width, accent);
        y += 1;
        put_trunc(buf, inner.x, y, &self.mem_summary, inner.width, body);
        y += 1;
        let gauge = Rect::new(inner.x, y, inner.width.min(36), 1);
        let pct_label = format!("{:.0}%", self.mem_used_pct * 100.0);
        draw_gauge(buf, gauge, design, self.mem_used_pct, &pct_label);
        y += 2;

        put_trunc(
            buf,
            inner.x,
            y,
            "ACTIVE SWAP PRIORITY STACK",
            inner.width,
            accent,
        );
        y += 1;
        if self.swaps_lines.is_empty() {
            put_trunc(
                buf,
                inner.x,
                y,
                "(no active swap devices)",
                inner.width,
                dim,
            );
        } else {
            for line in &self.swaps_lines {
                if y >= inner.bottom().saturating_sub(3) {
                    break;
                }
                put_trunc(buf, inner.x, y, line, inner.width, body);
                y += 1;
            }
        }
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "zram (pri 100) absorbs burst first · SD/USB card (pri 10) is overflow",
            inner.width,
            dim,
        );
    }

    fn activate(&mut self) -> Outcome {
        if self.list.selected_index() == Some(1) {
            let part = self.status.part.as_deref().unwrap_or("auxiliary swap");
            let confirm = Confirm::new(
                "Turn off auxiliary swap?",
                format!("Disable swap on {part} and remove its boot entry?"),
            )
            .with_detail("Files on the card are not erased.")
            .with_options(&["Cancel", "Turn it off"]);
            self.mode = SwapMode::ConfirmOff(confirm);
        }
        Outcome::Consumed
    }
}

impl Screen for SwapScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let SwapMode::ConfirmOff(confirm) = &self.mode {
            confirm.render(ctx.area, ctx.buf, ctx.design);
            return;
        }
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Auxiliary Swap"), true);
        let inner = Rect::new(
            left.x + 2,
            left.y + 1,
            left.width.saturating_sub(4),
            left.height.saturating_sub(2),
        );
        put_trunc(
            ctx.buf,
            inner.x,
            inner.y,
            "Enter acts · Escape closes · the card is not erased",
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(2).min(inner.bottom()),
            inner.width,
            inner.height.saturating_sub(2),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
        if let Some(right_area) = right {
            self.draw_telemetry(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if let SwapMode::ConfirmOff(confirm) = &mut self.mode {
            match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    self.turn_off_confirmed = true;
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.mode = SwapMode::Overview;
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        match key.code {
            KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Enter => return self.activate(),
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => self.activate(),
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if let SwapMode::ConfirmOff(confirm) = &self.mode {
            return confirm.hit(area, design, x, y);
        }
        let (left, _) = Self::split_panes(area);
        let rows = Rect::new(
            left.x + 2,
            left.y + 3,
            left.width.saturating_sub(4),
            left.height.saturating_sub(4),
        );
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let SwapMode::ConfirmOff(confirm) = &mut self.mode {
            if let Some(idx) = row {
                match confirm.click(idx) {
                    ConfirmAction::Chosen(1) => {
                        self.turn_off_confirmed = true;
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        self.mode = SwapMode::Overview;
                    }
                    ConfirmAction::None => {}
                }
            }
            return Outcome::Consumed;
        }
        if let Some(r) = row {
            self.list.set_cursor(r);
            return self.activate();
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
        Some(match self.mode {
            SwapMode::Overview => "swap.overview",
            SwapMode::ConfirmOff(_) => "swap.confirm_off",
        })
    }
}

/// `hayato menu swap` (`hayato-swap`): Memory & Auxiliary Swap Studio.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Auxiliary swap",
            format!("{said} (parted, util-linux)"),
        ));
    }

    let fstab_text = std::fs::read_to_string("/etc/fstab").map_err(|_| {
        crate::Failure::new(
            "/etc/fstab is missing or unreadable: nothing can be configured".to_string(),
            "the menu cannot run without /etc/fstab".to_string(),
        )
    })?;

    let status = Status::from_machine();

    if status.configured || status.active {
        let mut screen = SwapScreen::new(status.clone());
        let code = crate::run_screen(terminal, design, "Auxiliary swap", &mut screen)?;
        if code != 0 || !screen.turn_off_confirmed {
            return Ok(code);
        }

        let stripped = fstab::strip(&fstab_text);
        fstab::replace(&stripped).map_err(|f| crate::Failure::new("Auxiliary swap", f))?;
        ensure_resume_none();

        let mut steps_meta: Vec<String> = Vec::new();
        let mut commands = Vec::new();
        if let Some(ref part) = status.part {
            let listing = out(&["swapon", "--show=NAME", "--noheadings"]).unwrap_or_default();
            let joined = joined(&listing);
            if active_in(&joined, part) {
                steps_meta.push(format!("Deactivate swap on {part} (swapoff)"));
                commands.push(vec![
                    "sudo".to_string(),
                    "-n".to_string(),
                    "swapoff".to_string(),
                    part.clone(),
                ]);
            }
        }
        steps_meta.push("Reload systemd mount units (systemctl daemon-reload)".to_string());
        commands.push(vec![
            "sudo".to_string(),
            "-n".to_string(),
            "systemctl".to_string(),
            "daemon-reload".to_string(),
        ]);

        let tag = format!("swap-off-{}", std::process::id());
        let task = Task::start_steps(&tag, "Turn off auxiliary swap", steps_meta, false, commands)
            .map_err(|e| crate::Failure::new("Auxiliary swap", e.to_string()))?;
        let mut view = TaskView::new(task);
        crate::run_screen(terminal, design, "Auxiliary swap", &mut view)
    } else if let Some(part) = status.part.clone() {
        let Some(choice) = ask_existing(terminal, design, &part, status.label.as_deref())? else {
            return Ok(0);
        };
        if choice == "use" {
            return adopt(terminal, design, &part, &fstab_text);
        }
        create(terminal, design, &fstab_text)
    } else {
        create(terminal, design, &fstab_text)
    }
}

/// The question an existing, unconfigured swap area asks: adopt it in place, or
/// erase the card and start over. Adoption never writes to the card except for
/// its label, so it needs no confirm of its own; the erase path goes through
/// the create flow and its confirm.
fn ask_existing(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    part: &str,
    label: Option<&str>,
) -> Result<Option<String>, crate::Failure> {
    let mut picker = Picker::new(
        "Auxiliary swap",
        vec![Question::choose(
            "A swap area is already on the card",
            format!(
                "{part} · {} · it is not active and /etc/fstab does not name it",
                label.unwrap_or("no label")
            ),
            vec![
                row(
                    "Use it now and at boot",
                    "label it hayato_swap, add the fstab line, activate it at priority 10 · no data is erased",
                    Glyph::Chip,
                ),
                row(
                    "Erase it and create a new one",
                    "repartitions the card · every file on it is lost",
                    Glyph::Warning,
                ),
            ],
            vec!["use".to_string(), "recreate".to_string()],
        )],
    );
    let code = crate::run_screen(terminal, design, "Auxiliary swap", &mut picker)?;
    if code != 0 {
        return Ok(None);
    }
    Ok(Some(
        picker
            .answers()
            .first()
            .cloned()
            .unwrap_or_else(|| "recreate".to_string()),
    ))
}

/// The steps that adopt a swap area that already exists: give it the label the
/// fstab line names, then activate it at the auxiliary priority.
fn adopt_steps(part: &str) -> Vec<Step> {
    vec![
        Step::root("swap-label", &["swaplabel", "-L", fstab::LABEL, part]),
        Step::root("activation", &["swapon", "-p", "10", part]),
    ]
}

fn adopt(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    part: &str,
    fstab_text: &str,
) -> Result<i32, crate::Failure> {
    ensure_resume_none();
    fstab::replace(&fstab::with_swap(fstab_text))
        .map_err(|f| crate::Failure::new("Auxiliary swap", f))?;

    let mut commands = Vec::new();
    for s in adopt_steps(part) {
        let mut cmd = vec!["sudo".to_string(), "-n".to_string()];
        cmd.extend(s.argv);
        commands.push(cmd);
    }

    let tag = format!("swap-adopt-{}", std::process::id());
    let step_labels = vec![
        format!("Label {part} hayato_swap (swaplabel -L)"),
        format!("Activate swap at priority 10 (swapon -p 10 {part})"),
    ];
    let task = Task::start_steps(
        &tag,
        format!("Use {part} as auxiliary swap"),
        step_labels,
        false,
        commands,
    )
    .map_err(|e| crate::Failure::new("Auxiliary swap", e.to_string()))?;
    let mut view = TaskView::new(task);
    crate::run_screen(terminal, design, "Auxiliary swap", &mut view)
}

/// The create flow: pick a removable disk, choose the size, erase and set it up.
fn create(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    fstab_text: &str,
) -> Result<i32, crate::Failure> {
    let found = loop {
        let listing = out(&["lsblk", "-dnro", "NAME,SIZE,MODEL,TRAN"]).unwrap_or_default();
        let facts = Facts::from_machine();
        let candidates = disk::candidates(&listing, &facts);
        if !candidates.is_empty() {
            break candidates;
        }
        let mut empty_picker = Picker::new(
            "Auxiliary swap",
            vec![Question::choose(
                "No removable disks available",
                disk::NOTHING,
                vec![row(
                    "Rescan removable disks",
                    "plug in an unmounted USB stick or SD card and press Enter",
                    Glyph::Sync,
                )],
                vec!["rescan".to_string()],
            )],
        );
        let code = crate::run_screen(terminal, design, "Auxiliary swap", &mut empty_picker)?;
        if code != 0 {
            return Ok(code);
        }
    };

    let mut picker = Picker::new("Auxiliary swap", questions(&found));
    let code = crate::run_screen(terminal, design, "Auxiliary swap", &mut picker)?;
    if code != 0 {
        return Ok(code);
    }

    let answers = picker.answers();
    let Some(dev) = answers.first().cloned() else {
        return Ok(1);
    };
    let name = dev.trim_start_matches("/dev/");

    let now = Facts::from_machine();
    if disk::is_system_disk(name, &now) {
        return Err(crate::Failure::new(
            "Auxiliary swap",
            format!(
                "{dev} is built into this machine or is the disk this system runs from: never repartitioned here"
            ),
        ));
    }

    let existing = out(&["blkid", "-L", "hayato_swap"]).unwrap_or_default();
    let parent = out(&["lsblk", "-no", "PKNAME", &dev]).unwrap_or_default();
    let parent = parent.trim();
    if label_taken(&existing, &dev, parent, name) {
        return Err(crate::Failure::new(
            "Auxiliary swap",
            format!("{existing} already carries the hayato_swap label: turn that swap off first"),
        ));
    }

    let bytes = out(&["lsblk", "-bdno", "SIZE", &dev]).unwrap_or_default();
    let total_mib: u64 = bytes.trim().parse().unwrap_or(0);

    let size_opts = sizes(total_mib);
    let size_rows: Vec<hayato_ui::Row> = size_opts
        .iter()
        .map(|(_key, label)| row(label.clone(), String::new(), Glyph::Disk))
        .collect();
    let size_keys: Vec<String> = size_opts.iter().map(|(k, _)| k.clone()).collect();

    let mut picker = Picker::new(
        "Auxiliary swap",
        vec![
            Question::choose(
                "How much swap?",
                "{0} \u{00b7} more swap is slower than RAM",
                size_rows,
                size_keys,
            ),
            Question::ask(
                "Use this card for swap?",
                "{0} \u{00b7} every file on it is lost",
                "{0} \u{00b7} {1} of swap, label hayato_swap",
                "Erase it",
                "go",
            ),
        ],
    );
    picker.set_answers(vec![dev.clone()]);

    let code = crate::run_screen(terminal, design, "Auxiliary swap", &mut picker)?;
    if code != 0 {
        return Ok(code);
    }

    let answers = picker.answers();
    if answers.get(2) != Some(&"go".to_string()) {
        return Ok(0);
    }

    let Some(size_choice) = answers.get(1) else {
        return Ok(1);
    };
    let end = extent(size_choice, total_mib);
    let part = fstab::partition(&dev);

    let new_fstab = fstab::with_swap(fstab_text);
    ensure_resume_none();
    fstab::replace(&new_fstab).map_err(|f| crate::Failure::new("Auxiliary swap", f))?;

    let mut commands = Vec::new();
    for s in partition_steps(&dev, &end) {
        let mut cmd = vec!["sudo".to_string(), "-n".to_string()];
        cmd.extend(s.argv);
        commands.push(cmd);
    }
    commands.push(vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "for _i in $(seq 1 50); do [ -e '{part}' ] && exit 0; sleep 0.1; done; [ -e '{part}' ]"
        ),
    ]);
    for s in [swap_area_step(&part), reload_step(), activation_step(&part)] {
        let mut cmd = vec!["sudo".to_string(), "-n".to_string()];
        cmd.extend(s.argv);
        commands.push(cmd);
    }

    let tag = format!("swap-on-{}", std::process::id());
    let step_labels = vec![
        format!("Create GPT partition table on {dev}"),
        format!("Create linux-swap partition (1MiB..{end})"),
        format!("Wait for kernel partition node {part}"),
        format!("Write swap header (mkswap -L {})", fstab::LABEL),
        "Reload systemd mount units (systemctl daemon-reload)".to_string(),
        format!("Activate swap at priority 10 (swapon -p 10 {part})"),
    ];
    let task = Task::start_steps(
        &tag,
        format!("Configure auxiliary swap on {dev}"),
        step_labels,
        false,
        commands,
    )
    .map_err(|e| crate::Failure::new("Auxiliary swap", e.to_string()))?;

    let mut view = TaskView::new(task);
    crate::run_screen(terminal, design, "Auxiliary swap", &mut view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_the_machine_and_formats_a_line() {
        let status = Status {
            part: Some("/dev/sda1".to_string()),
            label: Some("hayato_swap".to_string()),
            size: Some("8G".to_string()),
            priority: Some(10),
            configured: true,
            active: true,
        };
        let line = status.line();
        assert!(line.contains("/dev/sda1"));
        assert!(line.contains("8G"));
        assert!(line.contains("pri=10"));
        assert!(line.contains("active"));
        assert!(line.contains("configured"));
    }

    #[test]
    fn status_without_partition_is_still_configured() {
        let status = Status {
            part: None,
            label: None,
            size: None,
            priority: None,
            configured: true,
            active: false,
        };
        assert!(
            status
                .detail()
                .contains("no auxiliary swap card is inserted")
        );
        assert!(status.line().contains("configured"));
    }

    #[test]
    fn a_swap_area_that_is_not_configured_is_offered_for_adoption() {
        let status = Status {
            part: Some("/dev/mmcblk1p1".to_string()),
            label: Some("an_older_label".to_string()),
            size: Some("8G".to_string()),
            priority: None,
            configured: false,
            active: false,
        };
        assert!(status.detail().contains("can adopt"));
        assert!(status.detail().contains("/dev/mmcblk1p1"));
    }

    #[test]
    fn adoption_relabels_before_it_activates() {
        let steps = adopt_steps("/dev/mmcblk1p1");
        assert_eq!(
            steps[0].argv,
            ["swaplabel", "-L", "hayato_swap", "/dev/mmcblk1p1"]
        );
        assert_eq!(steps[1].argv, ["swapon", "-p", "10", "/dev/mmcblk1p1"]);
        assert!(steps.iter().all(|step| step.root));
    }

    #[test]
    fn the_sizes_are_the_whole_card_and_2_4_8_gib_with_margin() {
        let opts = sizes(30 * 1024); // 30 GiB
        assert_eq!(opts[0].0, "100%");
        assert_eq!(opts[1].0, "2GiB");
        assert_eq!(opts[2].0, "4GiB");
        assert_eq!(opts[3].0, "8GiB");

        let small = sizes(1024); // 1 GiB - nothing fits
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].0, "100%");

        let medium = sizes(3 * 1024); // 3 GiB - only 2 GiB fits
        assert_eq!(medium.len(), 2);
        assert_eq!(medium[1].0, "2GiB");
    }

    #[test]
    fn the_extent_is_the_parted_end_for_the_chosen_size() {
        assert_eq!(extent("100%", 30 * 1024), "100%");
        assert_eq!(extent("2GiB", 30 * 1024), "2049MiB");
        assert_eq!(extent("4GiB", 30 * 1024), "4097MiB");
        assert_eq!(extent("8GiB", 30 * 1024), "8193MiB");
    }

    #[test]
    fn label_taken_refuses_a_different_disk_with_the_label() {
        assert!(label_taken("/dev/sdb1", "/dev/sda", "sda", "sdb"));
        assert!(!label_taken("/dev/sda1", "/dev/sda", "sda", "sda"));
        assert!(!label_taken("", "/dev/sda", "sda", "sdb"));
    }

    #[test]
    fn joined_merges_newlines_into_spaces_for_the_case_pattern() {
        let listing = "zram0\n/dev/sda1\n";
        let joined = joined(listing);
        assert_eq!(joined, " zram0 /dev/sda1 ");
        assert!(active_in(&joined, "/dev/sda1"));
        assert!(active_in(&joined, "zram0"));
    }

    #[test]
    fn active_in_finds_the_partition_in_the_joined_list() {
        let joined = " zram0 /dev/sda1 /dev/mmcblk1p1 ";
        assert!(active_in(joined, "/dev/sda1"));
        assert!(active_in(joined, "/dev/mmcblk1p1"));
        assert!(!active_in(joined, "/dev/sdb1"));
    }

    #[test]
    fn priority_of_reads_the_next_token_after_the_name() {
        let joined = "zram0 100 /dev/sda1 10 /dev/mmcblk1p1 10";
        assert_eq!(priority_of(joined, "/dev/sda1"), Some(10));
        assert_eq!(priority_of(joined, "/dev/mmcblk1p1"), Some(10));
        assert_eq!(priority_of(joined, "/dev/sdb1"), None);
    }

    #[test]
    fn the_partition_steps_are_argv_arrays_and_root() {
        let steps = partition_steps("/dev/sda", "100%");
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].argv,
            ["parted", "-s", "/dev/sda", "mktable", "gpt"]
        );
        assert_eq!(
            steps[1].argv,
            [
                "parted",
                "-s",
                "/dev/sda",
                "mkpart",
                "primary",
                "linux-swap",
                "1MiB",
                "100%"
            ]
        );
        assert!(steps.iter().all(|s| s.root));
    }

    #[test]
    fn the_swap_area_step_uses_mkswap_with_f_and_the_label() {
        let step = swap_area_step("/dev/sda1");
        assert_eq!(
            step.argv,
            ["mkswap", "-f", "-L", "hayato_swap", "/dev/sda1"]
        );
        assert!(step.root);
    }

    #[test]
    fn the_activation_step_uses_swapon_with_priority_10() {
        let step = activation_step("/dev/sda1");
        assert_eq!(step.argv, ["swapon", "-p", "10", "/dev/sda1"]);
        assert!(step.root);
    }

    #[test]
    fn the_reload_step_is_systemctl_daemon_reload() {
        let step = reload_step();
        assert_eq!(step.argv, ["systemctl", "daemon-reload"]);
        assert!(step.root);
    }

    #[test]
    fn a_wait_for_a_node_that_is_not_there_gives_up() {
        let start = Instant::now();
        assert!(!wait_for_in(
            "/dev/hayato-not-a-device",
            Duration::from_millis(150)
        ));
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn a_node_that_exists_is_not_waited_for() {
        assert!(wait_for_in("/dev/null", Duration::from_millis(0)));
    }
}
