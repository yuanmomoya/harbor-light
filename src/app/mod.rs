mod light;
mod menubar;
mod watch;
mod window;

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationDidChangeScreenParametersNotification, NSPanel, NSRunningApplication,
    NSStatusItem, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidTerminateApplicationNotification,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSString,
    NSTimer,
};

use crate::activity::{
    read_activities, remove_provider_activities, resolve_display_state, ProviderResets,
};
use crate::paths::{append_log, Paths};
use crate::providers::Provider;
use crate::sessions::scan_sessions;
use crate::status::{read_current, DisplayState};

use self::light::LightView;
use self::window::{create_panel, ensure_visible};

pub(crate) static NEEDS_REFRESH: AtomicBool = AtomicBool::new(true);

struct AppIvars {
    panel: OnceCell<Retained<NSPanel>>,
    light: OnceCell<Retained<LightView>>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    timer: OnceCell<Retained<NSTimer>>,
    displayed: Cell<u8>,
    poll_ticks: Cell<u8>,
    provider_resets: RefCell<ProviderResets>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "HarborLightDelegate"]
    #[ivars = AppIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

            let light = LightView::new(mtm);
            let panel = create_panel(mtm, &light);
            panel.orderFrontRegardless();

            let status_item = menubar::build(mtm, self.as_ref());
            menubar::set_state(&status_item, DisplayState::IDLE, mtm);

            unsafe {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    self,
                    sel!(screensDidChange:),
                    Some(NSApplicationDidChangeScreenParametersNotification),
                    None,
                );
                NSWorkspace::sharedWorkspace()
                    .notificationCenter()
                    .addObserver_selector_name_object(
                        self,
                        sel!(workspaceApplicationTerminated:),
                        Some(NSWorkspaceDidTerminateApplicationNotification),
                        None,
                    );
            }

            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    0.15,
                    self,
                    sel!(tick:),
                    None,
                    true,
                )
            };

            self.ivars().panel.set(panel).ok();
            self.ivars().light.set(light).ok();
            self.ivars().status_item.set(status_item).ok();
            self.ivars().timer.set(timer).ok();

            watch::spawn_watcher();
            for provider in Provider::ALL {
                if !is_provider_running(provider) {
                    self.reset_provider(provider, "application was not running at launch");
                }
            }
            self.refresh(true);
            append_log(&Paths::current(), "app launched");
        }
    }

    impl Delegate {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            let ticks = (self.ivars().poll_ticks.get() + 1) % 14;
            self.ivars().poll_ticks.set(ticks);
            // File notifications are normally instant; this periodic scan is
            // a low-cost fallback for coalesced or missed FSEvents.
            self.refresh(ticks == 0);
        }

        #[unsafe(method(screensDidChange:))]
        fn screens_did_change(&self, _notification: &NSNotification) {
            if let Some(panel) = self.ivars().panel.get() {
                ensure_visible(panel);
            }
        }

        #[unsafe(method(workspaceApplicationTerminated:))]
        fn workspace_application_terminated(&self, notification: &NSNotification) {
            let Some(user_info) = notification.userInfo() else {
                return;
            };
            let Some(application) =
                user_info.objectForKey(unsafe { NSWorkspaceApplicationKey })
            else {
                return;
            };
            let Some(application) = application.downcast_ref::<NSRunningApplication>() else {
                return;
            };
            let Some(bundle_id) = application.bundleIdentifier() else {
                return;
            };
            let bundle_id = bundle_id.to_string();
            if let Some(provider) = Provider::ALL.into_iter().find(|provider| {
                provider
                    .macos_bundle_ids()
                    .iter()
                    .any(|candidate| *candidate == bundle_id)
            }) {
                self.reset_provider(provider, "application terminated");
            }
        }

        #[unsafe(method(reinstallHooks:))]
        fn reinstall_hooks(&self, _sender: Option<&NSObject>) {
            let paths = Paths::current();
            if let Ok(exe) = crate::install::current_executable() {
                match crate::install::install_provider_hooks(&paths, &exe) {
                    Ok(()) => append_log(&paths, "reinstalled Codex and Cursor hooks"),
                    Err(err) => append_log(&paths, &format!("reinstall hooks failed: {err:#}")),
                }
            }
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&NSObject>) {
            let paths = Paths::current();
            if paths.launch_agent_plist().exists() {
                crate::install::launchctl_bootout();
                let _ = std::fs::remove_file(paths.launch_agent_plist());
                append_log(&paths, "launch agent removed");
            } else if let Ok(exe) = crate::install::current_executable() {
                if crate::install::write_launch_agent(&paths.launch_agent_plist(), &exe).is_ok() {
                    let _ = crate::install::launchctl_bootstrap(&paths.launch_agent_plist());
                    append_log(&paths, "launch agent installed");
                }
            }
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppIvars {
            panel: OnceCell::new(),
            light: OnceCell::new(),
            status_item: OnceCell::new(),
            timer: OnceCell::new(),
            displayed: Cell::new(DisplayState::IDLE.as_u8()),
            poll_ticks: Cell::new(0),
            provider_resets: RefCell::new(ProviderResets::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn reset_provider(&self, provider: Provider, reason: &str) {
        self.ivars()
            .provider_resets
            .borrow_mut()
            .insert(provider, chrono::Utc::now());
        if let Err(err) = remove_provider_activities(&Paths::current(), provider) {
            append_log(
                &Paths::current(),
                &format!("failed to clear {provider} activities: {err:#}"),
            );
        }
        NEEDS_REFRESH.store(true, Ordering::SeqCst);
        self.refresh(true);
        append_log(
            &Paths::current(),
            &format!("{} {reason}", provider.display_name()),
        );
    }

    fn refresh(&self, force: bool) {
        let dirty = NEEDS_REFRESH.swap(false, Ordering::SeqCst);
        if !dirty && !force {
            // Still re-evaluate done→idle even if no file event arrived.
            let displayed = DisplayState::from_u8(self.ivars().displayed.get());
            if !displayed.green_active() && !displayed.is_idle() {
                return;
            }
        }

        let paths = Paths::current();
        let hook = read_current(&paths).ok().flatten();
        let activities = read_activities(&paths).unwrap_or_else(|err| {
            append_log(&paths, &format!("read activities failed: {err:#}"));
            Vec::new()
        });
        let sessions = scan_sessions(&paths.sessions_dir());
        let next = resolve_display_state(
            hook.as_ref(),
            &activities,
            &sessions,
            &self.ivars().provider_resets.borrow(),
        );
        let prev = DisplayState::from_u8(self.ivars().displayed.get());
        if next == prev && !dirty {
            return;
        }
        self.ivars().displayed.set(next.as_u8());

        if let Some(light) = self.ivars().light.get() {
            let reduce = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
            light.apply_state(next, reduce);
        }
        if let Some(item) = self.ivars().status_item.get() {
            menubar::set_state(item, next, self.mtm());
        }
    }
}

fn is_provider_running(provider: Provider) -> bool {
    provider.macos_bundle_ids().iter().any(|bundle_id| {
        let bundle_id = NSString::from_str(bundle_id);
        !NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle_id).is_empty()
    })
}

pub fn run() -> Result<()> {
    let mtm = MainThreadMarker::new().expect("Harbor Light GUI must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    Ok(())
}
