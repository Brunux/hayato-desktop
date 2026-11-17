//! Format a disk: a USB stick or a card, wiped and made empty (`lazyos-format`).
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

use std::path::Path;
use std::time::{Duration, Instant};

use lazyos_ui::{Design, Glyph, Task, TaskView};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::disk::{self, Disk, Facts};
use super::pick::{Picker, Question, row};
use super::step::{self, Failed, Step};
use super::{missing, out};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["parted", "mkfs.exfat", "mkfs.ext4", "lsblk"];

/// The filesystems, in the order the menu shows them.
pub const FILESYSTEMS: [&str; 2] = ["exfat", "ext4"];

/// The label every formatted disk carries.
pub const LABEL: &str = "LAZYOS";

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

/// `lazyos menu format` (`lazyos-format`): the window, then the steps in a TaskView.
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

    let found = loop {
        let listing = out(&["lsblk", "-dnro", "NAME,SIZE,MODEL,TRAN"]).unwrap_or_default();
        let facts = Facts::from_machine();
        let candidates = disk::candidates(&listing, &facts);
        if !candidates.is_empty() {
            break candidates;
        }
        let mut empty_picker = Picker::new(
            "Format a disk",
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
        let code = crate::run_screen(terminal, design, "Format a disk", &mut empty_picker)?;
        if code != 0 {
            return Ok(code);
        }
    };

    let mut picker = Picker::new("Format a disk", questions(&found));
    let code = crate::run_screen(terminal, design, "Format a disk", &mut picker)?;
    if code != 0 {
        return Ok(code);
    }
    let answers = picker.answers().clone();
    let (Some(dev), Some(fs)) = (answers.first(), answers.get(1)) else {
        return Ok(1);
    };

    let name = dev.trim_start_matches("/dev/");
    let now = Facts::from_machine();
    if disk::is_system_disk(name, &now) {
        return Err(crate::Failure::new(
            "Format a disk",
            format!(
                "{dev} is built into this machine or is the disk this system runs from: never formatted here (use lazyos installer)"
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
            ["mkfs.exfat", "-F", "-L", "LAZYOS", "/dev/sda1"]
        );
        assert_eq!(
            mkfs("ext4", "/dev/sda1").argv,
            ["mkfs.ext4", "-F", "-q", "-L", "LAZYOS", "/dev/sda1"]
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
            "/dev/lazyos-not-a-device",
            Duration::from_millis(150)
        ));
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn a_node_that_exists_is_not_waited_for() {
        assert!(wait_for_in("/dev/null", Duration::from_millis(0)));
    }
}
