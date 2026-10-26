// SPDX-License-Identifier: GPL-3.0-or-later
use fingerprint_studio::{
    Config,
    layout::WorkspaceLayout,
    theme::{Palette, ThemeMode},
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

#[path = "appearance.rs"]
mod appearance;
#[path = "assets.rs"]
mod assets;
#[path = "ui.rs"]
mod ui;
use appearance::{AppearanceSettings, choose_theme};
use assets::{ToolbarAssets, asset, render_image, tinted_mark};

impl Studio {
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
}

pub(crate) fn run() -> anyhow::Result<()> {
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
