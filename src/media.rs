use crate::model::{Media, MediaKind};
use anyhow::{Context, Result};
use image::GenericImageView;
use std::{io::Cursor, time::Duration};

const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);

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

pub async fn download_preview(url: &str, max_width: u32, max_height: u32) -> Result<Vec<String>> {
    if url.starts_with("demo://") {
        return Ok(demo_preview(max_width.min(60) as usize));
    }
    // A hung CDN must never freeze the UI: the event loop awaits this call.
    let client = reqwest::Client::builder()
        .user_agent("XTUI/0.1")
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
    tokio::task::spawn_blocking(move || render_halfblocks(&bytes, max_width, max_height)).await?
}

fn demo_preview(width: usize) -> Vec<String> {
    let width = width.max(24);
    let mut lines = vec![" ".repeat(width); 16];
    lines[1] = format!("┌{}┐", "─".repeat(width.saturating_sub(2)));
    lines[14] = format!("└{}┘", "─".repeat(width.saturating_sub(2)));
    for line in lines.iter_mut().take(14).skip(2) {
        *line = format!("│{}│", " ".repeat(width.saturating_sub(2)));
    }
    for (row, text) in [
        (3, "XTUI  /  HOME"),
        (7, "@ada_codes  ·  now"),
        (9, "A quieter way to read the timeline."),
        (11, "◯ 38    ↻ 226    ♡ 2.4K"),
    ] {
        let clipped: String = text.chars().take(width.saturating_sub(8)).collect();
        let remaining = width.saturating_sub(5 + clipped.chars().count());
        lines[row] = format!("│   {clipped}{}│", " ".repeat(remaining));
    }
    lines
}

fn render_halfblocks(bytes: &[u8], max_width: u32, max_height: u32) -> Result<Vec<String>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("media format could not be detected")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .context("media is not a supported or safely sized image")?
        .grayscale();
    let (w, h) = image.dimensions();
    let scale = (max_width as f32 / w as f32)
        .min((max_height * 2) as f32 / h as f32)
        .min(1.0);
    let target_w = ((w as f32 * scale).round() as u32).max(1);
    let target_h = ((h as f32 * scale).round() as u32).max(2);
    let resized = image
        .resize_exact(target_w, target_h, image::imageops::FilterType::Triangle)
        .to_luma8();
    let levels = [' ', '░', '▒', '▓', '█'];
    let mut lines = Vec::new();
    for y in (0..target_h).step_by(2) {
        let mut line = String::new();
        for x in 0..target_w {
            let top = resized.get_pixel(x, y)[0] as usize;
            let bottom = resized.get_pixel(x, (y + 1).min(target_h - 1))[0] as usize;
            line.push(levels[((top + bottom) / 2) * levels.len() / 256]);
        }
        lines.push(line);
    }
    Ok(lines)
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
    async fn demo_preview_is_offline_and_bordered() {
        let lines = download_preview("demo://terminal", 40, 20).await.unwrap();
        assert!(lines.iter().any(|line| line.contains("XTUI")));
        assert!(lines.first().unwrap().trim().is_empty());
        assert!(lines[1].starts_with('┌'));
    }
}
