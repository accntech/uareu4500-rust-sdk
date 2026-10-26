use fingerprint_studio::theme::{Palette, PreferenceStore, ThemeMode};

#[test]
fn system_tracks_os_while_explicit_modes_override_it() {
    assert_eq!(ThemeMode::System.palette(false), Palette::light());
    assert_eq!(ThemeMode::System.palette(true), Palette::dark());
    assert_eq!(ThemeMode::Light.palette(true), Palette::light());
    assert_eq!(ThemeMode::Dark.palette(false), Palette::dark());
}

#[test]
fn appearance_preference_survives_restart_and_invalid_input_defaults_to_system() {
    let path = std::env::temp_dir()
        .join(format!("fingerprint-theme-{}", std::process::id()))
        .join("appearance");
    let store = PreferenceStore::new(path.clone());
    assert_eq!(store.load(), ThemeMode::System);
    store.save(ThemeMode::Dark).unwrap();
    assert_eq!(PreferenceStore::new(path.clone()).load(), ThemeMode::Dark);
    std::fs::write(&path, "not-a-theme").unwrap();
    assert_eq!(store.load(), ThemeMode::System);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

fn luminance(color: u32) -> f64 {
    let channel = |shift: u32| {
        let s = f64::from(((color >> shift) & 255u32) as u8) / 255.;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}
fn contrast(a: u32, b: u32) -> f64 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
#[test]
fn both_palettes_keep_body_labels_and_primary_actions_readable() {
    for palette in [Palette::light(), Palette::dark()] {
        assert!(contrast(palette.text, palette.background) >= 4.5);
        assert!(contrast(palette.muted, palette.surface) >= 4.5);
        assert!(contrast(palette.on_accent, palette.accent) >= 4.5);
        assert!(contrast(palette.warning, palette.warning_background) >= 4.5);
    }
}

#[test]
fn toolbar_cycles_light_dark_system_without_changing_system_meaning() {
    let mut mode = ThemeMode::System;
    for expected in [
        ThemeMode::Light,
        ThemeMode::Dark,
        ThemeMode::System,
        ThemeMode::Light,
    ] {
        mode = mode.next();
        assert_eq!(mode, expected);
    }
    assert_eq!(ThemeMode::Light.icon_name(), "sun");
    assert_eq!(ThemeMode::Dark.icon_name(), "moon");
    assert_eq!(ThemeMode::System.icon_name(), "monitor");
}
