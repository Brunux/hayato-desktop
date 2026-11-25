{{ header:/* }}
/* Hayato Filer: minimalistic Finder / elementary OS Files layout with Hayato
 * Cyber-HUD geometry and live theme tokens.
 *
 * This file is generated; edit rust/crates/hayato-theme/templates/filer.css.tpl.
 */

window.hayato-filer {
    background-color: {{ background }};
    color: {{ text }};
    font-size: 13px;
}

/* ── Top Header & Breadcrumbs ────────────────────────────────────────────── */
.filer-header {
    background-color: {{ surface }};
    border-bottom: 1px solid {{ border }};
    padding: 4px 8px;
    min-height: 36px;
}

.filer-nav-btn,
.filer-tool-btn {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 5px;
    color: {{ text }};
    padding: 4px 8px;
    min-height: 24px;
    min-width: 24px;
    box-shadow: none;
}

.filer-nav-btn:hover,
.filer-tool-btn:hover {
    background-color: {{ surface_alt }};
    border-color: {{ border }};
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
    border-radius: 5px;
    padding: 2px 6px;
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
}

.filer-crumb {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: {{ dim }};
    padding: 2px 6px;
    min-height: 20px;
    box-shadow: none;
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
    border-radius: 5px;
    padding: 2px 8px;
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
    min-height: 24px;
}

/* ── Left Sidebar (Places & Devices) ─────────────────────────────────────── */
.filer-sidebar {
    background-color: {{ surface }};
    border-right: 1px solid {{ border }};
    padding: 8px 6px;
}

.filer-section-label {
    color: {{ dim }};
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 1px;
    padding: 8px 8px 4px 8px;
}

.filer-place-row {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 5px;
    padding: 5px 8px;
    margin: 1px 0;
    color: {{ text }};
}

.filer-place-row:hover {
    background-color: {{ surface_alt }};
}

.filer-place-row.active {
    background-color: {{ selection }};
    border-left: 2px solid {{ accent }};
    color: {{ strong }};
    font-weight: 600;
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
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: {{ dim }};
    padding: 1px 5px;
    min-height: 18px;
    min-width: 18px;
    box-shadow: none;
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

.filer-grid-item {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 8px 6px;
    margin: 3px;
}

.filer-grid-item:hover {
    background-color: {{ surface }};
    border-color: {{ border }};
}

.filer-grid-item.selected {
    background-color: {{ selection }};
    border-color: {{ accent }};
}

.filer-list-row {
    background: transparent;
    border-left: 2px solid transparent;
    border-radius: 4px;
    padding: 4px 8px;
    margin: 1px 4px;
}

.filer-list-row:hover {
    background-color: {{ surface }};
}

.filer-list-row.selected {
    background-color: {{ selection }};
    border-left-color: {{ accent }};
}

.filer-item-name {
    color: {{ text }};
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
    border-radius: 8px;
    padding: 14px;
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
    border-radius: 4px;
    padding: 8px;
}

.filer-banner {
    background-color: {{ surface_alt }};
    border-bottom: 1px solid {{ border }};
    padding: 5px 10px;
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
    padding: 3px 10px;
    min-height: 24px;
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

/* ── Popovers & Menus ────────────────────────────────────────────────────── */
popover.menu > contents,
popover > contents {
    background-color: {{ surface }};
    color: {{ text }};
    border: 1px solid {{ border }};
    border-radius: 6px;
    padding: 4px;
}

popover.menu modelbutton:hover {
    background-color: {{ selection }};
    color: {{ strong }};
    border-radius: 4px;
}
