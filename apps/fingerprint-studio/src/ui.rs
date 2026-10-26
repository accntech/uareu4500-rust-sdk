// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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

impl Studio {
    pub(super) fn caption(&self, text: impl Into<SharedString>) -> Div {
        div()
            .text_xs()
            .text_color(rgb(self.colors.muted))
            .child(text.into())
    }

    pub(super) fn button(
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

    pub(super) fn icon_button(
        &self,
        action: ToolbarAction,
        cx: &Context<Self>,
    ) -> impl IntoElement {
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

    pub(super) fn mode_tab(
        &self,
        mode: Mode,
        focus_index: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = self.colors;
        let selected = self.mode == mode;
        div()
            .id(mode.label())
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
            .child(mode.label())
    }
}

impl Studio {
    pub(super) fn primary_enabled(&self) -> bool {
        !self.stopping
            && if self.busy {
                self.active_mode.is_some()
            } else {
                self.blocker().is_none()
            }
    }

    pub(super) fn action_group(&self, cx: &Context<Self>) -> impl IntoElement {
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

    pub(super) fn mode_panel(&self, width: f32, cx: &Context<Self>) -> impl IntoElement {
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

    pub(super) fn detail_row(
        &self,
        label: &'static str,
        value: impl Into<SharedString>,
    ) -> impl IntoElement {
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

    pub(super) fn details_section(&self, cx: &Context<Self>) -> impl IntoElement {
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

    pub(super) fn preview_panel(&self, layout: WorkspaceLayout) -> impl IntoElement {
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
