use crate::model::{Media, MediaKind};
use anyhow::{Context, Result};
use image::{DynamicImage, ImageBuffer, Rgb};
use ratatui::layout::Rect;
use ratatui_image::{
    FontSize, Resize, StatefulImage,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};
use std::{collections::HashMap, env, io::Cursor, time::Duration};

const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);
pub const INLINE_MAX_COLS: u16 = 56;
pub const INLINE_MAX_ROWS: u16 = 16;
pub const PREVIEW_MAX_ROWS: u16 = 10;
pub const MODAL_MAX_COLS: u16 = 72;
pub const MODAL_MAX_ROWS: u16 = 28;

/// Decoded still used for native terminal graphics (Sixel / Kitty / iTerm2).
#[derive(Clone, Debug)]
pub struct PreviewImage {
    pub source: DynamicImage,
}

impl PreviewImage {
    pub fn width(&self) -> u32 {
        self.source.width()
    }

    pub fn height(&self) -> u32 {
        self.source.height()
    }
}

pub fn best_external_url(media: &Media) -> Option<String> {
    if matches!(media.kind, MediaKind::Video | MediaKind::AnimatedGif) {
        media
            .variants
            .iter()
            .filter(|v| v.content_type.as_deref() == Some("video/mp4"))
            .max_by_key(|v| v.bit_rate.unwrap_or(0))
            .map(|v| v.url.clone())
            .or_else(|| media.url.clone())
            .or_else(|| media.preview_url.clone())
    } else {
        media.url.clone().or_else(|| media.preview_url.clone())
    }
}

/// Still image used inside the TUI. Videos and GIFs prefer their poster
/// frame so we never try to decode an MP4 as a photo.
pub fn best_preview_url(media: &Media) -> Option<String> {
    if let Some(url) = media.preview_url.clone() {
        return Some(url);
    }
    let url = media.url.clone()?;
    if url.starts_with("demo://") || looks_like_image(&url) {
        Some(url)
    } else {
        None
    }
}

fn looks_like_image(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

pub async fn download_preview(url: &str) -> Result<PreviewImage> {
    if url.starts_with("demo://") {
        return Ok(demo_preview());
    }
    let client = reqwest::Client::builder()
        .user_agent("XTUI/0.3")
        .timeout(PREVIEW_TIMEOUT)
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response.content_length().unwrap_or(0) > MAX_PREVIEW_BYTES as u64 {
        anyhow::bail!("media preview exceeds the 8 MiB safety limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_PREVIEW_BYTES {
            anyhow::bail!("media preview exceeds the 8 MiB safety limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || decode_image(&bytes)).await?
}

fn decode_image(bytes: &[u8]) -> Result<PreviewImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("media format could not be detected")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(48 * 1024 * 1024);
    reader.limits(limits);
    let source = reader
        .decode()
        .context("media is not a supported or safely sized image")?;
    Ok(PreviewImage { source })
}

fn demo_preview() -> PreviewImage {
    let width = 320u32;
    let height = 180u32;
    let buffer = ImageBuffer::from_fn(width, height, |x, y| {
        let edge = x < 8 || y < 8 || x + 8 >= width || y + 8 >= height;
        if edge {
            Rgb([210, 210, 210])
        } else {
            let t = x as f32 / width as f32;
            let luma = 28 + (t * 90.0) as u8;
            Rgb([luma, luma, luma.saturating_add(8)])
        }
    });
    PreviewImage {
        source: DynamicImage::ImageRgb8(buffer),
    }
}

/// Detects Sixel / Kitty / iTerm2 after the alternate screen is up, and
/// keeps per-URL protocol state so images can be painted as real pixels.
pub struct ImageEngine {
    picker: Picker,
    states: HashMap<String, StatefulProtocol>,
}

impl ImageEngine {
    pub fn detect() -> Self {
        let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        if picker.protocol_type() == ProtocolType::Halfblocks {
            if let Some(forced) = forced_graphics_protocol() {
                let font = window_font_size()
                    .unwrap_or_else(|| picker.font_size())
                    .max_or_default();
                #[allow(deprecated)]
                let mut next = Picker::from_fontsize(font);
                next.set_protocol_type(forced);
                picker = next;
            }
        }
        Self {
            picker,
            states: HashMap::new(),
        }
    }

    pub fn font_size(&self) -> FontSize {
        self.picker.font_size()
    }

    pub fn drop_url(&mut self, url: &str) {
        self.states.remove(url);
    }

    pub fn slot_size(&self, image: &PreviewImage, max_cols: u16, max_rows: u16) -> (u16, u16) {
        fit_cells(image, self.font_size(), max_cols, max_rows)
    }

    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        url: &str,
        image: &PreviewImage,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if !self.states.contains_key(url) {
            self.states.insert(
                url.to_string(),
                self.picker.new_resize_protocol(image.source.clone()),
            );
        }
        let Some(state) = self.states.get_mut(url) else {
            return;
        };
        frame.render_stateful_widget(StatefulImage::new().resize(Resize::Fit(None)), area, state);
    }
}

trait FontSizeExt {
    fn max_or_default(self) -> FontSize;
}

impl FontSizeExt for FontSize {
    fn max_or_default(self) -> FontSize {
        if self.width == 0 || self.height == 0 {
            FontSize::new(10, 20)
        } else {
            self
        }
    }
}

pub fn default_font() -> FontSize {
    window_font_size().unwrap_or(FontSize::new(10, 20))
}

pub fn fit_cells(image: &PreviewImage, font: FontSize, max_cols: u16, max_rows: u16) -> (u16, u16) {
    let font = font.max_or_default();
    let max_cols = max_cols.max(1);
    let max_rows = max_rows.max(1);
    let native_cols = (image.width() as f32 / f32::from(font.width))
        .ceil()
        .max(1.0);
    let native_rows = (image.height() as f32 / f32::from(font.height))
        .ceil()
        .max(1.0);
    let scale = (f32::from(max_cols) / native_cols)
        .min(f32::from(max_rows) / native_rows)
        .min(1.0);
    let cols = (native_cols * scale)
        .round()
        .clamp(1.0, f32::from(max_cols)) as u16;
    let rows = (native_rows * scale)
        .round()
        .clamp(1.0, f32::from(max_rows)) as u16;
    (cols, rows)
}

fn forced_graphics_protocol() -> Option<ProtocolType> {
    if env::var_os("WT_SESSION").is_some()
        || env::var("TERM_PROGRAM").ok().as_deref() == Some("vscode")
    {
        return Some(ProtocolType::Sixel);
    }
    match env::var("TERM_PROGRAM").ok().as_deref() {
        Some("WezTerm") | Some("iTerm.app") | Some("Bobcat") => {
            return Some(ProtocolType::Iterm2);
        }
        Some("ghostty") => return Some(ProtocolType::Kitty),
        _ => {}
    }
    if env::var_os("KITTY_WINDOW_ID").is_some()
        || env::var("TERM")
            .ok()
            .is_some_and(|term| term.contains("kitty"))
    {
        return Some(ProtocolType::Kitty);
    }
    if env::var("TERM").ok().is_some_and(|term| {
        term.contains("sixel") || term.contains("mlterm") || term.contains("foot")
    }) {
        return Some(ProtocolType::Sixel);
    }
    #[cfg(windows)]
    {
        return Some(ProtocolType::Sixel);
    }
    #[cfg(not(windows))]
    None
}

fn window_font_size() -> Option<FontSize> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.width == 0 || size.height == 0 || size.columns == 0 || size.rows == 0 {
        return None;
    }
    Some(FontSize::new(
        (size.width / size.columns).max(1),
        (size.height / size.rows).max(1),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_prefers_highest_bitrate_mp4() {
        let m = Media {
            key: "x".into(),
            kind: MediaKind::Video,
            url: None,
            preview_url: Some("preview".into()),
            alt_text: None,
            variants: vec![
                crate::model::MediaVariant {
                    url: "low".into(),
                    bit_rate: Some(64),
                    content_type: Some("video/mp4".into()),
                },
                crate::model::MediaVariant {
                    url: "high".into(),
                    bit_rate: Some(128),
                    content_type: Some("video/mp4".into()),
                },
            ],
        };
        assert_eq!(best_external_url(&m).as_deref(), Some("high"));
    }

    #[tokio::test]
    async fn demo_preview_is_offline_and_sized() {
        let image = download_preview("demo://terminal").await.unwrap();
        assert!(image.width() >= 160);
        assert!(image.height() >= 80);
    }

    #[test]
    fn preview_url_prefers_stills_over_video_files() {
        let video = Media {
            key: "v".into(),
            kind: MediaKind::Video,
            url: Some("https://example.test/clip.mp4".into()),
            preview_url: Some("https://example.test/poster.jpg".into()),
            alt_text: None,
            variants: vec![],
        };
        assert_eq!(
            best_preview_url(&video).as_deref(),
            Some("https://example.test/poster.jpg")
        );
        let bare = Media {
            preview_url: None,
            ..video.clone()
        };
        assert_eq!(best_preview_url(&bare), None);
    }

    #[test]
    fn fit_cells_caps_the_slot() {
        let image = demo_preview();
        let (cols, rows) = fit_cells(&image, FontSize::new(10, 20), 20, 8);
        assert!(cols <= 20);
        assert!(rows <= 8);
        assert!(cols >= 1 && rows >= 1);
    }
}
