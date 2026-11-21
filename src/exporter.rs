use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[cfg(windows)]
fn post_key_to_resolve(vk_code: u16, scan_code: u32) -> Result<(), String> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextW, IsWindowVisible, PostMessageA, WM_KEYDOWN,
        WM_KEYUP,
    };

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }

        let mut title_buf = [0u16; 256];
        let title_len = GetWindowTextW(hwnd, &mut title_buf);
        if title_len > 0 {
            let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);
            if title.contains("DaVinci Resolve") {
                let target_ptr = lparam.0 as *mut Option<HWND>;
                *target_ptr = Some(hwnd);
                return BOOL(0);
            }
        }

        let mut class_buf = [0u16; 128];
        let class_len = GetClassNameW(hwnd, &mut class_buf);
        if class_len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);
            if class_name == "Qt653QWindowIcon" {
                let target_ptr = lparam.0 as *mut Option<HWND>;
                *target_ptr = Some(hwnd);
                return BOOL(0);
            }
        }

        BOOL(1)
    }

    let mut found_hwnd: Option<HWND> = None;
    unsafe {
        let _ = EnumWindows(
            Some(enum_cb),
            LPARAM(&mut found_hwnd as *mut Option<HWND> as isize),
        );
    }

    let hwnd = found_hwnd
        .ok_or_else(|| "DaVinci Resolve window not found".to_string())?;

    let wparam = WPARAM(vk_code as usize);
    let lparam_down = LPARAM(((scan_code << 16) | 1) as isize);
    let lparam_up = LPARAM((0xC000_0001u32 | (scan_code << 16)) as i32 as isize);
    unsafe {
        PostMessageA(hwnd, WM_KEYDOWN, wparam, lparam_down)
            .map_err(|e| format!("PostMessageA WM_KEYDOWN failed: {e}"))?;
        PostMessageA(hwnd, WM_KEYUP, wparam, lparam_up)
            .map_err(|e| format!("PostMessageA WM_KEYUP failed: {e}"))?;
    }

    Ok(())
}

#[cfg(windows)]
pub(crate) fn trigger_resolve_export() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F12;
    post_key_to_resolve(VK_F12.0, 0x58)
}

#[cfg(windows)]
pub(crate) fn trigger_resolve_import() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F11;
    post_key_to_resolve(VK_F11.0, 0x57)
}

#[cfg(not(windows))]
pub(crate) fn trigger_resolve_export() -> Result<(), String> {
    Err("Win32 PostMessageA export is only supported on Windows".to_string())
}

#[cfg(not(windows))]
pub(crate) fn trigger_resolve_import() -> Result<(), String> {
    Err("Win32 PostMessageA import is only supported on Windows".to_string())
}

pub(crate) fn export_and_wait_otio(repo_path: &Path) -> Result<PathBuf, String> {
    let timelines_dir = repo_path.join("Timelines");
    let _ = std::fs::create_dir_all(&timelines_dir);
    let cut_otio = timelines_dir.join("cut.otio");

    let prev_mtime = cut_otio
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    trigger_resolve_export()?;

    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Ok(meta) = cut_otio.metadata() {
            if let Ok(mtime) = meta.modified() {
                if mtime > prev_mtime {
                    return Ok(cut_otio);
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    if cut_otio.is_file() {
        Ok(cut_otio)
    } else {
        Err("Timed out waiting 500ms for Timelines/cut.otio".to_string())
    }
}
