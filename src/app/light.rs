use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSColor, NSView};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{ns_string, MainThreadMarker, NSNumber, NSObjectProtocol};
use objc2_quartz_core::{
    kCAFillModeForwards, kCAMediaTimingFunctionEaseInEaseOut, CABasicAnimation, CALayer,
    CAMediaTiming, CAMediaTimingFunction, CATransaction,
};

use crate::status::DisplayState;

pub const WINDOW_WIDTH: f64 = 128.0;
pub const WINDOW_HEIGHT: f64 = 38.0;

const DOT_SIZE: f64 = 16.0;
const DOT_GAP: f64 = 9.0;
const GLOW_SIZE: f64 = 32.0;

pub(crate) struct LightIvars {
    red_glow: OnceCell<Retained<CALayer>>,
    yellow_glow: OnceCell<Retained<CALayer>>,
    green_glow: OnceCell<Retained<CALayer>>,
    red: OnceCell<Retained<CALayer>>,
    yellow: OnceCell<Retained<CALayer>>,
    green: OnceCell<Retained<CALayer>>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[name = "CodexTrafficLightView"]
    #[ivars = LightIvars]
    pub(crate) struct LightView;

    unsafe impl NSObjectProtocol for LightView {}
);

impl LightView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let frame = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );
        let this = Self::alloc(mtm).set_ivars(LightIvars {
            red_glow: OnceCell::new(),
            yellow_glow: OnceCell::new(),
            green_glow: OnceCell::new(),
            red: OnceCell::new(),
            yellow: OnceCell::new(),
            green: OnceCell::new(),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.finish_init();
        this
    }

    fn finish_init(&self) {
        self.setWantsLayer(true);
        let Some(root) = self.layer() else {
            return;
        };
        root.setMasksToBounds(false);
        root.setBackgroundColor(Some(&NSColor::clearColor().CGColor()));

        let scale = self
            .window()
            .and_then(|w| w.screen())
            .map(|s| s.backingScaleFactor())
            .unwrap_or(2.0);
        root.setContentsScale(scale);

        let background = CALayer::layer();
        background.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        ));
        background.setCornerRadius(WINDOW_HEIGHT / 2.0);
        background.setBackgroundColor(Some(&srgb(0.055, 0.06, 0.07, 0.94).CGColor()));
        background.setBorderWidth(0.75);
        background.setBorderColor(Some(&srgb(1.0, 1.0, 1.0, 0.14).CGColor()));
        background.setContentsScale(scale);
        root.addSublayer(&background);

        let total_width = DOT_SIZE * 3.0 + DOT_GAP * 2.0;
        let start_x = (WINDOW_WIDTH - total_width) / 2.0;
        let yellow_x = start_x + DOT_SIZE + DOT_GAP;
        let green_x = start_x + (DOT_SIZE + DOT_GAP) * 2.0;
        let red_glow = make_glow(start_x + DOT_SIZE / 2.0, scale);
        let yellow_glow = make_glow(yellow_x + DOT_SIZE / 2.0, scale);
        let green_glow = make_glow(green_x + DOT_SIZE / 2.0, scale);
        let red = make_dot(start_x, scale);
        let yellow = make_dot(yellow_x, scale);
        let green = make_dot(green_x, scale);

        root.addSublayer(&red_glow);
        root.addSublayer(&yellow_glow);
        root.addSublayer(&green_glow);
        root.addSublayer(&red);
        root.addSublayer(&yellow);
        root.addSublayer(&green);

        self.ivars().red_glow.set(red_glow).ok();
        self.ivars().yellow_glow.set(yellow_glow).ok();
        self.ivars().green_glow.set(green_glow).ok();
        self.ivars().red.set(red).ok();
        self.ivars().yellow.set(yellow).ok();
        self.ivars().green.set(green).ok();
        self.apply_state(DisplayState::IDLE, false);
    }

    pub fn apply_state(&self, state: DisplayState, reduce_motion: bool) {
        let Some(red_glow) = self.ivars().red_glow.get() else {
            return;
        };
        let Some(yellow_glow) = self.ivars().yellow_glow.get() else {
            return;
        };
        let Some(green_glow) = self.ivars().green_glow.get() else {
            return;
        };
        let Some(red) = self.ivars().red.get() else {
            return;
        };
        let Some(yellow) = self.ivars().yellow.get() else {
            return;
        };
        let Some(green) = self.ivars().green.get() else {
            return;
        };

        let red_active = state.red_active();
        let yellow_active = state.yellow_active();
        let green_active = state.green_active();

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        red_glow.removeAllAnimations();
        yellow_glow.removeAllAnimations();
        green_glow.removeAllAnimations();
        red.removeAllAnimations();
        yellow.removeAllAnimations();
        green.removeAllAnimations();

        set_lamp(
            red,
            red_glow,
            &srgb(1.0, 0.30, 0.24, 1.0),
            &srgb(0.30, 0.07, 0.065, 1.0),
            red_active,
        );
        set_lamp(
            yellow,
            yellow_glow,
            &srgb(1.0, 0.86, 0.20, 1.0),
            &srgb(0.30, 0.23, 0.055, 1.0),
            yellow_active,
        );
        set_lamp(
            green,
            green_glow,
            &srgb(0.22, 1.0, 0.42, 1.0),
            &srgb(0.055, 0.26, 0.10, 1.0),
            green_active,
        );
        CATransaction::commit();

        if reduce_motion {
            return;
        }

        if red_active {
            add_opacity_animation(red, 0.68, 1.0, 0.28);
            add_opacity_animation(red_glow, 0.22, 1.0, 0.28);
        }
        if yellow_active {
            if red_active {
                // A waiting conversation plus another active conversation is
                // one alert: flash both lamps in sync.
                add_opacity_animation(yellow, 0.68, 1.0, 0.28);
                add_opacity_animation(yellow_glow, 0.22, 1.0, 0.28);
            } else {
                add_opacity_animation(yellow, 0.82, 1.0, 1.0);
                add_opacity_animation(yellow_glow, 0.32, 1.0, 1.0);
            }
        }
        if green_active {
            add_pop_animation(green);
            add_pop_animation(green_glow);
        }
    }
}

fn make_glow(center_x: f64, scale: f64) -> Retained<CALayer> {
    let glow = CALayer::layer();
    glow.setFrame(CGRect::new(
        CGPoint::new(
            center_x - GLOW_SIZE / 2.0,
            (WINDOW_HEIGHT - GLOW_SIZE) / 2.0,
        ),
        CGSize::new(GLOW_SIZE, GLOW_SIZE),
    ));
    glow.setCornerRadius(GLOW_SIZE / 2.0);
    glow.setMasksToBounds(false);
    glow.setContentsScale(scale);
    glow.setHidden(true);
    glow
}

fn make_dot(x: f64, scale: f64) -> Retained<CALayer> {
    let dot = CALayer::layer();
    dot.setFrame(CGRect::new(
        CGPoint::new(x, (WINDOW_HEIGHT - DOT_SIZE) / 2.0),
        CGSize::new(DOT_SIZE, DOT_SIZE),
    ));
    dot.setCornerRadius(DOT_SIZE / 2.0);
    dot.setBorderWidth(0.65);
    dot.setBorderColor(Some(&srgb(1.0, 1.0, 1.0, 0.18).CGColor()));
    dot.setMasksToBounds(false);
    dot.setContentsScale(scale);
    dot
}

fn set_lamp(
    layer: &CALayer,
    glow: &CALayer,
    active: &NSColor,
    inactive: &NSColor,
    is_active: bool,
) {
    let color = if is_active { active } else { inactive };
    layer.setBackgroundColor(Some(&color.CGColor()));
    layer.setOpacity(if is_active { 1.0 } else { 0.72 });
    layer.setBorderColor(Some(
        &srgb(1.0, 1.0, 1.0, if is_active { 0.46 } else { 0.18 }).CGColor(),
    ));
    if is_active {
        let shadow = active.CGColor();
        layer.setShadowColor(Some(&shadow));
    } else {
        layer.setShadowColor(None);
    }
    layer.setShadowOpacity(if is_active { 1.0 } else { 0.0 });
    layer.setShadowRadius(if is_active { 5.0 } else { 0.0 });
    layer.setShadowOffset(CGSize::new(0.0, 0.0));

    let glow_color = active.colorWithAlphaComponent(0.26);
    glow.setBackgroundColor(Some(&glow_color.CGColor()));
    glow.setHidden(!is_active);
    glow.setOpacity(if is_active { 1.0 } else { 0.0 });
    if is_active {
        let shadow = active.CGColor();
        glow.setShadowColor(Some(&shadow));
    } else {
        glow.setShadowColor(None);
    }
    glow.setShadowOpacity(if is_active { 0.88 } else { 0.0 });
    glow.setShadowRadius(if is_active { 7.0 } else { 0.0 });
    glow.setShadowOffset(CGSize::new(0.0, 0.0));
}

fn srgb(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
}

fn add_opacity_animation(layer: &CALayer, from: f32, to: f32, duration: f64) {
    let anim = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
    let from_v = NSNumber::numberWithDouble(from as f64);
    let to_v = NSNumber::numberWithDouble(to as f64);
    unsafe {
        anim.setFromValue(Some(&from_v));
        anim.setToValue(Some(&to_v));
    }
    anim.setDuration(duration);
    anim.setAutoreverses(true);
    anim.setRepeatCount(f32::INFINITY);
    let timing =
        unsafe { CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionEaseInEaseOut) };
    anim.setTimingFunction(Some(&timing));
    layer.addAnimation_forKey(&anim, Some(ns_string!("pulse")));
}

fn add_pop_animation(layer: &CALayer) {
    let anim = CABasicAnimation::animationWithKeyPath(Some(ns_string!("transform.scale")));
    let from_v = NSNumber::numberWithDouble(0.55);
    let to_v = NSNumber::numberWithDouble(1.0);
    unsafe {
        anim.setFromValue(Some(&from_v));
        anim.setToValue(Some(&to_v));
    }
    anim.setDuration(0.32);
    let timing =
        unsafe { CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionEaseInEaseOut) };
    anim.setTimingFunction(Some(&timing));
    unsafe { anim.setFillMode(kCAFillModeForwards) };
    anim.setRemovedOnCompletion(false);
    layer.addAnimation_forKey(&anim, Some(ns_string!("pop")));
}
