use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::fs;
use std::mem::{size_of, zeroed};
use std::path::Path;
use std::ptr::{null, null_mut};

use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    CloseHandle, HINSTANCE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint, GetDC,
    MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, FindWindowW, GetClientRect, GetCursorPos, GetMessageW, GetWindowLongPtrW,
    GetWindowRect, LoadCursorW, LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TrackPopupMenu, TranslateMessage, UpdateLayeredWindow, CREATESTRUCTW,
    CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HICON, HTCAPTION, HWND_TOPMOST, IDC_ARROW,
    IDI_APPLICATION, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SPI_GETWORKAREA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, ULW_ALPHA, WM_APP, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_EXITSIZEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_RBUTTONUP, WM_TIMER,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::activity::{
    read_activities, remove_provider_activities, resolve_display_state, ProviderResets,
};
use crate::paths::{append_log, Paths};
use crate::providers::Provider;
use crate::sessions::scan_sessions;
use crate::status::{read_current, DisplayState};

pub(crate) const WINDOW_CLASS_NAME: &str = "HarborLightTrafficWindow";
const WINDOW_TITLE: &str = "Harbor Light";
const TIMER_ID: usize = 1;
const TRAY_ID: u32 = 1;
const TRAY_MESSAGE: u32 = WM_APP + 1;
const FORCE_SHOW_MESSAGE: u32 = WM_APP + 2;
const MENU_REINSTALL_HOOKS: i32 = 1001;
const MENU_QUIT: i32 = 1002;
const LOGICAL_WIDTH: i32 = 128;
const LOGICAL_HEIGHT: i32 = 38;
const EDGE_MARGIN: i32 = 24;
const SUPERSAMPLE: i32 = 4;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SavedPosition {
    x: i32,
    y: i32,
}

struct AppState {
    paths: Paths,
    displayed: DisplayState,
    tick: u32,
    phase: f32,
    dpi: u32,
    provider_running: BTreeMap<Provider, bool>,
    provider_resets: ProviderResets,
}

impl AppState {
    fn new() -> Self {
        let paths = Paths::current();
        let running_processes = running_process_names();
        let provider_running = Provider::ALL
            .into_iter()
            .map(|provider| (provider, provider_is_running(provider, &running_processes)))
            .collect::<BTreeMap<_, _>>();
        let now = Utc::now();
        let provider_resets = provider_running
            .iter()
            .filter_map(|(provider, running)| (!running).then_some((*provider, now)))
            .collect::<ProviderResets>();
        for provider in Provider::ALL {
            if !provider_running.get(&provider).copied().unwrap_or(false) {
                let _ = remove_provider_activities(&paths, provider);
            }
        }

        Self {
            paths,
            displayed: DisplayState::IDLE,
            tick: 0,
            phase: 0.0,
            dpi: 96,
            provider_running,
            provider_resets,
        }
    }

    fn refresh(&mut self, hwnd: HWND, force: bool) {
        let running_processes = running_process_names();
        for provider in Provider::ALL {
            let running = provider_is_running(provider, &running_processes);
            let was_running = self
                .provider_running
                .insert(provider, running)
                .unwrap_or(false);
            if was_running && !running {
                self.provider_resets.insert(provider, Utc::now());
                if let Err(error) = remove_provider_activities(&self.paths, provider) {
                    append_log(
                        &self.paths,
                        &format!("failed to clear {provider} activities: {error:#}"),
                    );
                }
                append_log(
                    &self.paths,
                    &format!("{} Windows process terminated", provider.display_name()),
                );
            }
        }

        let hook = read_current(&self.paths).ok().flatten();
        let activities = read_activities(&self.paths).unwrap_or_else(|error| {
            append_log(&self.paths, &format!("read activities failed: {error:#}"));
            Vec::new()
        });
        let sessions = scan_sessions(&self.paths.sessions_dir());
        let next =
            resolve_display_state(hook.as_ref(), &activities, &sessions, &self.provider_resets);
        if !force && next == self.displayed {
            return;
        }

        self.displayed = next;
        unsafe {
            update_tray_tip(hwnd, next);
            render_layered_window(hwnd, self);
        }
    }
}

pub fn run() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let class_name = wide(WINDOW_CLASS_NAME);
        let existing = FindWindowW(class_name.as_ptr(), null());
        if !existing.is_null() {
            // A running instance may have a transparent layered window after a
            // hidden installer launch. Ask it to paint and come to the front.
            let _ = PostMessageW(existing, FORCE_SHOW_MESSAGE, 0, 0);
            return Ok(());
        }

        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            bail!("GetModuleHandleW failed");
        }
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hIcon: load_app_icon(instance),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class_name.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 {
            bail!("RegisterClassW failed");
        }

        let dpi = GetDpiForSystem().max(96);
        let (width, height) = scaled_size(dpi);
        let position = initial_position(&Paths::current(), width, height);
        let title = wide(WINDOW_TITLE);
        let mut state = Box::new(AppState::new());
        state.dpi = dpi;
        let state_ptr = Box::into_raw(state);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            position.x,
            position.y,
            width,
            height,
            null_mut(),
            null_mut(),
            instance,
            state_ptr.cast::<c_void>(),
        );
        if hwnd.is_null() {
            drop(Box::from_raw(state_ptr));
            bail!("CreateWindowExW failed");
        }

        let window_dpi = GetDpiForWindow(hwnd).max(96);
        let (window_width, window_height) = scaled_size(window_dpi);
        (*state_ptr).dpi = window_dpi;
        if window_dpi != dpi {
            let _ = SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                window_width,
                window_height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        add_tray_icon(hwnd);
        let _ = SetTimer(hwnd, TIMER_ID, 50, None);
        (*state_ptr).refresh(hwnd, true);
        show_overlay(hwnd, &*state_ptr);
        append_log(&(*state_ptr).paths, "Windows app launched");

        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        if !create.is_null() {
            let state = (*create).lpCreateParams as *mut AppState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;

    match message {
        WM_TIMER if wparam == TIMER_ID => {
            if let Some(state) = state_ptr.as_mut() {
                state.tick = state.tick.wrapping_add(1);
                state.phase += 0.05;
                if state.tick % 20 == 0 {
                    state.refresh(hwnd, false);
                    ensure_visible(hwnd, &state.paths);
                    render_layered_window(hwnd, state);
                } else if !state.displayed.is_idle() {
                    render_layered_window(hwnd, state);
                }
            }
            0
        }
        WM_PAINT => {
            if let Some(state) = state_ptr.as_ref() {
                paint(hwnd, state);
            }
            0
        }
        WM_NCHITTEST => HTCAPTION as LRESULT,
        WM_EXITSIZEMOVE => {
            if let Some(state) = state_ptr.as_ref() {
                save_position(hwnd, &state.paths);
            }
            0
        }
        WM_DISPLAYCHANGE => {
            if let Some(state) = state_ptr.as_ref() {
                ensure_visible(hwnd, &state.paths);
                render_layered_window(hwnd, state);
            }
            0
        }
        WM_DPICHANGED => {
            if let Some(state) = state_ptr.as_mut() {
                state.dpi = (wparam as u32 & 0xffff).max(96);
                let suggested = lparam as *const RECT;
                if !suggested.is_null() {
                    let (width, height) = scaled_size(state.dpi);
                    let _ = SetWindowPos(
                        hwnd,
                        null_mut(),
                        (*suggested).left,
                        (*suggested).top,
                        width,
                        height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    render_layered_window(hwnd, state);
                }
            }
            0
        }
        TRAY_MESSAGE => {
            if lparam as u32 == WM_RBUTTONUP {
                show_tray_menu(hwnd, state_ptr.as_ref().map(|s| s.displayed));
            }
            0
        }
        FORCE_SHOW_MESSAGE => {
            if let Some(state) = state_ptr.as_ref() {
                show_overlay(hwnd, state);
            }
            0
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            remove_tray_icon(hwnd);
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !state_ptr.is_null() {
                drop(Box::from_raw(state_ptr));
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn show_overlay(hwnd: HWND, state: &AppState) {
    // The first ShowWindow is ignored when the process was started hidden
    // (Inno Setup `runhidden` inherits SW_HIDE). Call it twice.
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    ensure_visible(hwnd, &state.paths);
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    render_layered_window(hwnd, state);
}

unsafe fn paint(hwnd: HWND, state: &AppState) {
    let mut ps = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    if hdc.is_null() {
        return;
    }
    let _ = EndPaint(hwnd, &ps);

    render_layered_window(hwnd, state);
}

unsafe fn render_layered_window(hwnd: HWND, state: &AppState) {
    let mut rect: RECT = zeroed();
    if GetClientRect(hwnd, &mut rect) == 0 {
        return;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return;
    }

    let screen_dc = GetDC(null_mut());
    if screen_dc.is_null() {
        return;
    }

    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.is_null() {
        let _ = ReleaseDC(null_mut(), screen_dc);
        return;
    }

    let mut bitmap_info = BITMAPINFO::default();
    bitmap_info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..Default::default()
    };
    let mut bits = null_mut();
    let bitmap = CreateDIBSection(
        screen_dc,
        &bitmap_info,
        DIB_RGB_COLORS,
        &mut bits,
        null_mut(),
        0,
    );
    if bitmap.is_null() || bits.is_null() {
        if !bitmap.is_null() {
            let _ = DeleteObject(bitmap);
        }
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(null_mut(), screen_dc);
        return;
    }

    let pixels = render_pixels(width, height, state);
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
    let old_bitmap = SelectObject(mem_dc, bitmap);

    let mut window_rect: RECT = zeroed();
    if GetWindowRect(hwnd, &mut window_rect) != 0 {
        let destination = POINT {
            x: window_rect.left,
            y: window_rect.top,
        };
        let source = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            &destination,
            &size,
            mem_dc,
            &source,
            0,
            &blend,
            ULW_ALPHA,
        );
    }

    let _ = SelectObject(mem_dc, old_bitmap);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(null_mut(), screen_dc);
}

fn render_pixels(width: i32, height: i32, state: &AppState) -> Vec<u32> {
    let centers = [39, 64, 89].map(|value| scale(value, state.dpi));
    let center_y = height / 2;
    let lamp_radius = scale(8, state.dpi).max(6);
    let pulse = ((state.phase * std::f32::consts::PI).sin() + 1.0) * 0.5;
    let fast = ((state.phase * std::f32::consts::TAU * 1.8).sin() + 1.0) * 0.5;
    let intensities = [
        state.displayed.red_active().then_some(0.72 + fast * 0.28),
        state
            .displayed
            .yellow_active()
            .then_some(if state.displayed.red_active() {
                0.72 + fast * 0.28
            } else {
                0.68 + pulse * 0.32
            }),
        state
            .displayed
            .green_active()
            .then_some(0.82 + pulse * 0.18),
    ];
    let active_colors = [rgb(255, 66, 58), rgb(255, 218, 50), rgb(56, 255, 107)];
    let inactive_colors = [rgb(78, 18, 17), rgb(78, 59, 14), rgb(15, 68, 28)];
    let sample_count = (SUPERSAMPLE * SUPERSAMPLE) as u32;
    let inset = scale(1, state.dpi) as f32;
    let mut pixels = Vec::with_capacity((width * height) as usize);

    for py in 0..height {
        for px in 0..width {
            let mut red = 0u32;
            let mut green = 0u32;
            let mut blue = 0u32;
            let mut alpha = 0u32;

            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let x = px as f32 + (sx as f32 + 0.5) / SUPERSAMPLE as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SUPERSAMPLE as f32;
                    let mut color = capsule_sample(x, y, width as f32, height as f32, inset);

                    for index in 0..centers.len() {
                        draw_lamp_sample(
                            &mut color,
                            x,
                            y,
                            centers[index] as f32,
                            center_y as f32,
                            lamp_radius as f32,
                            active_colors[index],
                            inactive_colors[index],
                            intensities[index],
                        );
                    }

                    if let Some(color) = color {
                        red += color.red as u32;
                        green += color.green as u32;
                        blue += color.blue as u32;
                        alpha += 255;
                    }
                }
            }

            let average = |sum: u32| (sum + sample_count / 2) / sample_count;
            pixels.push(
                average(blue)
                    | (average(green) << 8)
                    | (average(red) << 16)
                    | (average(alpha) << 24),
            );
        }
    }

    pixels
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

fn capsule_sample(x: f32, y: f32, width: f32, height: f32, inset: f32) -> Option<Rgb> {
    let mut color = inside_capsule(x, y, 0.0, 0.0, width, height).then_some(rgb(24, 25, 29));
    if inside_capsule(x, y, inset, inset, width - inset, height - inset) {
        color = Some(rgb(13, 15, 18));
    }
    color
}

#[allow(clippy::too_many_arguments)]
fn draw_lamp_sample(
    color: &mut Option<Rgb>,
    x: f32,
    y: f32,
    cx: f32,
    cy: f32,
    radius: f32,
    active: Rgb,
    inactive: Rgb,
    intensity: Option<f32>,
) {
    if let Some(intensity) = intensity {
        for (extra, alpha) in [(10, 0.16), (7, 0.24), (4, 0.34)] {
            let extra = extra as f32 * radius / 8.0;
            paint_circle(
                color,
                x,
                y,
                cx,
                cy,
                radius + extra,
                mix(rgb(13, 15, 18), active, alpha * intensity),
            );
        }
        paint_circle(
            color,
            x,
            y,
            cx,
            cy,
            radius + 1.0,
            mix(rgb(190, 195, 200), active, intensity),
        );
        paint_circle(
            color,
            x,
            y,
            cx,
            cy,
            radius,
            mix(inactive, active, intensity),
        );
        let highlight = (radius / 3.0).max(2.0);
        paint_circle(
            color,
            x,
            y,
            cx - radius / 3.0,
            cy - radius / 3.0,
            highlight,
            mix(active, rgb(255, 255, 255), 0.62 * intensity),
        );
    } else {
        paint_circle(color, x, y, cx, cy, radius + 1.0, rgb(61, 64, 70));
        paint_circle(color, x, y, cx, cy, radius, inactive);
    }
}

fn paint_circle(
    current: &mut Option<Rgb>,
    x: f32,
    y: f32,
    cx: f32,
    cy: f32,
    radius: f32,
    color: Rgb,
) {
    let dx = x - cx;
    let dy = y - cy;
    if dx * dx + dy * dy <= radius * radius {
        *current = Some(color);
    }
}

fn inside_capsule(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let radius = (bottom - top) / 2.0;
    let center_x = x.clamp(left + radius, right - radius);
    let center_y = (top + bottom) / 2.0;
    let dx = x - center_x;
    let dy = y - center_y;
    dx * dx + dy * dy <= radius * radius
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = TRAY_MESSAGE;
    data.hIcon = load_app_icon(GetModuleHandleW(null()));
    copy_wide(&mut data.szTip, "Harbor Light：空闲");
    let _ = Shell_NotifyIconW(NIM_ADD, &data);
}

unsafe fn load_app_icon(instance: HINSTANCE) -> HICON {
    // winresource embeds the application icon with numeric resource id 1.
    #[allow(clippy::manual_dangling_ptr)]
    let icon = LoadIconW(instance, 1usize as *const u16);
    if icon.is_null() {
        LoadIconW(null_mut(), IDI_APPLICATION)
    } else {
        icon
    }
}

unsafe fn update_tray_tip(hwnd: HWND, state: DisplayState) {
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_TIP;
    copy_wide(
        &mut data.szTip,
        &format!("Harbor Light：{}", state.label_zh()),
    );
    let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let data = tray_data(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
}

unsafe fn show_tray_menu(hwnd: HWND, state: Option<DisplayState>) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let status = wide(&format!(
        "状态：{}",
        state.unwrap_or(DisplayState::IDLE).label_zh()
    ));
    let reinstall = wide("重新安装 Hooks");
    let quit = wide("退出 Harbor Light");
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, status.as_ptr());
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, null());
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_REINSTALL_HOOKS as usize,
        reinstall.as_ptr(),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT as usize, quit.as_ptr());
    let mut point: POINT = zeroed();
    let _ = GetCursorPos(&mut point);
    let _ = SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD,
        point.x,
        point.y,
        0,
        hwnd,
        null(),
    );
    let _ = DestroyMenu(menu);
    match command {
        MENU_REINSTALL_HOOKS => {
            let paths = Paths::current();
            if let Ok(exe) = crate::install::current_executable() {
                match crate::install::install_provider_hooks(&paths, &exe) {
                    Ok(()) => append_log(&paths, "Windows tray reinstalled provider hooks"),
                    Err(error) => {
                        append_log(&paths, &format!("Windows hook reinstall failed: {error:#}"))
                    }
                }
            }
        }
        MENU_QUIT => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ID;
    data
}

fn initial_position(paths: &Paths, width: i32, height: i32) -> SavedPosition {
    if let Ok(raw) = fs::read_to_string(paths.windows_position_file()) {
        if let Ok(saved) = serde_json::from_str::<SavedPosition>(&raw) {
            if point_is_on_monitor(saved.x + width / 2, saved.y + height / 2) {
                return saved;
            }
        }
    }
    default_position(width, height)
}

fn default_position(width: i32, _height: i32) -> SavedPosition {
    let mut work: RECT = unsafe { zeroed() };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            (&mut work as *mut RECT).cast::<c_void>(),
            0,
        )
    };
    if ok == 0 {
        return SavedPosition { x: 200, y: 80 };
    }
    SavedPosition {
        x: work.right - width - EDGE_MARGIN,
        y: work.top + EDGE_MARGIN,
    }
}

fn ensure_visible(hwnd: HWND, paths: &Paths) {
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return;
    }
    let center_x = (rect.left + rect.right) / 2;
    let center_y = (rect.top + rect.bottom) / 2;
    if point_is_on_monitor(center_x, center_y) {
        return;
    }
    let position = default_position(rect.right - rect.left, rect.bottom - rect.top);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            null_mut(),
            position.x,
            position.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    write_position(&paths.windows_position_file(), position);
}

fn save_position(hwnd: HWND, paths: &Paths) {
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
        write_position(
            &paths.windows_position_file(),
            SavedPosition {
                x: rect.left,
                y: rect.top,
            },
        );
    }
}

fn write_position(path: &Path, position: SavedPosition) {
    if let Ok(body) = serde_json::to_vec_pretty(&position) {
        let temp = path.with_extension("json.tmp");
        if fs::write(&temp, body).is_ok() {
            let _ = fs::rename(temp, path);
        }
    }
}

fn point_is_on_monitor(x: i32, y: i32) -> bool {
    !unsafe { MonitorFromPoint(POINT { x, y }, 0) }.is_null()
}

fn running_process_names() -> BTreeSet<String> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return BTreeSet::new();
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut names = BTreeSet::new();
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(entry.szExeFile.len());
                names
                    .insert(String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase());
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        names
    }
}

fn provider_is_running(provider: Provider, processes: &BTreeSet<String>) -> bool {
    provider
        .windows_process_names()
        .iter()
        .any(|name| processes.contains(*name))
}

fn scaled_size(dpi: u32) -> (i32, i32) {
    (scale(LOGICAL_WIDTH, dpi), scale(LOGICAL_HEIGHT, dpi))
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

const fn rgb(red: u8, green: u8, blue: u8) -> Rgb {
    Rgb { red, green, blue }
}

fn mix(from: Rgb, to: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| {
        let a = a as f32;
        let b = b as f32;
        (a + (b - a) * amount).round() as u8
    };
    rgb(
        channel(from.red, to.red),
        channel(from.green, to.green),
        channel(from.blue, to.blue),
    )
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wide<const N: usize>(target: &mut [u16; N], text: &str) {
    target.fill(0);
    for (dest, value) in target
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(text.encode_utf16())
    {
        *dest = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_scaling_keeps_logical_size() {
        assert_eq!(scaled_size(96), (128, 38));
        assert_eq!(scaled_size(144), (192, 57));
        assert_eq!(scaled_size(192), (256, 76));
    }

    #[test]
    fn color_mix_reaches_both_ends() {
        let black = rgb(0, 0, 0);
        let green = rgb(56, 255, 107);
        assert_eq!(mix(black, green, 0.0), black);
        assert_eq!(mix(black, green, 1.0), green);
    }

    #[test]
    fn supersampled_capsule_has_translucent_edge_pixels() {
        let state = AppState {
            paths: Paths::current(),
            displayed: DisplayState::IDLE,
            tick: 0,
            phase: 0.0,
            dpi: 96,
            provider_running: BTreeMap::new(),
            provider_resets: ProviderResets::new(),
        };
        let pixels = render_pixels(LOGICAL_WIDTH, LOGICAL_HEIGHT, &state);
        let alpha_at = |x: i32, y: i32| (pixels[(y * LOGICAL_WIDTH + x) as usize] >> 24) as u8;

        assert_eq!(alpha_at(0, 0), 0);
        assert_eq!(alpha_at(LOGICAL_WIDTH / 2, LOGICAL_HEIGHT / 2), 255);
        assert!((1..255).contains(&alpha_at(5, 5)));
    }
}
