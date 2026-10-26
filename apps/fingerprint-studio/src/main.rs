// SPDX-License-Identifier: GPL-3.0-or-later
use fingerprint_studio::{
    Config,
    layout::WorkspaceLayout,
    theme::{Palette, PreferenceStore, ThemeMode},
    workflow::{Command, Event, Mode, Worker},
};
use gpui::{prelude::*, *};
use std::{sync::Arc, time::Duration};

actions!(
    studio,
    [
        Cancel,
        CloseWindow,
        Quit,
        CompactWindow,
        StandardWindow,
        ToggleFullScreen,
        ThemeLight,
        ThemeDark,
        ThemeSystem
    ]
);

#[derive(Clone, Copy)]
enum Intent {
    Select(Mode),
    Primary,
    Refresh,
    Reset,
    Details,
    CycleTheme,
}

#[derive(Clone, Copy)]
enum ToolbarAction {
    Refresh,
    Appearance,
}

struct Studio {
    worker: Worker,
    focus: Vec<FocusHandle>,
    mode: Mode,
    active_mode: Option<Mode>,
    busy: bool,
    stopping: bool,
    failed: bool,
    devices: usize,
    samples: usize,
    enrolled: bool,
    recognition: bool,
    scale: String,
    status: String,
    details: String,
    preview: Option<Arc<RenderImage>>,
    last_verification: Option<(uareu::recognition::MatchResult, usize)>,
    icon: Option<Arc<RenderImage>>,
    mark: Option<Arc<RenderImage>>,
    narrow: bool,
    show_details: bool,
    experimental: bool,
    colors: Palette,
    theme_mode: ThemeMode,
    mark_color: u32,
}

struct AppearanceSettings {
    mode: ThemeMode,
    store: Option<PreferenceStore>,
    save_error: Option<String>,
}
impl Global for AppearanceSettings {}
impl AppearanceSettings {
    fn load() -> Self {
        let store = PreferenceStore::for_current_user();
        let mode = store
            .as_ref()
            .map_or(ThemeMode::System, PreferenceStore::load);
        Self {
            mode,
            store,
            save_error: None,
        }
    }
}
fn choose_theme(mode: ThemeMode, cx: &mut App) {
    cx.update_global::<AppearanceSettings, _>(|settings, _| {
        settings.mode = mode;
        settings.save_error = settings
            .store
            .as_ref()
            .and_then(|store| store.save(mode).err())
            .map(|error| {
                format!(
                    "Appearance applies for this session; preference could not be saved: {error}"
                )
            });
    });
    cx.refresh_windows();
}
fn tinted_mark(bytes: &[u8], color: u32) -> Option<Arc<RenderImage>> {
    let mut pixels = image::load_from_memory(bytes).ok()?.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0[..3].copy_from_slice(&[(color >> 16) as u8, (color >> 8) as u8, color as u8]);
    }
    Some(render_image(pixels))
}

struct ToolbarAssets;
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
struct ToolbarTooltip {
    text: SharedString,
    colors: Palette,
}
impl Render for ToolbarTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.surface))
            .text_color(rgb(self.colors.text))
            .text_xs()
            .child(self.text.clone())
    }
}

fn render_image(mut buffer: image::RgbaImage) -> Arc<RenderImage> {
    // GPUI caches BGRA pixels. Grayscale captures are already channel-neutral.
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

fn asset(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    image::load_from_memory(bytes)
        .ok()
        .map(|image| render_image(image.into_rgba8()))
}

impl Studio {
    fn caption(&self, text: impl Into<SharedString>) -> Div {
        div()
            .text_xs()
            .text_color(rgb(self.colors.muted))
            .child(text.into())
    }

    fn new(config: Config, worker: Worker, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |view, cx| {
            loop {
                Timer::after(Duration::from_millis(50)).await;
                if view.update(cx, |view, cx| view.receive(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut view = Self {
            worker,
            focus: (0..8).map(|_| cx.focus_handle()).collect(),
            mode: if config.recognition_enabled() {
                Mode::Enroll
            } else {
                Mode::Capture
            },
            active_mode: None,
            busy: false,
            stopping: false,
            failed: false,
            devices: 0,
            samples: 0,
            enrolled: false,
            recognition: config.recognition_enabled(),
            scale: config.scale_label(),
            narrow: false,
            show_details: false,
            experimental: config.experimental_ppi,
            colors: Palette::light(),
            theme_mode: ThemeMode::System,
            mark_color: 0,
            status: "Looking for a fingerprint reader…".into(),
            details: "Captured images appear here".into(),
            preview: None,
            last_verification: None,
            icon: asset(include_bytes!("../assets/fingerprint-studio.png.bin")),
            mark: asset(include_bytes!("../assets/fingerprint-mark.png.bin")),
        };
        view.dispatch(Command::Refresh, None, cx);
        view
    }

    fn receive(&mut self, cx: &mut Context<Self>) {
        while let Ok(event) = self.worker.try_recv() {
            match event {
                Event::Status(message) => self.status = message,
                Event::Preview { capture, details } => {
                    let image = capture.image;
                    let pixels = image
                        .pixels
                        .iter()
                        .flat_map(|pixel| [*pixel, *pixel, *pixel, 255])
                        .collect();
                    if let Some(buffer) =
                        image::RgbaImage::from_raw(image.width as u32, image.height as u32, pixels)
                    {
                        self.preview = Some(render_image(buffer));
                        self.details = details;
                    }
                }
                Event::Verification { result, attempt } => {
                    self.last_verification = Some((result, attempt));
                }
                Event::Progress { samples, enrolled } => {
                    self.samples = samples;
                    self.enrolled = enrolled;
                }
                Event::Complete {
                    message,
                    failed,
                    devices,
                    samples,
                    enrolled,
                } => {
                    self.status = message;
                    self.failed = failed;
                    self.devices = devices;
                    self.samples = samples;
                    self.enrolled = enrolled;
                    self.busy = false;
                    self.stopping = false;
                    self.active_mode = None;
                }
            }
            cx.notify();
        }
    }

    fn dispatch(&mut self, command: Command, mode: Option<Mode>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.worker.send(command) {
            Ok(()) => {
                self.busy = true;
                self.failed = false;
                self.active_mode = mode;
                self.status = if mode.is_some() {
                    "Preparing the reader…".into()
                } else {
                    "Updating session…".into()
                };
            }
            Err(error) => {
                self.failed = true;
                self.status = format!("Cannot start the operation: {error:#}");
            }
        }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.worker.cancel();
            self.stopping = true;
            self.status =
                "Stopping safely… Any comparison already underway will finish first.".into();
            cx.notify();
        }
    }

    fn act(&mut self, intent: Intent, cx: &mut Context<Self>) {
        match intent {
            Intent::Details => self.show_details = !self.show_details,
            Intent::CycleTheme => {
                let next = cx.global::<AppearanceSettings>().mode.next();
                choose_theme(next, cx);
            }
            Intent::Select(mode) => {
                if mode != self.mode {
                    if self.busy {
                        self.stop(cx);
                    }
                    self.mode = mode;
                    // Switching intention never starts a second reader operation.
                    if !self.busy {
                        self.failed = false;
                        self.status = "Ready.".into();
                    }
                }
            }
            Intent::Primary if self.busy => self.stop(cx),
            Intent::Primary if self.mode == Mode::Enroll && self.enrolled => {
                self.act(Intent::Select(Mode::Verify), cx);
            }
            Intent::Primary if self.blocker().is_none() => {
                if self.mode == Mode::Verify {
                    self.last_verification = None;
                }
                self.preview = None;
                self.details = "Waiting for the next impression".into();
                self.dispatch(Command::Start(self.mode), Some(self.mode), cx);
            }
            Intent::Primary => {}
            Intent::Refresh if !self.busy => self.dispatch(Command::Refresh, None, cx),
            Intent::Reset if !self.busy => {
                self.last_verification = None;
                self.preview = None;
                self.details = "Captured images appear here".into();
                self.dispatch(Command::Reset, None, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn blocker(&self) -> Option<&'static str> {
        if self.mode == Mode::Enroll && self.enrolled {
            return None;
        }
        if self.devices == 0 {
            return Some("Connect your reader, then choose Refresh.");
        }
        if self.mode != Mode::Capture && !self.recognition {
            return Some(
                "Matching is not configured. Open the SourceAFIS app, or use Capture mode.",
            );
        }
        if self.mode == Mode::Verify && !self.enrolled {
            return Some("Enroll a finger first. Switch to Enroll to collect three impressions.");
        }
        None
    }

    fn primary_label(&self) -> &'static str {
        if self.stopping {
            return "Stopping…";
        }
        if self.busy {
            return match self.active_mode {
                Some(Mode::Enroll) => "Stop enrollment",
                Some(Mode::Verify) => "Stop verification",
                Some(Mode::Capture) => "Cancel capture",
                None => "Updating…",
            };
        }
        match self.mode {
            Mode::Capture => "Capture fingerprint",
            Mode::Enroll if self.enrolled => "Switch to verification",
            Mode::Enroll if self.samples > 0 => "Resume enrollment",
            Mode::Enroll => "Start enrollment",
            Mode::Verify => "Start verification",
        }
    }

    fn button(
        &self,
        label: &'static str,
        enabled: bool,
        primary: bool,
        intent: Intent,
        focus_index: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = self.colors;
        div()
            .id(label)
            .flex_none()
            .track_focus(&self.focus[focus_index])
            .border_1()
            .rounded_md()
            .px_4()
            .py(px(if self.narrow { 10. } else { 6. }))
            .min_h(px(if self.narrow { 44. } else { 32. }))
            .flex()
            .items_center()
            .justify_center()
            .when(primary, |style| style.w_full())
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .border_color(rgb(if primary {
                colors.accent
            } else {
                colors.border
            }))
            .bg(rgb(if primary {
                colors.accent
            } else {
                colors.background
            }))
            .text_color(rgb(if primary {
                colors.on_accent
            } else {
                colors.text
            }))
            .when(
                matches!(intent, Intent::Details | Intent::Refresh),
                |style| {
                    style
                        .border_color(rgb(colors.background))
                        .text_color(rgb(colors.muted))
                },
            )
            .focus(move |style| style.border_color(rgb(colors.accent)))
            .when(!enabled, |style| style.opacity(0.45))
            .when(enabled, |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| {
                        style.bg(rgb(if primary {
                            colors.accent_hover
                        } else {
                            colors.hover
                        }))
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        window.focus(&view.focus[focus_index]);
                        view.act(intent, cx);
                    }))
                    .on_key_down(cx.listener(move |view, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            view.act(intent, cx);
                        }
                    }))
            })
            .child(label)
    }

    fn icon_button(&self, action: ToolbarAction, cx: &Context<Self>) -> impl IntoElement {
        let colors = self.colors;
        let (id, path, tooltip, enabled, intent, focus_index): (
            &'static str,
            SharedString,
            SharedString,
            bool,
            Intent,
            usize,
        ) = match action {
            ToolbarAction::Refresh => (
                "refresh-readers",
                "toolbar/refresh.svg".into(),
                if self.busy {
                    "Refresh unavailable while scanning"
                } else {
                    "Refresh readers"
                }
                .into(),
                !self.busy,
                Intent::Refresh,
                4,
            ),
            ToolbarAction::Appearance => (
                "cycle-appearance",
                format!("toolbar/{}.svg", self.theme_mode.icon_name()).into(),
                format!(
                    "Appearance: {}. Switch to {}.",
                    self.theme_mode.label(),
                    self.theme_mode.next().label()
                )
                .into(),
                true,
                Intent::CycleTheme,
                7,
            ),
        };
        div()
            .id(id)
            .track_focus(&self.focus[focus_index])
            .size(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.background))
            .text_color(rgb(colors.muted))
            .focus(move |style| style.border_color(rgb(colors.accent)))
            .tooltip(move |_, cx| {
                cx.new(|_| ToolbarTooltip {
                    text: tooltip.clone(),
                    colors,
                })
                .into()
            })
            .when(!enabled, |style| style.opacity(0.45))
            .when(enabled, |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(colors.hover)).text_color(rgb(colors.text)))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        window.focus(&view.focus[focus_index]);
                        view.act(intent, cx);
                    }))
                    .on_key_down(cx.listener(move |view, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            view.act(intent, cx);
                        }
                    }))
            })
            .child(svg().path(path).size(px(16.)).text_color(rgb(colors.muted)))
    }

    fn mode_tab(&self, mode: Mode, focus_index: usize, cx: &Context<Self>) -> impl IntoElement {
        let colors = self.colors;
        let selected = self.mode == mode;
        div()
            .id(mode_name(mode))
            .track_focus(&self.focus[focus_index])
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .h(px(if self.narrow { 44. } else { 32. }))
            .rounded_md()
            .border_1()
            .border_color(rgb(if selected {
                colors.border
            } else {
                colors.surface
            }))
            .bg(rgb(if selected {
                colors.selected
            } else {
                colors.surface
            }))
            .text_color(rgb(if selected {
                colors.accent
            } else {
                colors.muted
            }))
            .text_sm()
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .cursor_pointer()
            .focus(move |style| style.border_color(rgb(colors.accent)))
            .hover(move |style| style.text_color(rgb(colors.accent)))
            .on_click(cx.listener(move |view, _, window, cx| {
                window.focus(&view.focus[focus_index]);
                view.act(Intent::Select(mode), cx);
            }))
            .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                let target = match event.keystroke.key.as_str() {
                    "enter" | "space" => Some(mode),
                    "left" => Some(match mode {
                        Mode::Capture => Mode::Verify,
                        Mode::Enroll => Mode::Capture,
                        Mode::Verify => Mode::Enroll,
                    }),
                    "right" => Some(match mode {
                        Mode::Capture => Mode::Enroll,
                        Mode::Enroll => Mode::Verify,
                        Mode::Verify => Mode::Capture,
                    }),
                    _ => None,
                };
                if let Some(target) = target {
                    let index = match target {
                        Mode::Capture => 0,
                        Mode::Enroll => 1,
                        Mode::Verify => 2,
                    };
                    window.focus(&view.focus[index]);
                    cx.stop_propagation();
                    view.act(Intent::Select(target), cx);
                }
            }))
            .child(mode_name(mode))
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Capture => "Capture",
        Mode::Enroll => "Enroll",
        Mode::Verify => "Verify",
    }
}

impl Studio {
    fn primary_enabled(&self) -> bool {
        !self.stopping
            && if self.busy {
                self.active_mode.is_some()
            } else {
                self.blocker().is_none()
            }
    }

    fn action_group(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex_none()
            .flex()
            .gap_2()
            .when(!self.narrow, |style| style.flex_col())
            .child(
                div()
                    .min_w_0()
                    .when(self.narrow, |style| style.flex_1())
                    .child(self.button(
                        self.primary_label(),
                        self.primary_enabled(),
                        true,
                        Intent::Primary,
                        3,
                        cx,
                    )),
            )
            .when(
                !self.busy && (self.samples > 0 || self.preview.is_some()),
                |style| {
                    style.child(self.button(
                        if self.narrow {
                            "Clear"
                        } else {
                            "Clear session"
                        },
                        true,
                        false,
                        Intent::Reset,
                        5,
                        cx,
                    ))
                },
            )
    }

    fn mode_panel(&self, width: f32, cx: &Context<Self>) -> impl IntoElement {
        let heading = match self.mode {
            Mode::Capture => "Capture a fingerprint",
            Mode::Enroll if self.enrolled => "Enrollment ready",
            Mode::Enroll => "Enroll one finger",
            Mode::Verify => "Continuous verification",
        };
        let instruction = match self.mode {
            Mode::Capture => "Place a finger on the reader to capture an image.",
            Mode::Enroll if self.enrolled => "Switch to Verify to test new finger placements.",
            Mode::Enroll => {
                "Place and lift the same finger three times. Impressions are added automatically."
            }
            Mode::Verify => "Place a finger to match it. Lift to scan again. Stop when finished.",
        };
        let status_visible = self.busy || self.failed || self.preview.is_some() || self.enrolled;
        div()
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(heading),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(self.colors.muted))
                    .child(instruction),
            )
            .when(status_visible, |style| {
                style.child(
                    div()
                        .text_sm()
                        .text_color(rgb(if self.failed {
                            self.colors.danger
                        } else {
                            self.colors.text
                        }))
                        .child(self.status.clone()),
                )
            })
            .when(self.mode == Mode::Enroll, |style| {
                style.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().flex().gap_2().children((0..3).map(|index| {
                            div().flex_1().h(px(4.)).rounded_full().bg(rgb(
                                if index < self.samples {
                                    self.colors.accent
                                } else {
                                    self.colors.border
                                },
                            ))
                        })))
                        .child(self.caption(format!("{} of 3 impressions accepted", self.samples))),
                )
            })
            .when(self.mode == Mode::Verify, |style| {
                style.child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(rgb(self.colors.surface))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(self.caption("Last comparison"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .when_some(self.last_verification.as_ref(), |style, (result, _)| {
                                    style.child(div().size(px(7.)).rounded_full().bg(rgb(
                                        if result.matched {
                                            self.colors.success
                                        } else {
                                            self.colors.danger
                                        },
                                    )))
                                })
                                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(
                                    match &self.last_verification {
                                        Some((result, _)) if result.matched => {
                                            "Fingerprint matched"
                                        }
                                        Some(_) => "Fingerprint did not match",
                                        None => "Waiting for the first scan",
                                    },
                                )),
                        )
                        .when_some(self.last_verification.as_ref(), |style, (_, attempt)| {
                            style.child(self.caption(format!("Scan {attempt}")))
                        }),
                )
            })
            .when_some(
                if self.busy { None } else { self.blocker() },
                |style, reason| {
                    style.child(
                        div()
                            .text_sm()
                            .text_color(rgb(self.colors.muted))
                            .child(reason),
                    )
                },
            )
            .when(!self.narrow, |style| style.child(self.action_group(cx)))
            .child(self.details_section(cx))
            .child(self.caption("Local session · cleared when the app closes"))
    }

    fn detail_row(&self, label: &'static str, value: impl Into<SharedString>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .items_start()
            .gap_3()
            .py_1()
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(self.colors.muted))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(rgb(self.colors.text))
                    .child(value.into()),
            )
    }

    fn details_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = self.colors;
        div()
            .w_full()
            .border_t_1()
            .border_color(rgb(colors.border))
            .pt_2()
            .child(
                div().flex().child(
                    div()
                        .id("details-disclosure")
                        .track_focus(&self.focus[6])
                        .h(px(28.))
                        .px_1()
                        .border_1()
                        .rounded_sm()
                        .border_color(rgb(colors.background))
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .focus(move |style| style.border_color(rgb(colors.accent)))
                        .hover(move |style| style.bg(rgb(colors.hover)))
                        .child(
                            svg()
                                .path(if self.show_details {
                                    "toolbar/chevron-down.svg"
                                } else {
                                    "toolbar/chevron-right.svg"
                                })
                                .size(px(12.))
                                .text_color(rgb(colors.muted)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(colors.muted))
                                .child("Details"),
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            window.focus(&view.focus[6]);
                            view.act(Intent::Details, cx);
                        }))
                        .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                view.act(Intent::Details, cx);
                            }
                        })),
                ),
            )
            .when(self.show_details, |style| {
                style.child(
                    div()
                        .id("diagnostics")
                        .w_full()
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .pl_5()
                        .pt_1()
                        .pb_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(self.detail_row("Scale", self.scale.clone()))
                        .child(self.detail_row(
                            "Matcher",
                            if self.recognition {
                                "SourceAFIS · local"
                            } else {
                                "Not configured"
                            },
                        ))
                        .when(self.preview.is_some(), |style| {
                            style.child(self.detail_row("Image", self.details.clone()))
                        })
                        .when_some(self.last_verification.as_ref(), |style, (result, _)| {
                            style.child(self.detail_row(
                                "Score",
                                format!("{:.1} / {:.1} threshold", result.score, result.threshold),
                            ))
                        })
                        .when_some(
                            cx.global::<AppearanceSettings>().save_error.clone(),
                            |style, error| style.child(self.detail_row("Appearance", error)),
                        ),
                )
            })
    }

    fn preview_panel(&self, layout: WorkspaceLayout) -> impl IntoElement {
        div()
            .w(px(layout.preview_width))
            .h(px(layout.preview_height))
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.surface))
            .p_3()
            .flex()
            .items_center()
            .justify_center()
            .when_some(self.preview.clone(), |style, preview| {
                style.child(img(preview).size_full().object_fit(ObjectFit::Contain))
            })
            .when(self.preview.is_none(), |style| {
                style.when_some(self.mark.clone(), |style, mark| {
                    style.child(img(mark).size(px(64.)))
                })
            })
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme_mode = cx.global::<AppearanceSettings>().mode;
        let system_dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        self.colors = self.theme_mode.palette(system_dark);
        if self.mark_color != self.colors.accent {
            self.mark = tinted_mark(
                include_bytes!("../assets/fingerprint-mark.png.bin"),
                self.colors.accent,
            );
            self.mark_color = self.colors.accent;
        }
        let layout = WorkspaceLayout::for_width(f32::from(window.viewport_size().width));
        self.narrow = layout.stacked;
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(if cfg!(target_os = "macos") {
                ".SystemUIFont"
            } else {
                "sans-serif"
            })
            .text_color(rgb(self.colors.text))
            .bg(rgb(self.colors.background))
            .key_context("Studio")
            .on_action(cx.listener(|view, _: &Cancel, _, cx| view.stop(cx)))
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    let mut order = Vec::with_capacity(8);
                    if !view.busy {
                        order.push(4);
                    }
                    order.push(7);
                    order.extend([0, 1, 2]);
                    if view.narrow {
                        order.push(6);
                    }
                    if view.primary_enabled() {
                        order.push(3);
                    }
                    if !view.busy && (view.samples > 0 || view.preview.is_some()) {
                        order.push(5);
                    }
                    if !view.narrow {
                        order.push(6);
                    }
                    let current = order
                        .iter()
                        .position(|index| view.focus[*index].is_focused(window));
                    let next = match (current, event.keystroke.modifiers.shift) {
                        (Some(index), true) => (index + order.len() - 1) % order.len(),
                        (Some(index), false) => (index + 1) % order.len(),
                        (None, true) => order.len() - 1,
                        (None, false) => 0,
                    };
                    window.focus(&view.focus[order[next]]);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .h(px(44.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .bg(rgb(self.colors.surface))
                    .pl(px(if cfg!(target_os = "macos") { 88. } else { 16. }))
                    .pr_3()
                    .child(
                        div()
                            .id("title-drag")
                            .h_full()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(MouseButton::Left, |event, window, _| {
                                if event.click_count == 2 {
                                    if cfg!(target_os = "macos") {
                                        window.titlebar_double_click();
                                    } else {
                                        window.zoom_window();
                                    }
                                } else {
                                    window.start_window_move();
                                }
                            })
                            .when_some(self.icon.clone(), |style, icon| {
                                style.child(img(icon).size(px(20.)))
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Fingerprint Studio"),
                            ),
                    )
                    .when(!cfg!(target_os = "macos"), |style| {
                        style.child(
                            div()
                                .id("close-window")
                                .px_3()
                                .cursor_pointer()
                                .text_sm()
                                .child("Close")
                                .on_click(|_, window, _| window.remove_window()),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .min_h(px(44.))
                    .px(px(layout.padding))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(self.colors.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(rgb(if self.devices > 0 {
                                        self.colors.success
                                    } else {
                                        self.colors.muted
                                    })),
                            )
                            .child(div().text_sm().child(if self.devices > 0 {
                                "Reader connected"
                            } else {
                                "No reader"
                            }))
                            .when(self.experimental, |style| {
                                style.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(self.colors.warning))
                                        .bg(rgb(self.colors.warning_background))
                                        .child("Experimental"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(self.icon_button(ToolbarAction::Refresh, cx))
                            .child(self.icon_button(ToolbarAction::Appearance, cx)),
                    ),
            )
            .child(
                div().flex_none().w_full().flex().justify_center().child(
                    div()
                        .w(px(layout.content_width + 2. * layout.padding))
                        .px(px(layout.padding))
                        .pt_4()
                        .pb_3()
                        .child(
                            div()
                                .w(px(if layout.stacked {
                                    layout.content_width
                                } else {
                                    280.
                                }))
                                .p(px(2.))
                                .rounded_md()
                                .bg(rgb(self.colors.surface))
                                .flex()
                                .gap_1()
                                .child(self.mode_tab(Mode::Capture, 0, cx))
                                .child(self.mode_tab(Mode::Enroll, 1, cx))
                                .child(self.mode_tab(Mode::Verify, 2, cx)),
                        ),
                ),
            )
            .child(
                div()
                    .id("workspace-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .w(px(layout.content_width + 2. * layout.padding))
                            .px(px(layout.padding))
                            .pt_1()
                            .pb_5()
                            .flex_none()
                            .flex()
                            .gap(px(layout.gap))
                            .when(layout.stacked, |style| style.flex_col())
                            .when(!layout.stacked, |style| {
                                style.child(self.preview_panel(layout))
                            })
                            .child(self.mode_panel(layout.controls_width, cx))
                            .when(layout.stacked, |style| {
                                style.child(self.preview_panel(layout))
                            }),
                    ),
            )
            .when(layout.stacked, |style| {
                style.child(
                    div()
                        .flex_none()
                        .border_t_1()
                        .border_color(rgb(self.colors.border))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .w(px(layout.content_width + 2. * layout.padding))
                                .px(px(layout.padding))
                                .py_3()
                                .child(self.action_group(cx)),
                        ),
                )
            })
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Fingerprint Studio\nUsage: cargo run -p fingerprint-studio -- [--engine PATH] [--ppi PPI | --experimental-ppi NOMINAL_PPI] [--threshold SCORE]\nChoose Capture, Enroll, or Verify in the app. Enrollment collects three place-and-lift impressions automatically. Verification repeats until stopped.\nRecognition requires a local SourceAFIS helper and explicit pixel scale. Default demo threshold: 40."
        );
        return Ok(());
    }
    let config = Config::parse_args(args)?;
    let worker = Worker::start(config.clone())?;
    Application::new()
        .with_assets(ToolbarAssets)
        .run(move |cx: &mut App| {
            cx.set_global(AppearanceSettings::load());
            cx.on_action(|_: &ThemeLight, cx| choose_theme(ThemeMode::Light, cx));
            cx.on_action(|_: &ThemeDark, cx| choose_theme(ThemeMode::Dark, cx));
            cx.on_action(|_: &ThemeSystem, cx| choose_theme(ThemeMode::System, cx));
            cx.on_action(|_: &Quit, cx| cx.quit());
            // Native menu commands must also work when the menu owns keyboard focus.
            cx.on_action(|_: &CloseWindow, cx| {
                if let Some(handle) = cx
                    .active_window()
                    .or_else(|| cx.windows().into_iter().next())
                {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            });
            cx.on_action(|_: &CompactWindow, cx| {
                if let Some(handle) = cx
                    .active_window()
                    .or_else(|| cx.windows().into_iter().next())
                {
                    let _ =
                        handle.update(cx, |_, window, _| window.resize(size(px(420.), px(720.))));
                }
            });
            cx.on_action(|_: &StandardWindow, cx| {
                if let Some(handle) = cx
                    .active_window()
                    .or_else(|| cx.windows().into_iter().next())
                {
                    let _ =
                        handle.update(cx, |_, window, _| window.resize(size(px(840.), px(570.))));
                }
            });
            cx.on_action(|_: &ToggleFullScreen, cx| {
                if let Some(handle) = cx
                    .active_window()
                    .or_else(|| cx.windows().into_iter().next())
                {
                    let _ = handle.update(cx, |_, window, _| window.toggle_fullscreen());
                }
            });
            cx.bind_keys([
                KeyBinding::new("secondary-q", Quit, None),
                KeyBinding::new("secondary-w", CloseWindow, Some("Studio")),
                KeyBinding::new("escape", Cancel, Some("Studio")),
            ]);
            cx.set_menus(vec![
                Menu {
                    name: "Fingerprint Studio".into(),
                    items: vec![MenuItem::action("Quit Fingerprint Studio", Quit)],
                },
                Menu {
                    name: "Appearance".into(),
                    items: vec![
                        MenuItem::action("Light", ThemeLight),
                        MenuItem::action("Dark", ThemeDark),
                        MenuItem::action("System", ThemeSystem),
                    ],
                },
                Menu {
                    name: "Window".into(),
                    items: vec![
                        MenuItem::action("Standard Size", StandardWindow),
                        MenuItem::action("Compact Size", CompactWindow),
                        MenuItem::action("Toggle Full Screen", ToggleFullScreen),
                        MenuItem::separator(),
                        MenuItem::action("Close Window", CloseWindow),
                    ],
                },
            ]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let display_size = cx.primary_display().map(|display| display.bounds().size);
            let width = display_size.map_or(px(840.), |size| (size.width - px(64.)).min(px(840.)));
            let height =
                display_size.map_or(px(570.), |size| (size.height - px(100.)).min(px(570.)));
            let bounds = Bounds::centered(None, size(width, height), cx);
            if let Err(error) = cx.open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: Some("Fingerprint Studio".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(16.), px(16.))),
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(360.), px(480.))),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                |window, cx| {
                    window.set_window_title("Fingerprint Studio");
                    window
                        .observe_window_appearance(|_, cx| cx.refresh_windows())
                        .detach();
                    let view = cx.new(|cx| Studio::new(config, worker, cx));
                    window.focus(&view.read(cx).focus[1]);
                    view
                },
            ) {
                eprintln!("Cannot open Fingerprint Studio: {error:#}");
                cx.quit();
            }
            cx.activate(true);
        });
    Ok(())
}
