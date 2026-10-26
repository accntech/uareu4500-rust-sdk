// SPDX-License-Identifier: GPL-3.0-or-later
use fingerprint_studio::theme::{PreferenceStore, ThemeMode};
use gpui::{App, BorrowAppContext, Global};

pub(super) struct AppearanceSettings {
    pub(super) mode: ThemeMode,
    store: Option<PreferenceStore>,
    pub(super) save_error: Option<String>,
}
impl Global for AppearanceSettings {}
impl AppearanceSettings {
    pub(super) fn load() -> Self {
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
pub(super) fn choose_theme(mode: ThemeMode, cx: &mut App) {
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
