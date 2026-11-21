use git2::{IndexAddOption, Repository, Signature};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::ZipArchive;

pub(crate) fn find_latest_interchange_file(timelines_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(timelines_dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_interchange = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                ext.eq_ignore_ascii_case("drt")
                    || ext.eq_ignore_ascii_case("otio")
                    || ext.eq_ignore_ascii_case("otioz")
            })
            .unwrap_or(false);
        if !is_interchange {
            continue;
        }
        let modified = path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            best = Some((modified, path));
        }
    }

    best.map(|(_, p)| p)
}

pub(crate) fn unpack_otioz_if_needed(
    otioz_path: &Path,
    timelines_dir: &Path,
    staging_media_dir: &Path,
) -> Result<PathBuf, String> {
    let ext = otioz_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if !ext.eq_ignore_ascii_case("otioz") {
        return Ok(otioz_path.to_path_buf());
    }

    let file = File::open(otioz_path).map_err(|e| format!("Failed to open .otioz: {e}"))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("Invalid .otioz zip: {e}"))?;

    std::fs::create_dir_all(timelines_dir)
        .map_err(|e| format!("Timelines dir error: {e}"))?;
    std::fs::create_dir_all(staging_media_dir)
        .map_err(|e| format!("staging_media dir error: {e}"))?;

    let mut extracted_otio: Option<PathBuf> = None;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Zip entry error: {e}"))?;
        if entry.is_dir() {
            continue;
        }

        let Some(rel) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let file_name = rel
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("asset");

        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("Zip read error: {e}"))?;

        if file_name.to_ascii_lowercase().ends_with(".otio") {
            let dest = timelines_dir.join(file_name);
            std::fs::write(&dest, &buf).map_err(|e| format!("Write .otio error: {e}"))?;
            extracted_otio = Some(dest);
        } else {
            let dest = staging_media_dir.join(file_name);
            std::fs::write(&dest, &buf).map_err(|e| format!("Write media error: {e}"))?;
        }
    }

    extracted_otio.ok_or_else(|| "No .otio timeline found inside .otioz bundle".to_string())
}

pub(crate) fn stage_local_assets_for_pass(repo_path: &Path) -> Result<usize, String> {
    let staging_dir = repo_path.join("staging_media");
    let staging_audio = staging_dir.join("audio");
    let staging_screenshots = staging_dir.join("screenshots");
    std::fs::create_dir_all(&staging_audio)
        .map_err(|e| format!("Failed to create staging_media/audio: {e}"))?;
    std::fs::create_dir_all(&staging_screenshots)
        .map_err(|e| format!("Failed to create staging_media/screenshots: {e}"))?;

    for keep in [
        staging_dir.join(".gitkeep"),
        staging_audio.join(".gitkeep"),
        staging_screenshots.join(".gitkeep"),
    ] {
        if !keep.exists() {
            std::fs::write(&keep, b"").map_err(|e| format!(".gitkeep error: {e}"))?;
        }
    }

    let mut staged_count = 0usize;
    let media_audio = repo_path.join("Media").join("Audio");
    if media_audio.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&media_audio) {
            for entry in entries.flatten() {
                let src = entry.path();
                if src.is_file() && src.file_name().and_then(|n| n.to_str()) != Some(".gitkeep") {
                    if let Some(name) = src.file_name() {
                        let dest = staging_audio.join(name);
                        if !dest.exists() {
                            std::fs::copy(&src, &dest)
                                .map_err(|e| format!("Stage media copy error: {e}"))?;
                            staged_count += 1;
                        }
                    }
                }
            }
        }
    }

    Ok(staged_count)
}

pub(crate) fn migrate_staging_media_to_dir(
    repo_path: &Path,
    dest_root: &Path,
) -> Result<usize, String> {
    let staging_dir = repo_path.join("staging_media");
    if !staging_dir.is_dir() {
        return Ok(0);
    }

    let mut moved_files = Vec::new();
    let scan_dirs = [
        (staging_dir.clone(), "staging_media"),
        (staging_dir.join("audio"), "staging_media/audio"),
    ];

    for (dir, rel_prefix) in scan_dirs {
        if !dir.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let src = entry.path();
                let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !src.is_file() || name == ".gitkeep" {
                    continue;
                }

                let is_audio = rel_prefix == "staging_media/audio"
                    || src
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|ext| {
                            matches!(
                                ext.to_ascii_lowercase().as_str(),
                                "wav" | "mp3" | "aac" | "flac" | "aiff" | "m4a"
                            )
                        })
                        .unwrap_or(false);

                let target_dir = if is_audio {
                    dest_root.join("Audio")
                } else {
                    dest_root.to_path_buf()
                };
                std::fs::create_dir_all(&target_dir)
                    .map_err(|e| format!("Failed to create {}: {e}", target_dir.display()))?;

                let dest = target_dir.join(name);
                if !dest.exists() {
                    std::fs::copy(&src, &dest).map_err(|e| {
                        format!("Failed to copy {} to {}: {e}", src.display(), dest.display())
                    })?;
                }
                std::fs::remove_file(&src)
                    .map_err(|e| format!("Failed to remove {}: {e}", src.display()))?;
                moved_files.push(format!("{rel_prefix}/{name}"));
            }
        }
    }

    if moved_files.is_empty() {
        return Ok(0);
    }

    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;

    for keep in [
        staging_dir.join(".gitkeep"),
        staging_dir.join("audio").join(".gitkeep"),
    ] {
        if !keep.exists() {
            let _ = std::fs::write(&keep, b"");
        }
    }

    for rel in &moved_files {
        let _ = index.remove_path(Path::new(rel));
    }
    let _ = index.add_all(
        ["staging_media/.gitkeep", "staging_media/audio/.gitkeep"].iter(),
        IndexAddOption::DEFAULT,
        None,
    );
    index.write().map_err(|e| format!("Index write error: {e}"))?;

    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Tree write error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Tree find error: {e}"))?;
    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Zane", "zane@editor.local"))
        .map_err(|e| format!("Signature error: {e}"))?;
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = head.iter().collect();

    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "chore: migrated staging_media to Z:\\ and purged from repo",
        &tree,
        &parents,
    )
    .map_err(|e| format!("Staging purge commit error: {e}"))?;

    Ok(moved_files.len())
}

fn extract_tree_interchange_file(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    dest_dir: &Path,
    prefix: &str,
) -> Option<PathBuf> {
    let timelines_entry = tree.get_name("Timelines")?;
    let timelines_tree = repo.find_tree(timelines_entry.id()).ok()?;
    for entry in timelines_tree.iter() {
        let name = entry.name()?;
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".otio") || lower.ends_with(".drt") {
            let blob = repo.find_blob(entry.id()).ok()?;
            let _ = std::fs::create_dir_all(dest_dir);
            let out_path = dest_dir.join(format!("{prefix}_{name}"));
            if std::fs::write(&out_path, blob.content()).is_ok() {
                return Some(out_path);
            }
        }
    }
    None
}

pub(crate) fn launch_dual_tab_conflict_import(
    repo: &Repository,
    repo_path: &Path,
    incoming_branch: &str,
    incoming_commit: &git2::Commit<'_>,
) -> Result<Vec<PathBuf>, String> {
    let conflict_dir = repo_path.join("Timelines").join("_conflict_review");
    let mut extracted = Vec::new();

    if let Ok(main_branch) = repo.find_branch("main", git2::BranchType::Local) {
        if let Ok(main_commit) = main_branch.get().peel_to_commit() {
            if let Ok(main_tree) = main_commit.tree() {
                if let Some(p) =
                    extract_tree_interchange_file(repo, &main_tree, &conflict_dir, "main")
                {
                    extracted.push(p);
                }
            }
        }
    }

    if let Ok(inc_tree) = incoming_commit.tree() {
        let slug = incoming_branch.replace(['/', '\\', ' '], "_");
        if let Some(p) = extract_tree_interchange_file(repo, &inc_tree, &conflict_dir, &slug) {
            extracted.push(p);
        }
    }

    if extracted.is_empty() {
        return Err("No .otio or .drt timeline files found in main/incoming branches".to_string());
    }

    for path in &extracted {
        import_timeline_into_running_resolve(path)?;
    }

    Ok(extracted)
}

#[cfg(windows)]
pub(crate) fn import_timeline_into_running_resolve(timeline_path: &Path) -> Result<(), String> {
    use std::mem::size_of;
    use std::thread;
    use std::time::Duration;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_I, VK_RETURN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE,
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

    if !timeline_path.is_file() {
        return Err(format!(
            "Timeline interchange file not found: {}",
            timeline_path.display()
        ));
    }

    let mut found_hwnd: Option<HWND> = None;
    unsafe {
        let _ = EnumWindows(
            Some(enum_cb),
            LPARAM(&mut found_hwnd as *mut Option<HWND> as isize),
        );
    }

    let hwnd = found_hwnd
        .ok_or_else(|| "DaVinci Resolve window not found (is Resolve running?)".to_string())?;

    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
    thread::sleep(Duration::from_millis(200));

    // Ctrl+Shift+I opens File -> Import -> Timeline in DaVinci Resolve
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
    thread::sleep(Duration::from_millis(600));

    // Type full path into the File Open dialog and press Enter
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
    thread::sleep(Duration::from_millis(500));

    // Confirm Resolve's Timeline Import Options modal
    let confirm = [key_input(VK_RETURN, false), key_input(VK_RETURN, true)];
    unsafe {
        SendInput(&confirm, size_of::<INPUT>() as i32);
    }

    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn import_timeline_into_running_resolve(_timeline_path: &Path) -> Result<(), String> {
    Err("Win32 timeline automation is only supported on Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn otioz_unpacks_and_staging_media_migrates_and_purges() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_interchange_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        let timelines_dir = temp_root.join("Timelines");
        let staging_dir = temp_root.join("staging_media");
        let dest_z = temp_root.join("Z_Media");
        std::fs::create_dir_all(&timelines_dir).unwrap();
        std::fs::create_dir_all(&staging_dir).unwrap();

        let repo = Repository::init(&temp_root).unwrap();

        // Create a dummy .otioz bundle containing an .otio and a .wav
        let otioz_path = timelines_dir.join("pass.otioz");
        {
            let file = File::create(&otioz_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("content.otio", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(br#"{"name": "Pass Cut"}"#).unwrap();
            zip.start_file("media/music.wav", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"RIFF_AUDIO").unwrap();
            zip.finish().unwrap();
        }

        let unpacked = unpack_otioz_if_needed(&otioz_path, &timelines_dir, &staging_dir).unwrap();
        assert!(unpacked.is_file());
        assert!(staging_dir.join("music.wav").is_file());

        // Commit staging_media/music.wav
        let mut index = repo.index().unwrap();
        index
            .add_path(Path::new("staging_media/music.wav"))
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("Test", "test@local").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "stage pass", &tree, &[])
            .unwrap();

        let migrated = migrate_staging_media_to_dir(&temp_root, &dest_z).unwrap();
        assert_eq!(migrated, 1);
        assert!(dest_z.join("Audio").join("music.wav").is_file());
        assert!(!staging_dir.join("music.wav").exists());
        assert!(staging_dir.join(".gitkeep").is_file());

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
