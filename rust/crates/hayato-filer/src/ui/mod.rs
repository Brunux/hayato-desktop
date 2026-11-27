//! Main GTK4 window assembly for `hayato-filer`: Finder/elementary minimalism + Hayato Cyber-HUD.

pub mod properties;
pub mod quicklook;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, DragSource,
    DropTarget, Entry, EventControllerFocus, EventControllerKey, FlowBox, FlowBoxChild,
    GestureClick, GestureDrag, Label, ListBox, ListBoxRow, Orientation, Overlay, Paned, Picture,
    Popover, ProgressBar, ScrolledWindow, SelectionMode, Separator, SizeGroup, SizeGroupMode,
    Widget,
};

use crate::fs::archive::{self, ArchiveFormat};
use crate::fs::devices::{self, DeviceEntry};
use crate::fs::ops::{self, OpKind, OpProgress};
use crate::fs::thumbs;
use crate::fs::trash;
use crate::fs::{FileEntry, format_mtime, format_size, free_space_bytes, list_dir};
use crate::prefs;
use crate::state::{AppState, SortField, SortOrder, ViewMode, home_dir, is_trash_files_dir};
use crate::tools;

pub struct FilerWindow {
    pub window: ApplicationWindow,
    state: RefCell<AppState>,
    devices: RefCell<Vec<DeviceEntry>>,
    device_sig: Cell<u64>,
    devices_loaded: Cell<bool>,
    _volume_monitor: gio::VolumeMonitor,
    back_btn: Button,
    fwd_btn: Button,
    crumbs_scroll: ScrolledWindow,
    crumbs_box: GtkBox,
    path_entry: Entry,
    filter_btn: Button,
    filter_entry: Entry,
    grid_btn: Button,
    list_btn: Button,
    sidebar_toggle_btn: Button,
    tab_bar: GtkBox,
    tabs_box: GtkBox,
    paned: Paned,
    sidebar_box: GtkBox,
    places_hdr: Label,
    places_box: GtkBox,
    sidebar_sep: Separator,
    devices_hdr: Label,
    devices_box: GtkBox,
    list_header: GtkBox,
    canvas_scroll: ScrolledWindow,
    flow_box: FlowBox,
    list_box: ListBox,
    overlay: Overlay,
    quicklook_slot: RefCell<Option<GtkBox>>,
    quicklook_scroll: RefCell<Option<ScrolledWindow>>,
    properties_slot: RefCell<Option<GtkBox>>,
    status_label: Label,
    progress_box: GtkBox,
    progress_bar: ProgressBar,
    progress_label: Label,
    active_cancel: RefCell<Option<Arc<AtomicBool>>>,
    /// Live watcher for the open folder (inotify via GIO).
    dir_monitor: RefCell<Option<gio::FileMonitor>>,
    refresh_scheduled: Cell<bool>,
    /// The widget under the current drag, held for the whole drag so a
    /// spring-loaded navigation cannot finalize the source mid-drag.
    drag_hold: RefCell<Option<Widget>>,
    /// Staging shelf: files collected from anywhere, dragged out as a batch.
    shelf_box: GtkBox,
    shelf_chips: GtkBox,
    shelf_btn: Button,
    shelf_badge: Label,
    shelf: RefCell<Vec<PathBuf>>,
    /// Horizontal SizeGroups keeping List View headers and rows pixel-aligned.
    col_size_sg: SizeGroup,
    col_mtime_sg: SizeGroup,
    col_kind_sg: SizeGroup,
    col_size_btn: RefCell<Option<Button>>,
    col_mtime_btn: RefCell<Option<Button>>,
    col_kind_btn: RefCell<Option<Button>>,
    /// Header New Tab button: hidden while the tab strip (which carries its own
    /// add button) is showing, so only one "+" is ever visible.
    new_tab_btn: Button,
}

impl FilerWindow {
    pub fn build(app: &Application, initial_dir: PathBuf) -> Rc<Self> {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Hayato Files")
            .default_width(980)
            .default_height(640)
            .build();
        window.add_css_class("hayato-filer");

        // ── Top Header Bar ────────────────────────────────────────────────
        let header = GtkBox::new(Orientation::Horizontal, 8);
        header.add_css_class("filer-header");

        let sidebar_toggle_btn = Button::with_label("\u{f0db}");
        sidebar_toggle_btn.add_css_class("filer-tool-btn");
        sidebar_toggle_btn.add_css_class("filer-glyph");
        sidebar_toggle_btn.set_tooltip_text(Some("Toggle Compact Sidebar (Icons / Full Names)"));

        let nav_group = GtkBox::new(Orientation::Horizontal, 2);
        nav_group.add_css_class("filer-seg-group");
        let back_btn = Button::with_label("\u{f053}");
        back_btn.add_css_class("filer-nav-btn");
        back_btn.add_css_class("filer-glyph");
        back_btn.set_tooltip_text(Some("Back (Alt+Left)"));

        let fwd_btn = Button::with_label("\u{f054}");
        fwd_btn.add_css_class("filer-nav-btn");
        fwd_btn.add_css_class("filer-glyph");
        fwd_btn.set_tooltip_text(Some("Forward (Alt+Right)"));
        nav_group.append(&back_btn);
        nav_group.append(&fwd_btn);

        let crumbs_box = GtkBox::new(Orientation::Horizontal, 0);
        let crumbs_scroll = ScrolledWindow::new();
        crumbs_scroll.add_css_class("filer-breadcrumbs");
        crumbs_scroll.set_policy(gtk4::PolicyType::External, gtk4::PolicyType::Never);
        crumbs_scroll.set_hexpand(true);
        crumbs_scroll.set_min_content_width(120);
        crumbs_scroll.set_child(Some(&crumbs_box));

        let path_entry = Entry::new();
        path_entry.add_css_class("filer-path-entry");
        path_entry.set_hexpand(true);
        path_entry.set_visible(false);

        let filter_entry = Entry::new();
        filter_entry.add_css_class("filer-filter-entry");
        filter_entry.set_placeholder_text(Some("Filter folder…"));
        filter_entry.set_width_chars(18);
        filter_entry.set_visible(false);

        let filter_btn = Button::with_label("\u{f002}");
        filter_btn.add_css_class("filer-tool-btn");
        filter_btn.add_css_class("filer-glyph");
        filter_btn.set_tooltip_text(Some("Filter folder (Ctrl+F)"));

        let view_group = GtkBox::new(Orientation::Horizontal, 2);
        view_group.add_css_class("filer-seg-group");
        let grid_btn = Button::with_label("\u{f009}");
        grid_btn.add_css_class("filer-tool-btn");
        grid_btn.add_css_class("filer-glyph");
        grid_btn.set_tooltip_text(Some("Icon Grid (Ctrl+1)"));

        let list_btn = Button::with_label("\u{f03a}");
        list_btn.add_css_class("filer-tool-btn");
        list_btn.add_css_class("filer-glyph");
        list_btn.set_tooltip_text(Some("Compact List (Ctrl+2)"));
        view_group.append(&grid_btn);
        view_group.append(&list_btn);

        let sort_btn = Button::with_label("\u{f0dc}");
        sort_btn.add_css_class("filer-tool-btn");
        sort_btn.add_css_class("filer-glyph");
        sort_btn.set_tooltip_text(Some("Sort & View Options"));

        let new_tab_btn = Button::with_label("\u{f067}");
        new_tab_btn.add_css_class("filer-tool-btn");
        new_tab_btn.add_css_class("filer-glyph");
        new_tab_btn.set_tooltip_text(Some("New Tab (Ctrl+T)"));

        let shelf_btn = Button::new();
        shelf_btn.add_css_class("filer-tool-btn");
        shelf_btn.set_tooltip_text(Some(
            "Staging Shelf — drop files here to collect them, then drag or move the batch",
        ));
        let shelf_btn_box = GtkBox::new(Orientation::Horizontal, 4);
        let shelf_ic = Label::new(Some("\u{f01c}"));
        shelf_ic.add_css_class("filer-glyph");
        let shelf_badge = Label::new(None);
        shelf_badge.add_css_class("filer-badge");
        shelf_badge.set_visible(false);
        shelf_btn_box.append(&shelf_ic);
        shelf_btn_box.append(&shelf_badge);
        shelf_btn.set_child(Some(&shelf_btn_box));

        header.append(&sidebar_toggle_btn);
        header.append(&nav_group);
        header.append(&crumbs_scroll);
        header.append(&path_entry);
        header.append(&filter_entry);
        header.append(&filter_btn);
        header.append(&view_group);
        header.append(&sort_btn);
        header.append(&shelf_btn);
        header.append(&new_tab_btn);

        // ── Minimalist Tab Bar (auto-hides when only 1 tab is open) ──────
        let tab_bar = GtkBox::new(Orientation::Horizontal, 4);
        tab_bar.add_css_class("filer-tabbar");
        tab_bar.set_visible(false);

        let tabs_box = GtkBox::new(Orientation::Horizontal, 4);
        let tabs_scroll = ScrolledWindow::new();
        tabs_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        tabs_scroll.set_hexpand(true);
        tabs_scroll.set_child(Some(&tabs_box));

        let tab_add_btn = Button::with_label("\u{f067}");
        tab_add_btn.add_css_class("filer-tab-add");
        tab_add_btn.add_css_class("filer-glyph");
        tab_add_btn.set_tooltip_text(Some("New Tab (Ctrl+T)"));

        tab_bar.append(&tabs_scroll);
        tab_bar.append(&tab_add_btn);

        // ── Left Sidebar (Places + Devices) ───────────────────────────────
        let sidebar_box = GtkBox::new(Orientation::Vertical, 2);
        sidebar_box.add_css_class("filer-sidebar");
        sidebar_box.set_size_request(192, -1);

        let places_hdr = Label::new(Some("PLACES"));
        places_hdr.add_css_class("filer-section-label");
        places_hdr.set_halign(Align::Start);
        let places_box = GtkBox::new(Orientation::Vertical, 1);

        let devices_hdr = Label::new(Some("DEVICES"));
        devices_hdr.add_css_class("filer-section-label");
        devices_hdr.set_halign(Align::Start);
        let devices_box = GtkBox::new(Orientation::Vertical, 2);

        let sidebar_sep = Separator::new(Orientation::Horizontal);
        sidebar_sep.add_css_class("filer-sidebar-sep");
        sidebar_sep.set_visible(false);

        sidebar_box.append(&places_hdr);
        sidebar_box.append(&places_box);
        sidebar_box.append(&sidebar_sep);
        sidebar_box.append(&devices_hdr);
        sidebar_box.append(&devices_box);

        let sidebar_scroll = ScrolledWindow::new();
        sidebar_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        sidebar_scroll.set_child(Some(&sidebar_box));

        // ── Sticky List View Column Header ────────────────────────────────
        let list_header = GtkBox::new(Orientation::Horizontal, 0);
        list_header.add_css_class("filer-list-header");
        list_header.set_visible(false);

        // ── Main Canvas (FlowBox Grid + ListBox Compact List) ─────────────
        let flow_box = FlowBox::new();
        flow_box.add_css_class("filer-canvas");
        flow_box.set_valign(Align::Start);
        flow_box.set_max_children_per_line(16);
        flow_box.set_min_children_per_line(2);
        flow_box.set_selection_mode(SelectionMode::Multiple);
        flow_box.set_activate_on_single_click(false);
        flow_box.set_homogeneous(true);
        flow_box.set_column_spacing(6);
        flow_box.set_row_spacing(6);
        flow_box.set_margin_start(12);
        flow_box.set_margin_end(12);
        flow_box.set_margin_top(12);
        flow_box.set_margin_bottom(12);

        let list_box = ListBox::new();
        list_box.add_css_class("filer-canvas");
        list_box.set_selection_mode(SelectionMode::Multiple);
        list_box.set_activate_on_single_click(false);

        let canvas_scroll = ScrolledWindow::new();
        canvas_scroll.add_css_class("filer-canvas");
        canvas_scroll.set_vexpand(true);
        canvas_scroll.set_hexpand(true);
        canvas_scroll.set_child(Some(&flow_box));

        // ── Bottom Monospace HUD Status Bar ───────────────────────────────
        let status_bar = GtkBox::new(Orientation::Horizontal, 10);
        status_bar.add_css_class("filer-statusbar");

        let status_label = Label::new(Some("Loading…"));
        status_label.set_halign(Align::Start);
        status_label.set_hexpand(true);

        let progress_box = GtkBox::new(Orientation::Horizontal, 6);
        progress_box.set_visible(false);
        let progress_label = Label::new(None);
        let progress_bar = ProgressBar::new();
        progress_bar.add_css_class("filer-progress");
        progress_bar.set_valign(Align::Center);
        let cancel_btn = Button::with_label("\u{f00d}");
        cancel_btn.add_css_class("filer-dev-action");
        cancel_btn.add_css_class("filer-glyph");
        progress_box.append(&progress_label);
        progress_box.append(&progress_bar);
        progress_box.append(&cancel_btn);

        status_bar.append(&status_label);
        status_bar.append(&progress_box);

        // ── Right Column & Paned Split ────────────────────────────────────
        let right_col = GtkBox::new(Orientation::Vertical, 0);
        right_col.append(&list_header);
        right_col.append(&canvas_scroll);

        let paned = Paned::new(Orientation::Horizontal);
        paned.set_start_child(Some(&sidebar_scroll));
        paned.set_end_child(Some(&right_col));
        paned.set_position(196);
        paned.set_shrink_start_child(false);
        paned.set_resize_start_child(false);
        paned.set_vexpand(true);

        // ── Drag-and-Drop Staging Shelf (hidden until it holds something) ──
        let shelf_box = GtkBox::new(Orientation::Horizontal, 8);
        shelf_box.add_css_class("filer-shelf");
        shelf_box.set_visible(false);
        let shelf_title = Label::new(Some("SHELF"));
        shelf_title.add_css_class("filer-shelf-title");
        shelf_title.set_valign(Align::Center);
        let shelf_chips = GtkBox::new(Orientation::Horizontal, 6);
        shelf_chips.set_valign(Align::Center);
        let shelf_scroll = ScrolledWindow::new();
        shelf_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        shelf_scroll.set_hexpand(true);
        shelf_scroll.set_child(Some(&shelf_chips));
        let shelf_move = Button::with_label("Move Here");
        shelf_move.add_css_class("filer-dev-action");
        let shelf_copy = Button::with_label("Copy Here");
        shelf_copy.add_css_class("filer-dev-action");
        let shelf_share = Button::with_label("Share");
        shelf_share.add_css_class("filer-dev-action");
        let shelf_clear = Button::with_label("Clear");
        shelf_clear.add_css_class("filer-dev-action");
        let shelf_close = Button::with_label("\u{f00d}");
        shelf_close.add_css_class("filer-dev-action");
        shelf_close.add_css_class("filer-glyph");
        shelf_close.set_tooltip_text(Some("Hide Shelf (keeps the files)"));
        shelf_box.append(&shelf_title);
        shelf_box.append(&shelf_scroll);
        shelf_box.append(&shelf_move);
        shelf_box.append(&shelf_copy);
        shelf_box.append(&shelf_share);
        shelf_box.append(&shelf_clear);
        shelf_box.append(&shelf_close);

        let root_col = GtkBox::new(Orientation::Vertical, 0);
        root_col.append(&header);
        root_col.append(&tab_bar);
        root_col.append(&paned);
        root_col.append(&shelf_box);
        root_col.append(&status_bar);

        let overlay = Overlay::new();
        overlay.set_child(Some(&root_col));
        window.set_child(Some(&overlay));

        let prefs = prefs::load();
        // A size saved on a bigger screen must not open a window this panel
        // cannot show; the monitor is the ceiling, the prefs floor is already
        // enforced by `FilerPrefs::sanitized`.
        let monitor = gtk4::prelude::WidgetExt::display(&window)
            .monitors()
            .item(0)
            .and_downcast::<gdk::Monitor>();
        let (w, h) = match monitor {
            Some(m) => {
                let geo = m.geometry();
                (
                    prefs.window_width.min(geo.width()),
                    prefs.window_height.min(geo.height()),
                )
            }
            None => (prefs.window_width, prefs.window_height),
        };
        window.set_default_size(w, h);
        let mut init_state = AppState::new(initial_dir);
        init_state.apply_prefs(&prefs);
        let volume_monitor = gio::VolumeMonitor::get();

        let this = Rc::new(Self {
            window,
            state: RefCell::new(init_state),
            devices: RefCell::new(Vec::new()),
            device_sig: Cell::new(devices::block_mount_signature()),
            devices_loaded: Cell::new(false),
            _volume_monitor: volume_monitor,
            back_btn,
            fwd_btn,
            crumbs_scroll,
            crumbs_box,
            path_entry,
            filter_btn,
            filter_entry,
            grid_btn,
            list_btn,
            sidebar_toggle_btn,
            tab_bar,
            tabs_box,
            paned,
            sidebar_box,
            places_hdr,
            places_box,
            sidebar_sep,
            devices_hdr,
            devices_box,
            list_header,
            canvas_scroll,
            flow_box,
            list_box,
            overlay,
            quicklook_slot: RefCell::new(None),
            quicklook_scroll: RefCell::new(None),
            properties_slot: RefCell::new(None),
            status_label,
            progress_box,
            progress_bar,
            progress_label,
            active_cancel: RefCell::new(None),
            dir_monitor: RefCell::new(None),
            refresh_scheduled: Cell::new(false),
            drag_hold: RefCell::new(None),
            shelf_box,
            shelf_chips,
            shelf_btn,
            shelf_badge,
            shelf: RefCell::new(Vec::new()),
            col_size_sg: SizeGroup::new(SizeGroupMode::Horizontal),
            col_mtime_sg: SizeGroup::new(SizeGroupMode::Horizontal),
            col_kind_sg: SizeGroup::new(SizeGroupMode::Horizontal),
            col_size_btn: RefCell::new(None),
            col_mtime_btn: RefCell::new(None),
            col_kind_btn: RefCell::new(None),
            new_tab_btn: new_tab_btn.clone(),
        });

        this.wire_signals(&sort_btn, &new_tab_btn, &tab_add_btn, &cancel_btn);
        this.wire_shelf(
            &shelf_move,
            &shelf_copy,
            &shelf_share,
            &shelf_clear,
            &shelf_close,
        );
        this.wire_pin_drop_target();
        this.set_sidebar_compact(prefs.sidebar_compact);
        this.update_view_buttons(prefs.view_mode);
        this.list_header
            .set_visible(prefs.view_mode == ViewMode::List);
        this.rebuild_list_header();
        this.render_tabs();
        this.refresh_devices();
        this.start_device_monitor();
        this.reload_dir();
        this.save_prefs_on_close();
        this
    }

    fn wire_signals(
        self: &Rc<Self>,
        sort_btn: &Button,
        new_tab_btn: &Button,
        tab_add_btn: &Button,
        cancel_btn: &Button,
    ) {
        // Compact / full sidebar toggle (icons-only vs full names)
        {
            let w = Rc::clone(self);
            self.sidebar_toggle_btn
                .connect_clicked(move |_| w.toggle_sidebar_compact());
        }

        // Back / Forward
        {
            let w = Rc::clone(self);
            self.back_btn.connect_clicked(move |_| {
                if w.state.borrow_mut().go_back().is_some() {
                    w.reload_dir();
                }
            });
        }
        {
            let w = Rc::clone(self);
            self.fwd_btn.connect_clicked(move |_| {
                if w.state.borrow_mut().go_forward().is_some() {
                    w.reload_dir();
                }
            });
        }

        // Click empty space in breadcrumb bar to edit path directly
        {
            let w = Rc::clone(self);
            let click = GestureClick::new();
            click.set_button(1);
            click.connect_released(move |_, _, _, _| w.show_path_entry());
            self.crumbs_scroll.add_controller(click);
        }

        // Editable path entry (Ctrl+L) — auto-reverts to breadcrumbs on blur
        {
            let w = Rc::clone(self);
            let focus = EventControllerFocus::new();
            focus.connect_leave(move |_| w.hide_path_entry());
            self.path_entry.add_controller(focus);
        }
        {
            let w = Rc::clone(self);
            self.path_entry.connect_activate(move |entry| {
                let raw = entry.text().to_string();
                let expanded = if let Some(rest) = raw.strip_prefix("~/") {
                    home_dir().join(rest)
                } else if raw == "~" {
                    home_dir()
                } else {
                    PathBuf::from(raw.trim())
                };
                w.hide_path_entry();
                if expanded.is_dir() {
                    w.navigate_to(expanded);
                } else if expanded.is_file() {
                    tools::open_file(&expanded);
                } else {
                    w.show_banner(&format!("Not found: {}", expanded.display()), "warn");
                }
            });
        }

        // Live filter toggle & input
        {
            let w = Rc::clone(self);
            self.filter_btn.connect_clicked(move |_| {
                let vis = !gtk4::prelude::WidgetExt::is_visible(&w.filter_entry);
                w.filter_entry.set_visible(vis);
                if vis {
                    w.filter_btn.add_css_class("active");
                    w.filter_entry.grab_focus();
                } else {
                    w.filter_btn.remove_css_class("active");
                    w.filter_entry.set_text("");
                }
            });
        }
        {
            let w = Rc::clone(self);
            self.filter_entry.connect_changed(move |entry| {
                w.state.borrow_mut().tab_mut().filter = entry.text().to_string();
                w.state.borrow_mut().recompute_visible();
                w.render_canvas();
            });
        }

        // View mode toggle (Grid / List)
        {
            let w = Rc::clone(self);
            self.grid_btn
                .connect_clicked(move |_| w.set_view_mode(ViewMode::Grid));
        }
        {
            let w = Rc::clone(self);
            self.list_btn
                .connect_clicked(move |_| w.set_view_mode(ViewMode::List));
        }

        // Sort popover
        {
            let w = Rc::clone(self);
            let btn = sort_btn.clone();
            sort_btn.connect_clicked(move |_| w.show_sort_popover(&btn));
        }

        // New tab buttons (header + tab bar)
        {
            let w = Rc::clone(self);
            new_tab_btn.connect_clicked(move |_| {
                let cwd = w.state.borrow().cwd().to_path_buf();
                w.open_new_tab(cwd);
            });
        }
        {
            let w = Rc::clone(self);
            tab_add_btn.connect_clicked(move |_| {
                let cwd = w.state.borrow().cwd().to_path_buf();
                w.open_new_tab(cwd);
            });
        }

        // Cancel active async file operation
        {
            let w = Rc::clone(self);
            cancel_btn.connect_clicked(move |_| {
                if let Some(flag) = w.active_cancel.borrow().as_ref() {
                    flag.store(true, Ordering::Relaxed);
                }
            });
        }

        // Selection change -> status bar update
        {
            let w = Rc::clone(self);
            self.flow_box
                .connect_selected_children_changed(move |_| w.update_status_bar());
        }
        {
            let w = Rc::clone(self);
            self.list_box
                .connect_selected_rows_changed(move |_| w.update_status_bar());
        }

        // Double-click / Enter activation
        {
            let w = Rc::clone(self);
            self.flow_box.connect_child_activated(move |_, child| {
                let idx = child.index();
                if idx >= 0 {
                    w.activate_index(idx as usize);
                }
            });
        }
        {
            let w = Rc::clone(self);
            self.list_box.connect_row_activated(move |_, row| {
                let idx = row.index();
                if idx >= 0 {
                    w.activate_index(idx as usize);
                }
            });
        }

        // Right-click context menu on canvas (Capture phase so FlowBox/ListBox
        // internal controllers never swallow button 3)
        {
            let w = Rc::clone(self);
            let click = GestureClick::new();
            click.set_button(3);
            click.set_propagation_phase(gtk4::PropagationPhase::Capture);
            click.connect_pressed(move |gesture, _, x, y| {
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                w.select_item_at_canvas_pos(x, y);
                w.show_canvas_context_menu(x, y);
            });
            self.canvas_scroll.add_controller(click);
        }

        // Drop target on empty canvas (drop external or dragged files into cwd)
        {
            let w = Rc::clone(self);
            let dt = DropTarget::new(
                gdk::FileList::static_type(),
                gdk::DragAction::COPY | gdk::DragAction::MOVE,
            );
            let scroll = self.canvas_scroll.clone();
            dt.connect_enter(move |_, _, _| {
                scroll.add_css_class("drop-hover");
                gdk::DragAction::MOVE
            });
            let scroll2 = self.canvas_scroll.clone();
            dt.connect_leave(move |_| scroll2.remove_css_class("drop-hover"));
            let scroll3 = self.canvas_scroll.clone();
            dt.connect_drop(move |target, val, _, _| {
                scroll3.remove_css_class("drop-hover");
                let Ok(fl) = val.get::<gdk::FileList>() else {
                    return false;
                };
                let paths: Vec<PathBuf> = fl.files().into_iter().filter_map(|f| f.path()).collect();
                let dst = w.state.borrow().cwd().to_path_buf();
                let is_copy = drop_is_copy(target);
                w.handle_drop(paths, dst, is_copy);
                true
            });
            self.canvas_scroll.add_controller(dt);
        }

        // Global keyboard shortcuts (Capture phase so focused sidebar/header
        // buttons never swallow Space for Quick Look or BackSpace for parent dir)
        {
            let w = Rc::clone(self);
            let key_ctl = EventControllerKey::new();
            key_ctl.set_propagation_phase(gtk4::PropagationPhase::Capture);
            key_ctl.connect_key_pressed(move |_, key, _, mods| w.handle_key(key, mods));
            self.window.add_controller(key_ctl);
        }
    }

    fn handle_key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);

        // If typing in path_entry, filter_entry, or a popover Entry (rename / new folder),
        // let the Entry handle normal typing and only intercept Escape where appropriate.
        if self.path_entry.has_focus() {
            if key == gdk::Key::Escape {
                self.hide_path_entry();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if self.filter_entry.has_focus() {
            if key == gdk::Key::Escape {
                self.filter_entry.set_text("");
                self.filter_entry.set_visible(false);
                self.filter_btn.remove_css_class("active");
                self.focus_canvas();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if gtk4::prelude::GtkWindowExt::focus(&self.window).is_some_and(|w| w.is::<Entry>()) {
            return glib::Propagation::Proceed;
        }

        // Quick Look dismiss, preview scroll, and item stepping
        if self.quicklook_slot.borrow().is_some() {
            match key {
                gdk::Key::Escape | gdk::Key::space => {
                    self.close_quicklook();
                    return glib::Propagation::Stop;
                }
                gdk::Key::Right => {
                    self.step_quicklook(1);
                    return glib::Propagation::Stop;
                }
                gdk::Key::Left => {
                    self.step_quicklook(-1);
                    return glib::Propagation::Stop;
                }
                gdk::Key::Down
                | gdk::Key::Up
                | gdk::Key::Page_Down
                | gdk::Key::Page_Up
                | gdk::Key::Home
                | gdk::Key::End => {
                    if let Some(sc) = self.quicklook_scroll.borrow().as_ref() {
                        let adj = sc.vadjustment();
                        let step = adj.step_increment().max(32.0);
                        let page = adj.page_increment().max(160.0);
                        let cur = adj.value();
                        let max = (adj.upper() - adj.page_size()).max(adj.lower());
                        let next = match key {
                            gdk::Key::Down => (cur + step).min(max),
                            gdk::Key::Up => (cur - step).max(adj.lower()),
                            gdk::Key::Page_Down => (cur + page).min(max),
                            gdk::Key::Page_Up => (cur - page).max(adj.lower()),
                            gdk::Key::Home => adj.lower(),
                            gdk::Key::End => max,
                            _ => cur,
                        };
                        adj.set_value(next);
                        return glib::Propagation::Stop;
                    }
                    match key {
                        gdk::Key::Down => self.step_quicklook(1),
                        gdk::Key::Up => self.step_quicklook(-1),
                        _ => {}
                    }
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            }
        }
        // Properties dismiss
        if self.properties_slot.borrow().is_some() && key == gdk::Key::Escape {
            self.close_properties();
            return glib::Propagation::Stop;
        }

        if ctrl && shift {
            match key {
                gdk::Key::N | gdk::Key::n => {
                    self.prompt_new_folder();
                    return glib::Propagation::Stop;
                }
                gdk::Key::T | gdk::Key::t => {
                    let cwd = self.state.borrow().cwd().to_path_buf();
                    tools::open_terminal_here(&cwd);
                    return glib::Propagation::Stop;
                }
                gdk::Key::ISO_Left_Tab | gdk::Key::Tab => {
                    if self.state.borrow_mut().prev_tab() {
                        self.on_tab_switched();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::S | gdk::Key::s => {
                    let sel = self.selected_paths();
                    if !sel.is_empty() {
                        tools::share_paths(&sel);
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::C | gdk::Key::c => {
                    let sel = self.selected_paths();
                    if !sel.is_empty() {
                        tools::copy_paths_to_clipboard(&sel);
                        self.show_banner("Copied path(s) to clipboard", "ok");
                    }
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        if ctrl {
            match key {
                gdk::Key::l | gdk::Key::L => {
                    self.show_path_entry();
                    return glib::Propagation::Stop;
                }
                gdk::Key::f | gdk::Key::F => {
                    self.filter_entry.set_visible(true);
                    self.filter_entry.grab_focus();
                    return glib::Propagation::Stop;
                }
                gdk::Key::h | gdk::Key::H => {
                    let mut st = self.state.borrow_mut();
                    st.show_hidden = !st.show_hidden;
                    st.recompute_visible();
                    drop(st);
                    self.render_canvas();
                    return glib::Propagation::Stop;
                }
                gdk::Key::_1 => {
                    self.set_view_mode(ViewMode::Grid);
                    return glib::Propagation::Stop;
                }
                gdk::Key::_2 => {
                    self.set_view_mode(ViewMode::List);
                    return glib::Propagation::Stop;
                }
                gdk::Key::a | gdk::Key::A => {
                    match self.state.borrow().view_mode {
                        ViewMode::Grid => self.flow_box.select_all(),
                        ViewMode::List => self.list_box.select_all(),
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::c | gdk::Key::C => {
                    let sel = self.selected_paths();
                    if !sel.is_empty() {
                        tools::write_file_clipboard(&sel, false);
                        self.show_banner(&format!("Copied {} item(s)", sel.len()), "ok");
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::x | gdk::Key::X => {
                    let sel = self.selected_paths();
                    if !sel.is_empty() {
                        tools::write_file_clipboard(&sel, true);
                        self.show_banner(&format!("Cut {} item(s)", sel.len()), "ok");
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::v | gdk::Key::V => {
                    self.paste_from_clipboard();
                    return glib::Propagation::Stop;
                }
                gdk::Key::r | gdk::Key::R => {
                    self.refresh_devices();
                    self.reload_dir();
                    return glib::Propagation::Stop;
                }
                gdk::Key::z | gdk::Key::Z => {
                    self.undo_last();
                    return glib::Propagation::Stop;
                }
                gdk::Key::i | gdk::Key::I => {
                    self.toggle_properties();
                    return glib::Propagation::Stop;
                }
                gdk::Key::b | gdk::Key::B => {
                    self.sidebar_box.set_visible(!self.sidebar_box.is_visible());
                    return glib::Propagation::Stop;
                }
                gdk::Key::t | gdk::Key::T => {
                    let cwd = self.state.borrow().cwd().to_path_buf();
                    self.open_new_tab(cwd);
                    return glib::Propagation::Stop;
                }
                gdk::Key::Tab | gdk::Key::Page_Down => {
                    if self.state.borrow_mut().next_tab() {
                        self.on_tab_switched();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Page_Up => {
                    if self.state.borrow_mut().prev_tab() {
                        self.on_tab_switched();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::w | gdk::Key::W => {
                    let idx = self.state.borrow().active_tab;
                    self.close_tab_at(idx);
                    return glib::Propagation::Stop;
                }
                gdk::Key::q | gdk::Key::Q => {
                    self.window.close();
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        if alt {
            match key {
                gdk::Key::Left => {
                    if self.state.borrow_mut().go_back().is_some() {
                        self.reload_dir();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Right => {
                    if self.state.borrow_mut().go_forward().is_some() {
                        self.reload_dir();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Up => {
                    if self.state.borrow_mut().go_parent().is_some() {
                        self.reload_dir();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    self.toggle_properties();
                    return glib::Propagation::Stop;
                }
                gdk::Key::_1 => return self.jump_tab_shortcut(0),
                gdk::Key::_2 => return self.jump_tab_shortcut(1),
                gdk::Key::_3 => return self.jump_tab_shortcut(2),
                gdk::Key::_4 => return self.jump_tab_shortcut(3),
                gdk::Key::_5 => return self.jump_tab_shortcut(4),
                gdk::Key::_6 => return self.jump_tab_shortcut(5),
                gdk::Key::_7 => return self.jump_tab_shortcut(6),
                gdk::Key::_8 => return self.jump_tab_shortcut(7),
                gdk::Key::_9 => return self.jump_tab_shortcut(8),
                _ => {}
            }
        }

        match key {
            gdk::Key::BackSpace => {
                if self.state.borrow_mut().go_parent().is_some() {
                    self.reload_dir();
                }
                glib::Propagation::Stop
            }
            gdk::Key::space => {
                self.toggle_quicklook();
                glib::Propagation::Stop
            }
            gdk::Key::F2 => {
                if let Some(entry) = self.selected_entries().into_iter().next() {
                    self.prompt_rename(&entry);
                }
                glib::Propagation::Stop
            }
            gdk::Key::F5 => {
                self.refresh_devices();
                self.reload_dir();
                glib::Propagation::Stop
            }
            gdk::Key::F9 => {
                self.sidebar_box.set_visible(!self.sidebar_box.is_visible());
                glib::Propagation::Stop
            }
            gdk::Key::Delete => {
                let sel = self.selected_paths();
                if !sel.is_empty() {
                    if shift || self.state.borrow().in_trash() {
                        self.confirm_permanent_delete(sel);
                    } else {
                        self.run_op(OpKind::Trash, sel, None);
                    }
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn jump_tab_shortcut(self: &Rc<Self>, idx: usize) -> glib::Propagation {
        self.switch_to_tab(idx);
        glib::Propagation::Stop
    }

    pub fn open_new_tab(self: &Rc<Self>, dir: PathBuf) {
        self.state.borrow_mut().new_tab(dir);
        self.on_tab_switched();
    }

    fn switch_to_tab(self: &Rc<Self>, idx: usize) {
        if self.state.borrow_mut().switch_tab(idx) {
            self.on_tab_switched();
        }
    }

    fn close_tab_at(self: &Rc<Self>, idx: usize) {
        if self.state.borrow_mut().close_tab(idx) {
            self.on_tab_switched();
        } else {
            self.window.close();
        }
    }

    fn on_tab_switched(self: &Rc<Self>) {
        let filter = self.state.borrow().tab().filter.clone();
        self.filter_entry.set_text(&filter);
        self.filter_entry.set_visible(!filter.is_empty());
        self.hide_path_entry();
        self.close_quicklook();
        self.close_properties();
        self.render_tabs();
        self.reload_dir();
    }

    fn render_tabs(self: &Rc<Self>) {
        while let Some(child) = self.tabs_box.first_child() {
            self.tabs_box.remove(&child);
        }
        let (tabs, active_idx) = {
            let st = self.state.borrow();
            let info: Vec<(String, &'static str, PathBuf, bool)> = st
                .tabs
                .iter()
                .map(|t| (t.title(), t.icon_glyph(), t.cwd.clone(), t.in_trash))
                .collect();
            (info, st.active_tab)
        };
        if tabs.len() <= 1 {
            self.tab_bar.set_visible(false);
            self.new_tab_btn.set_visible(true);
            return;
        }
        self.tab_bar.set_visible(true);
        self.new_tab_btn.set_visible(false);

        for (i, (title, glyph, cwd, in_trash)) in tabs.into_iter().enumerate() {
            let wrap = GtkBox::new(Orientation::Horizontal, 2);
            wrap.add_css_class("filer-tab");
            if i == active_idx {
                wrap.add_css_class("active");
            }

            let btn_inner = GtkBox::new(Orientation::Horizontal, 6);
            let ic = Label::new(Some(glyph));
            ic.add_css_class("filer-tab-icon");
            let lbl = Label::new(Some(&title));
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            lbl.set_max_width_chars(18);
            lbl.set_halign(Align::Start);
            lbl.set_hexpand(true);
            btn_inner.append(&ic);
            btn_inner.append(&lbl);

            let tab_btn = Button::new();
            tab_btn.set_child(Some(&btn_inner));
            tab_btn.add_css_class("filer-tab-btn");
            tab_btn.set_hexpand(true);
            tab_btn.set_tooltip_text(Some(&cwd.display().to_string()));
            let w_sw = Rc::clone(self);
            tab_btn.connect_clicked(move |_| w_sw.switch_to_tab(i));

            let close_btn = Button::with_label("\u{f00d}");
            close_btn.add_css_class("filer-tab-close");
            close_btn.set_tooltip_text(Some("Close Tab (Ctrl+W)"));
            let w_cl = Rc::clone(self);
            close_btn.connect_clicked(move |_| w_cl.close_tab_at(i));

            // Middle-click on tab closes it
            let mid = GestureClick::new();
            mid.set_button(2);
            let w_mid = Rc::clone(self);
            mid.connect_pressed(move |_, _, _, _| w_mid.close_tab_at(i));
            wrap.add_controller(mid);

            // Drag & drop files directly onto any tab! Hovering it for a moment
            // spring-loads: the tab opens so you can drop deeper.
            let w_spring = Rc::clone(self);
            self.attach_folder_drop_target(
                &wrap,
                cwd,
                in_trash,
                Some(Rc::new(move || w_spring.switch_to_tab(i))),
            );

            wrap.append(&tab_btn);
            wrap.append(&close_btn);
            self.tabs_box.append(&wrap);
        }
    }

    pub fn navigate_to(self: &Rc<Self>, path: PathBuf) {
        let is_trash = is_trash_files_dir(&path);
        self.state.borrow_mut().navigate(path, is_trash);
        self.filter_entry.set_text("");
        self.filter_entry.set_visible(false);
        self.close_quicklook();
        self.close_properties();
        self.reload_dir();
    }

    pub fn reload_dir(self: &Rc<Self>) {
        let (cwd, in_trash, can_back, can_fwd, mode, tab_idx) = {
            let st = self.state.borrow();
            (
                st.cwd().to_path_buf(),
                st.in_trash(),
                st.can_back(),
                st.can_forward(),
                st.view_mode,
                st.active_tab,
            )
        };
        self.back_btn.set_sensitive(can_back);
        self.fwd_btn.set_sensitive(can_fwd);
        self.update_view_buttons(mode);

        let title = if in_trash {
            "Trash — Hayato Files".to_string()
        } else {
            let name = cwd
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_string());
            format!("{name} — Hayato Files")
        };
        self.window.set_title(Some(&title));

        self.render_tabs();
        self.render_breadcrumbs(&cwd, in_trash);
        self.render_places();
        self.render_devices();
        self.watch_dir(&cwd);

        // Ensure Trash/files exists when viewing Trash
        if in_trash {
            let _ = std::fs::create_dir_all(&cwd);
        }

        let (tx, rx) = mpsc::channel();
        let dir_clone = cwd.clone();
        let _ = std::thread::Builder::new()
            .name("hayato-filer-ls".into())
            .spawn(move || {
                let _ = tx.send(list_dir(&dir_clone));
            });

        let w = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(16), move || match rx.try_recv() {
            Ok(Ok(entries)) => {
                let matches = {
                    let st = w.state.borrow();
                    st.active_tab == tab_idx && st.cwd() == cwd
                };
                if matches {
                    w.state.borrow_mut().set_entries(entries);
                    w.render_canvas();
                }
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                w.show_banner(&err, "danger");
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
    }

    // ── Live folder auto-refresh (inotify via GIO) ────────────────────────

    /// Watch the folder we are showing so a file written by a browser, a
    /// terminal or another app appears without an F5. The monitor is replaced
    /// on every navigation, so only the open folder is watched.
    fn watch_dir(self: &Rc<Self>, cwd: &Path) {
        *self.dir_monitor.borrow_mut() = None;
        let Ok(monitor) = gio::File::for_path(cwd)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            if let Some(w) = weak.upgrade() {
                w.schedule_refresh();
            }
        });
        *self.dir_monitor.borrow_mut() = Some(monitor);
    }

    /// Coalesce a burst of filesystem events into one reload.
    fn schedule_refresh(self: &Rc<Self>) {
        if self.refresh_scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(250), move || {
            if let Some(w) = weak.upgrade() {
                w.refresh_scheduled.set(false);
                w.reload_dir();
            }
        });
    }

    // ── Preferences persistence ───────────────────────────────────────────

    fn save_prefs_on_close(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(w) = weak.upgrade() {
                w.persist_prefs();
            }
            glib::Propagation::Proceed
        });
    }

    fn persist_prefs(&self) {
        let width = self.window.width().max(1);
        let height = self.window.height().max(1);
        let prefs = self.state.borrow().to_prefs(width, height);
        prefs::save(&prefs);
    }

    // ── Staging shelf ─────────────────────────────────────────────────────

    fn wire_shelf(
        self: &Rc<Self>,
        move_btn: &Button,
        copy_btn: &Button,
        share_btn: &Button,
        clear_btn: &Button,
        close_btn: &Button,
    ) {
        {
            let w = Rc::clone(self);
            self.shelf_btn.connect_clicked(move |_| w.toggle_shelf());
        }
        {
            let w = Rc::clone(self);
            move_btn.connect_clicked(move |_| {
                let paths = w.shelf.borrow().clone();
                if paths.is_empty() {
                    return;
                }
                let dst = w.state.borrow().cwd().to_path_buf();
                w.shelf.borrow_mut().clear();
                w.render_shelf();
                w.shelf_box.set_visible(false);
                w.shelf_btn.remove_css_class("active");
                w.run_op(OpKind::Move, paths, Some(dst));
            });
        }
        {
            let w = Rc::clone(self);
            copy_btn.connect_clicked(move |_| {
                let paths = w.shelf.borrow().clone();
                if paths.is_empty() {
                    return;
                }
                let dst = w.state.borrow().cwd().to_path_buf();
                w.run_op(OpKind::Copy, paths, Some(dst));
            });
        }
        {
            let w = Rc::clone(self);
            share_btn.connect_clicked(move |_| {
                let paths = w.shelf.borrow().clone();
                if !paths.is_empty() {
                    tools::share_paths(&paths);
                }
            });
        }
        {
            let w = Rc::clone(self);
            let b = self.shelf_box.clone();
            clear_btn.connect_clicked(move |_| {
                w.shelf.borrow_mut().clear();
                w.render_shelf();
                b.set_visible(false);
                w.shelf_btn.remove_css_class("active");
            });
        }
        {
            let w = Rc::clone(self);
            let b = self.shelf_box.clone();
            close_btn.connect_clicked(move |_| {
                b.set_visible(false);
                w.shelf_btn.remove_css_class("active");
            });
        }

        // Drop files onto the header Shelf button even while the shelf is closed!
        {
            let dt = DropTarget::new(
                gdk::FileList::static_type(),
                gdk::DragAction::COPY | gdk::DragAction::MOVE,
            );
            let sb = self.shelf_btn.clone();
            dt.connect_enter(move |_, _, _| {
                sb.add_css_class("drop-hover");
                gdk::DragAction::COPY
            });
            let sb2 = self.shelf_btn.clone();
            dt.connect_leave(move |_| sb2.remove_css_class("drop-hover"));
            let w = Rc::clone(self);
            let sb3 = self.shelf_btn.clone();
            dt.connect_drop(move |_, val, _, _| {
                sb3.remove_css_class("drop-hover");
                let Ok(fl) = val.get::<gdk::FileList>() else {
                    return false;
                };
                let paths: Vec<PathBuf> = fl.files().into_iter().filter_map(|f| f.path()).collect();
                w.add_to_shelf(paths);
                true
            });
            self.shelf_btn.add_controller(dt);
        }

        // Drop files onto the shelf to collect them.
        let dt = DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        let sb = self.shelf_box.clone();
        dt.connect_enter(move |_, _, _| {
            sb.add_css_class("drop-hover");
            gdk::DragAction::COPY
        });
        let sb2 = self.shelf_box.clone();
        dt.connect_leave(move |_| sb2.remove_css_class("drop-hover"));
        let w = Rc::clone(self);
        let sb3 = self.shelf_box.clone();
        dt.connect_drop(move |_, val, _, _| {
            sb3.remove_css_class("drop-hover");
            let Ok(fl) = val.get::<gdk::FileList>() else {
                return false;
            };
            let paths: Vec<PathBuf> = fl.files().into_iter().filter_map(|f| f.path()).collect();
            w.add_to_shelf(paths);
            true
        });
        self.shelf_box.add_controller(dt);

        // Drag the whole shelf out as one batch.
        let src = DragSource::new();
        src.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let w = Rc::clone(self);
        src.connect_prepare(move |_, _, _| {
            let paths = w.shelf.borrow().clone();
            if paths.is_empty() {
                return None;
            }
            *w.drag_hold.borrow_mut() = Some(w.shelf_chips.clone().upcast::<Widget>());
            let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
            let list = gdk::FileList::from_array(&files);
            Some(gdk::ContentProvider::for_value(&list.to_value()))
        });
        let w = Rc::clone(self);
        src.connect_drag_end(move |_, _, _| {
            *w.drag_hold.borrow_mut() = None;
        });
        self.shelf_box.add_controller(src);
    }

    fn toggle_shelf(self: &Rc<Self>) {
        let vis = !self.shelf_box.is_visible();
        self.shelf_box.set_visible(vis);
        if vis {
            self.shelf_btn.add_css_class("active");
            self.render_shelf();
        } else {
            self.shelf_btn.remove_css_class("active");
        }
    }

    fn add_to_shelf(self: &Rc<Self>, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        {
            let mut shelf = self.shelf.borrow_mut();
            for p in paths {
                if !shelf.contains(&p) {
                    shelf.push(p);
                }
            }
        }
        self.render_shelf();
        self.shelf_box.set_visible(true);
        self.shelf_btn.add_css_class("active");
    }

    fn remove_from_shelf(self: &Rc<Self>, path: &Path) {
        self.shelf.borrow_mut().retain(|p| p != path);
        self.render_shelf();
        if self.shelf.borrow().is_empty() {
            self.shelf_box.set_visible(false);
            self.shelf_btn.remove_css_class("active");
        }
    }

    fn render_shelf(self: &Rc<Self>) {
        // A chip names something that exists: a reference whose file was
        // moved away, trashed or deleted since it was collected is dropped
        // here, so dragging a chip never carries a path that is gone.
        self.shelf
            .borrow_mut()
            .retain(|p| std::fs::symlink_metadata(p).is_ok());
        while let Some(c) = self.shelf_chips.first_child() {
            self.shelf_chips.remove(&c);
        }
        let paths = self.shelf.borrow().clone();
        if paths.is_empty() {
            self.shelf_badge.set_visible(false);
        } else {
            self.shelf_badge.set_text(&paths.len().to_string());
            self.shelf_badge.set_visible(true);
        }
        if paths.is_empty() {
            let hint = Label::new(Some(
                "Drop files here to collect them, then drag the batch out",
            ));
            hint.add_css_class("filer-shelf-hint");
            self.shelf_chips.append(&hint);
            return;
        }
        for p in paths {
            let chip = GtkBox::new(Orientation::Horizontal, 4);
            chip.add_css_class("filer-shelf-chip");
            let is_dir = p.is_dir();
            let glyph = Label::new(Some(if is_dir { "\u{f07b}" } else { "\u{f15b}" }));
            glyph.add_css_class("filer-shelf-glyph");
            glyph.add_css_class(if is_dir { "icon-folder" } else { "icon-file" });
            let name = Label::new(Some(
                &p.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string()),
            ));
            name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            name.set_max_width_chars(18);
            let rm = Button::with_label("\u{f00d}");
            rm.add_css_class("filer-shelf-remove");
            rm.add_css_class("filer-glyph");
            rm.set_tooltip_text(Some(&p.display().to_string()));
            let w = Rc::clone(self);
            let p_rm = p.clone();
            rm.connect_clicked(move |_| w.remove_from_shelf(&p_rm));
            chip.append(&glyph);
            chip.append(&name);
            chip.append(&rm);
            self.shelf_chips.append(&chip);
        }
    }

    fn focus_canvas(&self) {
        match self.state.borrow().view_mode {
            ViewMode::Grid => {
                self.flow_box.grab_focus();
            }
            ViewMode::List => {
                self.list_box.grab_focus();
            }
        }
    }

    fn wrap_modal_scrim(card: GtkBox, on_close: impl Fn() + 'static) -> GtkBox {
        let backdrop = GtkBox::new(Orientation::Vertical, 0);
        backdrop.add_css_class("filer-backdrop");
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);
        backdrop.set_halign(Align::Fill);
        backdrop.set_valign(Align::Fill);

        card.set_halign(Align::Center);
        card.set_valign(Align::Center);
        card.set_vexpand(true);
        backdrop.append(&card);

        let click = GestureClick::new();
        click.set_button(1);
        let card_ref = card.clone();
        click.connect_released(move |_, _, x, y| {
            let alloc = card_ref.allocation();
            let inside = x >= f64::from(alloc.x())
                && x <= f64::from(alloc.x() + alloc.width())
                && y >= f64::from(alloc.y())
                && y <= f64::from(alloc.y() + alloc.height());
            if !inside {
                on_close();
            }
        });
        backdrop.add_controller(click);
        backdrop
    }

    // ── Quick Properties card ─────────────────────────────────────────────

    fn toggle_properties(self: &Rc<Self>) {
        if self.properties_slot.borrow().is_some() {
            self.close_properties();
            return;
        }
        let Some(entry) = self.selected_entries().into_iter().next() else {
            return;
        };
        self.close_quicklook();
        let w = Rc::clone(self);
        let card = properties::build_properties_card(&entry, move || w.close_properties());
        let w2 = Rc::clone(self);
        let modal = Self::wrap_modal_scrim(card, move || w2.close_properties());
        self.overlay.add_overlay(&modal);
        *self.properties_slot.borrow_mut() = Some(modal);
    }

    fn close_properties(&self) {
        if let Some(card) = self.properties_slot.borrow_mut().take() {
            self.overlay.remove_overlay(&card);
        }
    }

    fn set_view_mode(self: &Rc<Self>, mode: ViewMode) {
        self.state.borrow_mut().view_mode = mode;
        self.update_view_buttons(mode);
        self.list_header.set_visible(mode == ViewMode::List);
        self.rebuild_list_header();
        self.render_canvas();
    }

    fn rebuild_list_header(self: &Rc<Self>) {
        while let Some(c) = self.list_header.first_child() {
            self.list_header.remove(&c);
        }
        *self.col_size_btn.borrow_mut() = None;
        *self.col_mtime_btn.borrow_mut() = None;
        *self.col_kind_btn.borrow_mut() = None;

        let (active_field, order, size_w, mtime_w, kind_w) = {
            let st = self.state.borrow();
            (
                st.sort_field,
                st.sort_order,
                st.col_size_width,
                st.col_mtime_width,
                st.col_kind_width,
            )
        };
        let arrow = |f: SortField| -> &'static str {
            if f == active_field {
                match order {
                    SortOrder::Asc => " ▲",
                    SortOrder::Desc => " ▼",
                }
            } else {
                ""
            }
        };

        // Match build_list_row's 22px glyph + 10px box spacing so headers align with rows.
        let spacer = Label::new(None);
        spacer.set_size_request(32, -1);
        self.list_header.append(&spacer);

        let cols: [(SortField, &str, bool, i32); 4] = [
            (SortField::Name, "NAME", true, -1),
            (SortField::Size, "SIZE", false, size_w),
            (SortField::Modified, "MODIFIED", false, mtime_w),
            (SortField::Kind, "TYPE", false, kind_w),
        ];
        for (idx, (field, title, expand, col_w)) in cols.into_iter().enumerate() {
            if idx > 0 {
                let handle = GtkBox::new(Orientation::Horizontal, 0);
                handle.add_css_class("filer-col-resizer");
                handle.set_size_request(10, -1);
                handle.set_cursor_from_name(Some("col-resize"));
                handle.set_tooltip_text(Some("Drag to resize column"));

                let drag = GestureDrag::new();
                let start_widths = Rc::new(Cell::new((0i32, 0i32, 0i32)));
                let sw_begin = Rc::clone(&start_widths);
                let w_begin = Rc::clone(self);
                let h_begin = handle.clone();
                drag.connect_drag_begin(move |_, _, _| {
                    h_begin.add_css_class("dragging");
                    let st = w_begin.state.borrow();
                    sw_begin.set((st.col_size_width, st.col_mtime_width, st.col_kind_width));
                });

                let sw_upd = Rc::clone(&start_widths);
                let w_upd = Rc::clone(self);
                drag.connect_drag_update(move |_, dx, _| {
                    let (s0, m0, k0) = sw_upd.get();
                    let dx_i = dx.round() as i32;
                    let (min_w, max_w) = (prefs::MIN_COL_WIDTH, prefs::MAX_COL_WIDTH);
                    let (s1, m1, k1) = match field {
                        // Border Name | Size: moving right gives width to Name, shrinks Size.
                        SortField::Size => ((s0 - dx_i).clamp(min_w, max_w), m0, k0),
                        // Border Size | Modified: tracks pointer 1:1 between Size and Modified.
                        SortField::Modified => {
                            let min_dx = (min_w - s0).max(m0 - max_w);
                            let max_dx = (max_w - s0).min(m0 - min_w);
                            let d = if min_dx <= max_dx {
                                dx_i.clamp(min_dx, max_dx)
                            } else {
                                0
                            };
                            (s0 + d, m0 - d, k0)
                        }
                        // Border Modified | Type: tracks pointer 1:1 between Modified and Type.
                        SortField::Kind => {
                            let min_dx = (min_w - m0).max(k0 - max_w);
                            let max_dx = (max_w - m0).min(k0 - min_w);
                            let d = if min_dx <= max_dx {
                                dx_i.clamp(min_dx, max_dx)
                            } else {
                                0
                            };
                            (s0, m0 + d, k0 - d)
                        }
                        SortField::Name => (s0, m0, k0),
                    };
                    w_upd.apply_column_widths(s1, m1, k1);
                });

                let w_end = Rc::clone(self);
                let h_end = handle.clone();
                drag.connect_drag_end(move |_, _, _| {
                    h_end.remove_css_class("dragging");
                    w_end.persist_prefs();
                });

                handle.add_controller(drag);
                self.list_header.append(&handle);
            }

            let lbl = Label::new(Some(&format!("{title}{}", arrow(field))));
            lbl.set_halign(if expand { Align::Start } else { Align::End });
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            let btn = Button::new();
            btn.set_child(Some(&lbl));
            btn.add_css_class("filer-col-btn");
            if col_w > 0 {
                btn.set_size_request(col_w, -1);
            }
            match field {
                SortField::Size => {
                    self.col_size_sg.add_widget(&btn);
                    *self.col_size_btn.borrow_mut() = Some(btn.clone());
                }
                SortField::Modified => {
                    self.col_mtime_sg.add_widget(&btn);
                    *self.col_mtime_btn.borrow_mut() = Some(btn.clone());
                }
                SortField::Kind => {
                    self.col_kind_sg.add_widget(&btn);
                    *self.col_kind_btn.borrow_mut() = Some(btn.clone());
                }
                SortField::Name => {}
            }
            if field == active_field {
                btn.add_css_class("active");
            }
            if expand {
                btn.set_hexpand(true);
            }
            let w = Rc::clone(self);
            btn.connect_clicked(move |_| {
                {
                    let mut st = w.state.borrow_mut();
                    if st.sort_field == field {
                        st.sort_order = st.sort_order.toggle();
                    } else {
                        st.sort_field = field;
                        st.sort_order = SortOrder::Asc;
                    }
                    st.recompute_visible();
                }
                w.rebuild_list_header();
                w.render_canvas();
            });
            self.list_header.append(&btn);
        }
    }

    fn apply_column_widths(&self, size_w: i32, mtime_w: i32, kind_w: i32) {
        {
            let mut st = self.state.borrow_mut();
            st.col_size_width = size_w;
            st.col_mtime_width = mtime_w;
            st.col_kind_width = kind_w;
        }
        if let Some(b) = self.col_size_btn.borrow().as_ref() {
            b.set_size_request(size_w, -1);
        }
        if let Some(b) = self.col_mtime_btn.borrow().as_ref() {
            b.set_size_request(mtime_w, -1);
        }
        if let Some(b) = self.col_kind_btn.borrow().as_ref() {
            b.set_size_request(kind_w, -1);
        }
    }

    fn update_view_buttons(&self, mode: ViewMode) {
        if mode == ViewMode::Grid {
            self.grid_btn.add_css_class("active");
            self.list_btn.remove_css_class("active");
        } else {
            self.list_btn.add_css_class("active");
            self.grid_btn.remove_css_class("active");
        }
    }

    fn show_path_entry(&self) {
        let cwd = self.state.borrow().cwd().display().to_string();
        self.crumbs_scroll.set_visible(false);
        self.path_entry.set_text(&cwd);
        self.path_entry.set_visible(true);
        self.path_entry.grab_focus();
        self.path_entry.select_region(0, -1);
    }

    fn hide_path_entry(&self) {
        self.path_entry.set_visible(false);
        self.crumbs_scroll.set_visible(true);
    }

    fn render_breadcrumbs(self: &Rc<Self>, cwd: &Path, in_trash: bool) {
        while let Some(child) = self.crumbs_box.first_child() {
            self.crumbs_box.remove(&child);
        }
        if in_trash {
            let btn = Button::with_label("Trash");
            btn.add_css_class("filer-crumb");
            btn.add_css_class("current");
            self.crumbs_box.append(&btn);
            return;
        }

        let home = home_dir();
        let mut crumbs: Vec<(String, PathBuf)> = Vec::new();
        if let Ok(rel) = cwd.strip_prefix(&home) {
            crumbs.push(("~".to_string(), home.clone()));
            let mut acc = home;
            for comp in rel.components() {
                let s = comp.as_os_str().to_string_lossy().into_owned();
                acc = acc.join(&s);
                crumbs.push((s, acc.clone()));
            }
        } else {
            crumbs.push(("/".to_string(), PathBuf::from("/")));
            let mut acc = PathBuf::from("/");
            for comp in cwd.components().skip(1) {
                let s = comp.as_os_str().to_string_lossy().into_owned();
                acc = acc.join(&s);
                crumbs.push((s, acc.clone()));
            }
        }

        let last = crumbs.len().saturating_sub(1);
        for (i, (label, path)) in crumbs.into_iter().enumerate() {
            if i > 0 {
                let sep = Label::new(Some("›"));
                sep.add_css_class("filer-crumb-sep");
                self.crumbs_box.append(&sep);
            }
            let btn = Button::with_label(&label);
            btn.add_css_class("filer-crumb");
            if i == last {
                btn.add_css_class("current");
            }
            let w = Rc::clone(self);
            let target = path.clone();
            btn.connect_clicked(move |_| w.navigate_to(target.clone()));

            // Middle-click opens breadcrumb folder in a new tab
            let mid = GestureClick::new();
            mid.set_button(2);
            let w_mid = Rc::clone(self);
            let t_mid = path.clone();
            mid.connect_pressed(move |_, _, _, _| w_mid.open_new_tab(t_mid.clone()));
            btn.add_controller(mid);

            // Breadcrumb DropTarget: drag files onto any ancestor folder!
            let w_spring = Rc::clone(self);
            let p_spring = path.clone();
            self.attach_folder_drop_target(
                &btn,
                path,
                false,
                Some(Rc::new(move || w_spring.navigate_to(p_spring.clone()))),
            );
            self.crumbs_box.append(&btn);
        }

        // Scroll to the rightmost crumb once allocated so the current folder is always in view.
        let adj = self.crumbs_scroll.hadjustment();
        glib::idle_add_local_once(move || {
            let max = (adj.upper() - adj.page_size()).max(0.0);
            adj.set_value(max);
        });
    }

    fn render_places(self: &Rc<Self>) {
        while let Some(child) = self.places_box.first_child() {
            self.places_box.remove(&child);
        }
        let home = home_dir();
        let cur_cwd = self.state.borrow().cwd().to_path_buf();
        let cur_trash = self.state.borrow().in_trash();

        // Home is always static and fixed to the first entry (never draggable/reorderable).
        let home_active = !cur_trash && cur_cwd == home;
        let home_row = self.make_place_button("\u{f015}", "Home", home, false, home_active, None);
        self.places_box.append(&home_row);

        let items = self.state.borrow().ordered_places();
        for item in items {
            let active = if item.is_trash {
                cur_trash
            } else {
                !cur_trash && cur_cwd == item.path
            };
            let badge = if item.is_trash {
                let n = trash::trash_count();
                (n > 0).then(|| n.to_string())
            } else {
                None
            };
            let row = self.make_place_button(
                &item.glyph,
                &item.name,
                item.path.clone(),
                item.is_trash,
                active,
                badge.as_deref(),
            );
            self.attach_place_reorder(&row, item.key);
            if item.is_bookmark {
                self.attach_unpin_menu(&row, item.path);
            }
            self.places_box.append(&row);
        }
    }

    /// Drag-and-drop reordering strictly within the PLACES column.
    /// Uses a private `String` payload (`hayato-place-reorder:<key>`), never
    /// `gdk::FileList`, so a dragged Place/Pin is rejected everywhere outside
    /// PLACES (cannot be dropped onto the folder canvas, tabs, or devices).
    fn attach_place_reorder(self: &Rc<Self>, btn: &Button, key: String) {
        const PREFIX: &str = "hayato-place-reorder:";

        let src = DragSource::new();
        src.set_actions(gdk::DragAction::MOVE);
        let payload = format!("{PREFIX}{key}");
        src.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&payload.to_value()))
        });
        btn.add_controller(src);

        let dt = DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
        let b_enter = btn.clone();
        dt.connect_enter(move |_, _, _| {
            b_enter.add_css_class("reorder-hover");
            gdk::DragAction::MOVE
        });
        let b_leave = btn.clone();
        dt.connect_leave(move |_| b_leave.remove_css_class("reorder-hover"));
        let b_drop = btn.clone();
        let w = Rc::clone(self);
        let target_key = key;
        dt.connect_drop(move |_, val, _, _| {
            b_drop.remove_css_class("reorder-hover");
            let Ok(raw) = val.get::<String>() else {
                return false;
            };
            let Some(from_key) = raw.strip_prefix(PREFIX) else {
                return false;
            };
            if w.state.borrow_mut().reorder_place(from_key, &target_key) {
                w.render_places();
                return true;
            }
            false
        });
        btn.add_controller(dt);
    }

    fn toggle_sidebar_compact(self: &Rc<Self>) {
        let compact = !self.state.borrow().sidebar_compact;
        self.set_sidebar_compact(compact);
    }

    fn set_sidebar_compact(self: &Rc<Self>, compact: bool) {
        self.state.borrow_mut().sidebar_compact = compact;
        if compact {
            self.sidebar_box.add_css_class("compact");
            self.sidebar_box.set_size_request(50, -1);
            self.paned.set_position(52);
            self.places_hdr.set_visible(false);
            self.devices_hdr.set_visible(false);
            self.sidebar_sep
                .set_visible(!self.devices.borrow().is_empty());
            self.sidebar_toggle_btn.add_css_class("active");
        } else {
            self.sidebar_box.remove_css_class("compact");
            self.sidebar_box.set_size_request(192, -1);
            self.paned.set_position(196);
            self.places_hdr.set_visible(true);
            self.devices_hdr.set_visible(true);
            self.sidebar_sep.set_visible(false);
            self.sidebar_toggle_btn.remove_css_class("active");
        }
        self.render_places();
        self.render_devices();
    }

    fn make_place_button(
        self: &Rc<Self>,
        glyph: &str,
        name: &str,
        path: PathBuf,
        is_trash: bool,
        active: bool,
        badge: Option<&str>,
    ) -> Button {
        let compact = self.state.borrow().sidebar_compact;
        let inner = GtkBox::new(Orientation::Horizontal, if compact { 0 } else { 8 });
        if compact {
            inner.set_halign(Align::Center);
        }
        let g = Label::new(Some(glyph));
        g.add_css_class("filer-place-icon");
        g.set_size_request(18, -1);
        g.set_halign(Align::Center);
        inner.append(&g);

        if !compact {
            let lbl = Label::new(Some(name));
            lbl.set_hexpand(true);
            lbl.set_halign(Align::Start);
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            inner.append(&lbl);
            if let Some(b) = badge {
                let pill = Label::new(Some(b));
                pill.add_css_class("filer-badge");
                inner.append(&pill);
            }
        }

        let btn = Button::new();
        btn.set_child(Some(&inner));
        btn.add_css_class("filer-place-row");
        if active {
            btn.add_css_class("active");
        }
        if compact {
            let tip = match badge {
                Some(b) => format!("{name} ({b})"),
                None => name.to_string(),
            };
            btn.set_tooltip_text(Some(&tip));
        }
        let w = Rc::clone(self);
        let target = path.clone();
        btn.connect_clicked(move |_| w.navigate_to(target.clone()));

        // Middle-click opens place in a new tab
        let mid = GestureClick::new();
        mid.set_button(2);
        let w_mid = Rc::clone(self);
        let t_mid = path.clone();
        mid.connect_pressed(move |_, _, _, _| w_mid.open_new_tab(t_mid.clone()));
        btn.add_controller(mid);

        self.attach_folder_drop_target(&btn, path, is_trash, None);
        btn
    }

    /// Right-click on a pinned bookmark offers Open in New Tab / Unpin.
    fn attach_unpin_menu(self: &Rc<Self>, btn: &Button, path: PathBuf) {
        let rclick = GestureClick::new();
        rclick.set_button(3);
        rclick.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let w_rc = Rc::clone(self);
        let anchor = btn.clone();
        rclick.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            w_rc.show_bookmark_menu(&anchor, &path, x, y);
        });
        btn.add_controller(rclick);
    }

    fn show_bookmark_menu(self: &Rc<Self>, anchor: &Button, dir: &Path, x: f64, y: f64) {
        let pop = Popover::new();
        pop.set_parent(anchor);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.set_has_arrow(false);
        unparent_on_close(&pop);
        let col = GtkBox::new(Orientation::Vertical, 2);

        let w = Rc::clone(self);
        let d_tab = dir.to_path_buf();
        let p1 = pop.clone();
        col.append(&make_menu_item(
            "\u{f067}",
            "Open in New Tab",
            None,
            false,
            move || {
                p1.popdown();
                w.open_new_tab(d_tab.clone());
            },
        ));

        let w = Rc::clone(self);
        let d_un = dir.to_path_buf();
        let p2 = pop.clone();
        col.append(&make_menu_item(
            "\u{f00d}",
            "Unpin from Places",
            None,
            false,
            move || {
                p2.popdown();
                if w.state.borrow_mut().unpin_bookmark(&d_un) {
                    w.render_places();
                    w.show_banner("Unpinned from Places", "ok");
                }
            },
        ));

        pop.set_child(Some(&col));
        pop.popup();
    }

    /// Dropping a folder on the PLACES heading pins it, rather than moving it.
    fn wire_pin_drop_target(self: &Rc<Self>) {
        self.places_hdr
            .set_tooltip_text(Some("Drop a folder here to pin it to PLACES"));
        let dt = DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        let hdr = self.places_hdr.clone();
        dt.connect_enter(move |_, _, _| {
            hdr.add_css_class("drop-hover");
            gdk::DragAction::COPY
        });
        let hdr2 = self.places_hdr.clone();
        dt.connect_leave(move |_| hdr2.remove_css_class("drop-hover"));
        let w = Rc::clone(self);
        let hdr3 = self.places_hdr.clone();
        dt.connect_drop(move |_, val, _, _| {
            hdr3.remove_css_class("drop-hover");
            let Ok(fl) = val.get::<gdk::FileList>() else {
                return false;
            };
            let dirs: Vec<PathBuf> = fl
                .files()
                .into_iter()
                .filter_map(|f| f.path())
                .filter(|p| p.is_dir())
                .collect();
            let mut pinned = 0usize;
            for d in dirs {
                if w.state.borrow_mut().pin_bookmark(d) {
                    pinned += 1;
                }
            }
            if pinned > 0 {
                w.render_places();
                w.show_banner(&format!("Pinned {pinned} folder(s) to Places"), "ok");
            }
            pinned > 0
        });
        self.places_hdr.add_controller(dt);
    }

    /// Dynamically watches for USB/SD hotplug and mount/unmount events using both
    /// GIO's `VolumeMonitor` (instant DBus/mountinfo signals) and a lightweight
    /// sysfs/procfs signature check (zero subprocesses until a change occurs).
    fn start_device_monitor(self: &Rc<Self>) {
        let trigger = |weak: std::rc::Weak<Self>| {
            if let Some(w) = weak.upgrade() {
                w.device_sig.set(devices::block_mount_signature());
                w.refresh_devices();
            }
        };

        let weak = Rc::downgrade(self);
        self._volume_monitor
            .connect_volume_added(move |_, _| trigger(weak.clone()));
        let weak = Rc::downgrade(self);
        self._volume_monitor
            .connect_volume_removed(move |_, _| trigger(weak.clone()));
        let weak = Rc::downgrade(self);
        self._volume_monitor
            .connect_mount_added(move |_, _| trigger(weak.clone()));
        let weak = Rc::downgrade(self);
        self._volume_monitor
            .connect_mount_removed(move |_, _| trigger(weak.clone()));
        let weak = Rc::downgrade(self);
        self._volume_monitor
            .connect_drive_connected(move |_, _| trigger(weak.clone()));
        let weak = Rc::downgrade(self);
        self._volume_monitor
            .connect_drive_disconnected(move |_, _| trigger(weak.clone()));

        // Lightweight kernel sysfs/procfs fallback (works even without gvfs)
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(1500), move || {
            let Some(w) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !w.window.is_visible() {
                return glib::ControlFlow::Break;
            }
            let sig = devices::block_mount_signature();
            if sig != w.device_sig.get() {
                w.device_sig.set(sig);
                w.refresh_devices();
            }
            glib::ControlFlow::Continue
        });
    }

    pub fn refresh_devices(self: &Rc<Self>) {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("hayato-filer-devs".into())
            .spawn(move || {
                let _ = tx.send(devices::list_removable_devices());
            });
        let w = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(30), move || match rx.try_recv() {
            Ok(devs) => {
                let prev = w.devices.borrow().clone();
                let was_loaded = w.devices_loaded.replace(true);
                if was_loaded && devs != prev {
                    // Announce newly connected drives
                    for d in &devs {
                        if !prev.iter().any(|p| p.dev_path == d.dev_path) {
                            w.show_banner(&format!("Drive connected: {}", d.label), "ok");
                        }
                    }
                    // Handle disconnected or externally unmounted drives
                    let mut active_affected = false;
                    for p in &prev {
                        if let Some(old_mp) = &p.mountpoint {
                            let still_mounted = devs
                                .iter()
                                .any(|d| d.mountpoint.as_deref() == Some(old_mp.as_path()));
                            if !still_mounted {
                                let mut st = w.state.borrow_mut();
                                let active = st.active_tab;
                                for (i, tab) in st.tabs.iter_mut().enumerate() {
                                    if tab.cwd.starts_with(old_mp) {
                                        tab.cwd = home_dir();
                                        tab.in_trash = false;
                                        if i == active {
                                            active_affected = true;
                                        }
                                    }
                                }
                            }
                        }
                        if !devs.iter().any(|d| d.dev_path == p.dev_path) {
                            w.show_banner(&format!("Drive disconnected: {}", p.label), "warn");
                        }
                    }
                    *w.devices.borrow_mut() = devs;
                    w.render_devices();
                    if active_affected {
                        w.reload_dir();
                    }
                } else if !was_loaded {
                    *w.devices.borrow_mut() = devs;
                    w.render_devices();
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
    }

    fn render_devices(self: &Rc<Self>) {
        while let Some(child) = self.devices_box.first_child() {
            self.devices_box.remove(&child);
        }
        let compact = self.state.borrow().sidebar_compact;
        let devs = self.devices.borrow().clone();
        self.sidebar_sep.set_visible(compact && !devs.is_empty());
        if devs.is_empty() {
            if !compact {
                let empty = Label::new(Some("No removable drives"));
                empty.add_css_class("filer-item-meta");
                empty.set_halign(Align::Start);
                empty.set_margin_start(8);
                self.devices_box.append(&empty);
            }
            return;
        }
        let cur_cwd = self.state.borrow().cwd().to_path_buf();
        for dev in devs {
            let wrap = GtkBox::new(Orientation::Vertical, 3);
            wrap.add_css_class("filer-device-card");
            if let Some(mp) = &dev.mountpoint
                && cur_cwd.starts_with(mp)
            {
                wrap.add_css_class("active");
            }

            let row = GtkBox::new(
                if compact {
                    Orientation::Vertical
                } else {
                    Orientation::Horizontal
                },
                if compact { 2 } else { 4 },
            );
            let main_btn = Button::new();
            main_btn.set_hexpand(true);
            main_btn.add_css_class("filer-device-btn");
            let lbl_box = GtkBox::new(Orientation::Horizontal, if compact { 0 } else { 8 });
            if compact {
                lbl_box.set_halign(Align::Center);
            }
            let icon = Label::new(Some("\u{f0a0}"));
            icon.add_css_class("filer-place-icon");
            icon.set_size_request(18, -1);
            icon.set_halign(Align::Center);
            lbl_box.append(&icon);
            if !compact {
                let name = Label::new(Some(&dev.label));
                name.set_halign(Align::Start);
                name.set_hexpand(true);
                name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                lbl_box.append(&name);
            } else {
                let tip = if dev.is_mounted() {
                    format!("{} (Mounted — click to open)", dev.label)
                } else {
                    format!("{} (Click to mount)", dev.label)
                };
                main_btn.set_tooltip_text(Some(&tip));
            }
            main_btn.set_child(Some(&lbl_box));

            let w_click = Rc::clone(self);
            let dev_click = dev.clone();
            main_btn.connect_clicked(move |_| {
                if let Some(mp) = &dev_click.mountpoint {
                    w_click.navigate_to(mp.clone());
                } else {
                    w_click.mount_and_open(&dev_click.dev_path, &dev_click.label);
                }
            });
            if let Some(mp) = &dev.mountpoint {
                let mid = GestureClick::new();
                mid.set_button(2);
                let w_mid = Rc::clone(self);
                let mp_mid = mp.clone();
                mid.connect_pressed(move |_, _, _, _| w_mid.open_new_tab(mp_mid.clone()));
                main_btn.add_controller(mid);
            }
            row.append(&main_btn);

            if dev.is_mounted() {
                let eject_btn = Button::with_label("\u{f052}");
                eject_btn.add_css_class("filer-dev-action");
                eject_btn.add_css_class("filer-glyph");
                eject_btn.set_tooltip_text(Some("Safely Eject"));
                let w_ej = Rc::clone(self);
                let dev_ej = dev.clone();
                eject_btn.connect_clicked(move |_| w_ej.eject_drive(dev_ej.clone()));
                row.append(&eject_btn);
            } else {
                let mnt_btn = Button::with_label("\u{f04b}");
                mnt_btn.add_css_class("filer-dev-action");
                mnt_btn.add_css_class("filer-glyph");
                mnt_btn.set_tooltip_text(Some("Mount Drive"));
                let w_mn = Rc::clone(self);
                let dp = dev.dev_path.clone();
                let dl = dev.label.clone();
                mnt_btn.connect_clicked(move |_| w_mn.mount_and_open(&dp, &dl));
                row.append(&mnt_btn);
            }
            wrap.append(&row);

            if let Some(mp) = &dev.mountpoint {
                if !compact {
                    let bar = ProgressBar::new();
                    bar.add_css_class("filer-usage-bar");
                    bar.set_fraction(dev.usage_fraction());
                    bar.set_margin_start(6);
                    bar.set_margin_end(6);
                    wrap.append(&bar);
                }
                self.attach_folder_drop_target(&wrap, mp.clone(), false, None);
            }
            // Right-click context menu on removable drive card
            {
                let rclick = GestureClick::new();
                rclick.set_button(3);
                rclick.set_propagation_phase(gtk4::PropagationPhase::Capture);
                let w_rc = Rc::clone(self);
                let dev_rc = dev.clone();
                let anchor = wrap.clone();
                rclick.connect_pressed(move |gesture, _, x, y| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    w_rc.show_device_context_menu(&anchor, &dev_rc, x, y);
                });
                wrap.add_controller(rclick);
            }

            self.devices_box.append(&wrap);
        }
    }

    fn show_device_context_menu(
        self: &Rc<Self>,
        anchor: &GtkBox,
        dev: &DeviceEntry,
        x: f64,
        y: f64,
    ) {
        let pop = Popover::new();
        pop.set_parent(anchor);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.set_has_arrow(false);
        unparent_on_close(&pop);

        let col = GtkBox::new(Orientation::Vertical, 2);
        let add = |icon: &str, label: &str, danger: bool, cb: Box<dyn Fn()>| {
            let p = pop.clone();
            let b = make_menu_item(icon, label, None, danger, move || {
                p.popdown();
                cb();
            });
            col.append(&b);
        };

        if let Some(mp) = dev.mountpoint.clone() {
            let w = Rc::clone(self);
            let mp1 = mp.clone();
            add(
                "\u{f07c}",
                "Open",
                false,
                Box::new(move || w.navigate_to(mp1.clone())),
            );
            let w = Rc::clone(self);
            add(
                "\u{f067}",
                "Open in New Tab",
                false,
                Box::new(move || w.open_new_tab(mp.clone())),
            );
            let w = Rc::clone(self);
            let d_ej = dev.clone();
            add(
                "\u{f052}",
                "Safely Eject",
                false,
                Box::new(move || w.eject_drive(d_ej.clone())),
            );
        } else {
            let w = Rc::clone(self);
            let dp = dev.dev_path.clone();
            let dl = dev.label.clone();
            add(
                "\u{f04b}",
                "Mount & Open",
                false,
                Box::new(move || w.mount_and_open(&dp, &dl)),
            );
        }

        let sep = Separator::new(Orientation::Horizontal);
        sep.add_css_class("filer-menu-sep");
        col.append(&sep);

        add(
            "\u{f0a0}",
            "Format Drive…",
            true,
            Box::new(tools::open_format_drive),
        );

        pop.set_child(Some(&col));
        pop.popup();
    }

    fn mount_and_open(self: &Rc<Self>, dev_path: &str, label: &str) {
        self.show_banner(&format!("Mounting {label}…"), "ok");
        let (tx, rx) = mpsc::channel();
        let dp = dev_path.to_string();
        let _ = std::thread::spawn(move || {
            let _ = tx.send(devices::mount_device(&dp));
        });
        let w = Rc::clone(self);
        let lbl = label.to_string();
        glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
            Ok(Ok(mp)) => {
                w.show_banner(&format!("Mounted {lbl}"), "ok");
                w.refresh_devices();
                w.navigate_to(mp);
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                w.show_banner(&e, "danger");
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
    }

    fn eject_drive(self: &Rc<Self>, dev: DeviceEntry) {
        // Leave the drive directory across all tabs before unmounting so we aren't holding it busy!
        if let Some(mp) = &dev.mountpoint {
            let mut active_affected = false;
            {
                let mut st = self.state.borrow_mut();
                let active = st.active_tab;
                for (i, tab) in st.tabs.iter_mut().enumerate() {
                    if tab.cwd.starts_with(mp) {
                        tab.cwd = home_dir();
                        tab.in_trash = false;
                        if i == active {
                            active_affected = true;
                        }
                    }
                }
            }
            if active_affected {
                self.reload_dir();
            } else {
                self.render_tabs();
            }
        }
        self.show_banner(&format!("Ejecting {}…", dev.label), "ok");
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::spawn(move || {
            let _ = tx.send(devices::eject_device(&dev));
        });
        let w = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
            Ok(Ok(msg)) => {
                w.show_banner(&msg, "ok");
                w.refresh_devices();
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                w.show_banner(&e, "warn");
                w.refresh_devices();
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
    }

    fn render_canvas(self: &Rc<Self>) {
        // Remember current selection so background inotify refreshes or
        // Grid <-> List switches never wipe the user's selected items.
        let prev_selected = self.selected_paths();

        let (mode, items, in_trash, has_filter) = {
            let st = self.state.borrow();
            let tab = st.tab();
            (
                st.view_mode,
                tab.visible.clone(),
                tab.in_trash,
                !tab.filter.trim().is_empty(),
            )
        };
        if items.is_empty() {
            while let Some(c) = self.flow_box.first_child() {
                self.flow_box.remove(&c);
            }
            while let Some(r) = self.list_box.first_child() {
                self.list_box.remove(&r);
            }
            let empty = GtkBox::new(Orientation::Vertical, 8);
            empty.add_css_class("filer-empty-state");
            empty.set_halign(Align::Center);
            empty.set_valign(Align::Center);
            empty.set_vexpand(true);
            empty.set_hexpand(true);

            let (glyph, title, sub) = if has_filter {
                ("\u{f002}", "No matching items", "Press Esc to clear filter")
            } else if in_trash {
                ("\u{f1f8}", "Trash is empty", "Deleted files appear here")
            } else {
                (
                    "\u{f07c}",
                    "This folder is empty",
                    "Drag files here or press Ctrl+Shift+N to create a folder",
                )
            };
            let g = Label::new(Some(glyph));
            g.add_css_class("filer-empty-glyph");
            let t = Label::new(Some(title));
            t.add_css_class("filer-empty-title");
            let s = Label::new(Some(sub));
            s.add_css_class("filer-empty-sub");
            empty.append(&g);
            empty.append(&t);
            empty.append(&s);
            self.canvas_scroll.set_child(Some(&empty));
            self.update_status_bar();
            return;
        }

        match mode {
            ViewMode::Grid => {
                self.canvas_scroll.set_child(Some(&self.flow_box));
                while let Some(c) = self.flow_box.first_child() {
                    self.flow_box.remove(&c);
                }
                for entry in &items {
                    let cell = self.build_grid_cell(entry);
                    let child = FlowBoxChild::new();
                    child.set_child(Some(&cell));
                    self.flow_box.insert(&child, -1);
                    if prev_selected.contains(&entry.path) {
                        self.flow_box.select_child(&child);
                    }
                }
            }
            ViewMode::List => {
                self.canvas_scroll.set_child(Some(&self.list_box));
                while let Some(r) = self.list_box.first_child() {
                    self.list_box.remove(&r);
                }
                for entry in &items {
                    let row_widget = self.build_list_row(entry);
                    let row = ListBoxRow::new();
                    row.set_child(Some(&row_widget));
                    self.list_box.append(&row);
                    if prev_selected.contains(&entry.path) {
                        self.list_box.select_row(Some(&row));
                    }
                }
            }
        }
        self.update_status_bar();
    }

    fn build_grid_cell(self: &Rc<Self>, entry: &FileEntry) -> GtkBox {
        let cell = GtkBox::new(Orientation::Vertical, 6);
        cell.add_css_class("filer-grid-item");
        cell.set_size_request(104, 102);
        cell.set_halign(Align::Center);

        let icon_box = GtkBox::new(Orientation::Vertical, 0);
        icon_box.set_size_request(60, 56);
        icon_box.set_halign(Align::Center);
        icon_box.set_valign(Align::Center);

        let glyph = Label::new(Some(entry.icon_glyph));
        glyph.add_css_class("filer-grid-glyph");
        glyph.add_css_class(entry.icon_class);
        glyph.set_halign(Align::Center);
        glyph.set_valign(Align::Center);
        icon_box.append(&glyph);

        // Async framed thumbnail for images
        if entry.is_image() {
            let (tx, rx) = mpsc::channel();
            let p = entry.path.clone();
            let mt = entry.modified_secs;
            let _ = std::thread::spawn(move || {
                let _ = tx.send(thumbs::ensure_thumbnail(&p, mt));
            });
            let ib = icon_box.clone();
            glib::timeout_add_local(Duration::from_millis(30), move || match rx.try_recv() {
                Ok(Some(thumb_path)) => {
                    while let Some(c) = ib.first_child() {
                        ib.remove(&c);
                    }
                    let frame = GtkBox::new(Orientation::Vertical, 0);
                    frame.add_css_class("filer-thumb");
                    frame.set_size_request(56, 56);
                    frame.set_halign(Align::Center);
                    frame.set_valign(Align::Center);
                    let pic = Picture::for_filename(&thumb_path);
                    pic.set_size_request(56, 56);
                    pic.set_content_fit(ContentFit::Cover);
                    pic.set_can_shrink(true);
                    frame.append(&pic);
                    ib.append(&frame);
                    glib::ControlFlow::Break
                }
                Ok(None) => glib::ControlFlow::Break,
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            });
        }

        let lbl = Label::new(Some(&entry.name));
        lbl.add_css_class("filer-item-name");
        if entry.is_dir {
            lbl.add_css_class("dir");
        }
        lbl.set_max_width_chars(14);
        lbl.set_lines(2);
        lbl.set_wrap(true);
        lbl.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        lbl.set_justify(gtk4::Justification::Center);

        cell.append(&icon_box);
        cell.append(&lbl);

        self.attach_drag_source(&cell, entry.path.clone());
        if entry.is_dir {
            let w_spring = Rc::clone(self);
            let p_spring = entry.path.clone();
            self.attach_folder_drop_target(
                &cell,
                entry.path.clone(),
                false,
                Some(Rc::new(move || w_spring.navigate_to(p_spring.clone()))),
            );
            let mid = GestureClick::new();
            mid.set_button(2);
            let w_mid = Rc::clone(self);
            let p_mid = entry.path.clone();
            mid.connect_pressed(move |_, _, _, _| w_mid.open_new_tab(p_mid.clone()));
            cell.add_controller(mid);
        }
        cell
    }

    fn build_list_row(self: &Rc<Self>, entry: &FileEntry) -> GtkBox {
        let row = GtkBox::new(Orientation::Horizontal, 10);
        row.add_css_class("filer-list-row");

        let glyph = Label::new(Some(entry.icon_glyph));
        glyph.add_css_class("filer-list-glyph");
        glyph.add_css_class(entry.icon_class);
        glyph.set_size_request(22, -1);
        glyph.set_halign(Align::Center);
        row.append(&glyph);

        let name = Label::new(Some(&entry.name));
        name.add_css_class("filer-item-name");
        if entry.is_dir {
            name.add_css_class("dir");
        }
        name.set_halign(Align::Start);
        name.set_hexpand(true);
        name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        row.append(&name);

        let size_txt = if entry.is_dir {
            "—".to_string()
        } else {
            format_size(entry.size)
        };
        let size_lbl = Label::new(Some(&size_txt));
        size_lbl.add_css_class("filer-item-meta");
        size_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        size_lbl.set_xalign(1.0);
        self.col_size_sg.add_widget(&size_lbl);
        row.append(&size_lbl);

        let mtime_lbl = Label::new(Some(&format_mtime(entry.modified_secs)));
        mtime_lbl.add_css_class("filer-item-meta");
        mtime_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        mtime_lbl.set_xalign(1.0);
        self.col_mtime_sg.add_widget(&mtime_lbl);
        row.append(&mtime_lbl);

        let kind_lbl = Label::new(Some(&entry.kind));
        kind_lbl.add_css_class("filer-item-meta");
        kind_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        kind_lbl.set_xalign(1.0);
        self.col_kind_sg.add_widget(&kind_lbl);
        row.append(&kind_lbl);

        self.attach_drag_source(&row, entry.path.clone());
        if entry.is_dir {
            let w_spring = Rc::clone(self);
            let p_spring = entry.path.clone();
            self.attach_folder_drop_target(
                &row,
                entry.path.clone(),
                false,
                Some(Rc::new(move || w_spring.navigate_to(p_spring.clone()))),
            );
            let mid = GestureClick::new();
            mid.set_button(2);
            let w_mid = Rc::clone(self);
            let p_mid = entry.path.clone();
            mid.connect_pressed(move |_, _, _, _| w_mid.open_new_tab(p_mid.clone()));
            row.add_controller(mid);
        }
        row
    }

    fn attach_drag_source(
        self: &Rc<Self>,
        widget: &(impl IsA<gtk4::Widget> + Clone),
        item_path: PathBuf,
    ) {
        let src = DragSource::new();
        src.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let w = Rc::clone(self);
        // Hold the dragged widget for the whole drag: a spring-loaded
        // navigation rebuilds the canvas, and letting the source be finalized
        // would cancel the drag the person is still holding.
        let held: Widget = Clone::clone(widget).upcast::<Widget>();
        src.connect_prepare(move |_, _, _| {
            *w.drag_hold.borrow_mut() = Some(held.clone());
            let mut paths = w.selected_paths();
            if !paths.contains(&item_path) {
                paths = vec![item_path.clone()];
            }
            let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
            let list = gdk::FileList::from_array(&files);
            Some(gdk::ContentProvider::for_value(&list.to_value()))
        });
        let w = Rc::clone(self);
        src.connect_drag_end(move |_, _, _| {
            *w.drag_hold.borrow_mut() = None;
        });
        widget.add_controller(src);
    }

    /// `spring` is the spring-load action: hold a drag over this widget for a
    /// moment and it runs (open the folder, switch the tab) so the drop can
    /// land deeper than one level.
    fn attach_folder_drop_target(
        self: &Rc<Self>,
        widget: &impl IsA<gtk4::Widget>,
        target_dir: PathBuf,
        is_trash: bool,
        spring: Option<Rc<dyn Fn()>>,
    ) {
        let dt = DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        let cls = if is_trash {
            "drop-hover-trash"
        } else {
            "drop-hover"
        };
        let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        // Hovering is true only while a drag is over this widget, so a timer
        // that outlives a leave (or a navigation) cannot fire a stale action.
        let hovering = Rc::new(Cell::new(false));

        let w_enter = widget.clone();
        let hover_enter = Rc::clone(&hovering);
        let timer_enter = Rc::clone(&timer);
        let spring_enter = spring.clone();
        dt.connect_enter(move |_, _, _| {
            w_enter.add_css_class(cls);
            hover_enter.set(true);
            if let Some(action) = spring_enter.clone() {
                if let Some(old) = timer_enter.borrow_mut().take() {
                    old.remove();
                }
                let hover = Rc::clone(&hover_enter);
                let timer_fire = Rc::clone(&timer_enter);
                let id = glib::timeout_add_local(Duration::from_millis(600), move || {
                    // The source ends with this callback: forget it, so a later
                    // leave or drop cannot try to remove a source that is gone
                    // (glib 0.22 panics on that and would abort the window).
                    *timer_fire.borrow_mut() = None;
                    if hover.get() {
                        action();
                    }
                    glib::ControlFlow::Break
                });
                *timer_enter.borrow_mut() = Some(id);
            }
            gdk::DragAction::MOVE
        });
        let w_leave = widget.clone();
        let hover_leave = Rc::clone(&hovering);
        let timer_leave = Rc::clone(&timer);
        dt.connect_leave(move |_| {
            w_leave.remove_css_class(cls);
            hover_leave.set(false);
            if let Some(id) = timer_leave.borrow_mut().take() {
                id.remove();
            }
        });

        let w_drop = widget.clone();
        let hover_drop = Rc::clone(&hovering);
        let this = Rc::clone(self);
        dt.connect_drop(move |target, val, _, _| {
            w_drop.remove_css_class(cls);
            hover_drop.set(false);
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            let Ok(fl) = val.get::<gdk::FileList>() else {
                return false;
            };
            let paths: Vec<PathBuf> = fl.files().into_iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() {
                return false;
            }
            if is_trash {
                this.run_op(OpKind::Trash, paths, None);
            } else {
                let is_copy = drop_is_copy(target);
                this.handle_drop(paths, target_dir.clone(), is_copy);
            }
            true
        });
        widget.add_controller(dt);
    }

    fn handle_drop(self: &Rc<Self>, paths: Vec<PathBuf>, dest_dir: PathBuf, force_copy: bool) {
        if paths.is_empty() {
            return;
        }
        let kind = if force_copy || !ops::same_filesystem(&paths[0], &dest_dir) {
            OpKind::Copy
        } else {
            OpKind::Move
        };
        self.run_op(kind, paths, Some(dest_dir));
    }

    fn paste_from_clipboard(self: &Rc<Self>) {
        let Some((is_cut, paths)) = tools::read_file_clipboard() else {
            self.show_banner("Clipboard has no files to paste", "warn");
            return;
        };
        let dst = self.state.borrow().cwd().to_path_buf();
        let kind = if is_cut { OpKind::Move } else { OpKind::Copy };
        self.run_op(kind, paths, Some(dst));
    }

    fn run_op(self: &Rc<Self>, kind: OpKind, sources: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
        let cancel = Arc::new(AtomicBool::new(false));
        *self.active_cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let (tx, rx) = mpsc::channel();
        ops::spawn_batch(kind, sources, dest_dir, tx, cancel);
        self.pump_progress(rx);
    }

    /// Extract archives in place (no undo: the extracted tree is the answer).
    fn run_extract(self: &Rc<Self>, archives: Vec<PathBuf>) {
        let cancel = Arc::new(AtomicBool::new(false));
        *self.active_cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let (tx, rx) = mpsc::channel();
        archive::spawn_extract(archives, tx, cancel);
        self.pump_progress(rx);
    }

    /// Compress the selection into the current folder.
    fn run_compress(self: &Rc<Self>, sources: Vec<PathBuf>, format: ArchiveFormat) {
        let dest = self.state.borrow().cwd().to_path_buf();
        let cancel = Arc::new(AtomicBool::new(false));
        *self.active_cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let (tx, rx) = mpsc::channel();
        archive::spawn_compress(sources, format, dest, tx, cancel);
        self.pump_progress(rx);
    }

    /// Drive the bottom progress bar from any worker's `OpProgress` channel and
    /// record the undo it hands back.
    fn pump_progress(self: &Rc<Self>, rx: mpsc::Receiver<OpProgress>) {
        self.progress_box.set_visible(true);
        self.progress_bar.set_fraction(0.0);
        self.progress_label.set_text("Working…");

        let w = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(30), move || {
            loop {
                match rx.try_recv() {
                    Ok(OpProgress::Update { label, fraction }) => {
                        w.progress_label.set_text(&label);
                        w.progress_bar.set_fraction(fraction);
                    }
                    Ok(OpProgress::Done { summary, undo }) => {
                        w.progress_box.set_visible(false);
                        *w.active_cancel.borrow_mut() = None;
                        let reversible = undo.is_some();
                        if let Some(action) = undo {
                            w.state.borrow_mut().push_undo(action);
                        }
                        let msg = if reversible {
                            format!("{summary} · Ctrl+Z to undo")
                        } else {
                            summary
                        };
                        w.show_banner(&msg, "ok");
                        w.reload_dir();
                        w.render_shelf();
                        return glib::ControlFlow::Break;
                    }
                    Ok(OpProgress::Error { message }) => {
                        w.progress_box.set_visible(false);
                        *w.active_cancel.borrow_mut() = None;
                        w.show_banner(&message, "danger");
                        w.reload_dir();
                        w.render_shelf();
                        return glib::ControlFlow::Break;
                    }
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        w.progress_box.set_visible(false);
                        return glib::ControlFlow::Break;
                    }
                }
            }
        });
    }

    /// Ctrl+Z: reverse the most recent rename, move, trash, copy or folder
    /// creation.
    fn undo_last(self: &Rc<Self>) {
        let Some(action) = self.state.borrow_mut().take_undo() else {
            self.show_banner("Nothing to undo", "warn");
            return;
        };
        let label = action.label();
        match ops::undo(&action) {
            Ok(msg) => {
                self.show_banner(&msg, "ok");
                self.reload_dir();
                self.render_shelf();
            }
            Err(e) => {
                // Put it back so a second Ctrl+Z can retry after the person
                // clears whatever blocked it.
                self.state.borrow_mut().push_undo(action);
                self.show_banner(&format!("Could not undo {label}: {e}"), "danger");
            }
        }
    }

    fn selected_entries(&self) -> Vec<FileEntry> {
        let st = self.state.borrow();
        let indices: Vec<usize> = match st.view_mode {
            ViewMode::Grid => self
                .flow_box
                .selected_children()
                .into_iter()
                .filter_map(|c| usize::try_from(c.index()).ok())
                .collect(),
            ViewMode::List => self
                .list_box
                .selected_rows()
                .into_iter()
                .filter_map(|r| usize::try_from(r.index()).ok())
                .collect(),
        };
        indices
            .into_iter()
            .filter_map(|i| st.tab().visible.get(i).cloned())
            .collect()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    fn activate_index(self: &Rc<Self>, idx: usize) {
        let entry = self.state.borrow().tab().visible.get(idx).cloned();
        let Some(entry) = entry else {
            return;
        };
        if entry.is_dir {
            self.navigate_to(entry.path);
        } else {
            tools::open_file(&entry.path);
        }
    }

    fn toggle_quicklook(self: &Rc<Self>) {
        if self.quicklook_slot.borrow().is_some() {
            self.close_quicklook();
            return;
        }
        let Some(entry) = self.selected_entries().into_iter().next() else {
            return;
        };
        self.open_quicklook_for(&entry);
    }

    fn open_quicklook_for(self: &Rc<Self>, entry: &FileEntry) {
        self.close_quicklook();
        self.close_properties();
        let w = Rc::clone(self);
        let (card, scroller) = quicklook::build_quicklook_card(entry, move || w.close_quicklook());
        let w2 = Rc::clone(self);
        let modal = Self::wrap_modal_scrim(card, move || w2.close_quicklook());
        self.overlay.add_overlay(&modal);
        *self.quicklook_scroll.borrow_mut() = scroller;
        *self.quicklook_slot.borrow_mut() = Some(modal);
    }

    fn step_quicklook(self: &Rc<Self>, delta: isize) {
        let (mode, len) = {
            let st = self.state.borrow();
            (st.view_mode, st.tab().visible.len())
        };
        if len == 0 {
            return;
        }
        let cur_idx: isize = match mode {
            ViewMode::Grid => self
                .flow_box
                .selected_children()
                .first()
                .map(|c| c.index() as isize)
                .unwrap_or(0),
            ViewMode::List => self
                .list_box
                .selected_rows()
                .first()
                .map(|r| r.index() as isize)
                .unwrap_or(0),
        };
        let next_idx = (cur_idx + delta).rem_euclid(len as isize) as usize;
        match mode {
            ViewMode::Grid => {
                self.flow_box.unselect_all();
                if let Some(child) = self.flow_box.child_at_index(next_idx as i32) {
                    self.flow_box.select_child(&child);
                }
            }
            ViewMode::List => {
                self.list_box.unselect_all();
                if let Some(row) = self.list_box.row_at_index(next_idx as i32) {
                    self.list_box.select_row(Some(&row));
                }
            }
        }
        if let Some(entry) = self.state.borrow().tab().visible.get(next_idx).cloned() {
            self.open_quicklook_for(&entry);
        }
    }

    fn close_quicklook(&self) {
        self.quicklook_scroll.borrow_mut().take();
        if let Some(card) = self.quicklook_slot.borrow_mut().take() {
            self.overlay.remove_overlay(&card);
        }
    }

    fn show_sort_popover(self: &Rc<Self>, anchor: &Button) {
        let pop = Popover::new();
        pop.set_parent(anchor);
        unparent_on_close(&pop);
        let col = GtkBox::new(Orientation::Vertical, 2);

        let (cur_field, hidden) = {
            let st = self.state.borrow();
            (st.sort_field, st.show_hidden)
        };

        for (lbl, field) in [
            ("Sort by Name", SortField::Name),
            ("Sort by Modified", SortField::Modified),
            ("Sort by Size", SortField::Size),
            ("Sort by Type", SortField::Kind),
        ] {
            let icon = if cur_field == field { "\u{f00c}" } else { "" };
            let w = Rc::clone(self);
            let p = pop.clone();
            let b = make_menu_item(icon, lbl, None, false, move || {
                p.popdown();
                {
                    let mut st = w.state.borrow_mut();
                    if st.sort_field == field {
                        st.sort_order = st.sort_order.toggle();
                    } else {
                        st.sort_field = field;
                        st.sort_order = SortOrder::Asc;
                    }
                    st.recompute_visible();
                }
                w.rebuild_list_header();
                w.render_canvas();
            });
            col.append(&b);
        }

        let sep = Separator::new(Orientation::Horizontal);
        sep.add_css_class("filer-menu-sep");
        col.append(&sep);

        let w = Rc::clone(self);
        let p = pop.clone();
        let b = make_menu_item(
            if hidden { "\u{f00c}" } else { "" },
            "Show Hidden Files",
            Some("Ctrl+H"),
            false,
            move || {
                p.popdown();
                {
                    let mut st = w.state.borrow_mut();
                    st.show_hidden = !st.show_hidden;
                    st.recompute_visible();
                }
                w.render_canvas();
            },
        );
        col.append(&b);

        pop.set_child(Some(&col));
        pop.popup();
    }

    fn select_item_at_canvas_pos(&self, x: f64, y: f64) {
        let mode = self.state.borrow().view_mode;
        match mode {
            ViewMode::Grid => {
                if let Some((fx, fy)) =
                    self.canvas_scroll
                        .translate_coordinates(&self.flow_box, x, y)
                {
                    if let Some(child) = self.flow_box.child_at_pos(fx as i32, fy as i32) {
                        if !child.is_selected() {
                            self.flow_box.unselect_all();
                            self.flow_box.select_child(&child);
                        }
                    } else {
                        self.flow_box.unselect_all();
                    }
                }
            }
            ViewMode::List => {
                if let Some((_, ly)) =
                    self.canvas_scroll
                        .translate_coordinates(&self.list_box, x, y)
                {
                    if let Some(row) = self.list_box.row_at_y(ly as i32) {
                        if !row.is_selected() {
                            self.list_box.unselect_all();
                            self.list_box.select_row(Some(&row));
                        }
                    } else {
                        self.list_box.unselect_all();
                    }
                }
            }
        }
    }

    fn show_canvas_context_menu(self: &Rc<Self>, x: f64, y: f64) {
        let pop = Popover::new();
        pop.set_parent(&self.canvas_scroll);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.set_has_arrow(false);
        unparent_on_close(&pop);

        let col = GtkBox::new(Orientation::Vertical, 2);
        let sel = self.selected_entries();
        let in_trash = self.state.borrow().in_trash();
        let cwd = self.state.borrow().cwd().to_path_buf();

        let add =
            |icon: &str, label: &str, shortcut: Option<&str>, danger: bool, cb: Box<dyn Fn()>| {
                let p = pop.clone();
                let b = make_menu_item(icon, label, shortcut, danger, move || {
                    p.popdown();
                    cb();
                });
                col.append(&b);
            };
        let add_sep = || {
            let sep = Separator::new(Orientation::Horizontal);
            sep.add_css_class("filer-menu-sep");
            col.append(&sep);
        };

        if let Some(first) = sel.first().cloned() {
            let paths: Vec<PathBuf> = sel.iter().map(|e| e.path.clone()).collect();
            if in_trash {
                let w = Rc::clone(self);
                let ps = paths.clone();
                add(
                    "\u{f0e2}",
                    "Restore from Trash",
                    None,
                    false,
                    Box::new(move || {
                        for p in &ps {
                            let _ = trash::restore_from_trash(p);
                        }
                        w.show_banner("Restored from Trash", "ok");
                        w.reload_dir();
                    }),
                );
                let w = Rc::clone(self);
                let ps = paths.clone();
                add(
                    "\u{f1f8}",
                    "Delete Permanently…",
                    Some("Del"),
                    true,
                    Box::new(move || w.confirm_permanent_delete(ps.clone())),
                );
            } else {
                let w = Rc::clone(self);
                let f_open = first.clone();
                add(
                    "\u{f07c}",
                    "Open",
                    Some("Enter"),
                    false,
                    Box::new(move || {
                        if f_open.is_dir {
                            w.navigate_to(f_open.path.clone());
                        } else {
                            tools::open_file(&f_open.path);
                        }
                    }),
                );
                if first.is_dir {
                    let w = Rc::clone(self);
                    let p_tab = first.path.clone();
                    add(
                        "\u{f067}",
                        "Open in New Tab",
                        Some("Mid-Click"),
                        false,
                        Box::new(move || w.open_new_tab(p_tab.clone())),
                    );
                }
                let w = Rc::clone(self);
                add(
                    "\u{f06e}",
                    "Quick Look",
                    Some("Space"),
                    false,
                    Box::new(move || w.toggle_quicklook()),
                );
                let archives: Vec<PathBuf> = sel
                    .iter()
                    .filter(|e| archive::is_archive(&e.path))
                    .map(|e| e.path.clone())
                    .collect();
                if !archives.is_empty() {
                    let w = Rc::clone(self);
                    let label = if archives.len() > 1 {
                        format!("Extract {} Archives Here", archives.len())
                    } else {
                        "Extract Here".to_string()
                    };
                    add(
                        "\u{f1c6}",
                        &label,
                        None,
                        false,
                        Box::new(move || w.run_extract(archives.clone())),
                    );
                }
                let w = Rc::clone(self);
                let ps_shelf = paths.clone();
                add(
                    "\u{f01c}",
                    "Add to Shelf",
                    None,
                    false,
                    Box::new(move || w.add_to_shelf(ps_shelf.clone())),
                );
                // Compact 1-row archive compressor: saves vertical menu height
                // while keeping all three formats one click away.
                let comp_row = GtkBox::new(Orientation::Horizontal, 4);
                comp_row.set_margin_start(8);
                comp_row.set_margin_end(8);
                comp_row.set_margin_top(2);
                comp_row.set_margin_bottom(2);
                let comp_ic = Label::new(Some("\u{f1c6}"));
                comp_ic.add_css_class("filer-menu-icon");
                comp_ic.set_size_request(16, -1);
                comp_ic.set_halign(Align::Center);
                let comp_lbl = Label::new(Some("Compress"));
                comp_lbl.set_halign(Align::Start);
                comp_lbl.set_hexpand(true);
                comp_lbl.set_margin_start(4);
                comp_row.append(&comp_ic);
                comp_row.append(&comp_lbl);
                for (short, format) in [
                    (".zip", ArchiveFormat::Zip),
                    (".tar.gz", ArchiveFormat::TarGz),
                    (".tar.xz", ArchiveFormat::TarXz),
                ] {
                    let b = Button::with_label(short);
                    b.add_css_class("filer-dev-action");
                    let w = Rc::clone(self);
                    let p = pop.clone();
                    let ps = paths.clone();
                    b.connect_clicked(move |_| {
                        p.popdown();
                        w.run_compress(ps.clone(), format);
                    });
                    comp_row.append(&b);
                }
                col.append(&comp_row);
                add_sep();

                let w = Rc::clone(self);
                let ps = paths.clone();
                add(
                    "\u{f0c4}",
                    "Cut",
                    Some("Ctrl+X"),
                    false,
                    Box::new(move || {
                        tools::write_file_clipboard(&ps, true);
                        w.show_banner(&format!("Cut {} item(s)", ps.len()), "ok");
                    }),
                );
                let w = Rc::clone(self);
                let ps = paths.clone();
                add(
                    "\u{f0c5}",
                    "Copy",
                    Some("Ctrl+C"),
                    false,
                    Box::new(move || {
                        tools::write_file_clipboard(&ps, false);
                        w.show_banner(&format!("Copied {} item(s)", ps.len()), "ok");
                    }),
                );
                if sel.len() == 1 {
                    let w = Rc::clone(self);
                    let f_ren = first.clone();
                    add(
                        "\u{f040}",
                        "Rename…",
                        Some("F2"),
                        false,
                        Box::new(move || w.prompt_rename(&f_ren)),
                    );
                }
                let w = Rc::clone(self);
                let ps = paths.clone();
                add(
                    "\u{f1f8}",
                    "Move to Trash",
                    Some("Del"),
                    true,
                    Box::new(move || w.run_op(OpKind::Trash, ps.clone(), None)),
                );
                add_sep();

                // Hayato Tools Integration
                let ps_share = paths.clone();
                add(
                    "\u{f064}",
                    "Share… (Hayato Share)",
                    Some("Ctrl+⇧+S"),
                    false,
                    Box::new(move || tools::share_paths(&ps_share)),
                );
                if first.is_audio() {
                    let p = first.path.clone();
                    add(
                        "\u{f001}",
                        "Play in Cassette",
                        None,
                        false,
                        Box::new(move || tools::play_in_cassette(&p)),
                    );
                }
                if first.is_image() {
                    let p = first.path.clone();
                    add(
                        "\u{f030}",
                        "Scan / OCR (Lens)",
                        None,
                        false,
                        Box::new(move || tools::open_in_lens(&p)),
                    );
                }
                if first.is_dir {
                    let p = first.path.clone();
                    add(
                        "\u{f120}",
                        "Open Terminal Here",
                        Some("Ctrl+⇧+T"),
                        false,
                        Box::new(move || tools::open_terminal_here(&p)),
                    );
                    let p2 = first.path.clone();
                    add(
                        "\u{f002}",
                        "Search Inside Folder…",
                        None,
                        false,
                        Box::new(move || tools::search_in_folder(&p2)),
                    );
                    let w = Rc::clone(self);
                    let p3 = first.path.clone();
                    add(
                        "\u{f005}",
                        "Pin to Sidebar",
                        None,
                        false,
                        Box::new(move || {
                            if w.state.borrow_mut().pin_bookmark(p3.clone()) {
                                w.render_places();
                                w.show_banner("Pinned to Places", "ok");
                            }
                        }),
                    );
                }
                let w = Rc::clone(self);
                add(
                    "\u{f05a}",
                    "Properties…",
                    Some("Ctrl+I"),
                    false,
                    Box::new(move || w.toggle_properties()),
                );
                let p_act = first.path.clone();
                add(
                    "\u{f013}",
                    "More Actions…",
                    None,
                    false,
                    Box::new(move || tools::open_file_actions(&p_act)),
                );
                add_sep();

                let w = Rc::clone(self);
                let ps_cp = paths.clone();
                add(
                    "\u{f0ea}",
                    "Copy Path",
                    Some("Ctrl+⇧+C"),
                    false,
                    Box::new(move || {
                        tools::copy_paths_to_clipboard(&ps_cp);
                        w.show_banner("Copied path(s)", "ok");
                    }),
                );
            }
        } else if in_trash {
            let w = Rc::clone(self);
            add(
                "\u{f1f8}",
                "Empty Trash…",
                None,
                true,
                Box::new(move || w.confirm_empty_trash()),
            );
        } else {
            let w = Rc::clone(self);
            add(
                "\u{f07b}",
                "New Folder…",
                Some("Ctrl+⇧+N"),
                false,
                Box::new(move || w.prompt_new_folder()),
            );
            let w = Rc::clone(self);
            add(
                "\u{f0ea}",
                "Paste",
                Some("Ctrl+V"),
                false,
                Box::new(move || w.paste_from_clipboard()),
            );
            add_sep();
            let d1 = cwd.clone();
            add(
                "\u{f120}",
                "Open Terminal Here",
                Some("Ctrl+⇧+T"),
                false,
                Box::new(move || tools::open_terminal_here(&d1)),
            );
            let d2 = cwd.clone();
            add(
                "\u{f002}",
                "Search in This Folder…",
                None,
                false,
                Box::new(move || tools::search_in_folder(&d2)),
            );
            let d3 = cwd.clone();
            add(
                "\u{f019}",
                "Receive via Hayato Share…",
                None,
                false,
                Box::new(move || tools::receive_in_folder(&d3)),
            );
        }

        // Open into whichever half of the canvas has more room, and wrap in a
        // natural-height ScrolledWindow so tall menus never clip off small windows.
        let canvas_h = self.canvas_scroll.height().max(300) as f64;
        if y > canvas_h * 0.55 {
            pop.set_position(gtk4::PositionType::Top);
        } else {
            pop.set_position(gtk4::PositionType::Bottom);
        }
        let max_h = (self.window.height() - 64).clamp(240, 540);
        let menu_scroll = ScrolledWindow::new();
        menu_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        menu_scroll.set_propagate_natural_width(true);
        menu_scroll.set_propagate_natural_height(true);
        menu_scroll.set_max_content_height(max_h);
        menu_scroll.set_child(Some(&col));

        pop.set_child(Some(&menu_scroll));
        pop.popup();
    }

    fn prompt_new_folder(self: &Rc<Self>) {
        let pop = Popover::new();
        pop.set_parent(&self.crumbs_scroll);
        unparent_on_close(&pop);
        let row = GtkBox::new(Orientation::Horizontal, 6);
        let entry = Entry::new();
        entry.set_placeholder_text(Some("New folder name…"));
        entry.set_width_chars(24);
        let ok = Button::with_label("Create");
        ok.add_css_class("filer-tool-btn");
        row.append(&entry);
        row.append(&ok);
        pop.set_child(Some(&row));

        let w = Rc::clone(self);
        let p = pop.clone();
        let e_ref = entry.clone();
        let create = move || {
            let name = e_ref.text().trim().to_string();
            p.popdown();
            if name.is_empty() || name.contains('/') {
                return;
            }
            let target = w.state.borrow().cwd().join(&name);
            match std::fs::create_dir(&target) {
                Ok(()) => {
                    w.state
                        .borrow_mut()
                        .push_undo(ops::UndoAction::Created(vec![target.clone()]));
                    w.show_banner(&format!("Created folder {name} · Ctrl+Z to undo"), "ok");
                    w.reload_dir();
                }
                Err(err) => w.show_banner(&format!("Failed: {err}"), "danger"),
            }
        };
        let c1 = create.clone();
        ok.connect_clicked(move |_| c1());
        entry.connect_activate(move |_| create());
        pop.popup();
        entry.grab_focus();
    }

    fn prompt_rename(self: &Rc<Self>, item: &FileEntry) {
        let pop = Popover::new();
        pop.set_parent(&self.crumbs_scroll);
        unparent_on_close(&pop);
        let row = GtkBox::new(Orientation::Horizontal, 6);
        let entry = Entry::new();
        entry.set_text(&item.name);
        entry.set_width_chars(28);
        let ok = Button::with_label("Rename");
        ok.add_css_class("filer-tool-btn");
        row.append(&entry);
        row.append(&ok);
        pop.set_child(Some(&row));

        let w = Rc::clone(self);
        let p = pop.clone();
        let e_ref = entry.clone();
        let old_path = item.path.clone();
        let do_rename = move || {
            let new_name = e_ref.text().trim().to_string();
            p.popdown();
            if new_name.is_empty() || new_name.contains('/') {
                return;
            }
            let Some(parent) = old_path.parent() else {
                return;
            };
            let new_path = parent.join(&new_name);
            match std::fs::rename(&old_path, &new_path) {
                Ok(()) => {
                    w.state.borrow_mut().push_undo(ops::UndoAction::Rename {
                        from: old_path.clone(),
                        to: new_path.clone(),
                    });
                    w.show_banner(&format!("Renamed to {new_name} · Ctrl+Z to undo"), "ok");
                    w.reload_dir();
                }
                Err(err) => w.show_banner(&format!("Rename failed: {err}"), "danger"),
            }
        };
        let r1 = do_rename.clone();
        ok.connect_clicked(move |_| r1());
        entry.connect_activate(move |_| do_rename());
        pop.popup();
        entry.grab_focus();
        // Finder-style: select only the filename stem before the extension
        if !item.is_dir
            && let Some(dot) = item.name.rfind('.')
            && dot > 0
        {
            entry.select_region(0, dot as i32);
        } else {
            entry.select_region(0, -1);
        }
    }

    fn confirm_permanent_delete(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let pop = Popover::new();
        pop.set_parent(&self.crumbs_scroll);
        unparent_on_close(&pop);
        let col = GtkBox::new(Orientation::Vertical, 8);
        let lbl = Label::new(Some(&format!(
            "Permanently delete {} item(s)? This cannot be undone.",
            paths.len()
        )));
        let btns = GtkBox::new(Orientation::Horizontal, 6);
        btns.set_halign(Align::End);
        let cancel = Button::with_label("Cancel");
        cancel.add_css_class("filer-tool-btn");
        let del = Button::with_label("Delete Permanently");
        del.add_css_class("filer-tool-btn");
        del.add_css_class("danger");

        let p1 = pop.clone();
        cancel.connect_clicked(move |_| p1.popdown());
        let w = Rc::clone(self);
        let p2 = pop.clone();
        del.connect_clicked(move |_| {
            p2.popdown();
            w.run_op(OpKind::Delete, paths.clone(), None);
        });
        btns.append(&cancel);
        btns.append(&del);
        col.append(&lbl);
        col.append(&btns);
        pop.set_child(Some(&col));
        pop.popup();
    }

    fn confirm_empty_trash(self: &Rc<Self>) {
        let pop = Popover::new();
        pop.set_parent(&self.crumbs_scroll);
        unparent_on_close(&pop);
        let col = GtkBox::new(Orientation::Vertical, 8);
        let lbl = Label::new(Some("Empty Trash permanently? This cannot be undone."));
        let btns = GtkBox::new(Orientation::Horizontal, 6);
        btns.set_halign(Align::End);
        let cancel = Button::with_label("Cancel");
        cancel.add_css_class("filer-tool-btn");
        let empty = Button::with_label("Empty Trash");
        empty.add_css_class("filer-tool-btn");
        empty.add_css_class("danger");

        let p1 = pop.clone();
        cancel.connect_clicked(move |_| p1.popdown());
        let w = Rc::clone(self);
        let p2 = pop.clone();
        empty.connect_clicked(move |_| {
            p2.popdown();
            if let Err(e) = trash::empty_trash() {
                w.show_banner(&e, "danger");
            } else {
                w.show_banner("Trash emptied", "ok");
                w.reload_dir();
            }
        });
        btns.append(&cancel);
        btns.append(&empty);
        col.append(&lbl);
        col.append(&btns);
        pop.set_child(Some(&col));
        pop.popup();
    }

    fn update_status_bar(&self) {
        let st = self.state.borrow();
        let tab = st.tab();
        let visible = tab.visible.len();
        let all_count = tab.entries.len();
        let filtered = !tab.filter.trim().is_empty();
        let show_hidden = st.show_hidden;
        let cwd = st.cwd().to_path_buf();
        drop(st);

        let count_part = if filtered {
            format!("{visible}/{all_count} items (filtered)")
        } else {
            format!("{visible} items")
        };
        let hidden_part = if show_hidden { " · hidden" } else { "" };
        let sel = self.selected_entries();
        let sel_part = if sel.is_empty() {
            String::new()
        } else {
            let bytes: u64 = sel.iter().map(|e| e.size).sum();
            format!(" · {} selected ({})", sel.len(), format_size(bytes))
        };
        let free_part = free_space_bytes(&cwd)
            .map(|b| format!(" · {} free", format_size(b)))
            .unwrap_or_default();
        self.status_label
            .set_text(&format!("{count_part}{hidden_part}{sel_part}{free_part}"));
    }

    fn show_banner(&self, msg: &str, tone: &str) {
        tools::notify(msg, tone);
    }
}

/// Whether a drop copies instead of moving.
///
/// A source that offers MOVE is one of ours (a canvas item, a tab, the shelf):
/// on Wayland the drag is offered as COPY, so `actions()` is what the source
/// allows and MOVE in that set is the intent. A source that offers only COPY
/// is another application (a browser, an archive tool) and is copied, and a
/// cross-filesystem drop is forced to a copy by `handle_drop` either way.
fn drop_is_copy(target: &DropTarget) -> bool {
    match target.current_drop() {
        // A drop that carries its `GdkDrag` came from this application (a
        // canvas item, a tab, the shelf): move it, as Finder does on one
        // volume. GDK reports every local drop as COPY, so the drag object is
        // the only reliable signal; `handle_drop` still copies across
        // filesystems, and a source that is another application (a browser,
        // an archive tool) is copied.
        Some(d) => d.drag().is_none(),
        None => true,
    }
}

fn unparent_on_close(pop: &Popover) {
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || {
            if p.parent().is_some() {
                p.unparent();
            }
        });
    });
}

fn make_menu_item(
    icon: &str,
    label: &str,
    shortcut: Option<&str>,
    danger: bool,
    on_click: impl Fn() + 'static,
) -> Button {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    let ic = Label::new(Some(icon));
    ic.add_css_class("filer-menu-icon");
    ic.set_size_request(16, -1);
    ic.set_halign(Align::Center);
    row.append(&ic);

    let lbl = Label::new(Some(label));
    lbl.set_halign(Align::Start);
    lbl.set_hexpand(true);
    row.append(&lbl);

    if let Some(sc) = shortcut {
        let sc_lbl = Label::new(Some(sc));
        sc_lbl.add_css_class("filer-menu-shortcut");
        sc_lbl.set_halign(Align::End);
        row.append(&sc_lbl);
    }

    let btn = Button::new();
    btn.set_child(Some(&row));
    btn.add_css_class("filer-menu-item");
    if danger {
        btn.add_css_class("danger");
    }
    btn.connect_clicked(move |_| on_click());
    btn
}
