{{ header:/* }}
/* Hayato Filer: minimalistic Finder / elementary OS Files layout with Hayato
 * Cyber-HUD geometry, vector Font Awesome icons, and live theme tokens.
 *
 * Colour rules (all 16 dark & light themes):
 * - Always use {{ text }} (with font-weight) for primary/emphasis text, never
 *   {{ strong }} (which blends into the background on Catppuccin Latte).
 * - Use translucent {{ text:rgb }} hairlines and hovers so passive chrome stays
 *   calm like Waybar while {{ accent }} is reserved for focus & active cues.
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
    background-color: rgba({{ text:rgb }}, 0.11);
    min-width: 1px;
}

/* ── Vector Font Awesome Icons & Semantic Theme Tints ────────────────────── */
.filer-glyph,
.filer-place-icon,
.filer-menu-icon,
.filer-grid-glyph,
.filer-list-glyph,
.filer-ql-glyph,
.filer-shelf-glyph,
.filer-empty-glyph {
    font-family: "FontAwesome", "Font Awesome 6 Free Solid", sans-serif;
}

.filer-glyph {
    font-size: 13px;
}

.filer-place-icon {
    font-size: 13px;
    color: rgba({{ text:rgb }}, 0.68);
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
    color: rgba({{ text:rgb }}, 0.36);
}

.icon-folder  { color: {{ accent }}; }
.icon-image   { color: {{ info }}; }
.icon-audio   { color: {{ ok }}; }
.icon-video   { color: {{ accent_alt }}; }
.icon-pdf     { color: {{ danger }}; }
.icon-archive { color: {{ warn }}; }
.icon-code    { color: {{ accent_alt }}; }
.icon-exec    { color: {{ ok }}; }
.icon-file    { color: rgba({{ text:rgb }}, 0.68); }

/* ── Top Header, Segmented Controls & Breadcrumbs ────────────────────────── */
.filer-header {
    background-color: {{ surface }};
    border-bottom: 1px solid rgba({{ text:rgb }}, 0.11);
    padding: 6px 10px;
    min-height: 40px;
}

.filer-seg-group {
    background-color: {{ background }};
    border: 1px solid rgba({{ text:rgb }}, 0.12);
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
    background-color: rgba({{ text:rgb }}, 0.08);
    color: {{ text }};
}

.filer-nav-btn:disabled {
    color: rgba({{ text:rgb }}, 0.36);
}

.filer-tool-btn.active {
    background-color: rgba({{ accent:rgb }}, 0.18);
    border-color: transparent;
    color: {{ accent }};
    font-weight: 600;
}

.filer-tool-btn.danger {
    background-color: rgba({{ danger:rgb }}, 0.16);
    border-color: rgba({{ danger:rgb }}, 0.45);
    color: {{ danger_text }};
    font-weight: 600;
}

.filer-tool-btn.danger:hover {
    background-color: rgba({{ danger:rgb }}, 0.26);
}

.filer-breadcrumbs {
    background-color: {{ background }};
    border: 1px solid rgba({{ text:rgb }}, 0.12);
    border-radius: 6px;
    padding: 2px 6px;
    min-height: 28px;
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
}

.filer-breadcrumbs:hover {
    border-color: rgba({{ text:rgb }}, 0.22);
}

.filer-crumb {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: rgba({{ text:rgb }}, 0.70);
    padding: 2px 6px;
    min-height: 22px;
}

.filer-crumb:hover {
    background-color: rgba({{ text:rgb }}, 0.08);
    color: {{ text }};
}

.filer-crumb.current {
    color: {{ text }};
    font-weight: 700;
}

.filer-crumb-sep {
    color: rgba({{ text:rgb }}, 0.42);
    padding: 0 2px;
}

.filer-path-entry,
.filer-filter-entry {
    background-color: {{ background }};
    color: {{ text }};
    border: 1px solid {{ accent }};
    border-radius: 6px;
    padding: 2px 8px;
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
    min-height: 26px;
}

/* ── Minimalist Finder / elementary OS Tab Bar ───────────────────────────── */
.filer-tabbar {
    background-color: {{ surface }};
    border-bottom: 1px solid rgba({{ text:rgb }}, 0.11);
    padding: 3px 8px;
    min-height: 30px;
}

.filer-tab {
    background-color: transparent;
    border: 1px solid transparent;
    border-bottom: 2px solid transparent;
    border-radius: 6px;
    padding: 1px 4px;
    min-width: 96px;
}

.filer-tab:hover {
    background-color: rgba({{ text:rgb }}, 0.08);
}

.filer-tab.active {
    background-color: {{ background }};
    border-color: rgba({{ text:rgb }}, 0.12);
    border-bottom-color: {{ accent }};
}

.filer-tab-btn {
    background-color: transparent;
    border: none;
    padding: 2px 6px;
    color: rgba({{ text:rgb }}, 0.70);
    font-size: 12px;
}

.filer-tab.active .filer-tab-btn {
    color: {{ text }};
    font-weight: 600;
}

.filer-tab-icon {
    font-family: "FontAwesome", "Font Awesome 6 Free Solid", sans-serif;
    font-size: 11px;
    color: rgba({{ text:rgb }}, 0.65);
}

.filer-tab.active .filer-tab-icon {
    color: {{ accent }};
}

.filer-tab-close {
    background-color: transparent;
    border: none;
    border-radius: 4px;
    color: rgba({{ text:rgb }}, 0.62);
    padding: 1px 4px;
    min-height: 18px;
    min-width: 18px;
    font-family: "FontAwesome", "Font Awesome 6 Free Solid", sans-serif;
    font-size: 10px;
}

.filer-tab-close:hover {
    background-color: rgba({{ text:rgb }}, 0.10);
    color: {{ danger_text }};
}

.filer-tab-add {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 5px;
    color: rgba({{ text:rgb }}, 0.70);
    padding: 2px 7px;
    min-height: 22px;
    min-width: 24px;
}

.filer-tab-add:hover {
    background-color: rgba({{ text:rgb }}, 0.08);
    color: {{ text }};
}

/* ── Left Sidebar (Places & Devices) ─────────────────────────────────────── */
.filer-sidebar {
    background-color: {{ surface }};
    padding: 8px 6px;
}

.filer-sidebar.compact {
    padding: 8px 4px;
}

.filer-sidebar.compact .filer-place-row {
    padding: 7px 4px;
}

.filer-sidebar.compact .filer-place-icon {
    font-size: 15px;
}

.filer-sidebar.compact .filer-device-card {
    padding: 4px 2px;
}

.filer-sidebar-sep {
    background-color: rgba({{ text:rgb }}, 0.11);
    min-height: 1px;
    margin: 6px 6px;
}

.filer-section-label {
    color: rgba({{ text:rgb }}, 0.60);
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
    background-color: rgba({{ text:rgb }}, 0.08);
    color: {{ text }};
}

.filer-place-row.active {
    background-color: rgba({{ accent:rgb }}, 0.14);
    border-left-color: {{ accent }};
    color: {{ text }};
    font-weight: 600;
}

.filer-place-row.reorder-hover {
    border-top: 2px solid {{ accent }};
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
    background-color: rgba({{ text:rgb }}, 0.08);
}

.filer-device-card.active {
    background-color: rgba({{ accent:rgb }}, 0.14);
    border-left-color: {{ accent }};
}

.filer-device-btn {
    background-color: transparent;
    border: none;
    padding: 2px 4px;
    color: {{ text }};
}

.filer-badge {
    background-color: rgba({{ text:rgb }}, 0.12);
    color: {{ text }};
    border-radius: 8px;
    padding: 1px 6px;
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
    font-weight: 600;
}

.filer-dev-action {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: rgba({{ text:rgb }}, 0.70);
    padding: 2px 6px;
    min-height: 20px;
    min-width: 22px;
}

.filer-dev-action:hover {
    background-color: rgba({{ text:rgb }}, 0.10);
    border-color: rgba({{ text:rgb }}, 0.16);
    color: {{ accent }};
}

.filer-usage-bar trough {
    background-color: rgba({{ text:rgb }}, 0.12);
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
    background-color: rgba({{ text:rgb }}, 0.06);
    border-color: rgba({{ text:rgb }}, 0.12);
}

flowboxchild:selected .filer-grid-item {
    background-color: rgba({{ accent:rgb }}, 0.16);
    border-color: {{ accent }};
}

.filer-thumb {
    border-radius: 6px;
    border: 1px solid rgba({{ text:rgb }}, 0.16);
    background-color: {{ surface }};
    overflow: hidden;
}

/* Sticky List View Column Header (blends cleanly into the canvas) */
.filer-list-header {
    background-color: {{ background }};
    border-bottom: 1px solid rgba({{ text:rgb }}, 0.10);
    padding: 4px 16px;
}

.filer-col-btn {
    background-color: transparent;
    border: none;
    border-radius: 4px;
    min-width: 0;
    min-height: 0;
    padding: 2px 4px;
    color: rgba({{ text:rgb }}, 0.62);
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.8px;
}

.filer-col-btn:hover,
.filer-col-btn.active {
    color: {{ text }};
}

.filer-col-resizer {
    background-color: transparent;
    border-left: 2px solid rgba({{ text:rgb }}, 0.14);
    margin: 2px 0px 2px 4px;
    min-width: 5px;
}

.filer-col-resizer:hover,
.filer-col-resizer.dragging {
    border-left-color: {{ accent }};
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
    background-color: rgba({{ text:rgb }}, 0.06);
}

list > row:selected .filer-list-row {
    background-color: rgba({{ accent:rgb }}, 0.16);
    border-left-color: {{ accent }};
}

.filer-item-name {
    color: {{ text }};
    font-size: 12px;
}

.filer-item-name.dir {
    color: {{ text }};
    font-weight: 600;
}

.filer-item-meta {
    color: rgba({{ text:rgb }}, 0.70);
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
}

/* ── Empty State ─────────────────────────────────────────────────────────── */
.filer-empty-state {
    padding: 32px;
}

.filer-empty-title {
    color: {{ text }};
    font-size: 14px;
    font-weight: 600;
}

.filer-empty-sub {
    color: rgba({{ text:rgb }}, 0.68);
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
}

/* ── Drag & Drop Highlighting ────────────────────────────────────────────── */
.drop-hover {
    background-color: rgba({{ accent:rgb }}, 0.16);
    border: 1px dashed {{ accent }};
    border-radius: 6px;
}

.drop-hover-trash {
    background-color: rgba({{ warn:rgb }}, 0.16);
    border: 1px dashed {{ warn_text }};
    border-radius: 6px;
}

/* ── Modal Backdrop Scrim & Quick Look / Properties Cards ────────────────── */
.filer-backdrop {
    background-color: rgba(0, 0, 0, 0.35);
}

.filer-quicklook {
    background-color: {{ surface }};
    border: 1px solid {{ accent }};
    border-radius: 10px;
    padding: 16px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
}

.filer-quicklook-title {
    color: {{ text }};
    font-weight: 700;
    font-size: 14px;
}

.filer-quicklook-text {
    background-color: {{ background }};
    color: {{ text }};
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
    border: 1px solid rgba({{ text:rgb }}, 0.14);
    border-radius: 6px;
    padding: 8px;
}

/* ── Drag-and-Drop Staging Shelf ─────────────────────────────────────────── */
.filer-shelf {
    background-color: {{ surface }};
    border-top: 1px solid rgba({{ text:rgb }}, 0.11);
    padding: 5px 10px;
    min-height: 36px;
}

.filer-shelf-title {
    color: rgba({{ text:rgb }}, 0.65);
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 1.2px;
}

.filer-shelf-hint {
    color: rgba({{ text:rgb }}, 0.50);
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
    padding: 2px 4px;
}

.filer-shelf-chip {
    background-color: {{ background }};
    border: 1px solid rgba({{ text:rgb }}, 0.16);
    border-radius: 6px;
    padding: 2px 4px 2px 8px;
    margin: 1px 0;
}

.filer-shelf-chip:hover {
    border-color: {{ accent }};
}

.filer-shelf-glyph {
    font-size: 12px;
    color: rgba({{ text:rgb }}, 0.68);
}

.filer-shelf-remove {
    background-color: transparent;
    border: none;
    border-radius: 4px;
    color: rgba({{ text:rgb }}, 0.65);
    padding: 1px 4px;
    min-height: 18px;
    min-width: 18px;
    font-size: 10px;
}

.filer-shelf-remove:hover {
    background-color: rgba({{ text:rgb }}, 0.10);
    color: {{ danger_text }};
}

/* ── Quick Properties Inspector ──────────────────────────────────────────── */
.filer-properties {
    background-color: {{ surface }};
    border: 1px solid {{ accent }};
    border-radius: 10px;
    padding: 16px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
}

.filer-properties-title {
    color: {{ text }};
    font-weight: 700;
    font-size: 14px;
}

.filer-properties-label {
    color: rgba({{ text:rgb }}, 0.68);
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
}

.filer-properties-value {
    color: {{ text }};
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
}

/* ── Bottom Monospace HUD Status Bar ─────────────────────────────────────── */
.filer-statusbar {
    background-color: {{ surface }};
    border-top: 1px solid rgba({{ text:rgb }}, 0.11);
    padding: 4px 12px;
    min-height: 26px;
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
    color: rgba({{ text:rgb }}, 0.70);
}

.filer-progress trough {
    background-color: rgba({{ text:rgb }}, 0.12);
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
    border: 1px solid rgba({{ text:rgb }}, 0.18);
    border-radius: 8px;
    padding: 6px;
    min-width: 215px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
}

.filer-menu-item {
    background-color: transparent;
    border: none;
    border-radius: 5px;
    padding: 5px 8px;
    color: {{ text }};
}

.filer-menu-item:hover {
    background-color: rgba({{ accent:rgb }}, 0.16);
    color: {{ text }};
}

.filer-menu-item.danger {
    color: {{ danger_text }};
}

.filer-menu-icon {
    font-size: 12px;
    color: rgba({{ text:rgb }}, 0.68);
}

.filer-menu-item:hover .filer-menu-icon {
    color: {{ accent }};
}

.filer-menu-item.danger .filer-menu-icon {
    color: {{ danger_text }};
}

.filer-menu-shortcut {
    color: rgba({{ text:rgb }}, 0.60);
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
}

.filer-menu-sep {
    background-color: rgba({{ text:rgb }}, 0.11);
    min-height: 1px;
    margin: 4px 2px;
}
