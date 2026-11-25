//! Space-bar Quick Look overlay: instant preview for images, text, audio, folders, and metadata.

use std::fs;
use std::io::Read;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Picture, ScrolledWindow, TextView};
use sha2::{Digest, Sha256};

use crate::fs::{FileEntry, format_mode, format_mtime, format_size};
use crate::tools;

pub fn build_quicklook_card(entry: &FileEntry, on_close: impl Fn() + 'static) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 10);
    card.add_css_class("filer-quicklook");
    card.set_halign(Align::Center);
    card.set_valign(Align::Center);
    card.set_size_request(540, 360);

    // Header: Filename + metadata
    let top = GtkBox::new(Orientation::Horizontal, 8);
    let title = Label::new(Some(&entry.name));
    title.add_css_class("filer-quicklook-title");
    title.set_halign(Align::Start);
    title.set_hexpand(true);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);

    let meta_str = format!(
        "{} · {} · {} · {}",
        entry.kind,
        if entry.is_dir {
            count_children(&entry.path)
        } else {
            format_size(entry.size)
        },
        format_mtime(entry.modified_secs),
        format_mode(entry.mode),
    );
    let meta = Label::new(Some(&meta_str));
    meta.add_css_class("filer-item-meta");
    meta.set_halign(Align::Start);

    let header_col = GtkBox::new(Orientation::Vertical, 2);
    header_col.set_hexpand(true);
    header_col.append(&title);
    header_col.append(&meta);
    top.append(&header_col);

    let close_btn = Button::with_label("\u{f00d}");
    close_btn.add_css_class("filer-nav-btn");
    close_btn.add_css_class("filer-glyph");
    close_btn.connect_clicked(move |_| on_close());
    top.append(&close_btn);
    card.append(&top);

    // Preview Body
    if entry.is_image() {
        let pic = Picture::for_filename(&entry.path);
        pic.set_can_shrink(true);
        pic.set_vexpand(true);
        pic.set_hexpand(true);
        pic.set_size_request(480, 260);
        card.append(&pic);
    } else if entry.is_text_previewable() {
        let mut buf = Vec::new();
        if let Ok(f) = fs::File::open(&entry.path) {
            let _ = f.take(64 * 1024).read_to_end(&mut buf);
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let tv = TextView::new();
        tv.set_editable(false);
        tv.set_cursor_visible(false);
        tv.set_monospace(true);
        tv.add_css_class("filer-quicklook-text");
        tv.buffer().set_text(&text);

        let sc = ScrolledWindow::new();
        sc.set_vexpand(true);
        sc.set_hexpand(true);
        sc.set_min_content_height(240);
        sc.set_child(Some(&tv));
        card.append(&sc);
    } else {
        let info_box = GtkBox::new(Orientation::Vertical, 8);
        info_box.set_vexpand(true);
        info_box.set_valign(Align::Center);
        info_box.set_halign(Align::Center);

        let icon = Label::new(Some(entry.icon_glyph));
        icon.add_css_class("filer-ql-glyph");
        icon.add_css_class(entry.icon_class);
        info_box.append(&icon);

        let path_lbl = Label::new(Some(&entry.path.display().to_string()));
        path_lbl.add_css_class("filer-item-meta");
        path_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        path_lbl.set_max_width_chars(58);
        info_box.append(&path_lbl);

        if entry.is_symlink
            && let Ok(target) = fs::read_link(&entry.path)
        {
            let link_lbl = Label::new(Some(&format!("→ {}", target.display())));
            link_lbl.add_css_class("filer-item-meta");
            info_box.append(&link_lbl);
        }

        if !entry.is_dir {
            let hash_lbl = Label::new(None);
            hash_lbl.add_css_class("filer-item-meta");
            let hash_btn = Button::with_label("Compute SHA-256");
            hash_btn.add_css_class("filer-tool-btn");
            let p_clone = entry.path.clone();
            let hl_clone = hash_lbl.clone();
            hash_btn.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                if let Ok(mut f) = fs::File::open(&p_clone) {
                    let mut hasher = Sha256::new();
                    let mut buf = [0u8; 64 * 1024];
                    while let Ok(n) = f.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                    }
                    hl_clone.set_text(&format!("sha256: {:x}", hasher.finalize()));
                } else {
                    hl_clone.set_text("Failed to read file");
                }
            });
            info_box.append(&hash_btn);
            info_box.append(&hash_lbl);
        }
        card.append(&info_box);
    }

    // Bottom quick actions
    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);

    let open_btn = Button::with_label(if entry.is_audio() {
        "Play in Cassette"
    } else {
        "Open"
    });
    open_btn.add_css_class("filer-tool-btn");
    let p_open = entry.path.clone();
    let is_audio = entry.is_audio();
    open_btn.connect_clicked(move |_| {
        if is_audio {
            tools::play_in_cassette(&p_open);
        } else {
            tools::open_file(&p_open);
        }
    });
    actions.append(&open_btn);

    let share_btn = Button::with_label("Share…");
    share_btn.add_css_class("filer-tool-btn");
    let p_share = entry.path.clone();
    share_btn.connect_clicked(move |_| {
        tools::share_paths(std::slice::from_ref(&p_share));
    });
    actions.append(&share_btn);

    if entry.is_image() {
        let lens_btn = Button::with_label("Scan / OCR");
        lens_btn.add_css_class("filer-tool-btn");
        let p_lens = entry.path.clone();
        lens_btn.connect_clicked(move |_| {
            tools::open_in_lens(&p_lens);
        });
        actions.append(&lens_btn);
    }

    let copy_btn = Button::with_label("Copy Path");
    copy_btn.add_css_class("filer-tool-btn");
    let p_copy = entry.path.clone();
    copy_btn.connect_clicked(move |_| {
        tools::copy_paths_to_clipboard(std::slice::from_ref(&p_copy));
    });
    actions.append(&copy_btn);

    card.append(&actions);
    card
}

fn count_children(dir: &std::path::Path) -> String {
    let n = fs::read_dir(dir)
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    format!("{n} items")
}
