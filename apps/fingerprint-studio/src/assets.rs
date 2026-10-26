// SPDX-License-Identifier: GPL-3.0-or-later
use gpui::{AssetSource, RenderImage, SharedString};
use std::sync::Arc;

pub(super) fn tinted_mark(bytes: &[u8], color: u32) -> Option<Arc<RenderImage>> {
    let mut pixels = image::load_from_memory(bytes).ok()?.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0[..3].copy_from_slice(&[(color >> 16) as u8, (color >> 8) as u8, color as u8]);
    }
    Some(render_image(pixels))
}

pub(super) struct ToolbarAssets;
impl AssetSource for ToolbarAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "toolbar/refresh.svg" => Some(include_bytes!("../assets/toolbar/refresh.svg")),
            "toolbar/sun.svg" => Some(include_bytes!("../assets/toolbar/sun.svg")),
            "toolbar/moon.svg" => Some(include_bytes!("../assets/toolbar/moon.svg")),
            "toolbar/monitor.svg" => Some(include_bytes!("../assets/toolbar/monitor.svg")),
            "toolbar/chevron-right.svg" => {
                Some(include_bytes!("../assets/toolbar/chevron-right.svg"))
            }
            "toolbar/chevron-down.svg" => {
                Some(include_bytes!("../assets/toolbar/chevron-down.svg"))
            }
            _ => None,
        };
        Ok(bytes.map(std::borrow::Cow::Borrowed))
    }
    fn list(&self, prefix: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok([
            "toolbar/refresh.svg",
            "toolbar/sun.svg",
            "toolbar/moon.svg",
            "toolbar/monitor.svg",
            "toolbar/chevron-right.svg",
            "toolbar/chevron-down.svg",
        ]
        .into_iter()
        .filter(|path| path.starts_with(prefix))
        .map(Into::into)
        .collect())
    }
}
pub(super) fn render_image(mut buffer: image::RgbaImage) -> Arc<RenderImage> {
    // GPUI caches BGRA pixels. Grayscale captures are already channel-neutral.
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

pub(super) fn asset(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    image::load_from_memory(bytes)
        .ok()
        .map(|image| render_image(image.into_rgba8()))
}
