use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFloatingWindowLevel, NSPanel, NSScreen,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSPoint, NSRect, NSSize};

use super::light::{LightView, WINDOW_HEIGHT, WINDOW_WIDTH};

const EDGE_MARGIN: f64 = 24.0;

pub fn create_panel(mtm: MainThreadMarker, light: &LightView) -> objc2::rc::Retained<NSPanel> {
    let frame = NSScreen::mainScreen(mtm)
        .map(|screen| default_frame(screen.visibleFrame()))
        .unwrap_or_else(|| panel_frame(200.0, 200.0));
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setTitle(ns_string!("Harbor Light"));
    panel.setOpaque(false);
    panel.setHasShadow(true);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setLevel(NSFloatingWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel.setIgnoresMouseEvents(false);
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setMovable(true);
    panel.setMovableByWindowBackground(true);
    panel.setHidesOnDeactivate(false);
    panel.setContentView(Some(light));

    let autosave_name = ns_string!("HarborLightTrafficPanel");
    if panel.setFrameUsingName_force(autosave_name, true) {
        let saved = panel.frame();
        panel.setFrame_display(
            NSRect::new(saved.origin, NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
            false,
        );
    }
    panel.setFrameAutosaveName(autosave_name);
    ensure_visible(&panel);
    panel
}

/// Keep the user's chosen monitor and position. Only reset when the display
/// arrangement changed and the window center is no longer on any screen.
pub fn ensure_visible(panel: &NSPanel) {
    let screens = NSScreen::screens(panel.mtm());
    let current = panel.frame();
    let is_visible = screens
        .iter()
        .any(|screen| center_is_inside(current, screen.visibleFrame()));
    if is_visible {
        return;
    }

    if let Some(main) = NSScreen::mainScreen(panel.mtm()) {
        panel.setFrame_display(default_frame(main.visibleFrame()), true);
        panel.saveFrameUsingName(ns_string!("HarborLightTrafficPanel"));
    }
}

fn default_frame(visible: NSRect) -> NSRect {
    let right = visible.origin.x + visible.size.width;
    let top = visible.origin.y + visible.size.height;
    panel_frame(
        right - WINDOW_WIDTH - EDGE_MARGIN,
        top - WINDOW_HEIGHT - EDGE_MARGIN,
    )
}

fn panel_frame(x: f64, y: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
}

fn center_is_inside(window: NSRect, screen: NSRect) -> bool {
    let x = window.origin.x + window.size.width / 2.0;
    let y = window.origin.y + window.size.height / 2.0;
    x >= screen.origin.x
        && x <= screen.origin.x + screen.size.width
        && y >= screen.origin.y
        && y <= screen.origin.y + screen.size.height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn defaults_near_the_main_screens_top_right_corner() {
        let visible = rect(0.0, 68.0, 1512.0, 876.0);
        let frame = default_frame(visible);
        assert_eq!(frame.origin.x, 1360.0);
        assert_eq!(frame.origin.y, 882.0);
        assert_eq!(frame.size.width, WINDOW_WIDTH);
        assert_eq!(frame.size.height, WINDOW_HEIGHT);
    }

    #[test]
    fn accepts_a_position_on_a_negative_origin_secondary_screen() {
        let secondary = rect(-2560.0, 0.0, 2560.0, 1440.0);
        let window = panel_frame(-1800.0, 900.0);
        assert!(center_is_inside(window, secondary));
    }

    #[test]
    fn detects_a_position_left_behind_after_monitor_removal() {
        let main = rect(0.0, 0.0, 1512.0, 982.0);
        let window = panel_frame(-1800.0, 900.0);
        assert!(!center_is_inside(window, main));
    }
}
