//! Hayato Software (`hayato menu software`): intent-first application catalog on native APT.
//!
//! Combines curated task discovery ("Edit videos", "Program in Python", "Office & documents"),
//! debounced Debian package fallback, installed app/package management, updates tracking,
//! safe cleanup (reusing `system::clean` + `apt autoremove`), and mandatory pre-flight simulation
//! through `system::pkg_guard` (`--no-install-recommends`, kernel/boot/session protection).

pub mod apt;
pub mod catalog;
pub mod meta_cache;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use hayato_ui::widgets::{Confirm, ConfirmAction, Input, InputAction, put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, Task, TaskView, TextStyle, draw_badge, draw_card_with,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use self::apt::{
    AccessFacts, CleanupSnapshot, DebianMatch, InstalledPkg, PackageMeta, TransactionPreview,
    UpdateEntry, format_bytes, format_kb,
};
use self::catalog::{AppWeight, Catalog, CatalogApp};
use super::pkg_guard::{self, Verdict};
use super::{missing, spawn_detached};
use crate::Failure;

pub const NEEDED: &[&str] = &["apt-cache", "apt-get", "dpkg-query", "sudo"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Discover,
    Installed,
    Updates,
    Cleanup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRef {
    Catalog(usize),
    Debian { package: String, summary: String },
}

impl AppRef {
    fn package_name<'a>(&'a self, cat: &'a Catalog) -> &'a str {
        match self {
            AppRef::Catalog(i) => cat.apps[*i].package.as_str(),
            AppRef::Debian { package, .. } => package.as_str(),
        }
    }

    fn display_name<'a>(&'a self, cat: &'a Catalog) -> &'a str {
        match self {
            AppRef::Catalog(i) => cat.apps[*i].name.as_str(),
            AppRef::Debian { package, .. } => package.as_str(),
        }
    }
}

#[derive(Clone, Debug)]
enum PendingAction {
    Install(String),
    Remove(String),
    UpdatePackages(Vec<String>),
    UpdatePackageLists,
    RunCleanup {
        autoremove: bool,
        apt_cache: bool,
        residual: Vec<String>,
        trash_thumbs: bool,
    },
}

enum ConfirmKind {
    Ask {
        action: PendingAction,
        confirm: Confirm,
        return_to: Box<Phase>,
    },
    Blocked {
        title: String,
        lines: Vec<String>,
        return_to: Box<Phase>,
    },
}

enum Phase {
    Tab(Tab),
    Category {
        intent_idx: usize,
    },
    Results {
        query: String,
    },
    Detail {
        app: AppRef,
        from: Box<Phase>,
    },
    Confirm(Box<ConfirmKind>),
    Running {
        task: Box<TaskView>,
        return_to: Box<Phase>,
    },
}

struct DetailPayload {
    package: String,
    meta: Option<PackageMeta>,
    access: AccessFacts,
    preview: Option<TransactionPreview>,
}

enum BgMsg {
    InitialUpdates(Vec<UpdateEntry>),
    BatchMeta(HashMap<String, PackageMeta>),
    DetailReady(Box<DetailPayload>),
    DebianSearch {
        query: String,
        matches: Vec<DebianMatch>,
    },
    CleanupReady(CleanupSnapshot),
}

#[derive(Clone)]
enum RowTag {
    Heading,
    Intent(usize),
    App(AppRef),
    UpdatePkg(usize),
    CleanItem(CleanKind),
    CleanRun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanKind {
    Autoremove,
    AptCache,
    Residual,
    TrashThumbs,
    OldKernelsNote,
}

#[derive(Default)]
struct HitBoxes {
    tabs: [(Tab, Rect); 4],
    search_box: Rect,
    advanced_btn: Rect,
    list_area: Rect,
    confirm_area: Rect,
    detail_buttons: Vec<(DetailBtn, Rect)>,
    update_all_btn: Rect,
    refresh_btn: Rect,
    clean_run_btn: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailBtn {
    Install,
    Update,
    Open,
    Remove,
    Back,
}

pub struct SoftwareScreen {
    catalog: Catalog,
    installed: HashMap<String, InstalledPkg>,
    manual: HashSet<String>,
    meta: HashMap<String, PackageMeta>,
    updates: Vec<UpdateEntry>,
    cleanup: Option<CleanupSnapshot>,
    clean_sel: [bool; 4], // [autoremove, apt_cache, residual, trash_thumbs]
    detail_access: HashMap<String, AccessFacts>,
    detail_preview: HashMap<String, TransactionPreview>,
    debian_results: Vec<DebianMatch>,
    searching_debian: bool,

    phase: Phase,
    search_input: Input,
    search_focused: bool,
    cursor_blink_on: bool,
    last_blink: Instant,

    list: ListState,
    row_tags: Vec<RowTag>,
    rows_height: Cell<u16>,
    banner: Option<String>,

    header: Header,
    keybar: Vec<KeyHint>,
    hits: RefCell<HitBoxes>,
    bg_rx: Option<Receiver<BgMsg>>,
    bg_tx: mpsc::Sender<BgMsg>,
    pub open_advanced_pkg: bool,
}

impl SoftwareScreen {
    pub fn new(initial_tab: Tab, initial_query: Option<&str>) -> Self {
        let catalog = Catalog::load();
        let installed = apt::dpkg_installed();
        let manual = apt::manual_packages();
        let (bg_tx, bg_rx) = mpsc::channel();

        let mut screen = Self {
            catalog,
            installed,
            manual,
            meta: HashMap::new(),
            updates: Vec::new(),
            cleanup: None,
            clean_sel: [true, true, false, false],
            detail_access: HashMap::new(),
            detail_preview: HashMap::new(),
            debian_results: Vec::new(),
            searching_debian: false,
            phase: Phase::Tab(initial_tab),
            search_input: Input::new(),
            search_focused: false,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            list: ListState::new(),
            row_tags: Vec::new(),
            rows_height: Cell::new(14),
            banner: None,
            header: Header::new("Hayato").subtitle("Software"),
            keybar: Vec::new(),
            hits: RefCell::new(HitBoxes::default()),
            bg_rx: Some(bg_rx),
            bg_tx,
            open_advanced_pkg: false,
        };

        if let Some(q) = initial_query.filter(|s| !s.trim().is_empty()) {
            screen.search_input.set(q.trim());
            screen.trigger_search(q.trim());
        } else {
            screen.rebuild_list();
            if initial_tab == Tab::Cleanup {
                screen.spawn_cleanup_scan();
            }
        }
        screen.spawn_initial_probes();
        screen
    }

    fn spawn_initial_probes(&self) {
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let ups = apt::fetch_upgradable();
            let _ = tx.send(BgMsg::InitialUpdates(ups));
        });
    }

    fn spawn_batch_meta(&self, pkgs: Vec<String>) {
        if pkgs.is_empty() {
            return;
        }
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let refs: Vec<&str> = pkgs.iter().map(String::as_str).collect();
            let map = apt::fetch_metadata_batch(&refs);
            let _ = tx.send(BgMsg::BatchMeta(map));
        });
    }

    fn spawn_detail_probe(&self, pkg: String, is_installed: bool, simulate: bool) {
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let meta = apt::fetch_metadata_batch(&[pkg.as_str()]).remove(&pkg);
            let access = apt::probe_access_facts(&pkg, is_installed);
            let preview = if simulate {
                Some(apt::simulate_transaction(&pkg, false))
            } else {
                None
            };
            let _ = tx.send(BgMsg::DetailReady(Box::new(DetailPayload {
                package: pkg,
                meta,
                access,
                preview,
            })));
        });
    }

    fn spawn_cleanup_scan(&self) {
        let tx = self.bg_tx.clone();
        let installed = self.installed.clone();
        std::thread::spawn(move || {
            let snap = apt::measure_cleanup(&installed);
            let _ = tx.send(BgMsg::CleanupReady(snap));
        });
    }

    fn spawn_debian_search(&mut self, query: String) {
        self.searching_debian = true;
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let matches = apt::search_debian_names(&query, 24);
            let _ = tx.send(BgMsg::DebianSearch { query, matches });
        });
    }

    fn poll_bg(&mut self) -> bool {
        let mut changed = false;
        let mut drained = Vec::new();
        if let Some(rx) = &self.bg_rx {
            while let Ok(msg) = rx.try_recv() {
                drained.push(msg);
            }
        }
        for msg in drained {
            changed = true;
            match msg {
                BgMsg::InitialUpdates(ups) => {
                    self.updates = ups;
                    self.rebuild_list();
                }
                BgMsg::BatchMeta(map) => {
                    self.meta.extend(map);
                    self.rebuild_list();
                }
                BgMsg::DetailReady(payload) => {
                    let DetailPayload {
                        package,
                        meta,
                        access,
                        preview,
                    } = *payload;
                    if let Some(m) = meta {
                        self.meta.insert(package.clone(), m);
                    }
                    self.detail_access.insert(package.clone(), access);
                    if let Some(p) = preview {
                        self.detail_preview.insert(package, p);
                    }
                }
                BgMsg::DebianSearch { query, matches } => {
                    if let Phase::Results { query: cur } = &self.phase
                        && *cur == query
                    {
                        self.searching_debian = false;
                        self.debian_results = matches;
                        self.rebuild_list();
                    }
                }
                BgMsg::CleanupReady(snap) => {
                    self.cleanup = Some(snap);
                    if matches!(self.phase, Phase::Tab(Tab::Cleanup)) {
                        self.rebuild_list();
                    }
                }
            }
        }
        changed
    }

    fn reload_local_packages(&mut self) {
        self.installed = apt::dpkg_installed();
        self.manual = apt::manual_packages();
        self.detail_access.clear();
        self.detail_preview.clear();
        self.spawn_initial_probes();
        if matches!(self.phase, Phase::Tab(Tab::Cleanup)) {
            self.cleanup = None;
            self.spawn_cleanup_scan();
        }
        self.rebuild_list();
    }

    // ─── Deterministic Snapshot Fixtures ─────────────────────────────────────

    fn snapshot_base() -> Self {
        let catalog = Catalog::load();
        let mut installed = HashMap::new();
        installed.insert(
            "shotcut".to_string(),
            InstalledPkg {
                name: "shotcut".to_string(),
                version: "26.11.15+ds-1".to_string(),
                installed_kb: 38_400,
            },
        );
        installed.insert(
            "firefox-esr".to_string(),
            InstalledPkg {
                name: "firefox-esr".to_string(),
                version: "153.5.0esr-1".to_string(),
                installed_kb: 215_040,
            },
        );
        installed.insert(
            "btop".to_string(),
            InstalledPkg {
                name: "btop".to_string(),
                version: "1.4.7-1".to_string(),
                installed_kb: 1_842,
            },
        );
        let manual = ["shotcut", "firefox-esr", "btop"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut meta = HashMap::new();
        meta.insert(
            "kdenlive".to_string(),
            PackageMeta {
                package: "kdenlive".to_string(),
                candidate: "26.08.3-1".to_string(),
                section: "video".to_string(),
                download_bytes: 4_718_592,
                installed_kb: 21_196,
                homepage: "https://kdenlive.org/".to_string(),
                origin: "http://deb.debian.org/debian forky/main".to_string(),
                summary: "non-linear video editor".to_string(),
                description: vec![
                    "Kdenlive is a multi-track non-linear video editor for Linux.".to_string(),
                    "Supports proxy editing, keyframes, titles and audio mixing.".to_string(),
                ],
            },
        );
        meta.insert(
            "shotcut".to_string(),
            PackageMeta {
                package: "shotcut".to_string(),
                candidate: "26.11.15+ds-1".to_string(),
                section: "video".to_string(),
                download_bytes: 3_200_000,
                installed_kb: 38_400,
                homepage: "https://shotcut.org/".to_string(),
                origin: "http://deb.debian.org/debian forky/main".to_string(),
                summary: "cross-platform video editor".to_string(),
                description: vec![
                    "Streamlined timeline editor with wide format support.".to_string(),
                ],
            },
        );
        let updates = vec![
            UpdateEntry {
                package: "openssl".to_string(),
                suite: "forky-security".to_string(),
                old_version: "3.5.1-1".to_string(),
                new_version: "3.5.2-1".to_string(),
                is_security: true,
                held_for_safety: false,
            },
            UpdateEntry {
                package: "firefox-esr".to_string(),
                suite: "forky".to_string(),
                old_version: "153.5.0esr-1".to_string(),
                new_version: "153.5.1esr-1".to_string(),
                is_security: false,
                held_for_safety: false,
            },
            UpdateEntry {
                package: "linux-image-arm64".to_string(),
                suite: "forky".to_string(),
                old_version: "7.2.7-1".to_string(),
                new_version: "7.3.5-1".to_string(),
                is_security: false,
                held_for_safety: true,
            },
        ];
        let mut detail_access = HashMap::new();
        detail_access.insert(
            "kdenlive".to_string(),
            AccessFacts {
                installed: false,
                services: Vec::new(),
                conffiles_count: 0,
                has_setuid: false,
            },
        );
        let mut detail_preview = HashMap::new();
        detail_preview.insert(
            "kdenlive".to_string(),
            TransactionPreview {
                package: "kdenlive".to_string(),
                remove: false,
                plan: pkg_guard::Plan {
                    installs: vec![
                        "kdenlive".to_string(),
                        "kdenlive-data".to_string(),
                        "melt".to_string(),
                    ],
                    upgrades: Vec::new(),
                    removes: Vec::new(),
                },
                verdict: Verdict::Allow,
                net_disk_kb: 88_064,
                blocked_or_err: None,
            },
        );
        let cleanup = Some(CleanupSnapshot {
            reclaimable: crate::system::clean::Reclaimable {
                trash_bytes: 24 * 1024 * 1024,
                thumb_bytes: 18 * 1024 * 1024,
                apt_bytes: 310 * 1024 * 1024,
                journal_bytes: 140 * 1024 * 1024,
                cache_bytes: 90 * 1024 * 1024,
                free_str: "14G".to_string(),
                total_str: "58G".to_string(),
                used_ratio: 75,
            },
            autoremove_pkgs: vec!["libfoo1".to_string(), "libbar2".to_string()],
            autoremove_kb: 145_408,
            residual_configs: vec!["old-app".to_string()],
            old_kernel_count: 1,
        });
        let (bg_tx, _bg_rx) = mpsc::channel();
        Self {
            catalog,
            installed,
            manual,
            meta,
            updates,
            cleanup,
            clean_sel: [true, true, false, false],
            detail_access,
            detail_preview,
            debian_results: Vec::new(),
            searching_debian: false,
            phase: Phase::Tab(Tab::Discover),
            search_input: Input::new(),
            search_focused: false,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            list: ListState::new(),
            row_tags: Vec::new(),
            rows_height: Cell::new(14),
            banner: None,
            header: Header::new("Hayato").subtitle("Software"),
            keybar: Vec::new(),
            hits: RefCell::new(HitBoxes::default()),
            bg_rx: None,
            bg_tx,
            open_advanced_pkg: false,
        }
    }

    pub fn for_snapshot_discover() -> Self {
        let mut s = Self::snapshot_base();
        s.rebuild_list();
        s
    }

    pub fn for_snapshot_category() -> Self {
        let mut s = Self::snapshot_base();
        s.phase = Phase::Category { intent_idx: 0 };
        s.rebuild_list();
        s
    }

    pub fn for_snapshot_detail() -> Self {
        let mut s = Self::snapshot_base();
        s.phase = Phase::Detail {
            app: AppRef::Catalog(0),
            from: Box::new(Phase::Category { intent_idx: 0 }),
        };
        s.rebuild_list();
        s
    }

    pub fn for_snapshot_updates() -> Self {
        let mut s = Self::snapshot_base();
        s.phase = Phase::Tab(Tab::Updates);
        s.rebuild_list();
        s
    }

    pub fn for_snapshot_cleanup() -> Self {
        let mut s = Self::snapshot_base();
        s.phase = Phase::Tab(Tab::Cleanup);
        s.rebuild_list();
        s
    }

    // ─── Row Construction & State Sync ───────────────────────────────────────

    fn update_badge_value(&self, pkg: &str, weight: Option<AppWeight>) -> String {
        if self.updates.iter().any(|u| u.package == pkg) {
            return "\u{2191} update".to_string();
        }
        if self.installed.contains_key(pkg) {
            return "\u{25cf} installed".to_string();
        }
        if let Some(m) = self.meta.get(pkg)
            && m.download_bytes > 0
        {
            return format_bytes(m.download_bytes);
        }
        weight.map(|w| w.short().to_string()).unwrap_or_default()
    }

    fn rebuild_list(&mut self) {
        let mut rows = Vec::new();
        let mut tags = Vec::new();

        match &self.phase {
            Phase::Tab(Tab::Discover) => {
                for (idx, intent) in self.catalog.intents.iter().enumerate() {
                    let apps = self.catalog.apps_for_intent(idx);
                    let names: Vec<&str> =
                        apps.iter().take(3).map(|(_, a)| a.name.as_str()).collect();
                    let installed_cnt = apps
                        .iter()
                        .filter(|(_, a)| self.installed.contains_key(&a.package))
                        .count();
                    let val = if installed_cnt > 0 {
                        format!("{installed_cnt}/{} on disk", apps.len())
                    } else {
                        format!("{} apps", apps.len())
                    };
                    rows.push(
                        Row::new(&intent.title)
                            .detail(names.join(", "))
                            .glyph(intent.ui_glyph())
                            .value(val),
                    );
                    tags.push(RowTag::Intent(idx));
                }
            }
            Phase::Category { intent_idx } => {
                for (app_idx, app) in self.catalog.apps_for_intent(*intent_idx) {
                    let is_inst = self.installed.contains_key(&app.package);
                    let has_upd = self.updates.iter().any(|u| u.package == app.package);
                    let glyph = if has_upd {
                        Glyph::Sync
                    } else if is_inst {
                        Glyph::Check
                    } else {
                        Glyph::Box
                    };
                    let val = self.update_badge_value(&app.package, Some(app.weight));
                    let detail = format!("{} · {}", app.weight.label(), app.summary);
                    rows.push(Row::new(&app.name).detail(detail).glyph(glyph).value(val));
                    tags.push(RowTag::App(AppRef::Catalog(app_idx)));
                }
            }
            Phase::Results { query } => {
                let res = self.catalog.search(query);
                if !res.intents.is_empty() {
                    rows.push(Row::heading("Matching Tasks"));
                    tags.push(RowTag::Heading);
                    for ri in res.intents.iter().take(4) {
                        let intent = &self.catalog.intents[ri.intent_idx];
                        rows.push(
                            Row::new(&intent.title)
                                .detail(&intent.blurb)
                                .glyph(intent.ui_glyph())
                                .value(format!("{} apps", intent.apps.len())),
                        );
                        tags.push(RowTag::Intent(ri.intent_idx));
                    }
                }
                if !res.apps.is_empty() {
                    rows.push(Row::heading("In Curated Catalog"));
                    tags.push(RowTag::Heading);
                    for ra in &res.apps {
                        let app = &self.catalog.apps[ra.app_idx];
                        let is_inst = self.installed.contains_key(&app.package);
                        let val = self.update_badge_value(&app.package, Some(app.weight));
                        rows.push(
                            Row::new(&app.name)
                                .detail(&app.summary)
                                .glyph(if is_inst { Glyph::Check } else { Glyph::Box })
                                .value(val),
                        );
                        tags.push(RowTag::App(AppRef::Catalog(ra.app_idx)));
                    }
                }
                if self.searching_debian {
                    rows.push(Row::heading("Searching Debian APT…"));
                    tags.push(RowTag::Heading);
                } else if !self.debian_results.is_empty() {
                    rows.push(Row::heading("More from Debian APT"));
                    tags.push(RowTag::Heading);
                    for dm in &self.debian_results {
                        if self.catalog.app_by_package(&dm.package).is_some() {
                            continue;
                        }
                        let is_inst = self.installed.contains_key(&dm.package);
                        rows.push(
                            Row::new(&dm.package)
                                .detail(&dm.summary)
                                .glyph(if is_inst { Glyph::Check } else { Glyph::Debian })
                                .value(if is_inst { "installed" } else { "Debian" }),
                        );
                        tags.push(RowTag::App(AppRef::Debian {
                            package: dm.package.clone(),
                            summary: dm.summary.clone(),
                        }));
                    }
                }
            }
            Phase::Tab(Tab::Installed) => {
                // Curated apps first, then remaining manually-installed packages.
                let mut added_pkgs = HashSet::new();
                rows.push(Row::heading("Installed Applications"));
                tags.push(RowTag::Heading);
                for (app_idx, app) in self.catalog.apps.iter().enumerate() {
                    if let Some(inst) = self.installed.get(&app.package) {
                        added_pkgs.insert(app.package.clone());
                        let val = format!("{} · {}", inst.version, format_kb(inst.installed_kb));
                        rows.push(
                            Row::new(&app.name)
                                .detail(&app.summary)
                                .glyph(Glyph::Check)
                                .value(val),
                        );
                        tags.push(RowTag::App(AppRef::Catalog(app_idx)));
                    }
                }
                let mut manual_rest: Vec<&InstalledPkg> = self
                    .installed
                    .values()
                    .filter(|p| {
                        self.manual.contains(&p.name)
                            && !added_pkgs.contains(&p.name)
                            && !pkg_guard::is_kernel_or_boot(&p.name)
                    })
                    .collect();
                manual_rest.sort_by(|a, b| a.name.cmp(&b.name));
                if !manual_rest.is_empty() {
                    rows.push(Row::heading("Manual System Packages"));
                    tags.push(RowTag::Heading);
                    for p in manual_rest.into_iter().take(120) {
                        let val = format!("{} · {}", p.version, format_kb(p.installed_kb));
                        rows.push(
                            Row::new(&p.name)
                                .detail("Debian manual package")
                                .glyph(Glyph::Box)
                                .value(val),
                        );
                        tags.push(RowTag::App(AppRef::Debian {
                            package: p.name.clone(),
                            summary: format!("Installed version {}", p.version),
                        }));
                    }
                }
            }
            Phase::Tab(Tab::Updates) => {
                for (i, u) in self.updates.iter().enumerate() {
                    let glyph = if u.held_for_safety {
                        Glyph::Lock
                    } else if u.is_security {
                        Glyph::Shield
                    } else {
                        Glyph::Sync
                    };
                    let val = if u.held_for_safety {
                        "held (hayato update)".to_string()
                    } else if u.is_security {
                        "security".to_string()
                    } else {
                        u.suite.clone()
                    };
                    rows.push(
                        Row::new(&u.package)
                            .detail(format!("{} \u{2192} {}", u.old_version, u.new_version))
                            .glyph(glyph)
                            .value(val),
                    );
                    tags.push(RowTag::UpdatePkg(i));
                }
            }
            Phase::Tab(Tab::Cleanup) => {
                if let Some(snap) = &self.cleanup {
                    let box_mark = |on: bool| if on { "[x]" } else { "[ ]" };
                    rows.push(
                        Row::new(format!(
                            "{} Unused dependencies (autoremove)",
                            box_mark(self.clean_sel[0])
                        ))
                        .detail(format!(
                            "{} packages no longer needed",
                            snap.autoremove_pkgs.len()
                        ))
                        .glyph(Glyph::Box)
                        .value(format_kb(snap.autoremove_kb)),
                    );
                    tags.push(RowTag::CleanItem(CleanKind::Autoremove));

                    rows.push(
                        Row::new(format!(
                            "{} Downloaded .deb cache",
                            box_mark(self.clean_sel[1])
                        ))
                        .detail("/var/cache/apt/archives")
                        .glyph(Glyph::Disk)
                        .value(format_bytes(snap.reclaimable.apt_bytes)),
                    );
                    tags.push(RowTag::CleanItem(CleanKind::AptCache));

                    rows.push(
                        Row::new(format!(
                            "{} Orphaned configs (rc state)",
                            box_mark(self.clean_sel[2])
                        ))
                        .detail(format!(
                            "{} removed package configs left on disk",
                            snap.residual_configs.len()
                        ))
                        .glyph(Glyph::Trash)
                        .value(format!("{} pkgs", snap.residual_configs.len())),
                    );
                    tags.push(RowTag::CleanItem(CleanKind::Residual));

                    let tt_bytes = snap
                        .reclaimable
                        .trash_bytes
                        .saturating_add(snap.reclaimable.thumb_bytes);
                    rows.push(
                        Row::new(format!(
                            "{} Trash & thumbnail caches",
                            box_mark(self.clean_sel[3])
                        ))
                        .detail("~/.local/share/Trash + ~/.cache/thumbnails")
                        .glyph(Glyph::Image)
                        .value(format_bytes(tt_bytes)),
                    );
                    tags.push(RowTag::CleanItem(CleanKind::TrashThumbs));

                    if snap.old_kernel_count > 0 {
                        rows.push(
                            Row::new(format!(
                                "! Old kernels: {} extra image(s)",
                                snap.old_kernel_count
                            ))
                            .detail("Protected in Software — manage via `hayato update`")
                            .glyph(Glyph::Warning)
                            .value("info only"),
                        );
                        tags.push(RowTag::CleanItem(CleanKind::OldKernelsNote));
                    }

                    rows.push(
                        Row::new("Clean selected items")
                            .detail("Runs autoremove, apt clean, or config purge safely")
                            .glyph(Glyph::Sparkles)
                            .value("Enter / c"),
                    );
                    tags.push(RowTag::CleanRun);
                } else {
                    rows.push(
                        Row::new("Measuring reclaimable space…")
                            .detail("Scanning apt cache, autoremove, trash & thumbnails")
                            .glyph(Glyph::Sync),
                    );
                    tags.push(RowTag::Heading);
                }
            }
            Phase::Detail { .. } | Phase::Confirm(_) | Phase::Running { .. } => {}
        }

        self.row_tags = tags;
        self.list.set_rows(rows);
        self.sync_chrome();
    }

    fn sync_chrome(&mut self) {
        let sec_count = self.updates.iter().filter(|u| u.is_security).count();
        let status = match &self.phase {
            Phase::Tab(Tab::Discover) => {
                if !self.updates.is_empty() {
                    format!(
                        "{} installed · {} updates ({sec_count} security)",
                        self.installed.len(),
                        self.updates.len()
                    )
                } else {
                    format!(
                        "{} packages installed · Source: Debian",
                        self.installed.len()
                    )
                }
            }
            Phase::Category { intent_idx } => self.catalog.intents[*intent_idx].title.clone(),
            Phase::Results { query } => format!("Search: \"{query}\""),
            Phase::Detail { app, .. } => app.display_name(&self.catalog).to_string(),
            Phase::Tab(Tab::Installed) => format!("{} installed", self.installed.len()),
            Phase::Tab(Tab::Updates) => {
                format!("{} upgradable ({sec_count} security)", self.updates.len())
            }
            Phase::Tab(Tab::Cleanup) => self
                .cleanup
                .as_ref()
                .map(|c| format!("{} reclaimable", format_bytes(c.total_reclaimable_bytes())))
                .unwrap_or_else(|| "Scanning…".to_string()),
            Phase::Confirm(k) => match &**k {
                ConfirmKind::Ask { .. } => "Confirm".to_string(),
                ConfirmKind::Blocked { .. } => "Blocked for safety".to_string(),
            },
            Phase::Running { .. } => "Running…".to_string(),
        };

        self.header = Header::new("Hayato").subtitle("Software").status(status);
        self.keybar = match &self.phase {
            Phase::Tab(Tab::Discover) | Phase::Category { .. } | Phase::Results { .. } => vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("/", "search"),
                KeyHint::new("1-4/Tab", "view"),
                KeyHint::new("a", "all apt"),
                KeyHint::new("Esc", "back"),
            ],
            Phase::Tab(Tab::Installed) => vec![
                KeyHint::new("Enter", "detail"),
                KeyHint::new("o", "open"),
                KeyHint::new("r", "remove"),
                KeyHint::new("1-4/Tab", "view"),
                KeyHint::new("Esc", "back"),
            ],
            Phase::Tab(Tab::Updates) => vec![
                KeyHint::new("u", "update one"),
                KeyHint::new("U", "update all"),
                KeyHint::new("Ctrl+U", "refresh lists"),
                KeyHint::new("1-4/Tab", "view"),
                KeyHint::new("Esc", "back"),
            ],
            Phase::Tab(Tab::Cleanup) => vec![
                KeyHint::new("Space", "toggle"),
                KeyHint::new("c/Enter", "clean"),
                KeyHint::new("1-4/Tab", "view"),
                KeyHint::new("Esc", "back"),
            ],
            Phase::Detail { .. } => vec![
                KeyHint::new("i", "install"),
                KeyHint::new("u", "update"),
                KeyHint::new("o", "open"),
                KeyHint::new("r", "remove"),
                KeyHint::new("Esc", "back"),
            ],
            Phase::Confirm(k) => match &**k {
                ConfirmKind::Ask { .. } => vec![
                    KeyHint::new("\u{2190}/\u{2192}", "choose"),
                    KeyHint::new("Enter", "confirm"),
                    KeyHint::new("Esc", "cancel"),
                ],
                ConfirmKind::Blocked { .. } => vec![KeyHint::new("Esc", "back")],
            },
            Phase::Running { .. } => vec![KeyHint::new("Esc", "back when done")],
        };
    }

    fn selected_tag(&self) -> Option<&RowTag> {
        self.list
            .selected_index()
            .and_then(|i| self.row_tags.get(i))
    }

    fn open_category(&mut self, intent_idx: usize) {
        self.phase = Phase::Category { intent_idx };
        let pkgs: Vec<String> = self
            .catalog
            .apps_for_intent(intent_idx)
            .into_iter()
            .filter(|(_, a)| !self.meta.contains_key(&a.package))
            .map(|(_, a)| a.package.clone())
            .collect();
        self.spawn_batch_meta(pkgs);
        self.rebuild_list();
    }

    fn trigger_search(&mut self, query: &str) {
        let q = query.trim().to_string();
        if q.is_empty() {
            self.phase = Phase::Tab(Tab::Discover);
            self.debian_results.clear();
            self.rebuild_list();
            return;
        }
        let res = self.catalog.search(&q);
        self.debian_results.clear();
        self.phase = Phase::Results { query: q.clone() };
        if res.apps.len() < 3 {
            self.spawn_debian_search(q);
        }
        self.rebuild_list();
    }

    fn open_detail(&mut self, app: AppRef) {
        let pkg = app.package_name(&self.catalog).to_string();
        let is_inst = self.installed.contains_key(&pkg);
        let return_to = std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover));
        self.phase = Phase::Detail {
            app,
            from: Box::new(return_to),
        };
        self.spawn_detail_probe(pkg, is_inst, !is_inst);
        self.sync_chrome();
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.search_focused = false;
        self.phase = Phase::Tab(tab);
        if tab == Tab::Cleanup && self.cleanup.is_none() {
            self.spawn_cleanup_scan();
        }
        self.rebuild_list();
    }

    fn cycle_tab(&mut self, forward: bool) {
        let tabs = [Tab::Discover, Tab::Installed, Tab::Updates, Tab::Cleanup];
        let cur = match &self.phase {
            Phase::Tab(t) => *t,
            Phase::Category { .. } | Phase::Results { .. } => Tab::Discover,
            _ => return,
        };
        let i = tabs.iter().position(|t| *t == cur).unwrap_or(0);
        let next = if forward {
            tabs[(i + 1) % tabs.len()]
        } else {
            tabs[(i + tabs.len() - 1) % tabs.len()]
        };
        self.switch_tab(next);
    }

    fn propose_transaction(&mut self, action: PendingAction) {
        let return_to = Box::new(std::mem::replace(
            &mut self.phase,
            Phase::Tab(Tab::Discover),
        ));
        match &action {
            PendingAction::Install(pkg) => {
                let preview = apt::simulate_transaction(pkg, false);
                match preview.verdict {
                    Verdict::Block(lines) => {
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Blocked {
                            title: format!("Install {pkg}: blocked for safety"),
                            lines,
                            return_to,
                        }));
                    }
                    Verdict::Warn(lines) => {
                        let detail = format!(
                            "{}\nAdds {} package(s) · {}",
                            lines.join("\n"),
                            preview.plan.installs.len(),
                            format_kb(preview.net_disk_kb.max(0) as u64)
                        );
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Ask {
                            confirm: Confirm::new(format!("Install {pkg}?"), detail)
                                .with_options(&["Cancel", "Install"]),
                            action,
                            return_to,
                        }));
                    }
                    Verdict::Allow => {
                        let summary = if let Some(err) = preview.blocked_or_err {
                            format!("Simulation note: {err}")
                        } else {
                            format!(
                                "Adds {} package(s) · ~{} disk · --no-install-recommends",
                                preview.plan.installs.len(),
                                format_kb(preview.net_disk_kb.max(0) as u64)
                            )
                        };
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Ask {
                            confirm: Confirm::new(format!("Install {pkg}?"), summary)
                                .with_options(&["Cancel", "Install"]),
                            action,
                            return_to,
                        }));
                    }
                }
            }
            PendingAction::Remove(pkg) => {
                let preview = apt::simulate_transaction(pkg, true);
                match preview.verdict {
                    Verdict::Block(lines) => {
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Blocked {
                            title: format!("Remove {pkg}: blocked for safety"),
                            lines,
                            return_to,
                        }));
                    }
                    Verdict::Warn(lines) => {
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Ask {
                            confirm: Confirm::new(format!("Remove {pkg}?"), lines.join("\n"))
                                .with_options(&["Cancel", "Remove"])
                                .destructive(true),
                            action,
                            return_to,
                        }));
                    }
                    Verdict::Allow => {
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Ask {
                            confirm: Confirm::new(
                                format!("Remove {pkg}?"),
                                format!(
                                    "Removes {} package(s) safely",
                                    preview.plan.removes.len().max(1)
                                ),
                            )
                            .with_options(&["Cancel", "Remove"])
                            .destructive(true),
                            action,
                            return_to,
                        }));
                    }
                }
            }
            PendingAction::UpdatePackages(pkgs) => {
                if pkgs.is_empty() {
                    self.phase = *return_to;
                    self.banner = Some("No upgradable non-kernel packages selected".to_string());
                    self.sync_chrome();
                    return;
                }
                let refs: Vec<&str> = pkgs.iter().map(String::as_str).collect();
                match pkg_guard::check_install(&refs) {
                    Ok(Verdict::Block(lines)) => {
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Blocked {
                            title: "Upgrade blocked for safety".to_string(),
                            lines,
                            return_to,
                        }));
                    }
                    _ => {
                        let msg = if pkgs.len() == 1 {
                            format!("Update {} (--no-install-recommends)?", pkgs[0])
                        } else {
                            format!("Update {} packages (--no-install-recommends)?", pkgs.len())
                        };
                        self.phase = Phase::Confirm(Box::new(ConfirmKind::Ask {
                            confirm: Confirm::new("Apply updates?", msg)
                                .with_options(&["Cancel", "Update"]),
                            action,
                            return_to,
                        }));
                    }
                }
            }
            PendingAction::UpdatePackageLists => {
                self.phase = *return_to;
                self.execute_pending(action);
                return;
            }
            PendingAction::RunCleanup {
                autoremove,
                apt_cache,
                residual,
                trash_thumbs,
            } => {
                let mut desc = Vec::new();
                if *autoremove {
                    desc.push("apt autoremove");
                }
                if *apt_cache {
                    desc.push("apt clean");
                }
                if *trash_thumbs && !desc.contains(&"trash") {
                    desc.push("trash & thumbnails");
                }
                if !residual.is_empty() {
                    desc.push("purge rc configs");
                }
                if desc.is_empty() {
                    self.phase = *return_to;
                    self.banner = Some("Select at least one cleanup item first".to_string());
                    self.sync_chrome();
                    return;
                }
                self.phase = Phase::Confirm(Box::new(ConfirmKind::Ask {
                    confirm: Confirm::new("Clean selected items?", desc.join(" · "))
                        .with_options(&["Cancel", "Clean"]),
                    action,
                    return_to,
                }));
            }
        }
        self.sync_chrome();
    }

    fn clone_nav_phase(phase: &Phase) -> Phase {
        match phase {
            Phase::Tab(t) => Phase::Tab(*t),
            Phase::Category { intent_idx } => Phase::Category {
                intent_idx: *intent_idx,
            },
            Phase::Results { query } => Phase::Results {
                query: query.clone(),
            },
            Phase::Detail { app, from } => Phase::Detail {
                app: app.clone(),
                from: Box::new(Self::clone_nav_phase(from)),
            },
            _ => Phase::Tab(Tab::Discover),
        }
    }

    fn execute_pending(&mut self, action: PendingAction) {
        let return_to = match &self.phase {
            Phase::Confirm(k) => match &**k {
                ConfirmKind::Ask { return_to, .. } | ConfirmKind::Blocked { return_to, .. } => {
                    Box::new(Self::clone_nav_phase(return_to))
                }
            },
            other => Box::new(Self::clone_nav_phase(other)),
        };

        let (tag, title, labels, steps): (String, String, Vec<String>, Vec<Vec<String>>) =
            match action {
                PendingAction::Install(pkg) => (
                    format!("sw-install-{}", std::process::id()),
                    format!("Install {pkg}"),
                    vec![format!("apt-get install --no-install-recommends {pkg}")],
                    vec![pkg_guard::install_argv(&pkg)],
                ),
                PendingAction::Remove(pkg) => (
                    format!("sw-remove-{}", std::process::id()),
                    format!("Remove {pkg}"),
                    vec![format!("apt-get remove -y {pkg}")],
                    vec![vec![
                        "sudo".into(),
                        "-n".into(),
                        "DEBIAN_FRONTEND=noninteractive".into(),
                        "apt-get".into(),
                        "remove".into(),
                        "-y".into(),
                        pkg,
                    ]],
                ),
                PendingAction::UpdatePackages(pkgs) => {
                    let mut cmd = vec![
                        "sudo".into(),
                        "-n".into(),
                        "DEBIAN_FRONTEND=noninteractive".into(),
                        "apt-get".into(),
                        "install".into(),
                        "-y".into(),
                        "--only-upgrade".into(),
                        "--no-install-recommends".into(),
                        "-o".into(),
                        "Dpkg::Options::=--force-confdef".into(),
                        "-o".into(),
                        "Dpkg::Options::=--force-confold".into(),
                    ];
                    cmd.extend(pkgs.clone());
                    (
                        format!("sw-upd-{}", std::process::id()),
                        format!("Update {} package(s)", pkgs.len()),
                        vec![format!("Upgrading {}", pkgs.join(", "))],
                        vec![cmd],
                    )
                }
                PendingAction::UpdatePackageLists => (
                    format!("sw-lists-{}", std::process::id()),
                    "Refresh package lists".to_string(),
                    vec!["apt-get update".to_string()],
                    vec![vec![
                        "sudo".into(),
                        "-n".into(),
                        "apt-get".into(),
                        "update".into(),
                    ]],
                ),
                PendingAction::RunCleanup {
                    autoremove,
                    apt_cache,
                    residual,
                    trash_thumbs,
                } => {
                    let mut lbls = Vec::new();
                    let mut cmds = Vec::new();
                    if autoremove {
                        lbls.push("Remove unused dependencies (apt autoremove)".to_string());
                        cmds.push(vec![
                            "sudo".into(),
                            "-n".into(),
                            "DEBIAN_FRONTEND=noninteractive".into(),
                            "apt-get".into(),
                            "autoremove".into(),
                            "-y".into(),
                        ]);
                    }
                    if apt_cache {
                        lbls.push("Clean downloaded .deb cache".to_string());
                        cmds.push(vec![
                            "sudo".into(),
                            "-n".into(),
                            "apt-get".into(),
                            "clean".into(),
                        ]);
                    }
                    if !residual.is_empty() {
                        lbls.push(format!("Purge {} residual config(s)", residual.len()));
                        let mut cmd =
                            vec!["sudo".into(), "-n".into(), "dpkg".into(), "--purge".into()];
                        cmd.extend(residual);
                        cmds.push(cmd);
                    }
                    if trash_thumbs {
                        lbls.push("Empty user trash & thumbnail cache".to_string());
                        cmds.push(vec![
                            "sh".into(),
                            "-c".into(),
                            "rm -rf \"$HOME/.local/share/Trash/files\" \"$HOME/.local/share/Trash/info\" \"$HOME/.cache/thumbnails\"".into(),
                        ]);
                    }
                    (
                        format!("sw-clean-{}", std::process::id()),
                        "Software Cleanup".to_string(),
                        lbls,
                        cmds,
                    )
                }
            };

        if let Ok(task) = Task::start_steps(&tag, title, labels, false, steps) {
            self.phase = Phase::Running {
                task: Box::new(TaskView::new(task)),
                return_to,
            };
            self.sync_chrome();
        }
    }

    fn open_app(&mut self, app: &AppRef) {
        let pkg = app.package_name(&self.catalog);
        if !self.installed.contains_key(pkg) {
            self.banner = Some(format!("{pkg} is not installed yet"));
            return;
        }
        let launch_cmd = match app {
            AppRef::Catalog(i) => self.catalog.apps[*i].launch.as_deref().unwrap_or(pkg),
            AppRef::Debian { package, .. } => package.as_str(),
        };
        if launch_cmd.starts_with("hayato ")
            || launch_cmd.starts_with("foot ")
            || super::have(launch_cmd)
        {
            spawn_detached(&format!("exec {launch_cmd}"));
            self.banner = Some(format!("Launched {launch_cmd}"));
        } else {
            // Try matching `.desktop` via gtk-launch or hayato-apps fallback
            if super::have("gtk-launch") {
                spawn_detached(&format!("exec gtk-launch {launch_cmd}"));
                self.banner = Some(format!("Launched {launch_cmd}"));
            } else {
                self.banner = Some(format!(
                    "{pkg} is a CLI/library package (open a terminal to run it)"
                ));
            }
        }
    }

    // ─── Rendering Helpers ───────────────────────────────────────────────────

    fn draw_top_bar(&self, buf: &mut Buffer, area: Rect, design: &Design) -> HitBoxes {
        let mut hb = HitBoxes::default();
        let active_tab = match &self.phase {
            Phase::Tab(t) => Some(*t),
            Phase::Category { .. } | Phase::Results { .. } => Some(Tab::Discover),
            _ => None,
        };
        let active_pill = Style::default()
            .fg(design.color(ColorRole::SelectionText))
            .bg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let idle_pill = Style::default()
            .fg(design.color(ColorRole::Text))
            .bg(design.color(ColorRole::Surface));

        let upd_lbl = if self.updates.is_empty() {
            " Updates ".to_string()
        } else {
            format!(" Updates({}) ", self.updates.len())
        };
        let tabs: [(Tab, &str); 4] = [
            (Tab::Discover, " Discover "),
            (Tab::Installed, " Installed "),
            (Tab::Updates, upd_lbl.as_str()),
            (Tab::Cleanup, " Cleanup "),
        ];

        let mut x = area.x;
        for (i, (tab, lbl)) in tabs.iter().enumerate() {
            let w = lbl.chars().count() as u16;
            if x + w > area.right() {
                break;
            }
            let st = if active_tab == Some(*tab) {
                active_pill
            } else {
                idle_pill
            };
            put(buf, x, area.y, lbl, w, st);
            hb.tabs[i] = (*tab, Rect::new(x, area.y, w, 1));
            x += w + 1;
        }

        // Right-aligned Advanced (`pkg`) pill + inline Search field (`ColorRole::Bg`)
        let adv_lbl = " All APT ";
        let adv_w = adv_lbl.len() as u16;
        let adv_x = area.right().saturating_sub(adv_w);
        if adv_x > x + 6 {
            put(buf, adv_x, area.y, adv_lbl, adv_w, idle_pill);
            hb.advanced_btn = Rect::new(adv_x, area.y, adv_w, 1);
            let search_w = adv_x.saturating_sub(x + 2).min(28);
            if search_w >= 12 {
                let sx = adv_x.saturating_sub(search_w + 1);
                let srect = Rect::new(sx, area.y, search_w, 1);
                let blink = self.cursor_blink_on || design.reduced_motion();
                self.search_input.render_field(
                    srect,
                    buf,
                    design,
                    "/ search apps…",
                    self.search_focused,
                    blink,
                    ColorRole::Bg,
                    TextStyle::Body,
                );
                hb.search_box = srect;
            }
        }
        hb
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 52) / 100).max(48) as u16;
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

    fn draw_inspector(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let inner = draw_card_with(
            buf,
            area,
            design,
            Some("Preview"),
            ColorRole::Surface,
            design.style(TextStyle::Title),
        );
        if inner.width < 18 || inner.height < 6 {
            return;
        }
        let x = inner.x + 1;
        let w = inner.width.saturating_sub(2);
        let mut y = inner.y;
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let body = design.style(TextStyle::Body);
        let dim = design.style(TextStyle::Dim);
        let ok = Style::default()
            .fg(design.color(ColorRole::Ok))
            .add_modifier(Modifier::BOLD);

        match self.selected_tag() {
            Some(RowTag::Intent(idx)) => {
                let intent = &self.catalog.intents[*idx];
                put_trunc(buf, x, y, &intent.title.to_uppercase(), w, accent);
                y += 1;
                put_trunc(buf, x, y, &intent.blurb, w, body);
                y += 2;
                for (_, app) in self.catalog.apps_for_intent(*idx) {
                    if y >= inner.bottom().saturating_sub(2) {
                        break;
                    }
                    let is_inst = self.installed.contains_key(&app.package);
                    let mark = if is_inst { "\u{25cf}" } else { "\u{25cb}" };
                    let mst = if is_inst { ok } else { dim };
                    put(buf, x, y, mark, 1, mst);
                    put_trunc(buf, x + 2, y, &app.name, 20, body);
                    put_trunc(
                        buf,
                        x + 23,
                        y,
                        app.weight.short(),
                        w.saturating_sub(23),
                        dim,
                    );
                    y += 1;
                    put_trunc(buf, x + 2, y, &app.summary, w.saturating_sub(2), dim);
                    y += 1;
                }
            }
            Some(RowTag::App(app_ref)) => {
                let pkg = app_ref.package_name(&self.catalog);
                let name = app_ref.display_name(&self.catalog);
                put_trunc(buf, x, y, name, w, accent);
                y += 2;
                if let Some(m) = self.meta.get(pkg) {
                    put_trunc(
                        buf,
                        x,
                        y,
                        &format!(
                            "Version {} · Download {}",
                            m.candidate,
                            format_bytes(m.download_bytes)
                        ),
                        w,
                        dim,
                    );
                    y += 1;
                    put_trunc(
                        buf,
                        x,
                        y,
                        &format!(
                            "On disk {} · {}",
                            format_kb(m.installed_kb),
                            m.source_badge().0
                        ),
                        w,
                        dim,
                    );
                    y += 2;
                }
                let pitch = match app_ref {
                    AppRef::Catalog(i) => self.catalog.apps[*i].summary.as_str(),
                    AppRef::Debian { summary, .. } => summary.as_str(),
                };
                put_trunc(buf, x, y, pitch, w, body);
                y += 2;
                put_trunc(
                    buf,
                    x,
                    y,
                    "Press Enter for App Detail, sizes & permissions.",
                    w,
                    dim,
                );
            }
            Some(RowTag::UpdatePkg(i)) => {
                let u = &self.updates[*i];
                put_trunc(buf, x, y, &u.package, w, accent);
                y += 2;
                put_trunc(
                    buf,
                    x,
                    y,
                    &format!("{} \u{2192} {}", u.old_version, u.new_version),
                    w,
                    body,
                );
                y += 1;
                put_trunc(buf, x, y, &format!("Suite: {}", u.suite), w, dim);
                y += 2;
                if u.held_for_safety {
                    put_trunc(
                        buf,
                        x,
                        y,
                        "Protected kernel/boot package. Upgrade via `hayato update`.",
                        w,
                        Style::default().fg(design.color(ColorRole::Warn)),
                    );
                } else {
                    put_trunc(
                        buf,
                        x,
                        y,
                        "Press [u] to update or [U] to update all safe.",
                        w,
                        dim,
                    );
                }
            }
            _ => {
                put_trunc(buf, x, y, "HAYATO SOFTWARE", w, accent);
                y += 2;
                put_trunc(
                    buf,
                    x,
                    y,
                    "Native Debian APT catalog with kernel/boot guard.",
                    w,
                    body,
                );
                y += 1;
                put_trunc(
                    buf,
                    x,
                    y,
                    "All installs use --no-install-recommends after simulation.",
                    w,
                    dim,
                );
            }
        }
    }

    fn draw_detail(
        &self,
        buf: &mut Buffer,
        area: Rect,
        design: &Design,
        app: &AppRef,
        hb: &mut HitBoxes,
    ) {
        let pkg = app.package_name(&self.catalog);
        let name = app.display_name(&self.catalog);
        let is_inst = self.installed.contains_key(pkg);
        let has_upd = self.updates.iter().any(|u| u.package == pkg);

        let inner = draw_card_with(
            buf,
            area,
            design,
            Some(name),
            ColorRole::Surface,
            design.style(TextStyle::Title),
        );
        if inner.width < 28 || inner.height < 10 {
            return;
        }
        let x = inner.x + 1;
        let w = inner.width.saturating_sub(2);
        let mut y = inner.y;
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);

        let (badge_txt, badge_role) = if has_upd {
            ("UPDATE AVAILABLE", ColorRole::Warn)
        } else if is_inst {
            ("INSTALLED", ColorRole::Ok)
        } else {
            ("AVAILABLE", ColorRole::Accent)
        };
        let bx = draw_badge(buf, x, y, design, badge_txt, badge_role);
        let pitch = match app {
            AppRef::Catalog(i) => self.catalog.apps[*i].summary.as_str(),
            AppRef::Debian { summary, .. } => summary.as_str(),
        };
        put_trunc(
            buf,
            bx + 2,
            y,
            pitch,
            inner.right().saturating_sub(bx + 3),
            body,
        );
        y += 2;

        // Metadata lines
        let inst_ver = self
            .installed
            .get(pkg)
            .map(|i| i.version.as_str())
            .unwrap_or("—");
        let cand_ver = self
            .meta
            .get(pkg)
            .map(|m| m.candidate.as_str())
            .unwrap_or("loading…");
        put_trunc(buf, x, y, "Package", 12, dim);
        put_trunc(
            buf,
            x + 12,
            y,
            &format!("{pkg} · installed: {inst_ver} · candidate: {cand_ver}"),
            w.saturating_sub(12),
            body,
        );
        y += 1;

        let (dl, disk) = self
            .meta
            .get(pkg)
            .map(|m| (format_bytes(m.download_bytes), format_kb(m.installed_kb)))
            .unwrap_or_else(|| ("…".to_string(), "…".to_string()));
        let delta = self
            .detail_preview
            .get(pkg)
            .map(|p| {
                format!(
                    " · +{} deps (+{})",
                    p.plan.installs.len(),
                    format_kb(p.net_disk_kb.max(0) as u64)
                )
            })
            .unwrap_or_default();
        put_trunc(buf, x, y, "Size", 12, dim);
        put_trunc(
            buf,
            x + 12,
            y,
            &format!("Download {dl} · On disk {disk}{delta}"),
            w.saturating_sub(12),
            body,
        );
        y += 1;

        if let Some(m) = self.meta.get(pkg) {
            let (badge, reason) = m.source_badge();
            put_trunc(buf, x, y, "Source", 12, dim);
            put_trunc(
                buf,
                x + 12,
                y,
                &format!("{badge} ({reason})"),
                w.saturating_sub(12),
                body,
            );
            y += 1;
            if !m.homepage.is_empty() {
                put_trunc(buf, x, y, "Home", 12, dim);
                put_trunc(buf, x + 12, y, &m.homepage, w.saturating_sub(12), dim);
                y += 1;
            }
        }
        y += 1;

        // Honest APT Permissions / System Impact section
        put_trunc(buf, x, y, "ACCESS & TRUST (APT)", w, accent);
        y += 1;
        put_trunc(
            buf,
            x,
            y,
            "! Runs with your full user permissions (system package · no sandbox)",
            w,
            Style::default().fg(design.color(ColorRole::Warn)),
        );
        y += 1;
        let facts_line = match self.detail_access.get(pkg) {
            Some(f) if f.installed => format!(
                "services: {} · setuid: {} · conffiles: {}",
                if f.services.is_empty() {
                    "none".to_string()
                } else {
                    f.services.join(", ")
                },
                if f.has_setuid { "yes" } else { "no" },
                f.conffiles_count
            ),
            _ => {
                "services / setuid / /etc files: unknown until installed (no AppStream)".to_string()
            }
        };
        put_trunc(buf, x, y, &facts_line, w, dim);
        y += 2;

        // Description lines if space allows
        if let Some(m) = self.meta.get(pkg) {
            for para in m.description.iter().take(4) {
                if y >= inner.bottom().saturating_sub(3) {
                    break;
                }
                if para.is_empty() {
                    continue;
                }
                put_trunc(buf, x, y, para, w, body);
                y += 1;
            }
        }

        // Action buttons row at bottom of detail card
        let by = inner.bottom().saturating_sub(2);
        let mut bx_cur = x;
        let primary_pill = Style::default()
            .fg(design.color(ColorRole::SelectionText))
            .bg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let sec_pill = Style::default()
            .fg(design.color(ColorRole::Text))
            .bg(design.color(ColorRole::Bg));
        let danger_pill = Style::default()
            .fg(design.color(ColorRole::Danger))
            .bg(design.color(ColorRole::Bg));

        let mut push_btn = |kind: DetailBtn, label: &str, style: Style| {
            let bw = label.len() as u16;
            if bx_cur + bw <= inner.right() {
                put(buf, bx_cur, by, label, bw, style);
                hb.detail_buttons.push((kind, Rect::new(bx_cur, by, bw, 1)));
                bx_cur += bw + 2;
            }
        };
        if has_upd {
            push_btn(DetailBtn::Update, " Update (u) ", primary_pill);
        }
        if !is_inst {
            push_btn(DetailBtn::Install, " Install (i) ", primary_pill);
        } else {
            push_btn(DetailBtn::Open, " Open (o) ", primary_pill);
            push_btn(DetailBtn::Remove, " Remove (r) ", danger_pill);
        }
        push_btn(DetailBtn::Back, " Back (Esc) ", sec_pill);
    }
}

impl Screen for SoftwareScreen {
    fn header(&self) -> Option<&Header> {
        if let Phase::Running { task, .. } = &self.phase {
            return task.header();
        }
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        if let Phase::Running { task, .. } = &self.phase {
            return task.keybar();
        }
        Some(&self.keybar)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.phase {
            Phase::Tab(Tab::Discover) => "discover",
            Phase::Tab(Tab::Installed) => "installed",
            Phase::Tab(Tab::Updates) => "updates",
            Phase::Tab(Tab::Cleanup) => "cleanup",
            Phase::Category { .. } => "category",
            Phase::Results { .. } => "results",
            Phase::Detail { .. } => "detail",
            Phase::Confirm(_) => "confirm",
            Phase::Running { .. } => "running",
        })
    }

    fn needs_tick(&self) -> Option<Duration> {
        if let Phase::Running { task, .. } = &self.phase {
            return task.needs_tick();
        }
        Some(Duration::from_millis(250))
    }

    fn tick(&mut self) -> Outcome {
        if let Phase::Running { task, .. } = &mut self.phase {
            let res = task.tick();
            if matches!(res, Outcome::Done(_)) {
                let Phase::Running { return_to, .. } =
                    std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                else {
                    unreachable!();
                };
                self.phase = *return_to;
                self.reload_local_packages();
                return Outcome::Consumed;
            }
            return res;
        }
        let mut changed = self.poll_bg();
        if self.search_focused && self.last_blink.elapsed().as_millis() >= 500 {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink = Instant::now();
            changed = true;
        }
        if changed {
            Outcome::Consumed
        } else {
            Outcome::Continue
        }
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let Phase::Running { task, .. } = &mut self.phase {
            task.draw(ctx);
            return;
        }
        if let Phase::Confirm(kind) = &mut self.phase {
            *self.hits.borrow_mut() = HitBoxes {
                confirm_area: ctx.area,
                ..Default::default()
            };
            match &mut **kind {
                ConfirmKind::Ask { confirm, .. } => {
                    confirm.render(ctx.area, ctx.buf, ctx.design);
                }
                ConfirmKind::Blocked { title, lines, .. } => {
                    let inner = draw_card_with(
                        ctx.buf,
                        ctx.area,
                        ctx.design,
                        Some(title),
                        ColorRole::Surface,
                        Style::default()
                            .fg(ctx.design.color(ColorRole::Danger))
                            .add_modifier(Modifier::BOLD),
                    );
                    let danger = Style::default()
                        .fg(ctx.design.color(ColorRole::Danger))
                        .add_modifier(Modifier::BOLD);
                    let body = ctx.design.style(TextStyle::Body);
                    for (y, line) in (inner.y + 1..).zip(lines) {
                        if y >= inner.bottom() {
                            break;
                        }
                        let st = if line.starts_with("would ") {
                            danger
                        } else {
                            body
                        };
                        put_trunc(
                            ctx.buf,
                            inner.x + 1,
                            y,
                            &format!("\u{2717} {line}"),
                            inner.width.saturating_sub(2),
                            st,
                        );
                    }
                }
            }
            return;
        }

        let area = ctx.area;
        if area.width < 40 || area.height < 10 {
            return;
        }
        let top_rect = Rect::new(area.x, area.y, area.width, 1);
        let mut hb = self.draw_top_bar(ctx.buf, top_rect, ctx.design);
        let body_area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );

        if let Phase::Detail { app, .. } = &self.phase {
            self.draw_detail(ctx.buf, body_area, ctx.design, app, &mut hb);
            *self.hits.borrow_mut() = hb;
            return;
        }

        let (left, right) = Self::split_panes(body_area);
        let title = match &self.phase {
            Phase::Tab(Tab::Discover) => "What do you want to do?",
            Phase::Tab(Tab::Installed) => "Installed Software",
            Phase::Tab(Tab::Updates) => "Available Updates",
            Phase::Tab(Tab::Cleanup) => "Cleanup & Reclaim Space",
            Phase::Category { intent_idx } => self.catalog.intents[*intent_idx].title.as_str(),
            Phase::Results { .. } => "Search Results",
            _ => "Software",
        };
        let card = draw_card_with(
            ctx.buf,
            left,
            ctx.design,
            Some(title),
            ColorRole::Surface,
            ctx.design.style(TextStyle::Title),
        );

        let mut y = card.y;
        if let Some(msg) = &self.banner {
            put_trunc(
                ctx.buf,
                card.x + 1,
                y,
                msg,
                card.width.saturating_sub(2),
                Style::default()
                    .fg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD),
            );
            y += 1;
        } else if matches!(self.phase, Phase::Tab(Tab::Updates)) {
            let btn_style = Style::default()
                .fg(ctx.design.color(ColorRole::SelectionText))
                .bg(ctx.design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD);
            let sec_style = Style::default()
                .fg(ctx.design.color(ColorRole::Text))
                .bg(ctx.design.color(ColorRole::Bg));
            let u_lbl = " Update all safe (U) ";
            let uw = u_lbl.len() as u16;
            put(ctx.buf, card.x + 1, y, u_lbl, uw, btn_style);
            hb.update_all_btn = Rect::new(card.x + 1, y, uw, 1);
            let r_lbl = " Refresh lists (Ctrl+U) ";
            let rw = r_lbl.len() as u16;
            let rx = card.x + 1 + uw + 2;
            if rx + rw <= card.right() {
                put(ctx.buf, rx, y, r_lbl, rw, sec_style);
                hb.refresh_btn = Rect::new(rx, y, rw, 1);
            }
            y += 2;
        }

        let list_rect = Rect::new(card.x, y, card.width, card.bottom().saturating_sub(y));
        self.rows_height.set(list_rect.height);
        hb.list_area = list_rect;
        *self.hits.borrow_mut() = hb;
        self.list
            .render(list_rect, ctx.buf, ctx.design, !self.search_focused);

        if let Some(right_area) = right {
            self.draw_inspector(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        self.cursor_blink_on = true;
        self.last_blink = Instant::now();

        if let Phase::Running { task, .. } = &mut self.phase {
            let res = task.key(key, design);
            if matches!(res, Outcome::Done(_)) {
                let Phase::Running { return_to, .. } =
                    std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                else {
                    unreachable!();
                };
                self.phase = *return_to;
                self.reload_local_packages();
                return Outcome::Consumed;
            }
            return res;
        }

        if let Phase::Confirm(kind) = &mut self.phase {
            match &mut **kind {
                ConfirmKind::Blocked { .. } => {
                    if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace) {
                        let Phase::Confirm(boxed) =
                            std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                        else {
                            unreachable!();
                        };
                        if let ConfirmKind::Blocked { return_to, .. } = *boxed {
                            self.phase = *return_to;
                        }
                        self.sync_chrome();
                    }
                    return Outcome::Consumed;
                }
                ConfirmKind::Ask {
                    action, confirm, ..
                } => match confirm.handle(key) {
                    ConfirmAction::Chosen(1) => {
                        let act = action.clone();
                        self.execute_pending(act);
                        return Outcome::Consumed;
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        let Phase::Confirm(boxed) =
                            std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                        else {
                            unreachable!();
                        };
                        if let ConfirmKind::Ask { return_to, .. } = *boxed {
                            self.phase = *return_to;
                        }
                        self.sync_chrome();
                        return Outcome::Consumed;
                    }
                    ConfirmAction::None => return Outcome::Consumed,
                },
            }
        }

        // Search input focused handling
        if self.search_focused {
            match key.code {
                KeyCode::Esc => {
                    self.search_focused = false;
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    self.search_focused = false;
                    let q = self.search_input.text().to_string();
                    self.trigger_search(&q);
                    return Outcome::Consumed;
                }
                _ => match self.search_input.handle(key) {
                    InputAction::Changed => return Outcome::Consumed,
                    InputAction::Submitted => {
                        self.search_focused = false;
                        let q = self.search_input.text().to_string();
                        self.trigger_search(&q);
                        return Outcome::Consumed;
                    }
                    InputAction::Cancelled => {
                        self.search_focused = false;
                        return Outcome::Consumed;
                    }
                    InputAction::Ignored => {}
                },
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('u') | KeyCode::Char('U'))
        {
            self.execute_pending(PendingAction::UpdatePackageLists);
            return Outcome::Consumed;
        }

        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.cycle_tab(key.code == KeyCode::Tab);
            return Outcome::Consumed;
        }

        if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Char('/') => {
                    self.search_focused = true;
                    return Outcome::Consumed;
                }
                KeyCode::Char('1') => {
                    self.switch_tab(Tab::Discover);
                    return Outcome::Consumed;
                }
                KeyCode::Char('2') => {
                    self.switch_tab(Tab::Installed);
                    return Outcome::Consumed;
                }
                KeyCode::Char('3') => {
                    self.switch_tab(Tab::Updates);
                    return Outcome::Consumed;
                }
                KeyCode::Char('4') => {
                    self.switch_tab(Tab::Cleanup);
                    return Outcome::Consumed;
                }
                KeyCode::Char('a') => {
                    self.open_advanced_pkg = true;
                    return Outcome::Done(0);
                }
                KeyCode::Esc | KeyCode::Backspace => {
                    return match &self.phase {
                        Phase::Detail { .. } => {
                            let Phase::Detail { from, .. } =
                                std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                            else {
                                unreachable!();
                            };
                            self.phase = *from;
                            self.rebuild_list();
                            Outcome::Consumed
                        }
                        Phase::Category { .. } | Phase::Results { .. } => {
                            self.switch_tab(Tab::Discover);
                            Outcome::Consumed
                        }
                        Phase::Tab(_) => Outcome::Done(hayato_ui::app::exit::ESCAPE),
                        _ => Outcome::Consumed,
                    };
                }
                KeyCode::Char('q') => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
                _ => {}
            }
        }

        // Detail page shortcuts
        if let Phase::Detail { app, .. } = &self.phase {
            let pkg = app.package_name(&self.catalog).to_string();
            let app_clone = app.clone();
            if key.modifiers.is_empty() {
                match key.code {
                    KeyCode::Char('i') | KeyCode::Enter => {
                        if !self.installed.contains_key(&pkg) {
                            self.propose_transaction(PendingAction::Install(pkg));
                        } else {
                            self.open_app(&app_clone);
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('u') => {
                        if self.updates.iter().any(|u| u.package == pkg) {
                            self.propose_transaction(PendingAction::UpdatePackages(vec![pkg]));
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('o') => {
                        self.open_app(&app_clone);
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('r') => {
                        if self.installed.contains_key(&pkg) {
                            self.propose_transaction(PendingAction::Remove(pkg));
                        }
                        return Outcome::Consumed;
                    }
                    _ => {}
                }
            }
            return Outcome::Continue;
        }

        // Context-specific keys on lists
        if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Char(' ') if matches!(self.phase, Phase::Tab(Tab::Cleanup)) => {
                    if let Some(RowTag::CleanItem(ck)) = self.selected_tag().cloned() {
                        match ck {
                            CleanKind::Autoremove => self.clean_sel[0] = !self.clean_sel[0],
                            CleanKind::AptCache => self.clean_sel[1] = !self.clean_sel[1],
                            CleanKind::Residual => self.clean_sel[2] = !self.clean_sel[2],
                            CleanKind::TrashThumbs => self.clean_sel[3] = !self.clean_sel[3],
                            CleanKind::OldKernelsNote => {}
                        }
                        self.rebuild_list();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('c') if matches!(self.phase, Phase::Tab(Tab::Cleanup)) => {
                    let residual = self
                        .cleanup
                        .as_ref()
                        .filter(|_| self.clean_sel[2])
                        .map(|c| c.residual_configs.clone())
                        .unwrap_or_default();
                    self.propose_transaction(PendingAction::RunCleanup {
                        autoremove: self.clean_sel[0],
                        apt_cache: self.clean_sel[1],
                        residual,
                        trash_thumbs: self.clean_sel[3],
                    });
                    return Outcome::Consumed;
                }
                KeyCode::Char('U') => {
                    let safe_pkgs: Vec<String> = self
                        .updates
                        .iter()
                        .filter(|u| !u.held_for_safety)
                        .map(|u| u.package.clone())
                        .collect();
                    self.propose_transaction(PendingAction::UpdatePackages(safe_pkgs));
                    return Outcome::Consumed;
                }
                KeyCode::Char('u') if matches!(self.phase, Phase::Tab(Tab::Updates)) => {
                    if let Some(RowTag::UpdatePkg(i)) = self.selected_tag().cloned() {
                        let u = &self.updates[i];
                        if u.held_for_safety {
                            self.banner = Some(format!(
                                "{} is held for safety — run `hayato update`",
                                u.package
                            ));
                        } else {
                            self.propose_transaction(PendingAction::UpdatePackages(vec![
                                u.package.clone(),
                            ]));
                        }
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('o') => {
                    if let Some(RowTag::App(app)) = self.selected_tag().cloned() {
                        self.open_app(&app);
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('r') => {
                    if let Some(RowTag::App(app)) = self.selected_tag().cloned() {
                        let pkg = app.package_name(&self.catalog).to_string();
                        if self.installed.contains_key(&pkg) {
                            self.propose_transaction(PendingAction::Remove(pkg));
                        }
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('i') => {
                    if let Some(RowTag::App(app)) = self.selected_tag().cloned() {
                        let pkg = app.package_name(&self.catalog).to_string();
                        self.propose_transaction(PendingAction::Install(pkg));
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    return self.activate_selected();
                }
                _ => {}
            }
        }

        if key.modifiers == KeyModifiers::SHIFT && key.code == KeyCode::Char('U') {
            let safe_pkgs: Vec<String> = self
                .updates
                .iter()
                .filter(|u| !u.held_for_safety)
                .map(|u| u.package.clone())
                .collect();
            self.propose_transaction(PendingAction::UpdatePackages(safe_pkgs));
            return Outcome::Consumed;
        }

        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => self.activate_selected(),
            ListAction::Moved => Outcome::Consumed,
            ListAction::Ignored => Outcome::Continue,
        }
    }

    fn hit(&self, _area: Rect, col: u16, row: u16, design: &Design) -> Option<usize> {
        let in_rect = |r: Rect| {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.right()
                && row >= r.y
                && row < r.bottom()
        };
        let hb = self.hits.borrow();
        if let Phase::Confirm(k) = &self.phase {
            return match &**k {
                ConfirmKind::Ask { confirm, .. } => confirm.hit(hb.confirm_area, design, col, row),
                ConfirmKind::Blocked { .. } => Some(0),
            };
        }
        for (i, (_, r)) in hb.tabs.iter().enumerate() {
            if in_rect(*r) {
                return Some(100 + i);
            }
        }
        if in_rect(hb.search_box) {
            return Some(110);
        }
        if in_rect(hb.advanced_btn) {
            return Some(111);
        }
        if in_rect(hb.update_all_btn) {
            return Some(112);
        }
        if in_rect(hb.refresh_btn) {
            return Some(113);
        }
        for (i, (_, r)) in hb.detail_buttons.iter().enumerate() {
            if in_rect(*r) {
                return Some(200 + i);
            }
        }
        self.list.hit_row(hb.list_area, col, row, design)
    }

    fn click(&mut self, index: Option<usize>) -> Outcome {
        let Some(index) = index else {
            return Outcome::Continue;
        };
        if let Phase::Confirm(k) = &mut self.phase {
            match &mut **k {
                ConfirmKind::Blocked { .. } => {
                    let Phase::Confirm(boxed) =
                        std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                    else {
                        unreachable!();
                    };
                    if let ConfirmKind::Blocked { return_to, .. } = *boxed {
                        self.phase = *return_to;
                    }
                    self.sync_chrome();
                }
                ConfirmKind::Ask {
                    action, confirm, ..
                } => match confirm.click(index) {
                    ConfirmAction::Chosen(1) => {
                        let act = action.clone();
                        self.execute_pending(act);
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        let Phase::Confirm(boxed) =
                            std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                        else {
                            unreachable!();
                        };
                        if let ConfirmKind::Ask { return_to, .. } = *boxed {
                            self.phase = *return_to;
                        }
                        self.sync_chrome();
                    }
                    ConfirmAction::None => {}
                },
            }
            return Outcome::Consumed;
        }

        match index {
            100 => self.switch_tab(Tab::Discover),
            101 => self.switch_tab(Tab::Installed),
            102 => self.switch_tab(Tab::Updates),
            103 => self.switch_tab(Tab::Cleanup),
            110 => self.search_focused = true,
            111 => {
                self.open_advanced_pkg = true;
                return Outcome::Done(0);
            }
            112 => {
                let safe_pkgs: Vec<String> = self
                    .updates
                    .iter()
                    .filter(|u| !u.held_for_safety)
                    .map(|u| u.package.clone())
                    .collect();
                self.propose_transaction(PendingAction::UpdatePackages(safe_pkgs));
            }
            113 => self.execute_pending(PendingAction::UpdatePackageLists),
            i if (200..300).contains(&i) => {
                let btn = self
                    .hits
                    .borrow()
                    .detail_buttons
                    .get(i - 200)
                    .map(|(b, _)| *b);
                if let (Some(btn), Phase::Detail { app, .. }) = (btn, &self.phase) {
                    let app_c = app.clone();
                    let pkg = app_c.package_name(&self.catalog).to_string();
                    match btn {
                        DetailBtn::Install => {
                            self.propose_transaction(PendingAction::Install(pkg));
                        }
                        DetailBtn::Update => {
                            self.propose_transaction(PendingAction::UpdatePackages(vec![pkg]));
                        }
                        DetailBtn::Open => self.open_app(&app_c),
                        DetailBtn::Remove => {
                            self.propose_transaction(PendingAction::Remove(pkg));
                        }
                        DetailBtn::Back => {
                            let Phase::Detail { from, .. } =
                                std::mem::replace(&mut self.phase, Phase::Tab(Tab::Discover))
                            else {
                                unreachable!();
                            };
                            self.phase = *from;
                            self.rebuild_list();
                        }
                    }
                }
            }
            row_idx => {
                self.search_focused = false;
                self.list.set_cursor(row_idx);
                return self.activate_selected();
            }
        }
        Outcome::Consumed
    }
}

impl SoftwareScreen {
    fn activate_selected(&mut self) -> Outcome {
        let Some(tag) = self.selected_tag().cloned() else {
            return Outcome::Consumed;
        };
        match tag {
            RowTag::Heading | RowTag::CleanItem(CleanKind::OldKernelsNote) => Outcome::Consumed,
            RowTag::Intent(idx) => {
                self.open_category(idx);
                Outcome::Consumed
            }
            RowTag::App(app) => {
                self.open_detail(app);
                Outcome::Consumed
            }
            RowTag::UpdatePkg(i) => {
                let u = &self.updates[i];
                if u.held_for_safety {
                    self.banner = Some(format!(
                        "{} is held for safety — run `hayato update`",
                        u.package
                    ));
                } else {
                    self.propose_transaction(PendingAction::UpdatePackages(vec![
                        u.package.clone(),
                    ]));
                }
                Outcome::Consumed
            }
            RowTag::CleanItem(ck) => {
                match ck {
                    CleanKind::Autoremove => self.clean_sel[0] = !self.clean_sel[0],
                    CleanKind::AptCache => self.clean_sel[1] = !self.clean_sel[1],
                    CleanKind::Residual => self.clean_sel[2] = !self.clean_sel[2],
                    CleanKind::TrashThumbs => self.clean_sel[3] = !self.clean_sel[3],
                    CleanKind::OldKernelsNote => {}
                }
                self.rebuild_list();
                Outcome::Consumed
            }
            RowTag::CleanRun => {
                let residual = self
                    .cleanup
                    .as_ref()
                    .filter(|_| self.clean_sel[2])
                    .map(|c| c.residual_configs.clone())
                    .unwrap_or_default();
                self.propose_transaction(PendingAction::RunCleanup {
                    autoremove: self.clean_sel[0],
                    apt_cache: self.clean_sel[1],
                    residual,
                    trash_thumbs: self.clean_sel[3],
                });
                Outcome::Consumed
            }
        }
    }
}

// ─── Headless CLI & Entry Point ──────────────────────────────────────────────

pub fn status_summary() -> String {
    let inst = apt::dpkg_installed().len();
    let ups = apt::fetch_upgradable();
    let sec = ups.iter().filter(|u| u.is_security).count();
    let clean = crate::system::clean::calculate_reclaimable();
    let rec_mb = clean.safe_reclaimable_bytes() / (1024 * 1024);
    format!(
        "installed={inst} updates={} security={sec} reclaimable={rec_mb}MB",
        ups.len()
    )
}

pub fn print_rows() {
    let cat = Catalog::load();
    for intent in &cat.intents {
        println!("{}\t{}\t{}", intent.id, intent.title, intent.apps.join(","));
    }
}

pub fn print_updates() {
    for u in apt::fetch_upgradable() {
        let tag = if u.held_for_safety {
            "held"
        } else if u.is_security {
            "security"
        } else {
            &u.suite
        };
        println!(
            "{}\t{}\t{}\t{}",
            u.package, u.old_version, u.new_version, tag
        );
    }
}

pub fn print_search(query: &str) {
    let cat = Catalog::load();
    let res = cat.search(query);
    for ri in &res.intents {
        let intent = &cat.intents[ri.intent_idx];
        println!("intent\t{}\t{}\t{}", intent.id, intent.title, intent.blurb);
    }
    for ra in &res.apps {
        let app = &cat.apps[ra.app_idx];
        println!("app\t{}\t{}\t{}", app.package, app.name, app.summary);
    }
}

pub fn menu_with_args(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(Failure::new(
            "Hayato Software",
            format!("{said} (apt, dpkg, sudo)"),
        ));
    }

    let mut initial_tab = Tab::Discover;
    let mut initial_query: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--updates" | "updates" => initial_tab = Tab::Updates,
            "--installed" | "installed" => initial_tab = Tab::Installed,
            "--cleanup" | "cleanup" => initial_tab = Tab::Cleanup,
            "--search" => {
                if let Some(q) = args.get(i + 1) {
                    initial_query = Some(q.clone());
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    loop {
        let mut screen = SoftwareScreen::new(initial_tab, initial_query.as_deref());
        let code = crate::run_screen(terminal, design, "Software", &mut screen)?;
        if screen.open_advanced_pkg {
            let _ = crate::system::pkg::menu(terminal, design)?;
            initial_query = None;
            continue;
        }
        return Ok(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_transitions_from_discover_to_category_and_detail() {
        let mut s = SoftwareScreen::for_snapshot_discover();
        assert_eq!(s.phase(), Some("discover"));
        s.open_category(0);
        assert_eq!(s.phase(), Some("category"));
        s.open_detail(AppRef::Catalog(0));
        assert_eq!(s.phase(), Some("detail"));
    }

    #[test]
    fn switching_tabs_updates_phase_name() {
        let mut s = SoftwareScreen::for_snapshot_discover();
        s.switch_tab(Tab::Installed);
        assert_eq!(s.phase(), Some("installed"));
        s.switch_tab(Tab::Updates);
        assert_eq!(s.phase(), Some("updates"));
        s.switch_tab(Tab::Cleanup);
        assert_eq!(s.phase(), Some("cleanup"));
    }
}
