// SPDX-License-Identifier: GPL-3.0-or-later
//! Content-driven geometry shared by the native desktop and narrow layouts.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceLayout {
    pub stacked: bool,
    pub padding: f32,
    pub content_width: f32,
    pub preview_width: f32,
    pub preview_height: f32,
    pub controls_width: f32,
    pub gap: f32,
}
impl WorkspaceLayout {
    pub fn for_width(width: f32) -> Self {
        let stacked = width < 700.0;
        let padding = if width < 400.0 {
            16.0
        } else if stacked {
            20.0
        } else {
            24.0
        };
        let outer_width = width.min(if stacked { 420.0 } else { 848.0 });
        let content_width = (outer_width - 2.0 * padding).max(0.0);
        let gap = if stacked { 16.0 } else { 24.0 };
        let preview_width = if stacked {
            content_width
        } else {
            ((content_width - gap) * 0.48).min(360.0)
        };
        Self {
            stacked,
            padding,
            content_width,
            preview_width,
            preview_height: preview_width * 290.0 / 384.0,
            controls_width: if stacked {
                content_width
            } else {
                content_width - preview_width - gap
            },
            gap,
        }
    }
}
