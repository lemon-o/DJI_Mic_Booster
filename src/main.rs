#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod autostart;
mod config;

use audio::{AudioController, AudioEngine};
use config::AppConfig;
use std::path::PathBuf;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, SetProcessWorkingSetSize, WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TRACKMOUSEEVENT, TME_LEAVE,
};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

// 自定义消息与常量
const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_SLIDER_CHANGED: u32 = WM_APP + 2;
const WM_APP_WAKEUP: u32 = WM_APP + 3;
const WM_APP_CHECK_DRIVER: u32 = WM_APP + 5;
const WM_APP_DEVICE_REFRESH: u32 = WM_APP + 6;
const WM_DEVICECHANGE: u32 = 0x0219;

const TBM_SETPOS: u32 = 0x0405;
const TBM_GETPOS: u32 = 0x0400;

const SS_OWNERDRAW_U: u32 = 0x0000000D;
const BS_OWNERDRAW_U: u32 = 0x0000000B;

// 控件 ID
const ID_IN_COMBO: isize = 101;
const ID_REFRESH_BTN: isize = 103;
const ID_SLIDER: isize = 104;
const ID_PRESET_6: isize = 105;
const ID_PRESET_12: isize = 106;
const ID_PRESET_14: isize = 107;
const ID_PRESET_18: isize = 108;
const ID_PRESET_22: isize = 109;
const ID_METER_CUSTOM: isize = 111;
const ID_CHECK_AUTOSTART: isize = 112;
const ID_CHECK_SILENT: isize = 113;

// 菜单 ID
const ID_TRAY_SHOW: usize = 201;
const ID_TRAY_MUTE: usize = 202;
const ID_TRAY_AUTOSTART: usize = 203;
const ID_TRAY_SILENT: usize = 204;
const ID_TRAY_EXIT: usize = 205;
const ID_MENU_DEVICE_BASE: usize = 300;

const TIMER_METER: usize = 1;
const TIMER_HOTPLUG: usize = 2;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

struct HoverControlData {
    old_proc: WNDPROC,
    is_hovered: bool,
}

unsafe extern "system" fn hover_button_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut HoverControlData;
    if data_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let data = &mut *data_ptr;
    let old_proc = data.old_proc;

    match msg {
        WM_MOUSEMOVE => {
            if !data.is_hovered {
                data.is_hovered = true;
                let mut tme: TRACKMOUSEEVENT = std::mem::zeroed();
                tme.cbSize = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
                tme.dwFlags = TME_LEAVE;
                tme.hwndTrack = hwnd;
                TrackMouseEvent(&mut tme);
                InvalidateRect(hwnd, null_mut(), 0);
            }
        }
        WM_MOUSELEAVE => {
            data.is_hovered = false;
            InvalidateRect(hwnd, null_mut(), 0);
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(0, IDC_HAND));
            return 1;
        }
        WM_DESTROY => {
            let _ = Box::from_raw(data_ptr);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return CallWindowProcW(old_proc, hwnd, msg, wparam, lparam);
        }
        _ => {}
    }

    CallWindowProcW(old_proc, hwnd, msg, wparam, lparam)
}

unsafe fn attach_hover_effect(hwnd: HWND) {
    let old_proc_raw = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
    let old_proc: WNDPROC = std::mem::transmute(old_proc_raw);
    let data = Box::new(HoverControlData {
        old_proc,
        is_hovered: false,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
    SetWindowLongPtrW(
        hwnd,
        GWLP_WNDPROC,
        hover_button_subclass_proc as *const () as usize as isize,
    );
}

unsafe fn is_control_hovered(hwnd: HWND) -> bool {
    let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut HoverControlData;
    if !data_ptr.is_null() {
        (*data_ptr).is_hovered
    } else {
        false
    }
}

struct SliderData {
    pos: f32, // 0.0 ~ 30.0
    is_dragging: bool,
    parent: HWND,
}

unsafe extern "system" fn slider_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SliderData;

    match msg {
        WM_CREATE => {
            let cs = lparam as *const CREATESTRUCTW;
            let parent = (*cs).hwndParent;
            let data = Box::new(SliderData { pos: 14.0, is_dragging: false, parent });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
            0
        }
        TBM_SETPOS => {
            if !data_ptr.is_null() {
                let data = &mut *data_ptr;
                data.pos = ((lparam as f32) / 10.0).clamp(0.0, 30.0);
                InvalidateRect(hwnd, null_mut(), 0);
            }
            0
        }
        TBM_GETPOS => {
            if !data_ptr.is_null() {
                let data = &*data_ptr;
                return (data.pos * 10.0).round() as LRESULT;
            }
            0
        }
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            if !data_ptr.is_null() {
                let data = &mut *data_ptr;
                data.is_dragging = true;
                update_slider_pos(hwnd, data, lparam);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !data_ptr.is_null() {
                let data = &mut *data_ptr;
                if data.is_dragging {
                    update_slider_pos(hwnd, data, lparam);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if !data_ptr.is_null() {
                let data = &mut *data_ptr;
                if data.is_dragging {
                    data.is_dragging = false;
                    ReleaseCapture();
                    update_slider_pos(hwnd, data, lparam);
                }
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !data_ptr.is_null() {
                let data = &*data_ptr;
                draw_flat_slider(hwnd, hdc, data.pos);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DESTROY => {
            if !data_ptr.is_null() {
                let _ = Box::from_raw(data_ptr);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn update_slider_pos(hwnd: HWND, data: &mut SliderData, lparam: LPARAM) {
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rc);
    let x = (lparam & 0xFFFF) as i16 as f32;
    let track_start = 10.0;
    let track_end = (rc.right - 10) as f32;
    let track_w = (track_end - track_start).max(1.0);

    let ratio = ((x - track_start) / track_w).clamp(0.0, 1.0);
    let val = (ratio * 30.0 * 2.0).round() / 2.0; // 0.5 dB 步进
    data.pos = val;

    InvalidateRect(hwnd, null_mut(), 0);
    SendMessageW(data.parent, WM_APP_SLIDER_CHANGED, (val * 10.0).round() as WPARAM, 0);
}

unsafe fn draw_flat_slider(hwnd: HWND, hdc: HDC, val: f32) {
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rc);

    // 背景纯白 (融入 Card 2)
    let brush_bg = CreateSolidBrush(rgb(255, 255, 255));
    FillRect(hdc, &rc, brush_bg);
    DeleteObject(brush_bg);

    let track_start = 10;
    let track_end = rc.right - 10;
    let track_w = (track_end - track_start) as f32;
    let track_y = (rc.bottom - 4) / 2;

    // 1. 底槽 (#E2E8F0)
    let brush_track_bg = CreateSolidBrush(rgb(226, 232, 240));
    let pen_track_bg = CreatePen(PS_SOLID as i32, 1, rgb(226, 232, 240));
    let old_b = SelectObject(hdc, brush_track_bg);
    let old_p = SelectObject(hdc, pen_track_bg);
    RoundRect(hdc, track_start, track_y, track_end, track_y + 4, 3, 3);

    // 2. 激活色带 (#2563EB)
    let ratio = (val / 30.0).clamp(0.0, 1.0);
    let thumb_x = track_start + (track_w * ratio).round() as i32;

    let brush_blue = CreateSolidBrush(rgb(37, 99, 235));
    let pen_blue = CreatePen(PS_SOLID as i32, 1, rgb(37, 99, 235));
    SelectObject(hdc, brush_blue);
    SelectObject(hdc, pen_blue);

    if thumb_x > track_start {
        RoundRect(hdc, track_start, track_y, thumb_x, track_y + 4, 3, 3);
    }

    // 3. 游标圆球 (白底 + 2px 蓝圈)
    let thumb_radius = 7;
    let thumb_cy = rc.bottom / 2;

    let pen_thumb = CreatePen(PS_SOLID as i32, 2, rgb(37, 99, 235));
    let brush_thumb = CreateSolidBrush(rgb(255, 255, 255));
    SelectObject(hdc, pen_thumb);
    SelectObject(hdc, brush_thumb);

    Ellipse(
        hdc,
        thumb_x - thumb_radius,
        thumb_cy - thumb_radius,
        thumb_x + thumb_radius,
        thumb_cy + thumb_radius,
    );

    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(brush_track_bg);
    DeleteObject(pen_track_bg);
    DeleteObject(brush_blue);
    DeleteObject(pen_blue);
    DeleteObject(brush_thumb);
    DeleteObject(pen_thumb);
}

struct AppState {
    config: AppConfig,
    controller: AudioController,
    engine: Option<AudioEngine>,

    input_devices: Vec<String>,
    output_devices: Vec<String>,
    selected_input: String,
    selected_output: String,

    gain_db: f32,
    is_muted: bool,
    display_peak: f32,

    hwnd_main: HWND,
    hwnd_in_combo: HWND,
    hwnd_refresh_btn: HWND,
    hwnd_gain_label: HWND,
    hwnd_slider: HWND,
    hwnd_meter: HWND,
    hwnd_chk_autostart: HWND,
    hwnd_chk_silent: HWND,

    font_bold: HFONT,
    font_normal: HFONT,
    font_small: HFONT,

    brush_bg: HBRUSH,
    brush_card: HBRUSH,
    nid: NOTIFYICONDATAW,
    tray_added: bool,
    msg_taskbar_created: u32,
    driver_prompted: bool,
    timer_ticks: u32,
}

impl AppState {
    fn new() -> Self {
        let config = AppConfig::load();
        let selected_input = config.input_device.clone();
        let gain_db = config.get_gain_for_device(&selected_input);
        let controller = AudioController::new();
        controller.set_gain_db(gain_db);

        if config.auto_start && !autostart::is_auto_start_registered() {
            autostart::set_auto_start(true, config.silent_start);
        }

        let msg_taskbar_created = unsafe {
            RegisterWindowMessageW(wide_str("TaskbarCreated").as_ptr())
        };

        Self {
            gain_db,
            selected_input,
            selected_output: config.output_device.clone(),
            config,
            controller,
            engine: None,
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            is_muted: false,
            display_peak: 0.0,
            hwnd_main: 0,
            hwnd_in_combo: 0,
            hwnd_refresh_btn: 0,
            hwnd_gain_label: 0,
            hwnd_slider: 0,
            hwnd_meter: 0,
            hwnd_chk_autostart: 0,
            hwnd_chk_silent: 0,
            font_bold: 0,
            font_normal: 0,
            font_small: 0,
            brush_bg: 0,
            brush_card: 0,
            nid: unsafe { std::mem::zeroed() },
            tray_added: false,
            msg_taskbar_created,
            driver_prompted: false,
            timer_ticks: 0,
        }
    }

    fn has_virtual_cable(&self) -> bool {
        self.output_devices.iter().any(|dev| {
            let lower = dev.to_lowercase();
            lower.contains("cable input") && !lower.contains("16ch")
        })
    }

    unsafe fn check_and_prompt_driver(&mut self, hwnd: HWND, is_manual: bool) {
        if self.has_virtual_cable() {
            return;
        }

        if self.driver_prompted && !is_manual {
            return;
        }
        self.driver_prompted = true;

        let prompt = "检测到系统尚未安装 VB-Audio 虚拟音频驱动（CABLE Input/Output）。\n\n本软件需要该驱动将放大后的人声实时传输给腾讯会议、OBS、微信等通话软件使用。\n\n是否立即启动驱动安装程序？";
        let title = "USB麦克风增益助手 - 驱动就绪提示";
        let ret = MessageBoxW(
            hwnd,
            wide_str(prompt).as_ptr(),
            wide_str(title).as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        );

        if ret == IDYES {
            self.launch_vbcable_installer(hwnd);
        }
    }

    unsafe fn launch_vbcable_installer(&self, hwnd: HWND) {
        if let Some(installer_path) = find_vbcable_installer() {
            let dir = installer_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let exe_w = wide_str(&installer_path.to_string_lossy());
            let dir_w = wide_str(&dir.to_string_lossy());

            let verb_w = wide_str("runas");
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.hwnd = hwnd;
            info.lpVerb = verb_w.as_ptr();
            info.lpFile = exe_w.as_ptr();
            info.lpDirectory = dir_w.as_ptr();
            info.nShow = SW_SHOWNORMAL;

            if ShellExecuteExW(&mut info) != 0 && info.hProcess != 0 {
                let h_proc = info.hProcess;
                let hwnd_usize = hwnd as usize;

                std::thread::spawn(move || {
                    WaitForSingleObject(h_proc, 0xFFFFFFFF);
                    CloseHandle(h_proc);
                    // 等待 Windows 音频子系统重新加载虚拟声卡设备
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    PostMessageW(hwnd_usize as HWND, WM_APP_DEVICE_REFRESH, 0, 0);
                });
            } else {
                let err = GetLastError();
                if err != 1223 {
                    let err_msg = format!("启动安装程序失败，错误代码: {}", err);
                    MessageBoxW(
                        hwnd,
                        wide_str(&err_msg).as_ptr(),
                        wide_str("错误").as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        } else {
            let not_found = "未在程序目录下找到 VBCABLE_Driver 驱动安装文件夹。\n\n是否打开 VB-Audio 官网下载虚拟声卡驱动？";
            if MessageBoxW(
                hwnd,
                wide_str(not_found).as_ptr(),
                wide_str("未找到驱动文件").as_ptr(),
                MB_YESNO | MB_ICONINFORMATION,
            ) == IDYES
            {
                ShellExecuteW(
                    0,
                    wide_str("open").as_ptr(),
                    wide_str("https://vb-audio.com/Cable/").as_ptr(),
                    null_mut(),
                    null_mut(),
                    SW_SHOWNORMAL,
                );
            }
        }
    }

    unsafe fn refresh_device_list(&mut self) {
        self.input_devices = audio::list_input_devices();
        self.output_devices = audio::list_output_devices();

        let prev_selected = self.selected_input.clone();
        if self.selected_input.is_empty() || !self.input_devices.contains(&self.selected_input) {
            self.selected_input = self.config
                .get_fallback_device(&self.input_devices)
                .or_else(|| {
                    self.input_devices.iter().find(|dev| {
                        let lower = dev.to_lowercase();
                        lower.contains("wireless mic") || lower.contains("dji") || lower.contains("usb")
                    }).cloned()
                })
                .or_else(|| self.input_devices.first().cloned())
                .unwrap_or_default();
        }

        // 若输入设备发生变动，恢复该设备独立保存的增益值并更新 UI
        if self.selected_input != prev_selected {
            self.gain_db = self.config.get_gain_for_device(&self.selected_input);
            self.controller.set_gain_db(self.gain_db);
            self.update_gain_ui();
        }

        if !self.selected_input.is_empty() {
            self.config.push_device_history(&self.selected_input);
        }

        // 后台全自动静默寻找并绑定 CABLE Input 虚拟声卡
        let mut found_cable = None;
        for dev in &self.output_devices {
            let lower = dev.to_lowercase();
            if lower.contains("cable input") && !lower.contains("16ch") {
                found_cable = Some(dev.clone());
                break;
            }
        }
        self.selected_output = found_cable.unwrap_or_default();

        self.save_settings();
        self.start_or_restart_stream();
        InvalidateRect(self.hwnd_in_combo, null_mut(), 1);
    }

    unsafe fn check_device_changes(&mut self, hwnd: HWND) {
        let current_inputs = audio::list_input_devices();
        let current_outputs = audio::list_output_devices();

        let had_stream_error = self.controller.get_last_error().is_some();
        if had_stream_error {
            self.controller.clear_error();
        }

        // 1. 检查是否有新设备插入
        let newly_inserted: Vec<String> = current_inputs
            .iter()
            .filter(|d| !self.input_devices.contains(d))
            .cloned()
            .collect();

        // 2. 检查当前活跃设备是否丢失（物理断开或 WASAPI 回调报告设备异常）
        let active_device_lost = !self.selected_input.is_empty()
            && (!current_inputs.contains(&self.selected_input) || had_stream_error);

        let mut need_stream_restart = false;
        let mut device_switched = false;

        if !newly_inserted.is_empty() {
            // 场景一：新设备插入 -> 自动切换到新插入的麦克风
            let new_mic = newly_inserted[0].clone();

            // 若已有当前设备，将当前设备推入历史记录备用
            if !self.selected_input.is_empty() {
                self.config.push_device_history(&self.selected_input);
            }

            self.selected_input = new_mic.clone();
            self.config.push_device_history(&new_mic);

            // 恢复新设备专属增益值并更新 UI
            self.gain_db = self.config.get_gain_for_device(&self.selected_input);
            self.controller.set_gain_db(self.gain_db);
            self.update_gain_ui();

            need_stream_restart = true;
            device_switched = true;
        } else if active_device_lost {
            // 场景二：当前麦克风拔出或流故障 -> 自动回退到上一个历史设备
            self.engine = None;

            // 候选设备：排除已断开/故障的当前设备
            let candidates: Vec<String> = current_inputs
                .iter()
                .filter(|d| *d != &self.selected_input)
                .cloned()
                .collect();

            if let Some(fallback_mic) = self.config.get_fallback_device(&candidates) {
                self.selected_input = fallback_mic;
                self.gain_db = self.config.get_gain_for_device(&self.selected_input);
                self.controller.set_gain_db(self.gain_db);
                self.update_gain_ui();
                need_stream_restart = true;
            } else {
                self.selected_input.clear();
            }
            device_switched = true;
        }

        // 3. 检查输出设备（CABLE Input 虚拟声卡）
        let mut found_cable = None;
        for dev in &current_outputs {
            let lower = dev.to_lowercase();
            if lower.contains("cable input") && !lower.contains("16ch") {
                found_cable = Some(dev.clone());
                break;
            }
        }
        let target_output = found_cable.unwrap_or_default();
        if self.selected_output != target_output {
            self.selected_output = target_output;
            need_stream_restart = true;
        }

        let lists_changed = self.input_devices != current_inputs || self.output_devices != current_outputs;
        self.input_devices = current_inputs;
        self.output_devices = current_outputs;

        if device_switched || need_stream_restart {
            self.save_settings();
            self.start_or_restart_stream();
        }

        if device_switched || lists_changed {
            InvalidateRect(self.hwnd_in_combo, null_mut(), 1);
            RedrawWindow(hwnd, null_mut(), 0, RDW_INVALIDATE | RDW_ALLCHILDREN);
        }
    }

    unsafe fn update_gain_ui(&self) {
        let linear_multiplier = 10.0f32.powf(self.gain_db / 20.0);
        let text = format!("前级增益量: +{:.1} dB  ({:.1}x 放大)", self.gain_db, linear_multiplier);
        SetWindowTextW(self.hwnd_gain_label, wide_str(&text).as_ptr());
        let pos = (self.gain_db * 10.0).round() as isize;
        SendMessageW(self.hwnd_slider, TBM_SETPOS, 0, pos);
        InvalidateRect(self.hwnd_slider, null_mut(), 0);
        RedrawWindow(self.hwnd_main, null_mut(), 0, RDW_INVALIDATE | RDW_ALLCHILDREN);
    }

    fn save_settings(&mut self) {
        self.config.set_gain_for_device(&self.selected_input, self.gain_db);
        self.config.input_device = self.selected_input.clone();
        self.config.output_device = self.selected_output.clone();
        self.config.save();
    }

    fn start_or_restart_stream(&mut self) {
        if self.selected_input.is_empty() || self.selected_output.is_empty() {
            self.engine = None;
            return;
        }

        self.engine = None;
        if let Ok(eng) = AudioEngine::start(&self.selected_input, &self.selected_output, &self.controller) {
            self.engine = Some(eng);
        }
    }

    fn toggle_mute(&mut self) {
        self.is_muted = !self.is_muted;
        self.controller.set_muted(self.is_muted);
    }

    unsafe fn set_autostart(&mut self, enable: bool) {
        self.config.auto_start = enable;
        autostart::set_auto_start(enable, self.config.silent_start);
        self.config.save();
        InvalidateRect(self.hwnd_chk_autostart, null_mut(), 1);
    }

    unsafe fn set_silent_start(&mut self, enable: bool) {
        self.config.silent_start = enable;
        if self.config.auto_start {
            autostart::set_auto_start(true, enable);
        }
        self.config.save();
        InvalidateRect(self.hwnd_chk_silent, null_mut(), 1);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;

    match msg {
        WM_CREATE => {
            let cs = lparam as *const CREATESTRUCTW;
            let state_box = (*cs).lpCreateParams as *mut AppState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_box as isize);
            let state = &mut *state_box;
            state.hwnd_main = hwnd;

            let inst = (*cs).hInstance;

            // 设置 Windows 标题栏为浅色原生样式
            let dark_mode_val: i32 = 0;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                &dark_mode_val as *const _ as *const _,
                std::mem::size_of::<i32>() as u32,
            );

            // 白色现代扁平画刷与字体
            state.brush_bg = CreateSolidBrush(rgb(248, 249, 250));   // #F8F9FA
            state.brush_card = CreateSolidBrush(rgb(255, 255, 255)); // #FFFFFF

            state.font_bold = create_font("Microsoft YaHei UI", 13, true);
            state.font_normal = create_font("Microsoft YaHei UI", 12, false);
            state.font_small = create_font("Microsoft YaHei UI", 11, false);

            // 1. 麦克风输入卡片 (顶部卡片，高 78)
            let h_lbl_dev = CreateWindowExW(
                0, wide_str("STATIC").as_ptr(), wide_str("麦克风输入 (Input)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                32, 24, 200, 18, hwnd, 0, inst, null_mut(),
            );
            SendMessageW(h_lbl_dev, WM_SETFONT, state.font_bold as WPARAM, 1);

            state.hwnd_refresh_btn = CreateWindowExW(
                0, wide_str("BUTTON").as_ptr(), wide_str("刷新").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW_U,
                350, 20, 58, 24, hwnd, ID_REFRESH_BTN, inst, null_mut(),
            );
            attach_hover_effect(state.hwnd_refresh_btn);

            // 扁平化下拉框按钮
            state.hwnd_in_combo = CreateWindowExW(
                0, wide_str("BUTTON").as_ptr(), wide_str("").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW_U,
                32, 48, 376, 30, hwnd, ID_IN_COMBO, inst, null_mut(),
            );
            attach_hover_effect(state.hwnd_in_combo);

            // 2. 前级增益与电平卡片 (中间卡片，高 144)
            let h_lbl_gain_sec = CreateWindowExW(
                0, wide_str("STATIC").as_ptr(), wide_str("前级增益调节").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                32, 114, 140, 18, hwnd, 0, inst, null_mut(),
            );
            SendMessageW(h_lbl_gain_sec, WM_SETFONT, state.font_bold as WPARAM, 1);

            state.hwnd_gain_label = CreateWindowExW(
                0, wide_str("STATIC").as_ptr(), wide_str("前级增益量: +14.0 dB  (5.0x 放大)").as_ptr(),
                WS_CHILD | WS_VISIBLE | (0x00000002), // SS_RIGHT
                176, 114, 232, 18, hwnd, 0, inst, null_mut(),
            );
            SendMessageW(state.hwnd_gain_label, WM_SETFONT, state.font_bold as WPARAM, 1);

            // 现代扁平滑块控件
            state.hwnd_slider = CreateWindowExW(
                0, wide_str("USBMicFlatSliderClass").as_ptr(), null_mut(),
                WS_CHILD | WS_VISIBLE,
                32, 136, 376, 24, hwnd, ID_SLIDER, inst, null_mut(),
            );
            SendMessageW(state.hwnd_slider, TBM_SETPOS, 0, (state.gain_db * 10.0).round() as isize);

            // 预设按钮 (5个按钮均分)
            let presets = [(ID_PRESET_6, "+6dB"), (ID_PRESET_12, "+12dB"), (ID_PRESET_14, "+14dB"), (ID_PRESET_18, "+18dB"), (ID_PRESET_22, "+22dB")];
            for (i, (id, text)) in presets.iter().enumerate() {
                let x = 32 + (i as i32) * 77;
                let h_btn = CreateWindowExW(
                    0, wide_str("BUTTON").as_ptr(), wide_str(text).as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_OWNERDRAW_U,
                    x, 168, 68, 26, hwnd, *id, inst, null_mut(),
                );
                SendMessageW(h_btn, WM_SETFONT, state.font_small as WPARAM, 1);
                attach_hover_effect(h_btn);
            }

            // 实时输出电平表
            let h_lbl_meter = CreateWindowExW(
                0, wide_str("STATIC").as_ptr(), wide_str("实时输出电平 (VU Meter)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                32, 204, 376, 16, hwnd, 0, inst, null_mut(),
            );
            SendMessageW(h_lbl_meter, WM_SETFONT, state.font_small as WPARAM, 1);

            state.hwnd_meter = CreateWindowExW(
                0, wide_str("STATIC").as_ptr(), null_mut(),
                WS_CHILD | WS_VISIBLE | SS_OWNERDRAW_U,
                32, 222, 376, 14, hwnd, ID_METER_CUSTOM, inst, null_mut(),
            );

            // 3. 底部选项复选框 (扁平自绘)
            state.hwnd_chk_autostart = CreateWindowExW(
                0, wide_str("BUTTON").as_ptr(), wide_str("").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW_U,
                32, 256, 120, 22, hwnd, ID_CHECK_AUTOSTART, inst, null_mut(),
            );
            attach_hover_effect(state.hwnd_chk_autostart);

            state.hwnd_chk_silent = CreateWindowExW(
                0, wide_str("BUTTON").as_ptr(), wide_str("").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW_U,
                168, 256, 240, 22, hwnd, ID_CHECK_SILENT, inst, null_mut(),
            );
            attach_hover_effect(state.hwnd_chk_silent);

            // 加载嵌入的程序图标 (ID 1)
            let h_icon = LoadIconW(inst, 1 as *const u16);
            let final_icon = if h_icon != 0 { h_icon } else { LoadIconW(0, IDI_APPLICATION) };

            SendMessageW(hwnd, WM_SETICON, 1, final_icon as isize);
            SendMessageW(hwnd, WM_SETICON, 0, final_icon as isize);

            // 配置系统托盘图标
            state.nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            state.nid.hWnd = hwnd;
            state.nid.uID = 1001;
            state.nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            state.nid.uCallbackMessage = WM_APP_TRAY;
            state.nid.hIcon = final_icon;
            let tip = wide_str("USB麦克风增益助手");
            let copy_len = tip.len().min(state.nid.szTip.len() - 1);
            state.nid.szTip[..copy_len].copy_from_slice(&tip[..copy_len]);
            if Shell_NotifyIconW(NIM_ADD, &state.nid) != 0 {
                state.tray_added = true;
            }

            state.refresh_device_list();
            state.update_gain_ui();

            SetTimer(hwnd, TIMER_METER, 33, None);
            PostMessageW(hwnd, WM_APP_CHECK_DRIVER, 0, 0);
            0
        }

        WM_APP_SLIDER_CHANGED => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.gain_db = (wparam as f32) / 10.0;
                state.controller.set_gain_db(state.gain_db);
                let linear_multiplier = 10.0f32.powf(state.gain_db / 20.0);
                let text = format!("前级增益量: +{:.1} dB  ({:.1}x 放大)", state.gain_db, linear_multiplier);
                SetWindowTextW(state.hwnd_gain_label, wide_str(&text).as_ptr());
                state.save_settings();
                InvalidateRect(hwnd, null_mut(), 0);
            }
            0
        }

        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);

            if !state_ptr.is_null() {
                let state = &*state_ptr;

                let pen_card_border = CreatePen(PS_SOLID as i32, 1, rgb(226, 232, 240)); // #E2E8F0
                let old_pen = SelectObject(hdc, pen_card_border);
                let old_brush = SelectObject(hdc, state.brush_card);

                // 卡片 1 (麦克风输入，高 78)
                RoundRect(hdc, 18, 14, 422, 92, 10, 10);
                // 卡片 2 (增益与电平，高 144)
                RoundRect(hdc, 18, 102, 422, 246, 10, 10);

                SelectObject(hdc, old_brush);
                SelectObject(hdc, old_pen);
                DeleteObject(pen_card_border);
            }

            EndPaint(hwnd, &ps);
            0
        }

        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = wparam as HDC;
            SetBkMode(hdc, TRANSPARENT as i32);
            let hwnd_ctl = lparam as HWND;

            if !state_ptr.is_null() {
                let state = &*state_ptr;
                SetTextColor(hdc, rgb(30, 41, 59));

                if hwnd_ctl == state.hwnd_chk_autostart || hwnd_ctl == state.hwnd_chk_silent {
                    return state.brush_bg as LRESULT;
                }

                return state.brush_card as LRESULT;
            }
            GetStockObject(WHITE_BRUSH as i32) as LRESULT
        }

        WM_DRAWITEM => {
            let dis = lparam as *const DRAWITEMSTRUCT;
            if dis.is_null() || state_ptr.is_null() {
                return 0;
            }
            let state = &*state_ptr;
            let id = (*dis).CtlID as isize;
            let hdc = (*dis).hDC;
            let rc = (*dis).rcItem;

            if id == ID_CHECK_AUTOSTART || id == ID_CHECK_SILENT {
                let is_hovered = is_control_hovered((*dis).hwndItem);
                let is_checked = if id == ID_CHECK_AUTOSTART {
                    state.config.auto_start
                } else {
                    state.config.silent_start
                };
                let text = if id == ID_CHECK_AUTOSTART {
                    "开机自启动"
                } else {
                    "静默启动 (仅驻留系统托盘)"
                };

                // 背景用窗体底色 #F8F9FA
                FillRect(hdc, &rc, state.brush_bg);

                // 扁平 16x16 方框
                let box_top = (rc.bottom - rc.top - 16) / 2;
                let box_left = 2;
                let box_rc = RECT { left: box_left, top: box_top, right: box_left + 16, bottom: box_top + 16 };

                if is_checked {
                    let box_bg = if is_hovered { rgb(29, 78, 216) } else { rgb(37, 99, 235) };
                    let brush_box = CreateSolidBrush(box_bg);
                    let pen_box = CreatePen(PS_SOLID as i32, 1, box_bg);
                    let old_b = SelectObject(hdc, brush_box);
                    let old_p = SelectObject(hdc, pen_box);
                    RoundRect(hdc, box_rc.left, box_rc.top, box_rc.right, box_rc.bottom, 4, 4);

                    // 纯白勾选符号 ✓
                    SetBkMode(hdc, TRANSPARENT as i32);
                    SetTextColor(hdc, rgb(255, 255, 255));
                    let old_font = SelectObject(hdc, state.font_small);
                    let mut check_rc = box_rc;
                    let check_str = wide_str("✓");
                    DrawTextW(hdc, check_str.as_ptr(), -1, &mut check_rc, DT_CENTER as u32 | DT_VCENTER as u32 | DT_SINGLELINE as u32);
                    SelectObject(hdc, old_font);

                    SelectObject(hdc, old_b);
                    SelectObject(hdc, old_p);
                    DeleteObject(brush_box);
                    DeleteObject(pen_box);
                } else {
                    let border_col = if is_hovered { rgb(148, 163, 184) } else { rgb(203, 213, 225) };
                    let brush_box = CreateSolidBrush(rgb(255, 255, 255));
                    let pen_box = CreatePen(PS_SOLID as i32, 1, border_col);
                    let old_b = SelectObject(hdc, brush_box);
                    let old_p = SelectObject(hdc, pen_box);
                    RoundRect(hdc, box_rc.left, box_rc.top, box_rc.right, box_rc.bottom, 4, 4);
                    SelectObject(hdc, old_b);
                    SelectObject(hdc, old_p);
                    DeleteObject(brush_box);
                    DeleteObject(pen_box);
                }

                // 绘制文本
                SetBkMode(hdc, TRANSPARENT as i32);
                let text_col = if is_hovered { rgb(15, 23, 42) } else { rgb(51, 65, 85) };
                SetTextColor(hdc, text_col);
                let old_font = SelectObject(hdc, state.font_normal);
                let mut text_rc = RECT { left: box_left + 24, top: 0, right: rc.right, bottom: rc.bottom };
                let wtext = wide_str(text);
                DrawTextW(hdc, wtext.as_ptr(), -1, &mut text_rc, DT_LEFT as u32 | DT_VCENTER as u32 | DT_SINGLELINE as u32);
                SelectObject(hdc, old_font);

                return 1;
            }

            if id == ID_IN_COMBO {
                // 100% 扁平化下拉框绘制
                let is_hovered = is_control_hovered((*dis).hwndItem);
                let is_pressed = ((*dis).itemState & 0x0001) != 0;
                let bg_color = if is_pressed { rgb(248, 250, 252) } else { rgb(255, 255, 255) };
                let border_color = if is_pressed {
                    rgb(37, 99, 235)
                } else if is_hovered {
                    rgb(59, 130, 246)
                } else {
                    rgb(203, 213, 225)
                };
                let brush_btn = CreateSolidBrush(bg_color);
                let pen_btn = CreatePen(PS_SOLID as i32, 1, border_color);
                let old_b = SelectObject(hdc, brush_btn);
                let old_p = SelectObject(hdc, pen_btn);
                RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 6, 6);
                SelectObject(hdc, old_b);
                SelectObject(hdc, old_p);
                DeleteObject(brush_btn);
                DeleteObject(pen_btn);

                // 绘制设备名称
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, if is_hovered { rgb(15, 23, 42) } else { rgb(30, 41, 59) });
                let old_font = SelectObject(hdc, state.font_normal);
                let mut text_rc = RECT { left: rc.left + 12, top: rc.top, right: rc.right - 28, bottom: rc.bottom };
                let dev_name = if state.selected_input.is_empty() {
                    if state.input_devices.is_empty() {
                        "无可用麦克风".to_string()
                    } else {
                        "请选择麦克风".to_string()
                    }
                } else {
                    state.selected_input.clone()
                };
                let wdev = wide_str(&dev_name);
                DrawTextW(hdc, wdev.as_ptr(), -1, &mut text_rc, DT_LEFT as u32 | DT_VCENTER as u32 | DT_SINGLELINE as u32 | 0x00004000); // DT_END_ELLIPSIS

                // 绘制右侧下拉箭头 ▾
                let mut arrow_rc = RECT { left: rc.right - 26, top: rc.top, right: rc.right - 8, bottom: rc.bottom };
                let arrow_col = if is_hovered { rgb(59, 130, 246) } else { rgb(100, 116, 139) };
                SetTextColor(hdc, arrow_col);
                let warrow = wide_str("▾");
                DrawTextW(hdc, warrow.as_ptr(), -1, &mut arrow_rc, DT_CENTER as u32 | DT_VCENTER as u32 | DT_SINGLELINE as u32);
                SelectObject(hdc, old_font);

                return 1;
            }

            if id == ID_METER_CUSTOM {
                // 扁平化平滑电平表
                let brush_meter_bg = CreateSolidBrush(rgb(241, 245, 249)); // #F1F5F9
                let pen_meter_border = CreatePen(PS_SOLID as i32, 1, rgb(226, 232, 240)); // #E2E8F0
                let old_b = SelectObject(hdc, brush_meter_bg);
                let old_p = SelectObject(hdc, pen_meter_border);

                RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 4, 4);

                let w = rc.right - rc.left - 4;
                let percent = state.display_peak.clamp(0.0, 1.0);
                let fill_w = (w as f32 * percent).round() as i32;

                if fill_w > 0 {
                    let bar_color = if state.display_peak > 0.88 {
                        rgb(239, 68, 68)
                    } else if state.display_peak > 0.65 {
                        rgb(245, 158, 11)
                    } else {
                        rgb(34, 197, 94)
                    };
                    let brush_bar = CreateSolidBrush(bar_color);
                    let pen_bar = CreatePen(PS_SOLID as i32, 1, bar_color);
                    SelectObject(hdc, brush_bar);
                    SelectObject(hdc, pen_bar);

                    RoundRect(hdc, rc.left + 2, rc.top + 2, rc.left + 2 + fill_w, rc.bottom - 2, 3, 3);

                    SelectObject(hdc, old_b);
                    SelectObject(hdc, old_p);
                    DeleteObject(brush_bar);
                    DeleteObject(pen_bar);
                } else {
                    SelectObject(hdc, old_b);
                    SelectObject(hdc, old_p);
                }

                DeleteObject(brush_meter_bg);
                DeleteObject(pen_meter_border);
                return 1;
            }

            // 预设按钮与刷新按钮扁平绘制
            let mut btn_text = String::new();
            let mut is_active = false;

            if id == ID_REFRESH_BTN {
                btn_text = "刷新".to_string();
            } else if id == ID_PRESET_6 {
                btn_text = "+6dB".to_string();
                is_active = (state.gain_db - 6.0).abs() < 0.1;
            } else if id == ID_PRESET_12 {
                btn_text = "+12dB".to_string();
                is_active = (state.gain_db - 12.0).abs() < 0.1;
            } else if id == ID_PRESET_14 {
                btn_text = "+14dB".to_string();
                is_active = (state.gain_db - 14.0).abs() < 0.1;
            } else if id == ID_PRESET_18 {
                btn_text = "+18dB".to_string();
                is_active = (state.gain_db - 18.0).abs() < 0.1;
            } else if id == ID_PRESET_22 {
                btn_text = "+22dB".to_string();
                is_active = (state.gain_db - 22.0).abs() < 0.1;
            }

            let is_hovered = is_control_hovered((*dis).hwndItem);
            let is_pressed = ((*dis).itemState & 0x0001) != 0; // ODS_SELECTED

            let (bg_color, border_color, text_color) = if id == ID_REFRESH_BTN {
                if is_pressed {
                    (rgb(219, 234, 254), rgb(37, 99, 235), rgb(29, 78, 216))
                } else if is_hovered {
                    (rgb(239, 246, 255), rgb(59, 130, 246), rgb(37, 99, 235))
                } else {
                    (rgb(248, 250, 252), rgb(203, 213, 225), rgb(71, 85, 105))
                }
            } else if is_active {
                // 激活选中状态
                if is_pressed {
                    (rgb(30, 64, 175), rgb(23, 37, 84), rgb(255, 255, 255))
                } else if is_hovered {
                    (rgb(29, 78, 216), rgb(30, 64, 175), rgb(255, 255, 255))
                } else {
                    (rgb(37, 99, 235), rgb(29, 78, 216), rgb(255, 255, 255))
                }
            } else {
                // 未激活状态
                if is_pressed {
                    (rgb(203, 213, 225), rgb(100, 116, 139), rgb(15, 23, 42))
                } else if is_hovered {
                    (rgb(226, 232, 240), rgb(148, 163, 184), rgb(15, 23, 42))
                } else {
                    (rgb(241, 245, 249), rgb(203, 213, 225), rgb(51, 65, 85))
                }
            };

            let brush = CreateSolidBrush(bg_color);
            let pen = CreatePen(PS_SOLID as i32, 1, border_color);
            let old_b = SelectObject(hdc, brush);
            let old_p = SelectObject(hdc, pen);

            RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 6, 6);

            SelectObject(hdc, old_b);
            SelectObject(hdc, old_p);
            DeleteObject(brush);
            DeleteObject(pen);

            let old_font = SelectObject(hdc, state.font_small);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, text_color);
            let mut text_rc = rc;
            let wstr = wide_str(&btn_text);
            DrawTextW(hdc, wstr.as_ptr(), -1, &mut text_rc, DT_CENTER as u32 | DT_VCENTER as u32 | DT_SINGLELINE as u32);
            SelectObject(hdc, old_font);

            1
        }

        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as isize;

            if !state_ptr.is_null() {
                let state = &mut *state_ptr;

                if id == ID_REFRESH_BTN {
                    state.refresh_device_list();
                    state.check_and_prompt_driver(hwnd, true);
                } else if id == ID_IN_COMBO {
                    // 弹出设备选择菜单
                    let mut pt = POINT { x: 32, y: 48 + 30 };
                    ClientToScreen(hwnd, &mut pt);

                    let hmenu = CreatePopupMenu();
                    for (i, dev) in state.input_devices.iter().enumerate() {
                        let flag = if dev == &state.selected_input { MF_CHECKED } else { MF_UNCHECKED };
                        AppendMenuW(hmenu, MF_STRING | flag, ID_MENU_DEVICE_BASE + i, wide_str(dev).as_ptr());
                    }
                    TrackPopupMenu(hmenu, TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, 0, hwnd, null_mut());
                    DestroyMenu(hmenu);
                } else if id >= ID_MENU_DEVICE_BASE as isize && id < (ID_MENU_DEVICE_BASE + state.input_devices.len()) as isize {
                    let idx = (id as usize) - ID_MENU_DEVICE_BASE;
                    state.selected_input = state.input_devices[idx].clone();
                    state.config.push_device_history(&state.selected_input);
                    // 恢复该麦克风设备单独保存的增益值并更新 UI
                    state.gain_db = state.config.get_gain_for_device(&state.selected_input);
                    state.controller.set_gain_db(state.gain_db);
                    state.update_gain_ui();
                    state.save_settings();
                    state.start_or_restart_stream();
                    InvalidateRect(state.hwnd_in_combo, null_mut(), 1);
                } else if id == ID_PRESET_6 {
                    state.gain_db = 6.0;
                    state.controller.set_gain_db(6.0);
                    state.update_gain_ui();
                    state.save_settings();
                } else if id == ID_PRESET_12 {
                    state.gain_db = 12.0;
                    state.controller.set_gain_db(12.0);
                    state.update_gain_ui();
                    state.save_settings();
                } else if id == ID_PRESET_14 {
                    state.gain_db = 14.0;
                    state.controller.set_gain_db(14.0);
                    state.update_gain_ui();
                    state.save_settings();
                } else if id == ID_PRESET_18 {
                    state.gain_db = 18.0;
                    state.controller.set_gain_db(18.0);
                    state.update_gain_ui();
                    state.save_settings();
                } else if id == ID_PRESET_22 {
                    state.gain_db = 22.0;
                    state.controller.set_gain_db(22.0);
                    state.update_gain_ui();
                    state.save_settings();
                } else if id == ID_CHECK_AUTOSTART {
                    let new_val = !state.config.auto_start;
                    state.set_autostart(new_val);
                } else if id == ID_CHECK_SILENT {
                    let new_val = !state.config.silent_start;
                    state.set_silent_start(new_val);
                } else if id == ID_TRAY_SHOW as isize {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                    if !state.has_virtual_cable() && !state.driver_prompted {
                        state.check_and_prompt_driver(hwnd, false);
                    }
                } else if id == ID_TRAY_MUTE as isize {
                    state.toggle_mute();
                } else if id == ID_TRAY_AUTOSTART as isize {
                    let new_val = !state.config.auto_start;
                    state.set_autostart(new_val);
                } else if id == ID_TRAY_SILENT as isize {
                    let new_val = !state.config.silent_start;
                    state.set_silent_start(new_val);
                } else if id == ID_TRAY_EXIT as isize {
                    Shell_NotifyIconW(NIM_DELETE, &state.nid);
                    DestroyWindow(hwnd);
                }
            }
            0
        }

        WM_APP_TRAY => {
            let event = lparam as u32;
            if event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK {
                ShowWindow(hwnd, SW_SHOW);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    if !state.has_virtual_cable() && !state.driver_prompted {
                        state.check_and_prompt_driver(hwnd, false);
                    }
                }
            } else if event == WM_RBUTTONUP && !state_ptr.is_null() {
                let state = &*state_ptr;
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                SetForegroundWindow(hwnd);

                let hmenu = CreatePopupMenu();
                AppendMenuW(hmenu, MF_STRING, ID_TRAY_SHOW, wide_str("打开主界面").as_ptr());
                AppendMenuW(hmenu, MF_SEPARATOR, 0, null_mut());

                let mute_flag = if state.is_muted { MF_CHECKED } else { MF_UNCHECKED };
                AppendMenuW(hmenu, MF_STRING | mute_flag, ID_TRAY_MUTE, wide_str("静音麦克风").as_ptr());

                let auto_flag = if state.config.auto_start { MF_CHECKED } else { MF_UNCHECKED };
                AppendMenuW(hmenu, MF_STRING | auto_flag, ID_TRAY_AUTOSTART, wide_str("开机自启").as_ptr());

                let silent_flag = if state.config.silent_start { MF_CHECKED } else { MF_UNCHECKED };
                AppendMenuW(hmenu, MF_STRING | silent_flag, ID_TRAY_SILENT, wide_str("静默启动").as_ptr());

                AppendMenuW(hmenu, MF_SEPARATOR, 0, null_mut());
                AppendMenuW(hmenu, MF_STRING, ID_TRAY_EXIT, wide_str("退出程序").as_ptr());

                TrackPopupMenu(hmenu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null_mut());
                DestroyMenu(hmenu);
            }
            0
        }

        WM_APP_WAKEUP => {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                if !state.has_virtual_cable() && !state.driver_prompted {
                    state.check_and_prompt_driver(hwnd, false);
                }
            }
            0
        }

        WM_APP_CHECK_DRIVER => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let args: Vec<String> = std::env::args().collect();
                let is_silent = args.iter().any(|arg| arg == "--silent");
                if !is_silent {
                    state.check_and_prompt_driver(hwnd, false);
                }
            }
            0
        }

        WM_APP_DEVICE_REFRESH => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.refresh_device_list();
                if state.has_virtual_cable() {
                    MessageBoxW(
                        hwnd,
                        wide_str("VB-Audio 虚拟声卡驱动检测成功！\n已自动绑定输出设备。\n\n提示：如部分会议/录音软件尚未识别该设备，建议根据提示重启电脑以完全激活。").as_ptr(),
                        wide_str("虚拟声卡就绪").as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
            0
        }

        WM_DEVICECHANGE => {
            // Windows 设备插拔广播，设置 350ms 去抖定时器，等待 WASAPI 音频端点就绪
            SetTimer(hwnd, TIMER_HOTPLUG, 350, None);
            0
        }

        WM_TIMER => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                if wparam == TIMER_HOTPLUG {
                    KillTimer(hwnd, TIMER_HOTPLUG);
                    state.check_device_changes(hwnd);
                } else if wparam == TIMER_METER {
                    state.timer_ticks = state.timer_ticks.wrapping_add(1);

                    // 若音频流检测到设备断开错误，或者每 30 次 tick (约 1 秒) 心跳检测，实时检查设备变动
                    if state.controller.get_last_error().is_some() || (state.timer_ticks % 30 == 0) {
                        state.check_device_changes(hwnd);
                    }

                    if !state.tray_added {
                        if Shell_NotifyIconW(NIM_ADD, &state.nid) != 0 {
                            state.tray_added = true;
                        }
                    }
                    if state.engine.is_some() {
                        let cur_peak = state.controller.get_peak();
                        if cur_peak >= state.display_peak {
                            state.display_peak = cur_peak;
                        } else {
                            state.display_peak = state.display_peak * 0.84 + cur_peak * 0.16;
                        }
                    } else {
                        state.display_peak *= 0.8;
                        if state.display_peak < 0.005 {
                            state.display_peak = 0.0;
                        }
                    }

                    InvalidateRect(state.hwnd_meter, null_mut(), 0);
                }
            }
            0
        }

        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }

        WM_DESTROY => {
            KillTimer(hwnd, TIMER_METER);
            KillTimer(hwnd, TIMER_HOTPLUG);
            if !state_ptr.is_null() {
                let state = Box::from_raw(state_ptr);
                Shell_NotifyIconW(NIM_DELETE, &state.nid);
                if state.font_bold != 0 { DeleteObject(state.font_bold); }
                if state.font_normal != 0 { DeleteObject(state.font_normal); }
                if state.font_small != 0 { DeleteObject(state.font_small); }
                if state.brush_bg != 0 { DeleteObject(state.brush_bg); }
                if state.brush_card != 0 { DeleteObject(state.brush_card); }
            }
            PostQuitMessage(0);
            0
        }

        msg if !state_ptr.is_null() && msg == (*state_ptr).msg_taskbar_created => {
            let state = &mut *state_ptr;
            if Shell_NotifyIconW(NIM_ADD, &state.nid) != 0 {
                state.tray_added = true;
            }
            0
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn create_font(face: &str, height_pt: i32, bold: bool) -> HFONT {
    let wface = wide_str(face);
    CreateFontW(
        -height_pt, 0, 0, 0,
        if bold { 700 } else { 400 },
        0, 0, 0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wface.as_ptr(),
    )
}

fn find_vbcable_installer() -> Option<PathBuf> {
    let setup_files = ["VBCABLE_Setup_x64.exe", "VBCABLE_Setup.exe"];
    let mut search_dirs = Vec::new();

    // 1. 可执行文件所在目录及其祖先目录
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            search_dirs.push(parent.join("VBCABLE_Driver"));
            if let Some(gp) = parent.parent().and_then(|p| p.parent()) {
                search_dirs.push(gp.join("VBCABLE_Driver"));
            }
        }
    }

    // 2. 当前工作目录
    search_dirs.push(PathBuf::from("VBCABLE_Driver"));

    for dir in search_dirs {
        for file in &setup_files {
            let candidate = dir.join(file);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 启用 Windows GDI 高 DPI 缩放 (Windows 10 1703+)
/// 保持原有逻辑坐标系不变，底层由 GDI 直接以屏幕物理高清像素光栅化文本与线条，彻底消除模糊拉伸
unsafe fn enable_gdi_dpi_scaling() {
    type SetProcessDpiAwarenessContextFn = unsafe extern "system" fn(isize) -> BOOL;
    type SetProcessDPIAwareFn = unsafe extern "system" fn() -> BOOL;

    let user32 = GetModuleHandleW(wide_str("user32.dll").as_ptr());
    if user32 != 0 {
        use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

        // Windows 10 1703+ 引入的 GDI DPI 缩放技术
        // DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED = -5
        let s_dpi_ctx = b"SetProcessDpiAwarenessContext\0";
        if let Some(proc) = GetProcAddress(user32, s_dpi_ctx.as_ptr()) {
            let func: SetProcessDpiAwarenessContextFn = std::mem::transmute(proc);
            if func(-5) != 0 {
                return;
            }
        }

        // 旧版 Windows 系统的平稳降级
        let s_dpi_aware = b"SetProcessDPIAware\0";
        if let Some(proc) = GetProcAddress(user32, s_dpi_aware.as_ptr()) {
            let func: SetProcessDPIAwareFn = std::mem::transmute(proc);
            func();
        }
    }
}

fn main() {
    unsafe {
        // 开启 Windows GDI 高清 DPI 缩放
        enable_gdi_dpi_scaling();

        // 单实例检测互斥体 (防止程序多开)
        let mutex_name = wide_str("Local\\USBMicBooster_SingleInstance_Mutex");
        SetLastError(0);
        let h_mutex = CreateMutexW(null_mut(), 1, mutex_name.as_ptr());
        if h_mutex != 0 && GetLastError() == ERROR_ALREADY_EXISTS {
            let class_name = wide_str("USBMicBoosterWin32Class");
            let mut hwnd_existing = FindWindowW(class_name.as_ptr(), null_mut());
            if hwnd_existing == 0 {
                let legacy_class_name = wide_str("DJIMicBoosterWin32Class");
                hwnd_existing = FindWindowW(legacy_class_name.as_ptr(), null_mut());
            }
            if hwnd_existing != 0 {
                PostMessageW(hwnd_existing, WM_APP_WAKEUP, 0, 0);
                ShowWindow(hwnd_existing, SW_SHOW);
                ShowWindow(hwnd_existing, SW_RESTORE);
                SetForegroundWindow(hwnd_existing);
            }
            CloseHandle(h_mutex);
            return;
        }

        let instance = GetModuleHandleW(null_mut());

        // 注册自定义扁平滑块窗口类
        let slider_class_name = wide_str("USBMicFlatSliderClass");
        let slider_wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(slider_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: 0,
            hCursor: LoadCursorW(0, IDC_HAND),
            hbrBackground: CreateSolidBrush(rgb(255, 255, 255)),
            lpszMenuName: null_mut(),
            lpszClassName: slider_class_name.as_ptr(),
        };
        RegisterClassW(&slider_wc);

        // 注册主窗口类
        let class_name = wide_str("USBMicBoosterWin32Class");
        let args: Vec<String> = std::env::args().collect();
        let is_silent = args.iter().any(|arg| arg == "--silent");
        let app_state = Box::new(AppState::new());

        let h_app_icon = LoadIconW(instance, 1 as *const u16);
        let final_icon = if h_app_icon != 0 { h_app_icon } else { LoadIconW(0, IDI_APPLICATION) };

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: final_icon,
            hCursor: LoadCursorW(0, IDC_ARROW),
            hbrBackground: CreateSolidBrush(rgb(248, 249, 250)),
            lpszMenuName: null_mut(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&wc);

        // 紧凑窗口尺寸 (客户区 440 x 292)
        let mut rect = RECT { left: 0, top: 0, right: 440, bottom: 292 };
        AdjustWindowRect(&mut rect, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, 0);
        let win_width = rect.right - rect.left;
        let win_height = rect.bottom - rect.top;

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let pos_x = (screen_w - win_width) / 2;
        let pos_y = (screen_h - win_height) / 2;

        let title = wide_str("USB麦克风增益助手");

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            pos_x,
            pos_y,
            win_width,
            win_height,
            0,
            0,
            instance,
            Box::into_raw(app_state) as *mut _,
        );

        if !is_silent {
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        } else {
            ShowWindow(hwnd, SW_HIDE);
        }

        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if h_mutex != 0 {
            CloseHandle(h_mutex);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_vbcable_installer() {
        let installer = find_vbcable_installer();
        assert!(installer.is_some(), "Should find VBCABLE installer");
        let path = installer.unwrap();
        assert!(path.exists(), "Installer path should exist");
        println!("Found installer at: {:?}", path);
    }

    #[test]
    fn test_dpi_symbols() {
        unsafe {
            enable_gdi_dpi_scaling();
        }
    }
}
