use git2::{build::CheckoutBuilder, Oid, Repository};
use std::path::{Path, PathBuf};

pub(crate) fn restore_commit_cut_otio(
    repo_path: &Path,
    full_oid: &str,
) -> Result<PathBuf, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let oid = Oid::from_str(full_oid).map_err(|e| format!("Invalid OID: {e}"))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| format!("Commit lookup error: {e}"))?;
    let tree = commit
        .tree()
        .map_err(|e| format!("Tree lookup error: {e}"))?;

    let cut_otio_rel = Path::new("Timelines/cut.otio");
    let mut opts = CheckoutBuilder::new();
    opts.force();
    if tree.get_path(cut_otio_rel).is_ok() {
        opts.path(cut_otio_rel);
    }

    repo.checkout_tree(tree.as_object(), Some(&mut opts))
        .map_err(|e| format!("Checkout Timelines/cut.otio error: {e}"))?;

    let restored_path = repo_path.join(cut_otio_rel);
    if !restored_path.is_file() {
        return Err(format!(
            "Restored file not found at {}",
            restored_path.display()
        ));
    }

    // Prefer headless F11 (sidecar_import.lua) without stealing focus; fallback to Ctrl+Shift+I
    if crate::exporter::trigger_resolve_import().is_err() {
        import_cut_otio_with_focus(&restored_path)?;
    }
    Ok(restored_path)
}

#[cfg(windows)]
fn import_cut_otio_with_focus(timeline_path: &Path) -> Result<(), String> {
    use std::mem::size_of;
    use std::thread;
    use std::time::Duration;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_I, VK_RETURN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextW, IsWindowVisible, SetForegroundWindow,
        ShowWindow, SW_RESTORE,
    };

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len > 0 {
            let title = String::from_utf16_lossy(&buf[..len as usize]);
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

    fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        Default::default()
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn unicode_char_inputs(ch: u16) -> [INPUT; 2] {
        [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: ch,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: ch,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ]
    }

    let mut found_hwnd: Option<HWND> = None;
    unsafe {
        let _ = EnumWindows(
            Some(enum_cb),
            LPARAM(&mut found_hwnd as *mut Option<HWND> as isize),
        );
    }

    let hwnd = found_hwnd
        .ok_or_else(|| "DaVinci Resolve window not found for restore".to_string())?;

    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
    thread::sleep(Duration::from_millis(150));

    // Ctrl + Shift + I
    let shortcut = [
        key_input(VK_CONTROL, false),
        key_input(VK_SHIFT, false),
        key_input(VK_I, false),
        key_input(VK_I, true),
        key_input(VK_SHIFT, true),
        key_input(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&shortcut, size_of::<INPUT>() as i32);
    }
    // 300ms dialog wait as specified
    thread::sleep(Duration::from_millis(300));

    let path_str = timeline_path.to_string_lossy();
    let mut text_inputs = Vec::with_capacity(path_str.len() * 2 + 2);
    for ch in path_str.encode_utf16() {
        text_inputs.extend_from_slice(&unicode_char_inputs(ch));
    }
    text_inputs.push(key_input(VK_RETURN, false));
    text_inputs.push(key_input(VK_RETURN, true));

    unsafe {
        SendInput(&text_inputs, size_of::<INPUT>() as i32);
    }
    thread::sleep(Duration::from_millis(300));

    let confirm = [key_input(VK_RETURN, false), key_input(VK_RETURN, true)];
    unsafe {
        SendInput(&confirm, size_of::<INPUT>() as i32);
    }

    Ok(())
}

#[cfg(not(windows))]
fn import_cut_otio_with_focus(_timeline_path: &Path) -> Result<(), String> {
    Err("Win32 restore import is only supported on Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;

    #[test]
    fn checkout_cut_otio_from_historical_commit() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_revert_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        let timelines_dir = temp_root.join("Timelines");
        std::fs::create_dir_all(&timelines_dir).unwrap();

        let repo = Repository::init(&temp_root).unwrap();
        let otio_path = timelines_dir.join("cut.otio");
        let sig = Signature::now("Zane", "zane@local").unwrap();

        std::fs::write(&otio_path, r#"{"name": "v1"}"#).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("Timelines/cut.otio")).unwrap();
        index.write().unwrap();
        let t1 = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let oid1 = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "v1", &t1, &[])
            .unwrap();

        std::fs::write(&otio_path, r#"{"name": "v2"}"#).unwrap();
        index.add_path(Path::new("Timelines/cut.otio")).unwrap();
        index.write().unwrap();
        let t2 = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let c1 = repo.find_commit(oid1).unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "v2", &t2, &[&c1])
            .unwrap();

        // Checkout v1's Timelines/cut.otio directly via git2
        let mut opts = CheckoutBuilder::new();
        opts.force().path(Path::new("Timelines/cut.otio"));
        repo.checkout_tree(t1.as_object(), Some(&mut opts)).unwrap();
        assert_eq!(std::fs::read_to_string(&otio_path).unwrap(), r#"{"name": "v1"}"#);

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
