use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Low,
            1 => Self::Normal,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionItem {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && px >= self.x
            && px <= self.x + self.w
            && py >= self.y
            && py <= self.y + self.h
    }
}

/// Read the session reduced-motion preference once (env var or ~/.config/hayato/reduced-motion).
pub fn reduced_motion_enabled() -> bool {
    if let Ok(val) = std::env::var("HAYATO_REDUCED_MOTION") {
        let v = val.trim();
        if !v.is_empty()
            && v != "0"
            && !v.eq_ignore_ascii_case("false")
            && !v.eq_ignore_ascii_case("no")
        {
            return true;
        }
    }
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .map(|base| base.join("hayato/reduced-motion"));
    if let Some(path) = cfg {
        if let Ok(text) = std::fs::read_to_string(path) {
            return matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "1" | "yes" | "true" | "on"
            );
        }
    }
    false
}

pub const ANIM_DURATION: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnimState {
    Opening { start: Instant },
    Active,
    Closing { start: Instant, reason: u32 },
}

/// What a keybinding asks the newest card to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    ActNewest,
    DismissNewest,
    DismissAll,
}

impl Control {
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "act-newest" => Some(Self::ActNewest),
            "dismiss-newest" => Some(Self::DismissNewest),
            "dismiss-all" => Some(Self::DismissAll),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub app_icon: String,
    pub actions: Vec<ActionItem>,
    pub urgency: Urgency,
    pub category: Option<String>,
    pub progress: Option<u8>,
    pub sync_tag: Option<String>,
    /// A click on the card opens this file's folder in Files.
    pub open_path: Option<PathBuf>,
    /// How many identical notifications this card stands for.
    pub repeat_count: u32,
    pub created_at: Instant,
    pub timeout: Duration,

    // Layout and animation state
    pub anim_state: AnimState,
    pub card_y: f32,
    pub card_height: f32,
    pub card_bounds: Rect,
    pub action_rects: Vec<(String, Rect)>,
    pub close_rect: Option<Rect>,
    pub is_hovered: bool,
    pub hovered_close: bool,
    pub hovered_action: Option<String>,
    pub hovered_at: Option<Instant>,
    #[allow(dead_code)]
    pub dismissing: bool,
    pub progress_color_override: Option<[u8; 4]>,
}

impl Notification {
    /// Update hover state and freeze/resume the expiry timer seamlessly so a
    /// card never vanishes while the user is reading it or aiming for a button.
    pub fn set_hovered(&mut self, hovered: bool) {
        if hovered == self.is_hovered {
            return;
        }
        let now = Instant::now();
        if hovered {
            self.is_hovered = true;
            if self.hovered_at.is_none() {
                self.hovered_at = Some(now);
            }
        } else {
            self.is_hovered = false;
            if let Some(since) = self.hovered_at.take() {
                let paused = now.saturating_duration_since(since);
                self.created_at += paused;
            }
        }
    }

    pub fn effective_elapsed(&self) -> Duration {
        let now = self.hovered_at.unwrap_or_else(Instant::now);
        now.saturating_duration_since(self.created_at)
    }

    /// Fraction of timeout remaining (`1.0` fresh -> `0.0` expired), or `None`
    /// for sticky/critical/OSD cards.
    pub fn remaining_fraction(&self) -> Option<f32> {
        if self.is_osd() || self.urgency == Urgency::Critical || self.timeout.is_zero() {
            return None;
        }
        let total = self.timeout.as_secs_f32();
        if total <= 0.0 {
            return None;
        }
        let elapsed = self.effective_elapsed().as_secs_f32();
        Some((1.0 - elapsed / total).clamp(0.0, 1.0))
    }

    pub fn is_expired(&self) -> bool {
        if self.urgency == Urgency::Critical || self.timeout.is_zero() || self.is_hovered {
            return false;
        }
        self.effective_elapsed() >= self.timeout
    }

    #[allow(dead_code)]
    pub fn is_animating(&self) -> bool {
        if std::env::var("HAYATO_REDUCED_MOTION").is_ok() {
            return false;
        }
        match self.anim_state {
            AnimState::Opening { start } => start.elapsed() < ANIM_DURATION,
            AnimState::Active => false,
            AnimState::Closing { start, .. } => start.elapsed() < ANIM_DURATION,
        }
    }

    pub fn is_closing(&self) -> bool {
        matches!(self.anim_state, AnimState::Closing { .. })
    }

    #[allow(dead_code)]
    pub fn is_opening(&self) -> bool {
        matches!(self.anim_state, AnimState::Opening { .. })
    }

    pub fn animation_factors(&self, reduced_motion: bool) -> (f32, f32) {
        if reduced_motion {
            return (1.0, 1.0);
        }
        match self.anim_state {
            AnimState::Opening { start } => {
                let t = (start.elapsed().as_secs_f32() / 0.150).min(1.0);
                let p = 1.0 - (1.0 - t).powi(3); // ease-out cubic
                (0.85 + 0.15 * p, p)
            }
            AnimState::Active => (1.0, 1.0),
            AnimState::Closing { start, .. } => {
                let t = (start.elapsed().as_secs_f32() / 0.150).min(1.0);
                let p = (1.0 - t).max(0.0).powi(3); // ease-in cubic
                (0.85 + 0.15 * p, p)
            }
        }
    }

    pub fn is_osd(&self) -> bool {
        self.sync_tag.as_deref() == Some("hayato-osd")
            || self.sync_tag.as_deref() == Some("volume")
            || self.sync_tag.as_deref() == Some("brightness")
            || self.sync_tag.as_deref() == Some("osd")
            || self.category.as_deref() == Some("osd")
            || self.app_name.to_lowercase() == "osd"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_notification(anim_state: AnimState) -> Notification {
        Notification {
            id: 1,
            app_name: "test".to_string(),
            summary: "Test".to_string(),
            body: "Test body".to_string(),
            app_icon: "".to_string(),
            actions: Vec::new(),
            urgency: Urgency::Normal,
            category: None,
            progress: None,
            sync_tag: None,
            open_path: None,
            repeat_count: 1,
            created_at: Instant::now(),
            timeout: Duration::from_secs(5),
            anim_state,
            card_y: 0.0,
            card_height: 70.0,
            card_bounds: Rect::new(0.0, 0.0, 300.0, 70.0),
            action_rects: Vec::new(),
            close_rect: None,
            is_hovered: false,
            hovered_close: false,
            hovered_action: None,
            hovered_at: None,
            dismissing: false,
            progress_color_override: None,
        }
    }

    #[test]
    fn zero_size_rect_never_matches_origin() {
        let empty = Rect::new(0.0, 0.0, 0.0, 0.0);
        assert!(!empty.contains(0.0, 0.0));
        let real = Rect::new(10.0, 10.0, 20.0, 20.0);
        assert!(real.contains(15.0, 15.0));
    }

    #[test]
    fn hover_pauses_expiry_until_leave() {
        let mut n = dummy_notification(AnimState::Active);
        n.timeout = Duration::from_millis(50);
        n.created_at = Instant::now() - Duration::from_millis(60);
        assert!(n.is_expired());
        n.set_hovered(true);
        assert!(!n.is_expired());
        n.set_hovered(false);
        assert!(n.is_expired());
    }

    #[test]
    fn test_animation_active() {
        let n = dummy_notification(AnimState::Active);
        assert!(!n.is_closing());
        assert!(!n.is_opening());
        assert!(!n.is_animating());
        let (scale, alpha) = n.animation_factors(false);
        assert_eq!(scale, 1.0);
        assert_eq!(alpha, 1.0);
    }

    #[test]
    fn test_animation_opening_factors() {
        // Just started opening
        let n = dummy_notification(AnimState::Opening {
            start: Instant::now(),
        });
        assert!(n.is_opening());
        assert!(!n.is_closing());
        assert!(n.is_animating());
        let (scale, alpha) = n.animation_factors(false);
        assert!((0.85..=0.87).contains(&scale));
        assert!((0.0..=0.05).contains(&alpha));

        // Finished opening
        let past = Instant::now() - Duration::from_millis(200);
        let n_done = dummy_notification(AnimState::Opening { start: past });
        assert!(!n_done.is_animating());
        let (scale, alpha) = n_done.animation_factors(false);
        assert_eq!(scale, 1.0);
        assert_eq!(alpha, 1.0);
    }

    #[test]
    fn test_animation_closing_factors() {
        // Just started closing
        let n = dummy_notification(AnimState::Closing {
            start: Instant::now(),
            reason: 1,
        });
        assert!(n.is_closing());
        assert!(!n.is_opening());
        assert!(n.is_animating());
        let (scale, alpha) = n.animation_factors(false);
        assert!((0.98..=1.0).contains(&scale));
        assert!((0.95..=1.0).contains(&alpha));

        // Finished closing
        let past = Instant::now() - Duration::from_millis(200);
        let n_done = dummy_notification(AnimState::Closing {
            start: past,
            reason: 1,
        });
        assert!(!n_done.is_animating());
        let (scale, alpha) = n_done.animation_factors(false);
        assert_eq!(scale, 0.85);
        assert_eq!(alpha, 0.0);
    }

    #[test]
    fn control_words_are_the_socket_protocol() {
        assert_eq!(Control::parse("act-newest"), Some(Control::ActNewest));
        assert_eq!(
            Control::parse(" dismiss-newest\n"),
            Some(Control::DismissNewest)
        );
        assert_eq!(Control::parse("dismiss-all"), Some(Control::DismissAll));
        assert_eq!(Control::parse("open"), None);
    }
}
