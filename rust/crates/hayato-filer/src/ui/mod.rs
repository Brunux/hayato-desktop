//! Main GTK4 window assembly for `hayato-filer`: Finder/elementary minimalism + Hayato Cyber-HUD.

pub mod quicklook;

use std::cell::RefCell;
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
    DropTarget, Entry, EventControllerKey, FlowBox, FlowBoxChild, GestureClick, Label, ListBox,
    ListBoxRow, Orientation, Overlay, Paned, Picture, Popover, ProgressBar, ScrolledWindow,
    SelectionMode, Separator,
};

use crate::fs::devices::{self, DeviceEntry};
use crate::fs::ops::{self, OpKind, OpProgress};
use crate::fs::thumbs;
use crate::fs::trash;
use crate::fs::{FileEntry, format_mtime, format_size, free_space_bytes, list_dir};
use crate::state::{
    AppState, SortField, SortOrder, ViewMode, home_dir, is_trash_files_dir, trash_files_dir,
};
use crate::tools;

pub struct FilerWindow {
    pub window: ApplicationWindow,
    state: RefCell<AppState>,
    devices: RefCell<Vec<DeviceEntry>>,
    back_btn: Button,
    fwd_btn: Button,
    crumbs_box: GtkBox,
    path_entry: Entry,
    filter_btn: Button,
    filter_entry: Entry,
    grid_btn: Button,
    list_btn: Button,
    sidebar_box: GtkBox,
    places_box: GtkBox,
    devices_box: GtkBox,
    banner_box: GtkBox,
    banner_label: Label,
    list_header: GtkBox,
    canvas_scroll: ScrolledWindow,
    flow_box: FlowBox,
    list_box: ListBox,
    overlay: Overlay,
    quicklook_slot: RefCell<Option<GtkBox>>,
    status_label: Label,
    progress_box: GtkBox,
    progress_bar: ProgressBar,
    progress_label: Label,
    active_cancel: RefCell<Option<Arc<AtomicBool>>>,
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
        crumbs_box.add_css_class("filer-breadcrumbs");
        crumbs_box.set_hexpand(true);

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

        header.append(&nav_group);
        header.append(&crumbs_box);
        header.append(&path_entry);
        header.append(&filter_entry);
        header.append(&filter_btn);
        header.append(&view_group);
        header.append(&sort_btn);

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

        sidebar_box.append(&places_hdr);
        sidebar_box.append(&places_box);
        sidebar_box.append(&devices_hdr);
        sidebar_box.append(&devices_box);

        let sidebar_scroll = ScrolledWindow::new();
        sidebar_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        sidebar_scroll.set_child(Some(&sidebar_box));

        // ── Notification Banner (hidden when idle) ────────────────────────
        let banner_box = GtkBox::new(Orientation::Horizontal, 8);
        banner_box.add_css_class("filer-banner");
        banner_box.set_visible(false);
        let banner_label = Label::new(None);
        banner_label.set_hexpand(true);
        banner_label.set_halign(Align::Start);
        let banner_close = Button::with_label("\u{f00d}");
        banner_close.add_css_class("filer-dev-action");
        banner_close.add_css_class("filer-glyph");
        let bb_close = banner_box.clone();
        banner_close.connect_clicked(move |_| bb_close.set_visible(false));
        banner_box.append(&banner_label);
        banner_box.append(&banner_close);

        // ── Sticky List View Column Header ────────────────────────────────
        let list_header = GtkBox::new(Orientation::Horizontal, 10);
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
        let cancel_btn = Button::with_label("✕");
        cancel_btn.add_css_class("filer-dev-action");
        progress_box.append(&progress_label);
        progress_box.append(&progress_bar);
        progress_box.append(&cancel_btn);

        status_bar.append(&status_label);
        status_bar.append(&progress_box);

        // ── Right Column & Paned Split ────────────────────────────────────
        let right_col = GtkBox::new(Orientation::Vertical, 0);
        right_col.append(&banner_box);
        right_col.append(&list_header);
        right_col.append(&canvas_scroll);

        let paned = Paned::new(Orientation::Horizontal);
        paned.set_start_child(Some(&sidebar_scroll));
        paned.set_end_child(Some(&right_col));
        paned.set_position(196);
        paned.set_shrink_start_child(false);
        paned.set_resize_start_child(false);
        paned.set_vexpand(true);

        let root_col = GtkBox::new(Orientation::Vertical, 0);
        root_col.append(&header);
        root_col.append(&paned);
        root_col.append(&status_bar);

        let overlay = Overlay::new();
        overlay.set_child(Some(&root_col));
        window.set_child(Some(&overlay));

        let mut init_state = AppState::new(initial_dir.clone());
        init_state.in_trash = is_trash_files_dir(&initial_dir);

        let this = Rc::new(Self {
            window,
            state: RefCell::new(init_state),
            devices: RefCell::new(Vec::new()),
            back_btn,
            fwd_btn,
            crumbs_box,
            path_entry,
            filter_btn,
            filter_entry,
            grid_btn,
            list_btn,
            sidebar_box,
            places_box,
            devices_box,
            banner_box,
            banner_label,
            list_header,
            canvas_scroll,
            flow_box,
            list_box,
            overlay,
            quicklook_slot: RefCell::new(None),
            status_label,
            progress_box,
            progress_bar,
            progress_label,
            active_cancel: RefCell::new(None),
        });

        this.wire_signals(&sort_btn, &cancel_btn);
        this.rebuild_list_header();
        this.refresh_devices();
        this.reload_dir();
        this
    }

    fn wire_signals(self: &Rc<Self>, sort_btn: &Button, cancel_btn: &Button) {
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

        // Editable path entry (Ctrl+L)
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
                    w.filter_entry.grab_focus();
                } else {
                    w.filter_entry.set_text("");
                }
            });
        }
        {
            let w = Rc::clone(self);
            self.filter_entry.connect_changed(move |entry| {
                w.state.borrow_mut().filter = entry.text().to_string();
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

        // Right-click context menu on canvas
        {
            let w = Rc::clone(self);
            let click = GestureClick::new();
            click.set_button(3);
            click.connect_pressed(move |_, _, x, y| {
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
                let dst = w.state.borrow().cwd.clone();
                let is_copy = target
                    .current_drop()
                    .is_some_and(|d| d.actions() == gdk::DragAction::COPY);
                w.handle_drop(paths, dst, is_copy);
                true
            });
            self.canvas_scroll.add_controller(dt);
        }

        // Global keyboard shortcuts
        {
            let w = Rc::clone(self);
            let key_ctl = EventControllerKey::new();
            key_ctl.connect_key_pressed(move |_, key, _, mods| w.handle_key(key, mods));
            self.window.add_controller(key_ctl);
        }
    }

    fn handle_key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);

        // If typing in path_entry or filter_entry, only intercept Escape
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
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }

        // Quick Look dismiss
        if self.quicklook_slot.borrow().is_some()
            && (key == gdk::Key::Escape || key == gdk::Key::space)
        {
            self.close_quicklook();
            return glib::Propagation::Stop;
        }

        if ctrl && shift {
            match key {
                gdk::Key::N | gdk::Key::n => {
                    self.prompt_new_folder();
                    return glib::Propagation::Stop;
                }
                gdk::Key::T | gdk::Key::t => {
                    let cwd = self.state.borrow().cwd.clone();
                    tools::open_terminal_here(&cwd);
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
                gdk::Key::b | gdk::Key::B => {
                    self.sidebar_box.set_visible(!self.sidebar_box.is_visible());
                    return glib::Propagation::Stop;
                }
                gdk::Key::w | gdk::Key::W | gdk::Key::q | gdk::Key::Q => {
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
                    if shift || self.state.borrow().in_trash {
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

    pub fn navigate_to(self: &Rc<Self>, path: PathBuf) {
        let is_trash = is_trash_files_dir(&path);
        self.state.borrow_mut().navigate(path, is_trash);
        self.filter_entry.set_text("");
        self.filter_entry.set_visible(false);
        self.close_quicklook();
        self.reload_dir();
    }

    pub fn reload_dir(self: &Rc<Self>) {
        let (cwd, in_trash, can_back, can_fwd, mode) = {
            let st = self.state.borrow();
            (
                st.cwd.clone(),
                st.in_trash,
                st.can_back(),
                st.can_forward(),
                st.view_mode,
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

        self.render_breadcrumbs(&cwd, in_trash);
        self.render_places();

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
                if w.state.borrow().cwd == cwd {
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
        let (active_field, order) = {
            let st = self.state.borrow();
            (st.sort_field, st.sort_order)
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

        let spacer = Label::new(None);
        spacer.set_size_request(24, -1);
        self.list_header.append(&spacer);

        let cols: [(SortField, &str, bool, i32); 4] = [
            (SortField::Name, "NAME", true, -1),
            (SortField::Size, "SIZE", false, 9),
            (SortField::Modified, "MODIFIED", false, 17),
            (SortField::Kind, "TYPE", false, 8),
        ];
        for (field, title, expand, width_chars) in cols {
            let lbl = Label::new(Some(&format!("{title}{}", arrow(field))));
            lbl.set_halign(if expand { Align::Start } else { Align::End });
            if width_chars > 0 {
                lbl.set_width_chars(width_chars);
            }
            let btn = Button::new();
            btn.set_child(Some(&lbl));
            btn.add_css_class("filer-col-btn");
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
        let cwd = self.state.borrow().cwd.display().to_string();
        self.crumbs_box.set_visible(false);
        self.path_entry.set_text(&cwd);
        self.path_entry.set_visible(true);
        self.path_entry.grab_focus();
        self.path_entry.select_region(0, -1);
    }

    fn hide_path_entry(&self) {
        self.path_entry.set_visible(false);
        self.crumbs_box.set_visible(true);
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

            // Breadcrumb DropTarget: drag files onto any ancestor folder!
            self.attach_folder_drop_target(&btn, path, false);
            self.crumbs_box.append(&btn);
        }
    }

    fn render_places(self: &Rc<Self>) {
        while let Some(child) = self.places_box.first_child() {
            self.places_box.remove(&child);
        }
        let home = home_dir();
        let trash_dir = trash_files_dir();
        let cur_cwd = self.state.borrow().cwd.clone();
        let cur_trash = self.state.borrow().in_trash;

        let defaults: [(&str, &str, PathBuf, bool); 8] = [
            ("\u{f015}", "Home", home.clone(), false),
            ("\u{f108}", "Desktop", home.join("Desktop"), false),
            ("\u{f15c}", "Documents", home.join("Documents"), false),
            ("\u{f019}", "Downloads", home.join("Downloads"), false),
            ("\u{f03e}", "Pictures", home.join("Pictures"), false),
            ("\u{f001}", "Music", home.join("Music"), false),
            ("\u{f008}", "Videos", home.join("Videos"), false),
            ("\u{f1f8}", "Trash", trash_dir.clone(), true),
        ];

        for (glyph, name, path, is_trash) in defaults {
            if !is_trash && path != home && !path.is_dir() {
                continue;
            }
            let active = if is_trash {
                cur_trash
            } else {
                !cur_trash && cur_cwd == path
            };
            let badge = if is_trash {
                let n = trash::trash_count();
                (n > 0).then(|| n.to_string())
            } else {
                None
            };
            let row = self.make_place_button(glyph, name, path, is_trash, active, badge.as_deref());
            self.places_box.append(&row);
        }

        let bookmarks = self.state.borrow().bookmarks.clone();
        for bm in bookmarks {
            let name = bm
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| bm.display().to_string());
            let active = !cur_trash && cur_cwd == bm;
            let row = self.make_place_button("\u{f005}", &name, bm, false, active, None);
            self.places_box.append(&row);
        }
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
        let inner = GtkBox::new(Orientation::Horizontal, 8);
        let g = Label::new(Some(glyph));
        g.add_css_class("filer-place-icon");
        g.set_size_request(18, -1);
        g.set_halign(Align::Center);
        let lbl = Label::new(Some(name));
        lbl.set_hexpand(true);
        lbl.set_halign(Align::Start);
        lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        inner.append(&g);
        inner.append(&lbl);
        if let Some(b) = badge {
            let pill = Label::new(Some(b));
            pill.add_css_class("filer-badge");
            inner.append(&pill);
        }

        let btn = Button::new();
        btn.set_child(Some(&inner));
        btn.add_css_class("filer-place-row");
        if active {
            btn.add_css_class("active");
        }
        let w = Rc::clone(self);
        let target = path.clone();
        btn.connect_clicked(move |_| w.navigate_to(target.clone()));

        self.attach_folder_drop_target(&btn, path, is_trash);
        btn
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
                *w.devices.borrow_mut() = devs;
                w.render_devices();
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
        let devs = self.devices.borrow().clone();
        if devs.is_empty() {
            let empty = Label::new(Some("No removable drives"));
            empty.add_css_class("filer-item-meta");
            empty.set_halign(Align::Start);
            empty.set_margin_start(8);
            self.devices_box.append(&empty);
            return;
        }
        let cur_cwd = self.state.borrow().cwd.clone();
        for dev in devs {
            let wrap = GtkBox::new(Orientation::Vertical, 3);
            wrap.add_css_class("filer-device-card");
            if let Some(mp) = &dev.mountpoint
                && cur_cwd.starts_with(mp)
            {
                wrap.add_css_class("active");
            }

            let row = GtkBox::new(Orientation::Horizontal, 4);
            let main_btn = Button::new();
            main_btn.set_hexpand(true);
            main_btn.add_css_class("filer-device-btn");
            let lbl_box = GtkBox::new(Orientation::Horizontal, 8);
            let icon = Label::new(Some("\u{f0a0}"));
            icon.add_css_class("filer-place-icon");
            icon.set_size_request(18, -1);
            icon.set_halign(Align::Center);
            let name = Label::new(Some(&dev.label));
            name.set_halign(Align::Start);
            name.set_hexpand(true);
            name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            lbl_box.append(&icon);
            lbl_box.append(&name);
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
                let bar = ProgressBar::new();
                bar.add_css_class("filer-usage-bar");
                bar.set_fraction(dev.usage_fraction());
                bar.set_margin_start(6);
                bar.set_margin_end(6);
                wrap.append(&bar);
                self.attach_folder_drop_target(&wrap, mp.clone(), false);
            }
            self.devices_box.append(&wrap);
        }
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
        // Leave the drive directory before unmounting so we aren't holding it busy!
        if let Some(mp) = &dev.mountpoint
            && self.state.borrow().cwd.starts_with(mp)
        {
            self.navigate_to(home_dir());
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
        let (mode, items, in_trash, has_filter) = {
            let st = self.state.borrow();
            (
                st.view_mode,
                st.visible.clone(),
                st.in_trash,
                !st.filter.trim().is_empty(),
            )
        };
        if items.is_empty() {
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
            self.attach_folder_drop_target(&cell, entry.path.clone(), false);
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
        size_lbl.set_width_chars(9);
        size_lbl.set_xalign(1.0);
        row.append(&size_lbl);

        let mtime_lbl = Label::new(Some(&format_mtime(entry.modified_secs)));
        mtime_lbl.add_css_class("filer-item-meta");
        mtime_lbl.set_width_chars(17);
        mtime_lbl.set_xalign(1.0);
        row.append(&mtime_lbl);

        let kind_lbl = Label::new(Some(&entry.kind));
        kind_lbl.add_css_class("filer-item-meta");
        kind_lbl.set_width_chars(8);
        kind_lbl.set_xalign(1.0);
        row.append(&kind_lbl);

        self.attach_drag_source(&row, entry.path.clone());
        if entry.is_dir {
            self.attach_folder_drop_target(&row, entry.path.clone(), false);
        }
        row
    }

    fn attach_drag_source(self: &Rc<Self>, widget: &impl IsA<gtk4::Widget>, item_path: PathBuf) {
        let src = DragSource::new();
        src.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let w = Rc::clone(self);
        src.connect_prepare(move |_, _, _| {
            let mut paths = w.selected_paths();
            if !paths.contains(&item_path) {
                paths = vec![item_path.clone()];
            }
            let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
            let list = gdk::FileList::from_array(&files);
            Some(gdk::ContentProvider::for_value(&list.to_value()))
        });
        widget.add_controller(src);
    }

    fn attach_folder_drop_target(
        self: &Rc<Self>,
        widget: &impl IsA<gtk4::Widget>,
        target_dir: PathBuf,
        is_trash: bool,
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
        let w_enter = widget.clone();
        dt.connect_enter(move |_, _, _| {
            w_enter.add_css_class(cls);
            gdk::DragAction::MOVE
        });
        let w_leave = widget.clone();
        dt.connect_leave(move |_| w_leave.remove_css_class(cls));

        let w_drop = widget.clone();
        let this = Rc::clone(self);
        dt.connect_drop(move |target, val, _, _| {
            w_drop.remove_css_class(cls);
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
                let is_copy = target
                    .current_drop()
                    .is_some_and(|d| d.actions() == gdk::DragAction::COPY);
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
        let dst = self.state.borrow().cwd.clone();
        let kind = if is_cut { OpKind::Move } else { OpKind::Copy };
        self.run_op(kind, paths, Some(dst));
    }

    fn run_op(self: &Rc<Self>, kind: OpKind, sources: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
        let cancel = Arc::new(AtomicBool::new(false));
        *self.active_cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let (tx, rx) = mpsc::channel();
        ops::spawn_batch(kind, sources, dest_dir, tx, cancel);

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
                    Ok(OpProgress::Done { summary }) => {
                        w.progress_box.set_visible(false);
                        *w.active_cancel.borrow_mut() = None;
                        w.show_banner(&summary, "ok");
                        w.reload_dir();
                        return glib::ControlFlow::Break;
                    }
                    Ok(OpProgress::Error { message }) => {
                        w.progress_box.set_visible(false);
                        *w.active_cancel.borrow_mut() = None;
                        w.show_banner(&message, "danger");
                        w.reload_dir();
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
            .filter_map(|i| st.visible.get(i).cloned())
            .collect()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    fn activate_index(self: &Rc<Self>, idx: usize) {
        let entry = self.state.borrow().visible.get(idx).cloned();
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
        let w = Rc::clone(self);
        let card = quicklook::build_quicklook_card(&entry, move || w.close_quicklook());
        self.overlay.add_overlay(&card);
        *self.quicklook_slot.borrow_mut() = Some(card);
    }

    fn close_quicklook(&self) {
        if let Some(card) = self.quicklook_slot.borrow_mut().take() {
            self.overlay.remove_overlay(&card);
        }
    }

    fn show_sort_popover(self: &Rc<Self>, anchor: &Button) {
        let pop = Popover::new();
        pop.set_parent(anchor);
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

    fn show_canvas_context_menu(self: &Rc<Self>, x: f64, y: f64) {
        let pop = Popover::new();
        pop.set_parent(&self.canvas_scroll);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.set_has_arrow(false);

        let col = GtkBox::new(Orientation::Vertical, 2);
        let sel = self.selected_entries();
        let in_trash = self.state.borrow().in_trash;
        let cwd = self.state.borrow().cwd.clone();

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
                let w = Rc::clone(self);
                add(
                    "\u{f06e}",
                    "Quick Look",
                    Some("Space"),
                    false,
                    Box::new(move || w.toggle_quicklook()),
                );
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
                Box::new(move || {
                    if let Err(e) = trash::empty_trash() {
                        w.show_banner(&e, "danger");
                    } else {
                        w.show_banner("Trash emptied", "ok");
                        w.reload_dir();
                    }
                }),
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
            add(
                "\u{f0a0}",
                "Format Drive…",
                None,
                false,
                Box::new(tools::open_format_drive),
            );
        }

        pop.set_child(Some(&col));
        pop.popup();
    }

    fn prompt_new_folder(self: &Rc<Self>) {
        let pop = Popover::new();
        pop.set_parent(&self.crumbs_box);
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
            let target = w.state.borrow().cwd.join(&name);
            match std::fs::create_dir(&target) {
                Ok(()) => {
                    w.show_banner(&format!("Created folder {name}"), "ok");
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
        pop.set_parent(&self.crumbs_box);
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
                    w.show_banner(&format!("Renamed to {new_name}"), "ok");
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
        pop.set_parent(&self.crumbs_box);
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

    fn update_status_bar(&self) {
        let st = self.state.borrow();
        let total = st.visible.len();
        let cwd = st.cwd.clone();
        drop(st);

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
            .set_text(&format!("{total} items{sel_part}{free_part}"));
    }

    fn show_banner(&self, msg: &str, tone: &str) {
        self.banner_box.remove_css_class("ok");
        self.banner_box.remove_css_class("warn");
        self.banner_box.remove_css_class("danger");
        self.banner_box.add_css_class(tone);
        self.banner_label.set_text(msg);
        self.banner_box.set_visible(true);

        let bb = self.banner_box.clone();
        let expected = msg.to_string();
        let bl = self.banner_label.clone();
        glib::timeout_add_local_once(Duration::from_secs(4), move || {
            if bl.text() == expected {
                bb.set_visible(false);
            }
        });
    }
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
