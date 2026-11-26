//! Alt+Enter / Ctrl+I Quick Properties: a compact monospace inspector card.
//!
//! The Quick Look overlay answers "what is this?"; this answers "exactly what
//! is it?" - byte count, permissions with a one-click executable bit, owner,
//! MIME type, symlink target, and an on-demand SHA-256. It reads with GIO and
//! libc, never a subprocess, except the hash the person asks for.

use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Separator};
use sha2::{Digest, Sha256};

use crate::fs::{FileEntry, format_mode, format_mtime, format_size};
use crate::tools;

pub fn build_properties_card(entry: &FileEntry, on_close: impl Fn() + 'static) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 10);
    card.add_css_class("filer-properties");
    card.set_halign(Align::Center);
    card.set_valign(Align::Center);
    card.set_size_request(520, -1);

    // Header: name + close
    let top = GtkBox::new(Orientation::Horizontal, 8);
    let title = Label::new(Some(&entry.name));
    title.add_css_class("filer-properties-title");
    title.set_halign(Align::Start);
    title.set_hexpand(true);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    let close_btn = Button::with_label("\u{f00d}");
    close_btn.add_css_class("filer-nav-btn");
    close_btn.add_css_class("filer-glyph");
    close_btn.set_tooltip_text(Some("Close (Esc)"));
    close_btn.connect_clicked(move |_| on_close());
    top.append(&title);
    top.append(&close_btn);
    card.append(&top);

    let grid = GtkBox::new(Orientation::Vertical, 3);
    card.append(&grid);

    grid.append(&row("Kind", &entry.kind));

    let size_val = if entry.is_dir {
        format!("{} ({} items)", format_size(entry.size), count_children(&entry.path))
    } else {
        format!("{} ({} bytes)", format_size(entry.size), entry.size)
    };
    grid.append(&row("Size", &size_val));

    grid.append(&row(
        "Where",
        &entry
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/".to_string()),
    ));
    grid.append(&row("Modified", &format_mtime(entry.modified_secs)));

    // Permissions row carries the executable toggle.
    let meta = fs::symlink_metadata(&entry.path).ok();
    let perm_row = GtkBox::new(Orientation::Horizontal, 8);
    perm_row.append(&row_label("Permissions"));
    let perm_val = Label::new(Some(&format!(
        "{}  {:04o}",
        format_mode(entry.mode),
        entry.mode & 0o7777
    )));
    perm_val.add_css_class("filer-properties-value");
    perm_val.set_halign(Align::Start);
    perm_val.set_hexpand(true);
    perm_row.append(&perm_val);
    // Only a regular file gets the executable toggle (a directory's x bit is
    // its right to be entered; a symlink would chmod the target).
    if !entry.is_dir && !entry.is_symlink {
        let executable = entry.mode & 0o111 != 0;
        let toggle = Button::with_label(if executable { "Remove executable" } else { "Make executable" });
        toggle.add_css_class("filer-tool-btn");
        let p = entry.path.clone();
        let pv = perm_val.clone();
        let tv = toggle.clone();
        toggle.connect_clicked(move |_| {
            let Ok(meta) = fs::symlink_metadata(&p) else {
                return;
            };
            let mode = meta.permissions().mode();
            let new_mode = if mode & 0o111 != 0 {
                mode & !0o111
            } else {
                mode | 0o111
            };
            if fs::set_permissions(&p, fs::Permissions::from_mode(new_mode)).is_ok() {
                pv.set_text(&format!("{}  {:04o}", format_mode(new_mode), new_mode & 0o7777));
                tv.set_label(if new_mode & 0o111 != 0 {
                    "Remove executable"
                } else {
                    "Make executable"
                });
            }
        });
        perm_row.append(&toggle);
    }
    grid.append(&perm_row);

    if let Some(m) = &meta {
        let owner = user_name(m.uid()).unwrap_or_else(|| m.uid().to_string());
        let group = group_name(m.gid()).unwrap_or_else(|| m.gid().to_string());
        grid.append(&row("Owner", &format!("{owner}:{group}")));
    }

    if let Some(mime) = mime_type(&entry.path) {
        grid.append(&row("MIME", &mime));
    }

    if entry.is_symlink
        && let Ok(target) = fs::read_link(&entry.path)
    {
        grid.append(&row("Links to", &target.display().to_string()));
    }

    // Hash + actions
    let sep = Separator::new(Orientation::Horizontal);
    sep.add_css_class("filer-menu-sep");
    card.append(&sep);

    let hash_lbl = Label::new(None);
    hash_lbl.add_css_class("filer-properties-value");
    hash_lbl.set_halign(Align::Start);
    hash_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    hash_lbl.set_max_width_chars(64);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);

    if !entry.is_dir {
        let hash_btn = Button::with_label("Compute SHA-256");
        hash_btn.add_css_class("filer-tool-btn");
        let p = entry.path.clone();
        let hl = hash_lbl.clone();
        hash_btn.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            match sha256_of(&p) {
                Ok(hash) => hl.set_text(&hash),
                Err(e) => hl.set_text(&format!("Failed: {e}")),
            }
        });
        actions.append(&hash_btn);
    }

    let copy_btn = Button::with_label("Copy Path");
    copy_btn.add_css_class("filer-tool-btn");
    let p_copy = entry.path.clone();
    copy_btn.connect_clicked(move |_| tools::copy_paths_to_clipboard(std::slice::from_ref(&p_copy)));
    actions.append(&copy_btn);

    card.append(&hash_lbl);
    card.append(&actions);
    card
}

fn row(label: &str, value: &str) -> GtkBox {
    let r = GtkBox::new(Orientation::Horizontal, 8);
    r.append(&row_label(label));
    let v = Label::new(Some(value));
    v.add_css_class("filer-properties-value");
    v.set_halign(Align::Start);
    v.set_hexpand(true);
    v.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    v.set_selectable(true);
    r.append(&v);
    r
}

fn row_label(text: &str) -> Label {
    let l = Label::new(Some(text));
    l.add_css_class("filer-properties-label");
    l.set_halign(Align::Start);
    l.set_width_chars(13);
    l
}

fn count_children(dir: &Path) -> usize {
    fs::read_dir(dir).map(|rd| rd.flatten().count()).unwrap_or(0)
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256: {:x}", hasher.finalize()))
}

fn mime_type(path: &Path) -> Option<String> {
    let info = gtk4::gio::File::for_path(path)
        .query_info(
            "standard::content-type",
            gtk4::gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gtk4::gio::Cancellable::NONE,
        )
        .ok()?;
    info.content_type().map(|s| s.to_string())
}

fn user_name(uid: u32) -> Option<String> {
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*pw).pw_name)
            .to_str()
            .ok()
            .map(str::to_string)
    }
}

fn group_name(gid: u32) -> Option<String> {
    unsafe {
        let gr = libc::getgrgid(gid);
        if gr.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*gr).gr_name)
            .to_str()
            .ok()
            .map(str::to_string)
    }
}
