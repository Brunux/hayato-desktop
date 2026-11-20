{{ header:/* }}
/* The games library and the HTML games: the active palette as CSS variables.
 *
 * The library page and every web game read these names, so a theme switch
 * repaints the grid and the games that use the palette without either of them
 * knowing which theme is active. The values are the same roles the GTK4 file
 * next door uses, so a game window and a document window are the same colours.
 *
 * This file is generated; edit the template in
 * rust/crates/hayato-theme/templates/games.css.tpl, not this file.
 */
:root {
  --bg: {{ background }};
  --surface: {{ surface }};
  --surface-alt: {{ surface_alt }};
  --border: {{ border }};
  --faint: {{ faint }};
  --dim: {{ dim }};
  --text: {{ text }};
  --strong: {{ strong }};
  --accent: {{ accent }};
  --accent-alt: {{ accent_alt }};
  --ok: {{ ok }};
  --warn: {{ warn }};
  --danger: {{ danger }};
  --info: {{ info }};
  --selection: {{ selection }};
}
