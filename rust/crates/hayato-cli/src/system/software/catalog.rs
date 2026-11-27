//! Curated intent-first application catalog (`desktop/packages/software/catalog.json`).
//!
//! Built directly into `hayato` via `include_str!` so Discover renders in < 50 ms
//! offline with zero disk/network dependency, while respecting optional overrides
//! at `/usr/share/hayato/software/catalog.json` or `~/.config/hayato/software/catalog.json`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use hayato_ui::Glyph;
use serde::Deserialize;

use crate::system::pkg_guard;

const BUILTIN_CATALOG_JSON: &str = include_str!("../../../../../../packages/software/catalog.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppWeight {
    Light,
    Medium,
    Heavy,
}

impl AppWeight {
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Heavy => "heavy · 4 GB note",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Heavy => "heavy",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Intent {
    pub id: String,
    pub title: String,
    pub blurb: String,
    #[serde(default)]
    pub glyph: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub apps: Vec<String>,
}

impl Intent {
    pub fn ui_glyph(&self) -> Glyph {
        match self.glyph.as_str() {
            "video" => Glyph::Video,
            "python" => Glyph::Python,
            "document" => Glyph::Document,
            "brush" => Glyph::Brush,
            "target" => Glyph::Target,
            "music" => Glyph::Music,
            "camera" => Glyph::Camera,
            "shield" => Glyph::Shield,
            "sliders" => Glyph::Sliders,
            "globe" => Glyph::Globe,
            "image" => Glyph::Image,
            "code" => Glyph::Code,
            _ => Glyph::Box,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct CatalogApp {
    pub id: String,
    pub name: String,
    pub package: String,
    pub summary: String,
    #[serde(default = "default_weight")]
    pub weight: AppWeight,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub launch: Option<String>,
}

fn default_weight() -> AppWeight {
    AppWeight::Light
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawCatalog {
    pub schema: u32,
    pub intents: Vec<Intent>,
    pub apps: Vec<CatalogApp>,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub intents: Vec<Intent>,
    pub apps: Vec<CatalogApp>,
    by_id: HashMap<String, usize>,
    by_pkg: HashMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedIntent {
    pub intent_idx: usize,
    pub score: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedApp {
    pub app_idx: usize,
    pub score: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogSearchResults {
    pub intents: Vec<RankedIntent>,
    pub apps: Vec<RankedApp>,
}

impl Catalog {
    pub fn load() -> Self {
        for path in override_paths() {
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(cat) = Self::from_json(&text)
            {
                return cat;
            }
        }
        Self::from_json(BUILTIN_CATALOG_JSON).expect("built-in software catalog JSON must be valid")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let raw: RawCatalog =
            serde_json::from_str(text).map_err(|e| format!("invalid catalog JSON: {e}"))?;
        if raw.schema != 1 {
            return Err(format!("unsupported catalog schema: {}", raw.schema));
        }
        let mut by_id = HashMap::new();
        let mut by_pkg = HashMap::new();
        for (i, app) in raw.apps.iter().enumerate() {
            if by_id.insert(app.id.clone(), i).is_some() {
                return Err(format!("duplicate app id: {}", app.id));
            }
            by_pkg.entry(app.package.clone()).or_insert(i);
            if pkg_guard::is_kernel_or_boot(&app.package)
                || pkg_guard::is_removal_protected(&app.package)
            {
                return Err(format!("protected package in catalog: {}", app.package));
            }
        }
        let mut seen_intents = HashSet::new();
        for intent in &raw.intents {
            if !seen_intents.insert(intent.id.as_str()) {
                return Err(format!("duplicate intent id: {}", intent.id));
            }
            for app_id in &intent.apps {
                if !by_id.contains_key(app_id) {
                    return Err(format!(
                        "intent '{}' references unknown app '{app_id}'",
                        intent.id
                    ));
                }
            }
        }
        Ok(Self {
            intents: raw.intents,
            apps: raw.apps,
            by_id,
            by_pkg,
        })
    }

    pub fn app_by_id(&self, id: &str) -> Option<(usize, &CatalogApp)> {
        self.by_id.get(id).map(|&i| (i, &self.apps[i]))
    }

    pub fn app_by_package(&self, pkg: &str) -> Option<(usize, &CatalogApp)> {
        self.by_pkg.get(pkg).map(|&i| (i, &self.apps[i]))
    }

    pub fn apps_for_intent(&self, intent_idx: usize) -> Vec<(usize, &CatalogApp)> {
        let Some(intent) = self.intents.get(intent_idx) else {
            return Vec::new();
        };
        intent
            .apps
            .iter()
            .filter_map(|id| self.app_by_id(id))
            .collect()
    }

    /// Tokenized intent-first search ("video editor" -> matches `video-edit` intent + apps).
    pub fn search(&self, query: &str) -> CatalogSearchResults {
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '+' && c != '-')
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
            return CatalogSearchResults::default();
        }

        let mut intents = Vec::new();
        for (idx, intent) in self.intents.iter().enumerate() {
            let title_lc = intent.title.to_ascii_lowercase();
            let blurb_lc = intent.blurb.to_ascii_lowercase();
            let mut score = 0usize;
            for tok in &tokens {
                if intent.id.eq_ignore_ascii_case(tok)
                    || intent.keywords.iter().any(|k| k.eq_ignore_ascii_case(tok))
                {
                    score += 6;
                } else if title_lc.contains(tok) {
                    score += 4;
                } else if intent
                    .keywords
                    .iter()
                    .any(|k| k.to_ascii_lowercase().contains(tok))
                    || blurb_lc.contains(tok)
                {
                    score += 2;
                }
            }
            if score > 0 {
                intents.push(RankedIntent {
                    intent_idx: idx,
                    score,
                });
            }
        }
        intents.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.intent_idx.cmp(&b.intent_idx))
        });

        let mut app_scores: HashMap<usize, usize> = HashMap::new();
        for (idx, app) in self.apps.iter().enumerate() {
            let name_lc = app.name.to_ascii_lowercase();
            let pkg_lc = app.package.to_ascii_lowercase();
            let sum_lc = app.summary.to_ascii_lowercase();
            let mut score = 0usize;
            for tok in &tokens {
                if pkg_lc == *tok || name_lc == *tok {
                    score += 10;
                } else if name_lc.contains(tok) || pkg_lc.contains(tok) {
                    score += 5;
                } else if app.keywords.iter().any(|k| k.eq_ignore_ascii_case(tok)) {
                    score += 4;
                } else if app
                    .keywords
                    .iter()
                    .any(|k| k.to_ascii_lowercase().contains(tok))
                    || sum_lc.contains(tok)
                {
                    score += 2;
                }
            }
            if score > 0 {
                app_scores.insert(idx, score);
            }
        }
        // Boost apps belonging to top-matching intents so "I want to edit videos"
        // surfaces every curated editor immediately even if a keyword wasn't repeated.
        for ri in &intents {
            for (app_idx, _) in self.apps_for_intent(ri.intent_idx) {
                *app_scores.entry(app_idx).or_insert(0) += ri.score;
            }
        }

        let mut apps: Vec<RankedApp> = app_scores
            .into_iter()
            .map(|(app_idx, score)| RankedApp { app_idx, score })
            .collect();
        apps.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| self.apps[a.app_idx].name.cmp(&self.apps[b.app_idx].name))
        });

        CatalogSearchResults { intents, apps }
    }
}

fn override_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(cfg) = std::env::var("XDG_CONFIG_HOME") {
        out.push(PathBuf::from(cfg).join("hayato/software/catalog.json"));
    } else if let Ok(home) = std::env::var("HOME") {
        out.push(PathBuf::from(home).join(".config/hayato/software/catalog.json"));
    }
    out.push(PathBuf::from("/usr/share/hayato/software/catalog.json"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_validates_cleanly() {
        let cat = Catalog::from_json(BUILTIN_CATALOG_JSON).unwrap();
        assert!(cat.intents.len() >= 12);
        assert!(cat.apps.len() >= 45);
        for intent in &cat.intents {
            assert!(!intent.apps.is_empty(), "empty intent: {}", intent.id);
        }
    }

    #[test]
    fn search_ranks_video_editor_intent_and_editors_first() {
        let cat = Catalog::load();
        let res = cat.search("video editor");
        assert!(!res.intents.is_empty());
        assert_eq!(cat.intents[res.intents[0].intent_idx].id, "video-edit");
        let top_pkgs: Vec<&str> = res
            .apps
            .iter()
            .take(5)
            .map(|r| cat.apps[r.app_idx].package.as_str())
            .collect();
        assert!(top_pkgs.contains(&"kdenlive"), "{top_pkgs:?}");
        assert!(top_pkgs.contains(&"shotcut"), "{top_pkgs:?}");
    }

    #[test]
    fn rejects_catalog_containing_protected_packages() {
        let bad = r#"{
            "schema": 1,
            "intents": [{"id":"bad","title":"Bad","blurb":"x","apps":["k"]}],
            "apps": [{"id":"k","name":"Kernel","package":"linux-image-arm64","summary":"no"}]
        }"#;
        assert!(Catalog::from_json(bad).is_err());
    }
}
