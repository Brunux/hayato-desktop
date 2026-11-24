//! Album art loader using `hayato_ui::pictures` (Sixel in `foot`, half-blocks
//! fallback everywhere else). Decodes at most one cover image per track change
//! and caches its `StatefulProtocol` for zero-cost redraws.

use std::path::{Path, PathBuf};

use hayato_ui::pictures::Pictures;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

pub struct AlbumArt {
    pictures: Pictures,
    loaded_path: Option<PathBuf>,
    protocol: Option<Box<StatefulProtocol>>,
}

impl AlbumArt {
    pub fn from_env() -> Self {
        let env = std::env::var("HAYATO_PICTURES").ok();
        Self {
            pictures: Pictures::from_env(env.as_deref()),
            loaded_path: None,
            protocol: None,
        }
    }

    #[allow(dead_code)]
    pub fn off() -> Self {
        Self {
            pictures: Pictures::Off,
            loaded_path: None,
            protocol: None,
        }
    }

    /// Ensure the cover at `path` is decoded (no-op if already loaded).
    pub fn set_path(&mut self, path: Option<&Path>) {
        let wanted = path.map(Path::to_path_buf);
        if wanted == self.loaded_path {
            return;
        }
        self.loaded_path = wanted.clone();
        self.protocol = None;

        let Some(p) = wanted else {
            return;
        };
        let Pictures::Ready(picker) = &self.pictures else {
            return;
        };
        if let Ok(img) = image::open(&p) {
            let thumb = img.thumbnail(320, 320);
            self.protocol = Some(Box::new(picker.new_resize_protocol(thumb)));
        }
    }

    pub fn has_image(&self) -> bool {
        self.protocol.is_some()
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        if let Some(proto) = self.protocol.as_mut() {
            StatefulImage::<StatefulProtocol>::default()
                .resize(Resize::Fit(None))
                .render(area, buf, proto.as_mut());
        }
    }
}
