{{ header:/* }}
/* Hayato Filer: minimalistic Finder / elementary OS Files layout with Hayato
 * Cyber-HUD geometry, vector Font Awesome icons, and live theme tokens.
 *
 * This file is generated; edit rust/crates/hayato-theme/templates/filer.css.tpl.
 */

window.hayato-filer {
    background-color: {{ background }};
    color: {{ text }};
    font-size: 13px;
}

window.hayato-filer button {
    background-image: none;
    box-shadow: none;
    outline: none;
}

window.hayato-filer paned > separator {
    background-color: {{ border }};
    min-width: 1px;
}

/* ── Vector Font Awesome Icons & Semantic Theme Tints ────────────────────── */
.filer-glyph,
.filer-place-icon,
.filer-menu-icon,
.filer-grid-glyph,
.filer-list-glyph,
.filer-ql-glyph,
.filer-empty-glyph {
    font-family: "FontAwesome", "Font Awesome 6 Free Solid", sans-serif;
}

.filer-glyph {
    font-size: 13px;
}

.filer-place-icon {
    font-size: 13px;
    color: {{ dim }};
}

.filer-place-row.active .filer-place-icon,
.filer-device-card.active .filer-place-icon {
    color: {{ accent }};
}

.filer-grid-glyph {
    font-size: 36px;
}

.filer-list-glyph {
    font-size: 15px;
}

.filer-ql-glyph {
    font-size: 56px;
}

.filer-empty-glyph {
    font-size: 44px;
    color: {{ faint }};
}

.icon-folder  { color: {{ accent }}; }
.icon-image   { color: {{ info }}; }
.icon-audio   { color: {{ ok }}; }
.icon-video   { color: {{ accent_alt }}; }
.icon-pdf     { color: {{ danger }}; }
.icon-archive { color: {{ warn }}; }
.icon-code    { color: {{ accent_alt }}; }
.icon-exec    { color: {{ ok }}; }
.icon-file    { color: {{ dim }}; }

/* ── Top Header, Segmented Controls & Breadcrumbs ────────────────────────── */
.filer-header {
    background-color: {{ surface }};
    border-bottom: 1px solid {{ border }};
    padding: 6px 10px;
    min-height: 40px;
}

.filer-seg-group {
    background-color: {{ background }};
    border: 1px solid {{ border }};
    border-radius: 6px;
    padding: 2px;
}

.filer-nav-btn,
.filer-tool-btn {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 5px;
    color: {{ text }};
    padding: 3px 8px;
    min-height: 24px;
    min-width: 26px;
}

.filer-nav-btn:hover,
.filer-tool-btn:hover {
    background-color: {{ surface_alt }};
    color: {{ strong }};
}

.filer-nav-btn:disabled {
    color: {{ faint }};
}

.filer-tool-btn.active {
    background-color: {{ selection }};
    border-color: {{ accent }};
    color: {{ strong }};
}

.filer-breadcrumbs {
    background-color: {{ background }};
    border: 1px solid {{ border }};
    border-radius: 6px;
    padding: 2px 6px;
    min-height: 28px;
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
}

.filer-crumb {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: {{ dim }};
    padding: 2px 6px;
    min-height: 22px;
}

.filer-crumb:hover {
    background-color: {{ surface_alt }};
    color: {{ strong }};
}

.filer-crumb.current {
    color: {{ strong }};
    font-weight: 700;
}

.filer-crumb-sep {
    color: {{ faint }};
    padding: 0 2px;
}

.filer-path-entry,
.filer-filter-entry {
    background-color: {{ background }};
    color: {{ strong }};
    border: 1px solid {{ accent }};
    border-radius: 6px;
    padding: 2px 8px;
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
    min-height: 26px;
}

/* ── Left Sidebar (Places & Devices) ─────────────────────────────────────── */
.filer-sidebar {
    background-color: {{ surface }};
    padding: 8px 6px;
}

.filer-section-label {
    color: {{ dim }};
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 1.2px;
    padding: 10px 8px 4px 8px;
}

.filer-place-row {
    background-color: transparent;
    border: 1px solid transparent;
    border-left: 2px solid transparent;
    border-radius: 5px;
    padding: 5px 8px;
    margin: 1px 0;
    color: {{ text }};
}

.filer-place-row:hover {
    background-color: {{ surface_alt }};
    color: {{ strong }};
}

.filer-place-row.active {
    background-color: {{ selection }};
    border-left-color: {{ accent }};
    color: {{ strong }};
    font-weight: 600;
}

.filer-device-card {
    background-color: transparent;
    border: 1px solid transparent;
    border-left: 2px solid transparent;
    border-radius: 5px;
    padding: 4px 6px;
    margin: 2px 0;
}

.filer-device-card:hover {
    background-color: {{ surface_alt }};
}

.filer-device-card.active {
    background-color: {{ selection }};
    border-left-color: {{ accent }};
}

.filer-device-btn {
    background-color: transparent;
    border: none;
    padding: 2px 4px;
    color: {{ text }};
}

.filer-badge {
    background-color: {{ surface_alt }};
    color: {{ dim }};
    border-radius: 8px;
    padding: 1px 6px;
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
}

.filer-dev-action {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: {{ dim }};
    padding: 2px 6px;
    min-height: 20px;
    min-width: 22px;
}

.filer-dev-action:hover {
    background-color: {{ background }};
    border-color: {{ border }};
    color: {{ accent }};
}

.filer-usage-bar trough {
    background-color: {{ background }};
    border-radius: 2px;
    min-height: 3px;
}

.filer-usage-bar progress {
    background-color: {{ accent }};
    border-radius: 2px;
    min-height: 3px;
}

/* ── Main Canvas (Grid & List Views) ─────────────────────────────────────── */
.filer-canvas {
    background-color: {{ background }};
    color: {{ text }};
}

flowboxchild {
    background: transparent;
    padding: 0;
    border-radius: 8px;
    outline: none;
}

flowboxchild:selected {
    background: transparent;
}

.filer-grid-item {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 8px;
    padding: 8px 6px;
    margin: 2px;
}

.filer-grid-item:hover {
    background-color: {{ surface }};
    border-color: {{ border }};
}

flowboxchild:selected .filer-grid-item {
    background-color: {{ selection }};
    border-color: {{ accent }};
}

.filer-thumb {
    border-radius: 6px;
    border: 1px solid {{ border }};
    background-color: {{ surface }};
    overflow: hidden;
}

/* Sticky List View Column Header */
.filer-list-header {
    background-color: {{ surface }};
    border-bottom: 1px solid {{ border }};
    padding: 3px 12px;
}

.filer-col-btn {
    background-color: transparent;
    border: none;
    border-radius: 4px;
    padding: 2px 4px;
    color: {{ dim }};
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.8px;
}

.filer-col-btn:hover,
.filer-col-btn.active {
    color: {{ strong }};
}

list > row {
    background: transparent;
    padding: 0;
    outline: none;
}

list > row:selected {
    background: transparent;
}

.filer-list-row {
    background-color: transparent;
    border-left: 2px solid transparent;
    border-radius: 4px;
    padding: 5px 10px;
    margin: 1px 6px;
}

.filer-list-row:hover {
    background-color: {{ surface }};
}

list > row:selected .filer-list-row {
    background-color: {{ selection }};
    border-left-color: {{ accent }};
}

.filer-item-name {
    color: {{ text }};
    font-size: 12px;
}

.filer-item-name.dir {
    color: {{ strong }};
    font-weight: 600;
}

.filer-item-meta {
    color: {{ dim }};
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
}

/* ── Empty State ─────────────────────────────────────────────────────────── */
.filer-empty-state {
    padding: 32px;
}

.filer-empty-title {
    color: {{ strong }};
    font-size: 14px;
    font-weight: 600;
}

.filer-empty-sub {
    color: {{ dim }};
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
}

/* ── Drag & Drop Highlighting ────────────────────────────────────────────── */
.drop-hover {
    background-color: {{ selection }};
    border: 1px dashed {{ accent }};
    border-radius: 6px;
}

.drop-hover-trash {
    background-color: {{ surface_alt }};
    border: 1px dashed {{ warn_text }};
    border-radius: 6px;
}

/* ── Quick Look Overlay & Banners ────────────────────────────────────────── */
.filer-quicklook {
    background-color: {{ surface }};
    border: 1px solid {{ accent }};
    border-radius: 10px;
    padding: 16px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
}

.filer-quicklook-title {
    color: {{ strong }};
    font-weight: 700;
    font-size: 14px;
}

.filer-quicklook-text {
    background-color: {{ background }};
    color: {{ text }};
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
    border: 1px solid {{ border }};
    border-radius: 6px;
    padding: 8px;
}

.filer-banner {
    background-color: {{ surface_alt }};
    border-bottom: 1px solid {{ border }};
    padding: 5px 12px;
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
}

.filer-banner.warn {
    color: {{ warn_text }};
}

.filer-banner.danger {
    color: {{ danger_text }};
}

.filer-banner.ok {
    color: {{ ok_text }};
}

/* ── Bottom Monospace HUD Status Bar ─────────────────────────────────────── */
.filer-statusbar {
    background-color: {{ surface }};
    border-top: 1px solid {{ border }};
    padding: 4px 12px;
    min-height: 26px;
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
    color: {{ dim }};
}

.filer-progress trough {
    background-color: {{ background }};
    border-radius: 2px;
    min-height: 5px;
    min-width: 90px;
}

.filer-progress progress {
    background-color: {{ accent }};
    border-radius: 2px;
    min-height: 5px;
}

/* ── Popovers & Finder-Style Context Menus ───────────────────────────────── */
popover > contents {
    background-color: {{ surface }};
    color: {{ text }};
    border: 1px solid {{ border }};
    border-radius: 8px;
    padding: 6px;
    min-width: 215px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
}

.filer-menu-item {
    background-color: transparent;
    border: none;
    border-radius: 5px;
    padding: 5px 8px;
    color: {{ text }};
}

.filer-menu-item:hover {
    background-color: {{ selection }};
    color: {{ strong }};
}

.filer-menu-item.danger {
    color: {{ danger_text }};
}

.filer-menu-icon {
    font-size: 12px;
    color: {{ dim }};
}

.filer-menu-item:hover .filer-menu-icon {
    color: {{ accent }};
}

.filer-menu-item.danger .filer-menu-icon {
    color: {{ danger_text }};
}

.filer-menu-shortcut {
    color: {{ dim }};
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
}

.filer-menu-sep {
    background-color: {{ border }};
    min-height: 1px;
    margin: 4px 2px;
}
