//! Format a disk: a USB stick or a card, wiped and made empty (`hayato-format`).
//!
//! The candidate list is [`disk::candidates`], and **the list is the safety**,
//! not the confirmation: nothing mounted, no ChromeOS kernel partition, no
//! pseudo device, and never a disk built into this machine or the one the
//! system runs from. What is left has nothing this system needs on it, and the
//! confirm still names the device, its model and its size with Cancel
//! selected, because the list can be wrong about a disk nobody thought of.
//!
//! The steps are [`Step`]s: a fresh GPT, one partition over the whole disk,
//! then the filesystem. exFAT is the default because Windows, macOS and Linux
//! all read it and, unlike FAT32, it has no 4 GB file limit; ext4 is for a
//! Linux machine that needs owners and permissions.
//!
//! It is **not stoppable**, like an apt install: a kill in the middle of
//! `mkfs` leaves a disk with half a filesystem, which is worse than waiting.

use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Confirm, ConfirmAction, Design, Glyph, Header, KeyHint, ListAction,
    ListState, Outcome, Row, Screen, Task, TaskView, TextStyle, draw_card, draw_empty,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::disk::{self, Disk, Facts};
use super::pick::{Question, row};
use super::step::{self, Failed, Step};
use super::{missing, out};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["parted", "mkfs.exfat", "mkfs.ext4", "lsblk"];

/// The filesystems, in the order the menu shows them.
pub const FILESYSTEMS: [&str; 2] = ["exfat", "ext4"];

/// The label every formatted disk carries.
pub const LABEL: &str = "HAYATO";

/// The steps that make `dev` one empty filesystem, and the partition they
/// produce. Refusing an unknown filesystem here is what keeps a menu answer
/// from becoming an argument to `parted` that nobody chose.
pub fn steps(dev: &str, fs: &str) -> Result<(Vec<Step>, String), String> {
    if !FILESYSTEMS.contains(&fs) {
        return Err(format!("`{fs}` is not a filesystem this menu writes"));
    }
    Ok((
        vec![
            Step::root("partition-table", &["parted", "-s", dev, "mktable", "gpt"]),
            Step::root(
                "partition",
                &["parted", "-s", dev, "mkpart", "primary", fs, "1MiB", "100%"],
            ),
        ],
        disk::partition(dev),
    ))
}

/// The filesystem step, once the partition node exists.
fn mkfs(fs: &str, part: &str) -> Step {
    if fs == "exfat" {
        // -F: the new partition starts where an old filesystem's signature may
        // still be, and mkfs.exfat refuses such a device. The refusal only
        // happens on a terminal, and passing -F is what keeps that from
        // deciding the outcome.
        Step::root("filesystem", &["mkfs.exfat", "-F", "-L", LABEL, part])
    } else {
        Step::root("filesystem", &["mkfs.ext4", "-F", "-q", "-L", LABEL, part])
    }
}

/// Wait for the partition node: udev creates it after the new table is
/// processed, and waiting is cheaper than failing on a race.
pub fn wait_for(node: &str) -> bool {
    wait_for_in(node, Duration::from_secs(5))
}

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

/// Erase `dev` and make it one filesystem. Every failure names the step that
/// failed and the reason its tool gave.
pub fn apply(dev: &str, fs: &str) -> Result<String, Failed> {
    let (steps, part) = steps(dev, fs).map_err(|reason| Failed {
        step: "filesystem",
        reason,
    })?;
    step::run(&steps)?;
    if !wait_for(&part) {
        return Err(Failed {
            step: "partition",
            reason: format!("{part} did not appear: the kernel still has the old table"),
        });
    }
    step::run(&[mkfs(fs, &part)])?;
    Ok(format!("{dev} is empty ({fs}, label {LABEL})"))
}

/// The questions a format asks, given the candidates: the disk, the
/// filesystem, and the confirm that names both.
pub fn questions(found: &[Disk]) -> Vec<Question> {
    let (rows, keys) = disk::rows(found);
    vec![
        Question::choose(
            "Which disk?",
            "Enter picks the filesystem · only disks with nothing mounted are listed",
            rows,
            keys,
        ),
        Question::choose(
            "Filesystem",
            "{0} · everything on it is erased",
            vec![
                row(
                    "exFAT",
                    "any machine: Windows, macOS, a camera, a TV; no 4 GB limit",
                    Glyph::Disk,
                ),
                row(
                    "ext4",
                    "Linux only, and it keeps owners and permissions",
                    Glyph::Disk,
                ),
            ],
            FILESYSTEMS.iter().map(|fs| fs.to_string()).collect(),
        ),
        Question::ask(
            "Erase this disk?",
            "{0} · every file on it is lost",
            "{0} · all of it becomes one {1} partition",
            "Erase it",
            "go",
        ),
    ]
}

enum FormatMode {
    Select,
    Confirm {
        dev: String,
        fs: &'static str,
        confirm: Confirm,
    },
}

/// Safe Disk Formatter Studio screen.
pub struct FormatScreen {
    disks: Vec<Disk>,
    fs_idx: usize,
    list: ListState,
    mode: FormatMode,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    banner: Option<String>,
    pub confirmed: Option<(String, String)>,
}

impl FormatScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            disks: Vec::new(),
            fs_idx: 0,
            list: ListState::new().with_empty("no removable disks available"),
            mode: FormatMode::Select,
            header: Header::new("Hayato").subtitle("Format Studio"),
            keybar: vec![
                KeyHint::new("Enter", "format disk"),
                KeyHint::new("Tab", "exFAT / ext4"),
                KeyHint::new("Ctrl+R", "rescan"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(16),
            banner: None,
            confirmed: None,
        };
        screen.rescan();
        screen
    }

    fn rescan(&mut self) {
        let listing = out(&["lsblk", "-dnro", "NAME,SIZE,MODEL,TRAN"]).unwrap_or_default();
        let facts = Facts::from_machine();
        self.disks = disk::candidates(&listing, &facts);
        let rows: Vec<Row> = self
            .disks
            .iter()
            .map(|d| {
                let tran = d.tran.as_deref().unwrap_or("removable");
                Row::new(format!("/dev/{}", d.name))
                    .glyph(Glyph::Disk)
                    .detail(format!("{} · {tran}", d.model))
                    .value(d.size.clone())
            })
            .collect();
        self.list.set_rows(rows);
        self.header =
            Header::new("Hayato")
                .subtitle("Format Studio")
                .status(if self.disks.is_empty() {
                    "No unmounted removable disks · plug one in and press Ctrl+R".into()
                } else {
                    format!(
                        "{} removable candidate{} · system & mounted disks excluded",
                        self.disks.len(),
                        if self.disks.len() == 1 { "" } else { "s" }
                    )
                });
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 48) / 100).max(46) as u16;
            let right_w = area.width.saturating_sub(left_w + gap);
            (
                Rect::new(area.x, area.y, left_w, area.height),
                Some(Rect::new(
                    area.x + left_w + gap,
                    area.y,
                    right_w,
                    area.height,
                )),
            )
        } else {
            (area, None)
        }
    }

    fn left_layout(area: Rect) -> (Rect, Rect) {
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        let fs_bar = Rect::new(inner.x, inner.y, inner.width, 2.min(inner.height));
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(3).min(inner.bottom()),
            inner.width,
            inner.height.saturating_sub(3),
        );
        (fs_bar, rows)
    }

    fn selected_disk(&self) -> Option<&Disk> {
        let idx = self.list.selected_index()?;
        self.disks.get(idx)
    }

    fn open_confirm(&mut self) {
        let Some(d) = self.selected_disk().cloned() else {
            self.rescan();
            self.banner = Some("Rescanned removable disks".into());
            return;
        };
        let dev = format!("/dev/{}", d.name);
        let fs = FILESYSTEMS[self.fs_idx];
        let confirm = Confirm::new(
            format!("Erase {dev}?"),
            format!(
                "{dev} ({}, {}) — ALL DATA WILL BE LOST · formatted as {fs} (label {LABEL})",
                d.model, d.size
            ),
        )
        .with_options(&["Cancel", "Erase & Format"]);
        self.mode = FormatMode::Confirm { dev, fs, confirm };
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Safety & Plan"), false);
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
        let warn = Style::default()
            .fg(design.color(ColorRole::Warn))
            .add_modifier(Modifier::BOLD);
        let fs = FILESYSTEMS[self.fs_idx];
        let mut y = inner.y;

        if let Some(d) = self.selected_disk() {
            let dev = format!("/dev/{}", d.name);
            let part = disk::partition(&dev);
            put_trunc(buf, inner.x, y, "TARGET DEVICE", inner.width, warn);
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("{dev}  ({})", d.size),
                inner.width,
                design.style(TextStyle::Title).add_modifier(Modifier::BOLD),
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Model      {}", d.model),
                inner.width,
                body,
            );
            y += 2;
            put_trunc(buf, inner.x, y, "Execution Plan", inner.width, accent);
            y += 1;
            for step in [
                format!("1. Fresh GPT partition table on {dev}"),
                format!("2. Single primary partition {part} (1MiB..100%)"),
                format!("3. Format {part} as {fs} (label {LABEL})"),
            ] {
                if y >= inner.bottom().saturating_sub(5) {
                    break;
                }
                put_trunc(buf, inner.x, y, &step, inner.width, body);
                y += 1;
            }
            y += 1;
        } else {
            put_trunc(buf, inner.x, y, "NO CANDIDATES FOUND", inner.width, accent);
            y += 2;
            put_trunc(
                buf,
                inner.x,
                y,
                "Plug in an unmounted USB stick or SD card and press Ctrl+R.",
                inner.width,
                body,
            );
            y += 2;
        }

        if y < inner.bottom().saturating_sub(3) {
            put_trunc(buf, inner.x, y, "Safety Guards", inner.width, accent);
            y += 1;
            for g in [
                "• Internal & root disks are never listed",
                "• Mounted partitions are excluded automatically",
                "• ChromeOS kernel partitions are protected",
            ] {
                if y >= inner.bottom().saturating_sub(1) {
                    break;
                }
                put_trunc(buf, inner.x, y, g, inner.width, dim);
                y += 1;
            }
        }
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "Tab switches exFAT/ext4 · Enter asks confirmation first",
            inner.width,
            dim,
        );
    }
}

impl Default for FormatScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen for FormatScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let FormatMode::Confirm { confirm, .. } = &self.mode {
            confirm.render(ctx.area, ctx.buf, ctx.design);
            return;
        }
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Removable Disks"), true);
        let (fs_bar, rows) = Self::left_layout(left);
        let accent = Style::default()
            .fg(ctx.design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let dim = ctx.design.style(TextStyle::Dim);
        let exfat_st = if self.fs_idx == 0 { accent } else { dim };
        let ext4_st = if self.fs_idx == 1 { accent } else { dim };
        let x1 = put(
            ctx.buf,
            fs_bar.x,
            fs_bar.y,
            if self.fs_idx == 0 {
                "[ ● exFAT · universal ]"
            } else {
                "  ○ exFAT · universal  "
            },
            fs_bar.width,
            exfat_st,
        );
        put_trunc(
            ctx.buf,
            x1 + 1,
            fs_bar.y,
            if self.fs_idx == 1 {
                "[ ● ext4 · Linux ]"
            } else {
                "  ○ ext4 · Linux  "
            },
            fs_bar.right().saturating_sub(x1 + 1),
            ext4_st,
        );
        if fs_bar.height >= 2 {
            let note = self
                .banner
                .as_deref()
                .unwrap_or("Tab toggles filesystem · Ctrl+R rescans · Enter formats");
            put_trunc(ctx.buf, fs_bar.x, fs_bar.y + 1, note, fs_bar.width, dim);
        }

        self.rows_height.set(rows.height);
        if self.disks.is_empty() {
            draw_empty(
                ctx.buf,
                rows,
                ctx.design,
                "No removable disks available",
                "Plug in an unmounted USB stick or SD card, then press Ctrl+R",
            );
        } else {
            self.list.render(rows, ctx.buf, ctx.design, true);
        }

        if let Some(right_area) = right {
            self.draw_inspector(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if let FormatMode::Confirm { dev, fs, confirm } = &mut self.mode {
            match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    let dev = dev.clone();
                    let fs = (*fs).to_string();
                    self.mode = FormatMode::Select;
                    self.confirmed = Some((dev, fs));
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.mode = FormatMode::Select;
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
        {
            self.rescan();
            self.banner = Some("Rescanned removable disks".into());
            return Outcome::Consumed;
        }

        match key.code {
            KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                self.fs_idx = (self.fs_idx + 1) % FILESYSTEMS.len();
                return Outcome::Consumed;
            }
            KeyCode::Enter => {
                self.open_confirm();
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => {
                self.open_confirm();
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if matches!(self.mode, FormatMode::Confirm { .. }) {
            return None;
        }
        let (left, _) = Self::split_panes(area);
        let (_, rows) = Self::left_layout(left);
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            self.open_confirm();
            return Outcome::Consumed;
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
            FormatMode::Select => "format.select",
            FormatMode::Confirm { .. } => "format.confirm",
        })
    }
}

/// `hayato menu format` (`hayato-format`): Safe Disk Formatter Studio, then TaskView.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Format a disk",
            format!("{said} (parted, exfatprogs, e2fsprogs)"),
        ));
    }

    let mut screen = FormatScreen::new();
    let code = crate::run_screen(terminal, design, "Format a disk", &mut screen)?;
    if code != 0 {
        return Ok(code);
    }
    let Some((dev, fs)) = screen.confirmed.take() else {
        return Ok(1);
    };
    let dev = &dev;
    let fs = &fs;

    let name = dev.trim_start_matches("/dev/");
    let now = Facts::from_machine();
    if disk::is_system_disk(name, &now) {
        return Err(crate::Failure::new(
            "Format a disk",
            format!(
                "{dev} is built into this machine or is the disk this system runs from: never formatted here (use hayato installer)"
            ),
        ));
    }

    let (part_steps, part) = steps(dev, fs).map_err(|r| crate::Failure::new("Format a disk", r))?;
    let fs_step = mkfs(fs, &part);

    let mut commands = Vec::new();
    for s in &part_steps {
        let mut cmd = if s.root {
            vec!["sudo".to_string(), "-n".to_string()]
        } else {
            Vec::new()
        };
        cmd.extend(s.argv.clone());
        commands.push(cmd);
    }
    commands.push(vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "for _i in $(seq 1 50); do [ -e '{part}' ] && exit 0; sleep 0.1; done; [ -e '{part}' ]"
        ),
    ]);
    {
        let mut cmd = if fs_step.root {
            vec!["sudo".to_string(), "-n".to_string()]
        } else {
            Vec::new()
        };
        cmd.extend(fs_step.argv.clone());
        commands.push(cmd);
    }

    let tag = format!("format-{}", std::process::id());
    let step_labels = vec![
        format!("Create GPT partition table on {dev}"),
        format!("Create primary {fs} partition (1MiB..100%)"),
        format!("Wait for kernel partition node {part}"),
        format!("Write {fs} filesystem (label {LABEL})"),
    ];
    let task = Task::start_steps(
        &tag,
        format!("Format {dev} ({fs})"),
        step_labels,
        false,
        commands,
    )
    .map_err(|e| crate::Failure::new("Format a disk", e.to_string()))?;

    let mut view = TaskView::new(task);
    crate::run_screen(terminal, design, "Format a disk", &mut view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_are_the_shells_and_the_partition_gets_its_p() {
        let (plan, part) = steps("/dev/sda", "exfat").unwrap();
        assert_eq!(part, "/dev/sda1");
        assert_eq!(plan[0].argv, ["parted", "-s", "/dev/sda", "mktable", "gpt"]);
        assert_eq!(
            plan[1].argv,
            [
                "parted", "-s", "/dev/sda", "mkpart", "primary", "exfat", "1MiB", "100%"
            ]
        );
        assert!(plan.iter().all(|step| step.root));
        let (_, part) = steps("/dev/mmcblk1", "ext4").unwrap();
        assert_eq!(part, "/dev/mmcblk1p1");
    }

    #[test]
    fn the_filesystem_step_is_the_right_tool_for_the_answer() {
        assert_eq!(
            mkfs("exfat", "/dev/sda1").argv,
            ["mkfs.exfat", "-F", "-L", "HAYATO", "/dev/sda1"]
        );
        assert_eq!(
            mkfs("ext4", "/dev/sda1").argv,
            ["mkfs.ext4", "-F", "-q", "-L", "HAYATO", "/dev/sda1"]
        );
    }

    #[test]
    fn a_filesystem_nobody_offered_is_refused_before_anything_runs() {
        let error = steps("/dev/sda", "ntfs; rm -rf /").unwrap_err();
        assert!(error.contains("not a filesystem"), "{error}");
    }

    #[test]
    fn the_confirm_names_the_device_and_the_filesystem() {
        let disk = Disk {
            name: "sda".into(),
            size: "29.5G".into(),
            model: "USB DISK 3.0".into(),
            tran: Some("usb".into()),
        };
        let asked = questions(&[disk]);
        assert_eq!(asked.len(), 3);
        let Question::Ask {
            note,
            message,
            verb,
            ..
        } = &asked[2]
        else {
            panic!("the third question is the confirm");
        };
        assert!(verb.contains("Erase"));
        assert_eq!(message, "{0} · all of it becomes one {1} partition");
        assert!(note.contains("every file on it is lost"));
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
