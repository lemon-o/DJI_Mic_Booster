use std::ptr::null_mut;
use windows_sys::Win32::System::Registry::*;

fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn set_auto_start(enable: bool) {
    unsafe {
        let subkey = wide_str("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let value_name = wide_str("DJIMicBooster");
        let mut hkey: HKEY = 0;

        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            &mut hkey,
        ) == 0 {
            if enable {
                if let Ok(exe_path) = std::env::current_exe() {
                    let path_str = format!("\"{}\"", exe_path.to_string_lossy());
                    let wpath = wide_str(&path_str);
                    let len_bytes = (wpath.len() * 2) as u32;
                    RegSetValueExW(
                        hkey,
                        value_name.as_ptr(),
                        0,
                        REG_SZ,
                        wpath.as_ptr() as *const u8,
                        len_bytes,
                    );
                }
            } else {
                RegDeleteValueW(hkey, value_name.as_ptr());
            }
            RegCloseKey(hkey);
        }
    }
}

pub fn is_auto_start_registered() -> bool {
    unsafe {
        let subkey = wide_str("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let value_name = wide_str("DJIMicBooster");
        let mut hkey: HKEY = 0;

        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        ) == 0 {
            let mut buf_type: u32 = 0;
            let mut buf_len: u32 = 0;
            let status = RegQueryValueExW(
                hkey,
                value_name.as_ptr(),
                null_mut(),
                &mut buf_type,
                null_mut(),
                &mut buf_len,
            );
            RegCloseKey(hkey);
            status == 0
        } else {
            false
        }
    }
}
