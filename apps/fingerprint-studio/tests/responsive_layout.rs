use fingerprint_studio::layout::WorkspaceLayout;

#[test]
fn narrow_windows_stack_full_width_without_horizontal_overflow() {
    for width in [360.0, 390.0, 420.0, 600.0, 699.0] {
        let layout = WorkspaceLayout::for_width(width);
        assert!(layout.stacked);
        assert_eq!(layout.preview_width, layout.content_width);
        assert_eq!(layout.controls_width, layout.content_width);
        assert!(layout.content_width + 2.0 * layout.padding <= width);
        assert!((layout.preview_width / layout.preview_height - 384.0 / 290.0).abs() < 0.001);
    }
}

#[test]
fn desktop_columns_fit_and_stop_growing_on_large_screens() {
    for width in [700.0, 840.0, 1000.0, 1920.0] {
        let layout = WorkspaceLayout::for_width(width);
        assert!(!layout.stacked);
        assert!(
            layout.preview_width + layout.gap + layout.controls_width
                <= layout.content_width + 0.01
        );
        assert!(layout.preview_width <= 360.0);
        assert!(layout.content_width <= 800.0);
        assert!((layout.preview_width / layout.preview_height - 384.0 / 290.0).abs() < 0.001);
    }
}
