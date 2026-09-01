use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2::MainThreadOnly;
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{ns_string, MainThreadMarker};

use crate::status::DisplayState;

pub fn build(mtm: MainThreadMarker, delegate: &AnyObject) -> Retained<NSStatusItem> {
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        button.setTitle(ns_string!("⚪"));
    }

    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Harbor Light"));

    let status = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("状态：空闲"),
            None,
            ns_string!(""),
        )
    };
    status.setEnabled(false);
    menu.addItem(&status);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let reinstall = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("重新安装 Hooks"),
            Some(sel!(reinstallHooks:)),
            ns_string!(""),
        )
    };
    unsafe { reinstall.setTarget(Some(delegate)) };
    menu.addItem(&reinstall);

    let login = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("切换开机自启"),
            Some(sel!(toggleLogin:)),
            ns_string!(""),
        )
    };
    unsafe { login.setTarget(Some(delegate)) };
    menu.addItem(&login);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("退出 Harbor Light"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        )
    };
    menu.addItem(&quit);

    item.setMenu(Some(&menu));
    item
}

pub fn set_state(item: &NSStatusItem, state: DisplayState, mtm: MainThreadMarker) {
    let emoji = match state {
        DisplayState::IDLE => "⚪",
        DisplayState::WORKING => "🟡",
        DisplayState::WAITING => "🔴",
        DisplayState::WAITING_AND_WORKING => "🔴🟡",
        DisplayState::DONE => "🟢",
        _ => "⚪",
    };
    if let Some(button) = item.button(mtm) {
        button.setTitle(&objc2_foundation::NSString::from_str(emoji));
    }
    if let Some(menu) = item.menu(mtm) {
        if let Some(first) = menu.itemAtIndex(0) {
            let title = format!("状态：{}", state.label_zh());
            first.setTitle(&objc2_foundation::NSString::from_str(&title));
        }
    }
}
