//! Smart File Intent Router (`hayato menu open PATH`).
//!
//! Replaces dead-end browser fallbacks and `NoProgram` refusals when a user
//! double-clicks or opens a `.deb` installer, archive (`.zip`/`.tar*`/`.7z`/`.rar`),
//! disk image (`.iso`/`.img`), file whose viewer is not installed yet, foreign
//! installer (`.exe`/`.msi`/`.dmg`/`.apk`), or unknown binary.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hayato_ui::task::{Task, TaskState, TaskView};
use hayato_ui::widgets::{Confirm, ConfirmAction, put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, LogPane, Outcome,
    Row, Screen, TextStyle, draw_badge, draw_card,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::human;
use super::open::{self, AppCandidate, FileIntent, Resolve, System};
use crate::Failure;
use crate::system::pkg_guard::{self, Verdict};
use crate::system::software::catalog::{AppWeight, Catalog};
use crate::system::{DUAL_PANE_MIN_WIDTH, file_actions, split_panes};

/// Metadata read from a `.deb` package via `dpkg-deb -f`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebMeta {
    pub package: String,
    pub version: String,
    pub arch: String,
    pub installed_size_kb: Option<u64>,
    pub summary: String,
}

#[derive(Clone, Debug)]
struct IntentSummary {
    subtitle: String,
    status: String,
    badge: String,
    badge_role: ColorRole,
    file_name: String,
    location_line: String,
    blurb: Vec<String>,
    inspector_title: String,
    inspector_lines: Vec<String>,
}

#[derive(Clone, Debug)]
enum IntentAction {
    InstallDeb {
        pkg_display: String,
        deb_path: PathBuf,
    },
    ExtractArchive {
        dest_dir: PathBuf,
        ext: String,
    },
    InstallExtractorAndExtract {
        pkg: String,
        label: String,
        dest_dir: PathBuf,
        ext: String,
    },
    MountDiskImage,
    InstallCatalogApp {
        package: String,
        name: String,
        weight: AppWeight,
    },
    OpenSoftwareCatalog {
        query: Option<String>,
    },
    InspectContents,
    OpenInBrowser,
    ShowInFiles,
    MoreFileActions,
    Share,
}

#[derive(Clone, Debug)]
enum PendingFileAction {
    Deb {
        pkg_display: String,
        deb_path: PathBuf,
    },
    Package {
        package: String,
        label: String,
        post: Option<PostRunAction>,
    },
    ExtractorAndExtract {
        pkg: String,
        dest_dir: PathBuf,
        ext: String,
    },
}

#[derive(Clone, Debug)]
enum PostRunAction {
    OpenFolder(PathBuf),
    OpenFile(PathBuf),
}

/// Single Phase enum owning mutually exclusive UI state (`desktop/AGENTS.md` §12).
enum FileOpenPhase {
    Intent {
        list: ListState,
        actions: Vec<IntentAction>,
    },
    Inspect {
        title: String,
        log: LogPane,
    },
    ConfirmGuard {
        pending: PendingFileAction,
        confirm: Confirm,
        list: ListState,
        actions: Vec<IntentAction>,
    },
    Blocked {
        title: String,
        lines: Vec<String>,
        list: ListState,
        actions: Vec<IntentAction>,
    },
    Running {
        post_action: Option<PostRunAction>,
        task: Box<TaskView>,
    },
}

pub struct FileOpenScreen {
    path: PathBuf,
    parent: PathBuf,
    summary: IntentSummary,
    phase: FileOpenPhase,
    header: Header,
    keybar: Vec<KeyHint>,
    list_area: Cell<Rect>,
    confirm_area: Cell<Rect>,
    banner: Option<(String, bool)>,
    pub pending_launch: Option<Vec<String>>,
}

impl FileOpenScreen {
    pub fn from_intent(
        path: PathBuf,
        size_bytes: u64,
        intent: FileIntent,
        deb: Option<DebMeta>,
    ) -> Self {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        let parent_short = shorten_home(&parent);
        let size_str = human(size_bytes);
        let catalog = Catalog::load();

        let (summary, rows, actions) = Self::build_model(
            &path,
            &parent,
            &file_name,
            &parent_short,
            &size_str,
            &intent,
            deb.as_ref(),
            Some(&catalog),
        );

        let mut list = ListState::new().with_empty("no actions");
        list.set_rows(rows);

        let mut screen = Self {
            path,
            parent,
            summary,
            phase: FileOpenPhase::Intent { list, actions },
            header: Header::new("Hayato"),
            keybar: Vec::new(),
            list_area: Cell::new(Rect::default()),
            confirm_area: Cell::new(Rect::default()),
            banner: None,
            pending_launch: None,
        };
        screen.sync_chrome();
        screen
    }

    pub fn for_path(path: PathBuf) -> Result<Self, String> {
        let meta =
            std::fs::metadata(&path).map_err(|_| format!("{} no longer exists", path.display()))?;
        let tools = System::from_env();
        let text = open::is_text(&path);
        let intent = match open::resolve(&path, text, &tools) {
            Ok(Resolve::Router(intent)) => intent,
            Ok(Resolve::Launch(_)) => FileIntent::UnknownOrBinary {
                is_executable: false,
            },
            Err(r) => return Err(r.message().to_string()),
        };
        let deb = if matches!(intent, FileIntent::DebPackage) {
            inspect_deb(&path)
        } else {
            None
        };
        Ok(Self::from_intent(path, meta.len(), intent, deb))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_model(
        path: &Path,
        parent: &Path,
        file_name: &str,
        parent_short: &str,
        size_str: &str,
        intent: &FileIntent,
        deb: Option<&DebMeta>,
        catalog: Option<&Catalog>,
    ) -> (IntentSummary, Vec<Row>, Vec<IntentAction>) {
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        let host_deb_arch = host_dpkg_arch();

        let summary = match intent {
            FileIntent::DebPackage => {
                let arch_ok = deb
                    .as_ref()
                    .map(|d| arch_compatible(&d.arch, host_deb_arch))
                    .unwrap_or(true);
                let pkg_display = deb
                    .as_ref()
                    .map(|d| d.package.clone())
                    .unwrap_or_else(|| file_name.trim_end_matches(".deb").to_string());
                if arch_ok {
                    rows.push(
                        Row::new("Install this app")
                            .glyph(Glyph::Check)
                            .detail("checks system safety, then installs dependencies"),
                    );
                    actions.push(IntentAction::InstallDeb {
                        pkg_display: pkg_display.clone(),
                        deb_path: path.to_path_buf(),
                    });
                } else {
                    let wrong = deb.map(|d| d.arch.as_str()).unwrap_or("other");
                    rows.push(
                        Row::new(format!("Incompatible package ({wrong})"))
                            .glyph(Glyph::Warning)
                            .detail(format!("cannot run on this {host_deb_arch} device")),
                    );
                    actions.push(IntentAction::InstallDeb {
                        pkg_display: pkg_display.clone(),
                        deb_path: path.to_path_buf(),
                    });
                }
                rows.push(
                    Row::new("Preview included files")
                        .glyph(Glyph::File)
                        .detail("see what files will be installed"),
                );
                actions.push(IntentAction::InspectContents);

                let (badge, badge_role, mut lines) = if let Some(d) = deb {
                    let badge = if arch_ok {
                        format!("READY TO INSTALL · {}", d.arch)
                    } else {
                        format!("WRONG DEVICE TYPE · {} (needs {host_deb_arch})", d.arch)
                    };
                    let role = if arch_ok {
                        ColorRole::Ok
                    } else {
                        ColorRole::Danger
                    };
                    let size_line = d
                        .installed_size_kb
                        .map(|kb| format!("{size_str} download · {} installed", human(kb * 1024)))
                        .unwrap_or_else(|| format!("{size_str} package"));
                    let lines = vec![
                        format!("App       {}", d.package),
                        format!("Version   {}", d.version),
                        format!("Size      {size_line}"),
                        format!("Summary   {}", d.summary),
                        String::new(),
                        "Safety check".to_string(),
                        "Hayato checks that no system or boot files".to_string(),
                        "are removed before asking you to confirm.".to_string(),
                    ];
                    (badge, role, lines)
                } else {
                    (
                        "APP PACKAGE (.deb)".to_string(),
                        ColorRole::Accent,
                        vec![
                            format!("File      {file_name}"),
                            format!("Folder    {parent_short}"),
                            "Safety check runs before installing.".to_string(),
                        ],
                    )
                };
                lines.push(String::new());
                lines.push(format!("Location  {parent_short}/{file_name}"));

                IntentSummary {
                    subtitle: "Open app package".to_string(),
                    status: format!("app installer (.deb) · {size_str}"),
                    badge,
                    badge_role,
                    file_name: file_name.to_string(),
                    location_line: format!("{parent_short} · {size_str}"),
                    blurb: vec![
                        "Debian app package — install safely with dependency check.".to_string(),
                    ],
                    inspector_title: "Package Details".to_string(),
                    inspector_lines: lines,
                }
            }

            FileIntent::Archive { ext } => {
                let stem = file_actions::archive_stem(file_name);
                let dest = unique_extract_dir(parent, stem);
                let dest_name = dest
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| stem.to_string());

                if let Some((missing_pkg, label)) = missing_extractor_for(ext) {
                    rows.push(
                        Row::new(format!("Install {label} & extract"))
                            .glyph(Glyph::Box)
                            .detail(format!("extracts to {dest_name}/ after quick install")),
                    );
                    actions.push(IntentAction::InstallExtractorAndExtract {
                        pkg: missing_pkg.to_string(),
                        label: label.to_string(),
                        dest_dir: dest.clone(),
                        ext: ext.clone(),
                    });
                } else {
                    rows.push(
                        Row::new(format!("Extract to \"{dest_name}/\""))
                            .glyph(Glyph::Box)
                            .detail("unpacks safely beside original without overwriting"),
                    );
                    actions.push(IntentAction::ExtractArchive {
                        dest_dir: dest.clone(),
                        ext: ext.clone(),
                    });
                }
                rows.push(
                    Row::new("Preview contents")
                        .glyph(Glyph::File)
                        .detail("browse files packed inside this archive"),
                );
                actions.push(IntentAction::InspectContents);

                IntentSummary {
                    subtitle: "Open archive".to_string(),
                    status: format!("archive (.{ext}) · {size_str}"),
                    badge: format!("COMPRESSED ARCHIVE (.{ext})"),
                    badge_role: ColorRole::Accent,
                    file_name: file_name.to_string(),
                    location_line: format!("{parent_short} · {size_str}"),
                    blurb: vec![
                        "Compressed folder — extract its files into a new folder to use them."
                            .to_string(),
                    ],
                    inspector_title: "Archive Details".to_string(),
                    inspector_lines: vec![
                        format!("Format    .{ext}"),
                        format!("Target    {parent_short}/{dest_name}/"),
                        String::new(),
                        "Existing files are never overwritten; a numbered".to_string(),
                        "folder is chosen if the destination exists.".to_string(),
                    ],
                }
            }

            FileIntent::DiskImage => {
                rows.push(
                    Row::new("Open as virtual folder")
                        .glyph(Glyph::Disk)
                        .detail("attach read-only drive and browse files inside"),
                );
                actions.push(IntentAction::MountDiskImage);

                IntentSummary {
                    subtitle: "Open disk image".to_string(),
                    status: format!("disk image · {size_str}"),
                    badge: "DISK IMAGE (.iso / .img)".to_string(),
                    badge_role: ColorRole::Accent,
                    file_name: file_name.to_string(),
                    location_line: format!("{parent_short} · {size_str}"),
                    blurb: vec![
                        "Disk image file — mount as a temporary folder to inspect its contents."
                            .to_string(),
                    ],
                    inspector_title: "Disk Image".to_string(),
                    inspector_lines: vec![
                        format!("File      {file_name}"),
                        format!("Size      {size_str}"),
                        String::new(),
                        "Mounts cleanly through udisks without writing".to_string(),
                        "anything to your USB or internal drives.".to_string(),
                    ],
                }
            }

            FileIntent::MissingApp {
                kind_label,
                ext,
                candidates,
            } => {
                let mut info_lines = Vec::new();
                for AppCandidate { package, label } in candidates {
                    let cat_app = catalog
                        .and_then(|c| c.app_by_package(package))
                        .map(|(_, a)| a);
                    let weight = cat_app.map(|a| a.weight).unwrap_or(AppWeight::Light);
                    let blurb = cat_app
                        .map(|a| a.summary.as_str())
                        .unwrap_or("Recommended Hayato app");
                    rows.push(
                        Row::new(format!("Install {label} & open"))
                            .glyph(Glyph::Box)
                            .detail(format!("{} · {blurb}", weight.label())),
                    );
                    actions.push(IntentAction::InstallCatalogApp {
                        package: (*package).to_string(),
                        name: (*label).to_string(),
                        weight,
                    });
                    info_lines.push(format!("{label} ({package})"));
                    info_lines.push(format!("  Weight  {}", weight.label()));
                    info_lines.push(format!("  About   {blurb}"));
                    info_lines.push(String::new());
                }
                if matches!(ext.as_str(), "pdf" | "html" | "htm") {
                    rows.push(
                        Row::new("Open in Web Browser")
                            .glyph(Glyph::Globe)
                            .detail("quick view without installing a dedicated reader"),
                    );
                    actions.push(IntentAction::OpenInBrowser);
                }
                rows.push(
                    Row::new("Browse Hayato Software")
                        .glyph(Glyph::Search)
                        .detail("pick another app from the curated catalog"),
                );
                actions.push(IntentAction::OpenSoftwareCatalog {
                    query: Some(kind_label.to_string()),
                });

                IntentSummary {
                    subtitle: "Open file".to_string(),
                    status: format!("{kind_label} · {size_str}"),
                    badge: "APP NEEDED TO OPEN".to_string(),
                    badge_role: ColorRole::Warn,
                    file_name: file_name.to_string(),
                    location_line: format!("{parent_short} · {size_str}"),
                    blurb: vec![format!(
                        "No app installed for .{ext} {kind_label} files yet."
                    )],
                    inspector_title: "Suggested App".to_string(),
                    inspector_lines: info_lines,
                }
            }

            FileIntent::ForeignInstaller { platform } => {
                let stem = file_name
                    .rsplit_once('.')
                    .map(|(s, _)| s)
                    .unwrap_or(file_name)
                    .replace(['_', '-', '.'], " ");
                rows.push(
                    Row::new("Find an alternative in Software")
                        .glyph(Glyph::Box)
                        .detail(format!("browse native apps matching \"{stem}\"")),
                );
                actions.push(IntentAction::OpenSoftwareCatalog {
                    query: Some(stem.trim().to_string()),
                });

                IntentSummary {
                    subtitle: "Open file".to_string(),
                    status: format!("{platform} installer · {size_str}"),
                    badge: format!("BUILT FOR {platform}"),
                    badge_role: ColorRole::Warn,
                    file_name: file_name.to_string(),
                    location_line: format!("{parent_short} · {size_str}"),
                    blurb: vec![
                        format!("Made for {platform} — this file can't run directly on Hayato."),
                        "Look for a native app in Hayato Software instead.".to_string(),
                    ],
                    inspector_title: "Why It Didn't Open".to_string(),
                    inspector_lines: vec![
                        format!("This file was built for {platform} computers."),
                        String::new(),
                        "Most everyday tools have native Debian apps or".to_string(),
                        "web equivalents inside Hayato Software.".to_string(),
                    ],
                }
            }

            FileIntent::UnknownOrBinary { is_executable } => {
                rows.push(
                    Row::new("Browse Hayato Software")
                        .glyph(Glyph::Box)
                        .detail("search apps by file type or task"),
                );
                actions.push(IntentAction::OpenSoftwareCatalog { query: None });

                let blurb = if *is_executable {
                    "Compiled program — open in Terminal if you trust its source."
                } else {
                    "Unknown file format — Hayato doesn't know which app opens this."
                };
                IntentSummary {
                    subtitle: "Open file".to_string(),
                    status: format!("binary file · {size_str}"),
                    badge: "UNRECOGNIZED FILE".to_string(),
                    badge_role: ColorRole::Warn,
                    file_name: file_name.to_string(),
                    location_line: format!("{parent_short} · {size_str}"),
                    blurb: vec![blurb.to_string()],
                    inspector_title: "File Info".to_string(),
                    inspector_lines: vec![
                        format!("Name      {file_name}"),
                        format!("Size      {size_str}"),
                        format!("Folder    {parent_short}"),
                        String::new(),
                        "Use \"More file actions\" below to inspect, share,".to_string(),
                        "or copy its path to the clipboard.".to_string(),
                    ],
                }
            }
        };

        rows.push(
            Row::new("Show in Files")
                .glyph(Glyph::Folder)
                .detail(format!("open {parent_short}")),
        );
        actions.push(IntentAction::ShowInFiles);

        rows.push(
            Row::new("Share…")
                .glyph(Glyph::Qr)
                .detail("send to phone or another computer"),
        );
        actions.push(IntentAction::Share);

        rows.push(
            Row::new("More file actions…")
                .glyph(Glyph::Sliders)
                .detail("compress, copy path, open terminal"),
        );
        actions.push(IntentAction::MoreFileActions);

        (summary, rows, actions)
    }

    fn sync_chrome(&mut self) {
        self.header = Header::new("Hayato")
            .subtitle(&self.summary.subtitle)
            .status(&self.summary.status);
        self.keybar = match &self.phase {
            FileOpenPhase::Intent { actions, .. } => {
                let mut hints = vec![
                    KeyHint::new("↑↓", "select"),
                    KeyHint::new("Enter", "choose"),
                ];
                if actions
                    .iter()
                    .any(|a| matches!(a, IntentAction::InspectContents))
                {
                    hints.push(KeyHint::new("I", "inspect"));
                }
                if actions
                    .iter()
                    .any(|a| matches!(a, IntentAction::OpenSoftwareCatalog { .. }))
                {
                    hints.push(KeyHint::new("S", "software"));
                }
                hints.push(KeyHint::new("F", "files"));
                hints.push(KeyHint::new("Esc", "close"));
                hints
            }
            FileOpenPhase::Inspect { .. } => vec![
                KeyHint::new("↑↓", "scroll"),
                KeyHint::new("PgUp/Dn", "page"),
                KeyHint::new("Esc", "back"),
            ],
            FileOpenPhase::ConfirmGuard { .. } => vec![
                KeyHint::new("←→", "choose"),
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "cancel"),
            ],
            FileOpenPhase::Blocked { .. } => vec![KeyHint::new("Esc", "back")],
            FileOpenPhase::Running { post_action, task } => {
                if task.task().state == TaskState::Done && post_action.is_some() {
                    vec![
                        KeyHint::new("Enter", "open now"),
                        KeyHint::new("Esc", "close"),
                    ]
                } else {
                    task.keybar().unwrap_or_default().to_vec()
                }
            }
        };
    }

    fn run_action(&mut self, action: IntentAction) -> Outcome {
        match action {
            IntentAction::InstallDeb {
                pkg_display,
                deb_path,
            } => {
                if let Some(meta) = inspect_deb(&deb_path)
                    && !arch_compatible(&meta.arch, host_dpkg_arch())
                {
                    self.enter_blocked(
                        format!("Cannot install {}", meta.package),
                        vec![
                            format!("This package was built for '{}' computers.", meta.arch),
                            format!(
                                "This device uses '{}' architecture and cannot run it.",
                                host_dpkg_arch()
                            ),
                        ],
                    );
                    return Outcome::Consumed;
                }
                let deb_arg = apt_deb_arg(&deb_path);
                let pending = PendingFileAction::Deb {
                    pkg_display: pkg_display.clone(),
                    deb_path: PathBuf::from(&deb_arg),
                };
                match pkg_guard::check_install(&[&deb_arg]) {
                    Ok(Verdict::Block(reasons)) => {
                        self.enter_blocked(format!("Blocked installing {pkg_display}"), reasons);
                        Outcome::Consumed
                    }
                    Ok(Verdict::Warn(reasons)) => {
                        self.enter_confirm(
                            format!("Install {pkg_display}?"),
                            reasons,
                            "Install",
                            pending,
                        );
                        Outcome::Consumed
                    }
                    Ok(Verdict::Allow) => {
                        self.enter_confirm(
                            format!("Install {pkg_display}?"),
                            vec![format!(
                                "Install {pkg_display} and any required system dependencies."
                            )],
                            "Install",
                            pending,
                        );
                        Outcome::Consumed
                    }
                    Err(e) => {
                        self.banner = Some((format!("Package check failed: {e}"), true));
                        Outcome::Consumed
                    }
                }
            }

            IntentAction::ExtractArchive { dest_dir, ext } => {
                self.start_extract(dest_dir, &ext, None)
            }

            IntentAction::InstallExtractorAndExtract {
                pkg,
                label,
                dest_dir,
                ext,
            } => {
                let pending = PendingFileAction::ExtractorAndExtract {
                    pkg: pkg.clone(),
                    dest_dir,
                    ext,
                };
                self.check_and_confirm_package(&pkg, &label, AppWeight::Light, None, pending)
            }

            IntentAction::MountDiskImage => self.mount_disk_image(),

            IntentAction::InstallCatalogApp {
                package,
                name,
                weight,
            } => {
                let post = Some(PostRunAction::OpenFile(self.path.clone()));
                let pending = PendingFileAction::Package {
                    package: package.clone(),
                    label: name.clone(),
                    post: post.clone(),
                };
                self.check_and_confirm_package(&package, &name, weight, post, pending)
            }

            IntentAction::InspectContents => {
                let lines = read_archive_listing(&self.path);
                let mut log = LogPane::new();
                log.set_lines(lines);
                log.follow(false);
                let old = std::mem::replace(
                    &mut self.phase,
                    FileOpenPhase::Inspect {
                        title: format!("Contents · {}", self.summary.file_name),
                        log,
                    },
                );
                // Remember previous list/actions on return via rebuild or stash? Wait: let's keep a clean return!
                // Even simpler: re-enter Intent cleanly or transition back by storing nothing extra!
                let _ = old;
                self.sync_chrome();
                Outcome::Consumed
            }

            IntentAction::OpenSoftwareCatalog { query } => {
                let mut argv = vec![
                    super::self_exe(),
                    "menu".to_string(),
                    "software".to_string(),
                ];
                if let Some(q) = query.filter(|s| !s.is_empty()) {
                    argv.push("--search".to_string());
                    argv.push(q);
                }
                self.pending_launch = Some(argv);
                Outcome::Done(hayato_ui::app::exit::OK)
            }

            IntentAction::OpenInBrowser => {
                let url = format!("file://{}", self.path.display());
                self.pending_launch = Some(vec!["chromium".to_string(), url]);
                Outcome::Done(hayato_ui::app::exit::OK)
            }

            IntentAction::ShowInFiles => {
                self.pending_launch = Some(vec![
                    "hayato-files".to_string(),
                    "--".to_string(),
                    self.parent.display().to_string(),
                ]);
                Outcome::Done(hayato_ui::app::exit::OK)
            }

            IntentAction::MoreFileActions => {
                self.pending_launch = Some(vec![
                    super::self_exe(),
                    "menu".to_string(),
                    "file-actions".to_string(),
                    self.path.display().to_string(),
                ]);
                Outcome::Done(hayato_ui::app::exit::OK)
            }

            IntentAction::Share => {
                self.pending_launch = Some(vec![
                    super::self_exe(),
                    "share".to_string(),
                    self.path.display().to_string(),
                ]);
                Outcome::Done(hayato_ui::app::exit::OK)
            }
        }
    }

    fn check_and_confirm_package(
        &mut self,
        package: &str,
        label: &str,
        weight: AppWeight,
        _post: Option<PostRunAction>,
        pending: PendingFileAction,
    ) -> Outcome {
        match pkg_guard::check_install(&[package]) {
            Ok(Verdict::Block(reasons)) => {
                self.enter_blocked(format!("Blocked installing {label}"), reasons);
                Outcome::Consumed
            }
            Ok(Verdict::Warn(mut reasons)) => {
                if weight == AppWeight::Heavy {
                    reasons.push("Note: heavy app on 4 GB devices.".to_string());
                }
                self.enter_confirm(format!("Install {label}?"), reasons, "Install", pending);
                Outcome::Consumed
            }
            Ok(Verdict::Allow) => {
                let mut lines = vec![format!(
                    "{label} will be installed and then open {}.",
                    self.summary.file_name
                )];
                if weight == AppWeight::Heavy {
                    lines.push("Note: heavy app on 4 GB devices.".to_string());
                }
                self.enter_confirm(format!("Install {label}?"), lines, "Install", pending);
                Outcome::Consumed
            }
            Err(e) => {
                self.banner = Some((format!("Could not check package: {e}"), true));
                Outcome::Consumed
            }
        }
    }

    fn enter_confirm(
        &mut self,
        title: String,
        body: Vec<String>,
        verb: &str,
        pending: PendingFileAction,
    ) {
        let message = body.first().cloned().unwrap_or_default();
        let detail = if body.len() > 1 {
            body[1..].join(" ")
        } else {
            String::new()
        };
        let confirm = Confirm::new(title, message)
            .with_detail(detail)
            .with_options(&["Cancel", verb]);
        let dummy = FileOpenPhase::Blocked {
            title: String::new(),
            lines: Vec::new(),
            list: ListState::new(),
            actions: Vec::new(),
        };
        if let FileOpenPhase::Intent { list, actions } = std::mem::replace(&mut self.phase, dummy) {
            self.phase = FileOpenPhase::ConfirmGuard {
                pending,
                confirm,
                list,
                actions,
            };
        }
        self.sync_chrome();
    }

    fn enter_blocked(&mut self, title: String, lines: Vec<String>) {
        let dummy = FileOpenPhase::Inspect {
            title: String::new(),
            log: LogPane::new(),
        };
        let (list, actions) = match std::mem::replace(&mut self.phase, dummy) {
            FileOpenPhase::Intent { list, actions }
            | FileOpenPhase::ConfirmGuard { list, actions, .. } => (list, actions),
            _ => (ListState::new(), Vec::new()),
        };
        self.phase = FileOpenPhase::Blocked {
            title,
            lines,
            list,
            actions,
        };
        self.sync_chrome();
    }

    fn return_to_intent(&mut self) {
        let dummy = FileOpenPhase::Inspect {
            title: String::new(),
            log: LogPane::new(),
        };
        match std::mem::replace(&mut self.phase, dummy) {
            FileOpenPhase::ConfirmGuard { list, actions, .. }
            | FileOpenPhase::Blocked { list, actions, .. } => {
                self.phase = FileOpenPhase::Intent { list, actions };
            }
            FileOpenPhase::Inspect { .. } => {
                // Reconstruct Intent rows deterministically from path.
                if let Ok(fresh) = Self::for_path(self.path.clone()) {
                    self.phase = fresh.phase;
                }
            }
            other => {
                self.phase = other;
            }
        }
        self.sync_chrome();
    }

    fn execute_pending(&mut self, pending: PendingFileAction) -> Outcome {
        match pending {
            PendingFileAction::Deb {
                pkg_display,
                deb_path,
            } => {
                let deb_str = deb_path.display().to_string();
                let cmd = pkg_guard::install_argv(&deb_str);
                let title = format!("Install {pkg_display}");
                let tag = format!("deb-{}", std::process::id());
                match Task::start_steps(
                    &tag,
                    title,
                    vec![format!("Install {pkg_display}")],
                    true,
                    vec![cmd],
                ) {
                    Ok(task) => {
                        self.phase = FileOpenPhase::Running {
                            post_action: None,
                            task: Box::new(TaskView::new(task)),
                        };
                        self.sync_chrome();
                    }
                    Err(e) => {
                        self.banner = Some((format!("Could not start installer: {e}"), true));
                        self.return_to_intent();
                    }
                }
                Outcome::Consumed
            }

            PendingFileAction::Package {
                package,
                label,
                post,
            } => {
                let cmd = pkg_guard::install_argv(&package);
                let title = format!("Install {label}");
                let tag = format!("openpkg-{}", std::process::id());
                match Task::start_steps(
                    &tag,
                    title,
                    vec![format!("Install {label} ({package})")],
                    true,
                    vec![cmd],
                ) {
                    Ok(task) => {
                        self.phase = FileOpenPhase::Running {
                            post_action: post,
                            task: Box::new(TaskView::new(task)),
                        };
                        self.sync_chrome();
                    }
                    Err(e) => {
                        self.banner = Some((format!("Could not start apt: {e}"), true));
                        self.return_to_intent();
                    }
                }
                Outcome::Consumed
            }

            PendingFileAction::ExtractorAndExtract { pkg, dest_dir, ext } => {
                let _ = std::fs::create_dir_all(&dest_dir);
                let install_cmd = pkg_guard::install_argv(&pkg);
                let extract_cmd = extract_command(&self.path, &dest_dir, &ext);
                let title = format!("Extract {}", self.summary.file_name);
                let tag = format!("extract-{}", std::process::id());
                match Task::start_steps(
                    &tag,
                    title,
                    vec![
                        format!("Install {pkg}"),
                        format!(
                            "Unpack into {}/",
                            dest_dir
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "folder".into())
                        ),
                    ],
                    true,
                    vec![install_cmd, extract_cmd],
                ) {
                    Ok(task) => {
                        self.phase = FileOpenPhase::Running {
                            post_action: Some(PostRunAction::OpenFolder(dest_dir)),
                            task: Box::new(TaskView::new(task)),
                        };
                        self.sync_chrome();
                    }
                    Err(e) => {
                        self.banner = Some((format!("Could not start extraction: {e}"), true));
                        self.return_to_intent();
                    }
                }
                Outcome::Consumed
            }
        }
    }

    fn start_extract(
        &mut self,
        dest_dir: PathBuf,
        ext: &str,
        _post: Option<PostRunAction>,
    ) -> Outcome {
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            self.banner = Some((format!("Cannot create folder: {e}"), true));
            return Outcome::Consumed;
        }
        let cmd = extract_command(&self.path, &dest_dir, ext);
        let dir_label = dest_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "extracted".into());
        let title = format!("Extract {}", self.summary.file_name);
        let tag = format!("unpack-{}", std::process::id());
        match Task::start_steps(
            &tag,
            title,
            vec![format!("Unpack into {dir_label}/")],
            true,
            vec![cmd],
        ) {
            Ok(task) => {
                self.phase = FileOpenPhase::Running {
                    post_action: Some(PostRunAction::OpenFolder(dest_dir)),
                    task: Box::new(TaskView::new(task)),
                };
                self.sync_chrome();
            }
            Err(e) => {
                self.banner = Some((format!("Could not start extractor: {e}"), true));
            }
        }
        Outcome::Consumed
    }

    fn mount_disk_image(&mut self) -> Outcome {
        let res = Command::new("udisksctl")
            .args(["loop-setup", "-r", "-f", &self.path.display().to_string()])
            .output();
        let Ok(out) = res else {
            self.banner = Some(("udisksctl could not run".into(), true));
            return Outcome::Consumed;
        };
        let txt = String::from_utf8_lossy(&out.stdout);
        let Some(pos) = txt.find("/dev/loop") else {
            let err = String::from_utf8_lossy(&out.stderr);
            self.banner = Some((format!("Could not attach image: {}", err.trim()), true));
            return Outcome::Consumed;
        };
        let loop_dev = txt[pos..].trim().trim_end_matches('.');
        let mnt = Command::new("udisksctl")
            .args(["mount", "-b", loop_dev])
            .output();
        if let Ok(mo) = mnt {
            let mtxt = String::from_utf8_lossy(&mo.stdout);
            if let Some(at) = mtxt.find(" at ") {
                let mp = mtxt[at + 4..].trim().trim_end_matches('.');
                self.pending_launch = Some(vec![
                    "hayato-files".to_string(),
                    "--".to_string(),
                    mp.to_string(),
                ]);
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
        }
        self.banner = Some(("Mounted loop device, check Places & Drives".into(), false));
        Outcome::Consumed
    }

    fn trigger_post_action(&mut self, post: PostRunAction) -> Outcome {
        match post {
            PostRunAction::OpenFolder(dir) => {
                self.pending_launch = Some(vec![
                    "hayato-files".to_string(),
                    "--".to_string(),
                    dir.display().to_string(),
                ]);
            }
            PostRunAction::OpenFile(p) => {
                self.pending_launch = Some(vec![
                    super::self_exe(),
                    "open".to_string(),
                    p.display().to_string(),
                ]);
            }
        }
        Outcome::Done(hayato_ui::app::exit::OK)
    }
}

impl Screen for FileOpenScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let design = ctx.design;
        match &mut self.phase {
            FileOpenPhase::Intent { list, .. } => {
                let dual = ctx.area.width >= DUAL_PANE_MIN_WIDTH;
                let (left, right) = if dual {
                    split_panes(ctx.area, 52, 42)
                } else {
                    (ctx.area, None)
                };

                let title = format!("Open \"{}\"", self.summary.file_name);
                draw_card(ctx.buf, left, design, Some(&title), true);
                let inner = Rect::new(
                    left.x + 2,
                    left.y + 1,
                    left.width.saturating_sub(4),
                    left.height.saturating_sub(2),
                );
                if inner.width >= 10 && inner.height >= 4 {
                    let mut y = inner.y;
                    let bold_name = design.style(TextStyle::Body).add_modifier(Modifier::BOLD);
                    let dim = design.style(TextStyle::Dim);
                    let loc_w = self.summary.location_line.chars().count() as u16;
                    let left_w = inner.width.saturating_sub(loc_w + 2);
                    put_trunc(
                        ctx.buf,
                        inner.x,
                        y,
                        &self.summary.file_name,
                        left_w,
                        bold_name,
                    );
                    if inner.width > loc_w + 4 {
                        put_trunc(
                            ctx.buf,
                            inner.right().saturating_sub(loc_w),
                            y,
                            &self.summary.location_line,
                            loc_w,
                            dim,
                        );
                    }
                    y += 1;
                    for line in self.summary.blurb.iter().take(2) {
                        if y < inner.bottom() {
                            put_trunc(ctx.buf, inner.x, y, line, inner.width, dim);
                            y += 1;
                        }
                    }
                    if let Some((msg, is_err)) = &self.banner
                        && y < inner.bottom()
                    {
                        let st = Style::default().fg(design.color(if *is_err {
                            ColorRole::Danger
                        } else {
                            ColorRole::Ok
                        }));
                        put_trunc(ctx.buf, inner.x, y, msg, inner.width, st);
                        y += 1;
                    }
                    if y < inner.bottom() {
                        let rule: String = "─".repeat(inner.width as usize);
                        put(ctx.buf, inner.x, y, &rule, inner.width, dim);
                        y += 1;
                    }
                    let list_rect =
                        Rect::new(inner.x, y, inner.width, inner.bottom().saturating_sub(y));
                    self.list_area.set(list_rect);
                    list.render(list_rect, ctx.buf, design, true);
                }

                if let Some(right_area) = right {
                    draw_card(
                        ctx.buf,
                        right_area,
                        design,
                        Some(&self.summary.inspector_title),
                        false,
                    );
                    let r_in = Rect::new(
                        right_area.x + 2,
                        right_area.y + 1,
                        right_area.width.saturating_sub(4),
                        right_area.height.saturating_sub(2),
                    );
                    if r_in.width >= 8 && r_in.height >= 2 {
                        draw_badge(
                            ctx.buf,
                            r_in.x,
                            r_in.y,
                            design,
                            &self.summary.badge,
                            self.summary.badge_role,
                        );
                        let body = design.style(TextStyle::Body);
                        let dim = design.style(TextStyle::Dim);
                        for (ry, line) in
                            (r_in.y + 2..r_in.bottom()).zip(self.summary.inspector_lines.iter())
                        {
                            let style = if line.starts_with("Location")
                                || line.starts_with("  ")
                                || line.starts_with("Safety")
                            {
                                dim
                            } else {
                                body
                            };
                            put_trunc(ctx.buf, r_in.x, ry, line, r_in.width, style);
                        }
                    }
                }
            }

            FileOpenPhase::Inspect { title, log } => {
                draw_card(ctx.buf, ctx.area, design, Some(title), true);
                let inner = Rect::new(
                    ctx.area.x + 2,
                    ctx.area.y + 1,
                    ctx.area.width.saturating_sub(4),
                    ctx.area.height.saturating_sub(2),
                );
                log.render(inner, ctx.buf, design);
            }

            FileOpenPhase::ConfirmGuard { confirm, .. } => {
                self.confirm_area.set(ctx.area);
                confirm.render(ctx.area, ctx.buf, design);
            }

            FileOpenPhase::Blocked { title, lines, .. } => {
                draw_card(ctx.buf, ctx.area, design, Some(title), true);
                let inner = Rect::new(
                    ctx.area.x + 2,
                    ctx.area.y + 1,
                    ctx.area.width.saturating_sub(4),
                    ctx.area.height.saturating_sub(2),
                );
                let err_style = Style::default().fg(design.color(ColorRole::Danger));
                for (y, line) in (inner.y..inner.bottom()).zip(lines.iter()) {
                    put_trunc(ctx.buf, inner.x, y, line, inner.width, err_style);
                }
            }

            FileOpenPhase::Running { task, .. } => {
                task.draw(ctx);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match &mut self.phase {
            FileOpenPhase::Intent { list, actions } => {
                if key.code == KeyCode::Esc {
                    return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                }
                match list.handle(key, self.list_area.get().height.max(1), design) {
                    ListAction::Activated(i) => {
                        if let Some(action) = actions.get(i).cloned() {
                            return self.run_action(action);
                        }
                        return Outcome::Consumed;
                    }
                    ListAction::Moved => return Outcome::Consumed,
                    ListAction::Ignored => {}
                }
                if key.modifiers == KeyModifiers::NONE {
                    match key.code {
                        KeyCode::Char('i') | KeyCode::Char('I') => {
                            if let Some(a) = actions
                                .iter()
                                .find(|a| matches!(a, IntentAction::InspectContents))
                                .cloned()
                            {
                                return self.run_action(a);
                            }
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') => {
                            return self.run_action(IntentAction::ShowInFiles);
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            if let Some(a) = actions
                                .iter()
                                .find(|a| matches!(a, IntentAction::OpenSoftwareCatalog { .. }))
                                .cloned()
                            {
                                return self.run_action(a);
                            }
                        }
                        _ => {}
                    }
                }
                Outcome::Continue
            }

            FileOpenPhase::Inspect { log, .. } => {
                if key.code == KeyCode::Esc {
                    self.return_to_intent();
                    return Outcome::Consumed;
                }
                if log.handle(key, 20) {
                    Outcome::Consumed
                } else {
                    Outcome::Continue
                }
            }

            FileOpenPhase::ConfirmGuard {
                confirm, pending, ..
            } => match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    let pending = pending.clone();
                    self.execute_pending(pending)
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.return_to_intent();
                    Outcome::Consumed
                }
                ConfirmAction::None => Outcome::Consumed,
            },

            FileOpenPhase::Blocked { .. } => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ')) {
                    self.return_to_intent();
                    Outcome::Consumed
                } else {
                    Outcome::Continue
                }
            }

            FileOpenPhase::Running { post_action, task } => {
                if task.task().state == TaskState::Done
                    && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
                    && let Some(post) = post_action.take()
                {
                    return self.trigger_post_action(post);
                }
                task.key(key, design)
            }
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        match &self.phase {
            FileOpenPhase::Intent { list, .. } => list.hit_row(self.list_area.get(), x, y, design),
            FileOpenPhase::ConfirmGuard { confirm, .. } => {
                let _ = area;
                confirm.hit(self.confirm_area.get(), design, x, y)
            }
            _ => None,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(idx) = row else {
            return Outcome::Continue;
        };
        match &mut self.phase {
            FileOpenPhase::Intent { actions, .. } => {
                if let Some(action) = actions.get(idx).cloned() {
                    return self.run_action(action);
                }
                Outcome::Continue
            }
            FileOpenPhase::ConfirmGuard { pending, .. } => {
                if idx == 1 {
                    let p = pending.clone();
                    self.execute_pending(p)
                } else {
                    self.return_to_intent();
                    Outcome::Consumed
                }
            }
            _ => Outcome::Continue,
        }
    }

    fn tick(&mut self) -> Outcome {
        if let FileOpenPhase::Running { task, .. } = &mut self.phase {
            let outcome = task.tick();
            self.sync_chrome();
            return outcome;
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
        Some(match &self.phase {
            FileOpenPhase::Intent { .. } => "open.intent",
            FileOpenPhase::Inspect { .. } => "open.inspect",
            FileOpenPhase::ConfirmGuard { .. } => "open.confirm",
            FileOpenPhase::Blocked { .. } => "open.blocked",
            FileOpenPhase::Running { .. } => "open.running",
        })
    }
}

pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--")
        .collect();
    let Some(&raw_path) = words.first() else {
        return Err(Failure::new("Open file", "usage: hayato menu open PATH"));
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
    let Some(path) = open::path_from_arg(raw_path, &cwd) else {
        return Err(Failure::new(
            "Open file",
            format!("invalid path: {raw_path}"),
        ));
    };
    let mut screen = FileOpenScreen::for_path(path).map_err(|e| Failure::new("Open file", e))?;
    let code = crate::run_screen(terminal, design, "Open", &mut screen)?;
    if let Some(argv) = screen.pending_launch.take() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let _ = crate::menu::spawn_detached(&argv, &home);
        return Ok(0);
    }
    Ok(code)
}

fn shorten_home(path: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let s = path.display().to_string();
    if !home.is_empty()
        && let Some(rest) = s.strip_prefix(&home)
    {
        if rest.is_empty() {
            "~".to_string()
        } else {
            format!("~{rest}")
        }
    } else {
        s
    }
}

fn host_dpkg_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

pub fn arch_compatible(pkg_arch: &str, host_arch: &str) -> bool {
    let a = pkg_arch.trim();
    a.is_empty() || a == "all" || a == host_arch
}

fn apt_deb_arg(path: &Path) -> String {
    if path.is_absolute() {
        path.display().to_string()
    } else {
        format!("./{}", path.display())
    }
}

pub fn inspect_deb(path: &Path) -> Option<DebMeta> {
    let out = Command::new("dpkg-deb")
        .args([
            "-f",
            &path.display().to_string(),
            "Package",
            "Version",
            "Architecture",
            "Installed-Size",
            "Description",
        ])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut package = String::new();
    let mut version = String::new();
    let mut arch = String::new();
    let mut installed_size_kb = None;
    let mut summary = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Package:") {
            package = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Version:") {
            version = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Architecture:") {
            arch = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Installed-Size:") {
            installed_size_kb = v.trim().parse::<u64>().ok();
        } else if let Some(v) = line.strip_prefix("Description:") {
            summary = v.trim().to_string();
        }
    }
    if package.is_empty() {
        return None;
    }
    Some(DebMeta {
        package,
        version,
        arch,
        installed_size_kb,
        summary,
    })
}

pub fn unique_extract_dir(parent: &Path, stem: &str) -> PathBuf {
    let clean = if stem.is_empty() { "Extracted" } else { stem };
    let first = parent.join(clean);
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let cand = parent.join(format!("{clean} ({n})"));
        if !cand.exists() {
            return cand;
        }
    }
    parent.join(format!("{clean}-{}", std::process::id()))
}

fn missing_extractor_for(ext: &str) -> Option<(&'static str, &'static str)> {
    let tools = System::from_env();
    use super::open::Tools as _;
    match ext {
        "7z" if !tools.has("7z") && !tools.has("7za") => Some(("7zip", "7-Zip")),
        "rar" if !tools.has("unrar") && !tools.has("unar") => Some(("unar", "Archive Extractor")),
        _ => None,
    }
}

fn extract_command(archive: &Path, dest: &Path, ext: &str) -> Vec<String> {
    let arc_s = archive.display().to_string();
    let dest_s = dest.display().to_string();
    match ext {
        "zip" => vec!["unzip".into(), "-o".into(), arc_s, "-d".into(), dest_s],
        "7z" => vec![
            "7z".into(),
            "x".into(),
            "-y".into(),
            format!("-o{dest_s}"),
            arc_s,
        ],
        "rar" => vec!["unar".into(), "-o".into(), dest_s, arc_s],
        _ => vec!["tar".into(), "-xvf".into(), arc_s, "-C".into(), dest_s],
    }
}

fn read_archive_listing(path: &Path) -> Vec<String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let p = path.display().to_string();
    let output = if name.ends_with(".deb") {
        Command::new("dpkg-deb").args(["-c", &p]).output()
    } else if name.ends_with(".zip") {
        Command::new("unzip").args(["-l", &p]).output()
    } else if name.ends_with(".7z") {
        Command::new("7z").args(["l", &p]).output()
    } else {
        Command::new("tar").args(["-tf", &p]).output()
    };
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .take(500)
            .map(|l| l.to_string())
            .collect(),
        Ok(o) => vec![format!(
            "Could not inspect contents: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )],
        Err(e) => vec![format!("Could not start inspector: {e}")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    fn render_buf(screen: &mut FileOpenScreen, w: u16, h: u16, tablet: bool) -> String {
        let mut design = Design::solarized();
        if tablet {
            design = design.with_density(hayato_ui::Density::Tablet);
        }
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let mut ctx = BodyCtx {
            area,
            buf: &mut buf,
            design: &design,
        };
        screen.draw(&mut ctx);
        let mut out = String::new();
        for y in 0..h {
            let mut row = String::new();
            for x in 0..w {
                row.push_str(buf[(x, y)].symbol());
            }
            out.push_str(row.trim_end());
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_all_intent_modes_at_compact_and_wide_grids() {
        let deb = DebMeta {
            package: "hayato-notes".into(),
            version: "0.4.2".into(),
            arch: host_dpkg_arch().to_string(),
            installed_size_kb: Some(14_200),
            summary: "Fast offline markdown notebook".into(),
        };
        let mut deb_screen = FileOpenScreen::from_intent(
            PathBuf::from("/tmp/hayato-notes_0.4.2_arm64.deb"),
            4_800_000,
            FileIntent::DebPackage,
            Some(deb),
        );
        assert_eq!(deb_screen.phase(), Some("open.intent"));
        let wide = render_buf(&mut deb_screen, 152, 45, false);
        assert!(wide.contains("Package Details"));
        assert!(wide.contains("Install this app"));
        let compact = render_buf(&mut deb_screen, 80, 24, true);
        assert!(compact.contains("Install this app"));

        let mut docx = FileOpenScreen::from_intent(
            PathBuf::from("/tmp/Report.docx"),
            420_000,
            FileIntent::MissingApp {
                kind_label: "office document",
                ext: "docx".into(),
                candidates: vec![AppCandidate {
                    package: "libreoffice-writer",
                    label: "LibreOffice Writer",
                }],
            },
            None,
        );
        let d_out = render_buf(&mut docx, 80, 24, false);
        assert!(d_out.contains("Install LibreOffice Writer & open"));

        let mut exe = FileOpenScreen::from_intent(
            PathBuf::from("/tmp/Setup.exe"),
            18_000_000,
            FileIntent::ForeignInstaller {
                platform: "Windows",
            },
            None,
        );
        let e_out = render_buf(&mut exe, 80, 24, false);
        assert!(e_out.contains("Made for Windows"));
        assert!(e_out.contains("Find an alternative in Software"));
    }

    #[test]
    fn unique_extract_dir_skips_collisions() {
        let dir = std::env::temp_dir().join(format!("hayato-openui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bundle")).unwrap();
        assert_eq!(unique_extract_dir(&dir, "bundle"), dir.join("bundle (2)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arch_compatibility_checks_arm64_and_all() {
        assert!(arch_compatible("arm64", "arm64"));
        assert!(arch_compatible("all", "arm64"));
        assert!(!arch_compatible("amd64", "arm64"));
    }
}
