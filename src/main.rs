#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai_engine;
mod app;
mod branch_manager;
mod git_engine;
mod graph_draw;
mod mount_engine;
mod otio_diff;
mod otio_engine;
mod projects;
mod tickets;
mod ui;
mod watcher;

pub(crate) use app::AppView;

const EMBEDDED_SIDECAR_UTILITY_PY: &str = include_str!("../sidecar_utility.py");
const EMBEDDED_SIDECAR_IMPORT_PY: &str = include_str!("../sidecar_import.py");

use chrono::Utc;
use eframe::egui;
use git2::{BranchType, Oid, Repository, Sort};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{mpsc::Receiver, Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CommitBranchKind {
    Main,
    Pass,
    Experiment,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageMountState {
    MountedZ,
    MountingZ,
    LocalProject,
}

#[derive(Clone, Debug)]
pub(crate) struct CommitChangeRow {
    pub(crate) track: String,
    pub(crate) is_audio: bool,
    pub(crate) media_file: String,
    pub(crate) media_path: Option<String>,
    pub(crate) change_type: String,
    pub(crate) position: String,
}

#[derive(Clone)]
pub(crate) struct GitCommitNode {
    pub(crate) full_id: String,
    pub(crate) short_id: String,
    pub(crate) message: String,
    pub(crate) author: String,
    pub(crate) time: String,
    pub(crate) timestamp_secs: i64,
    pub(crate) tag: Option<String>,
    pub(crate) parent_ids: Vec<String>,
    pub(crate) lane: usize,
    pub(crate) is_merge: bool,
    pub(crate) branch_kind: CommitBranchKind,
    pub(crate) branch_name: Option<String>,
    pub(crate) tracks: Vec<String>,
}

pub(crate) type CommitNodeInfo = GitCommitNode;

#[derive(Clone, Debug, Default)]
pub(crate) struct PrReviewMeta {
    pub(crate) author: String,
    pub(crate) commit_count: usize,
    pub(crate) total_cuts: usize,
    pub(crate) audio_edits: usize,
    pub(crate) duration_delta_sec: f64,
}

pub(crate) struct ResolveSidecarApp {
    pub(crate) repo_path: PathBuf,
    pub(crate) commits: Vec<GitCommitNode>,
    pub(crate) status_message: String,
    pub(crate) resolve_running: bool,
    pub(crate) storage_state: StorageMountState,
    pub(crate) current_branch: String,
    pub(crate) selected_commit_oid: Option<String>,
    pub(crate) selected_burst_range: Option<(String, String)>,
    pub(crate) selected_commit_changes: Vec<CommitChangeRow>,
    pub(crate) expanded_group_ids: HashSet<String>,
    pub(crate) show_fork_sandbox_modal: bool,
    pub(crate) fork_sandbox_name: String,
    pub(crate) ai_model_state: Arc<Mutex<ai_engine::AiModelState>>,
    pub(crate) remote_sync_state: git_engine::RemoteSyncState,
    pub(crate) z_drive_connected: bool,
    pub(crate) pr_review_branch: Option<String>,
    pub(crate) pr_review_changes: Vec<CommitChangeRow>,
    pub(crate) pr_review_meta: PrReviewMeta,
    pub(crate) show_tickets_drawer: bool,
    pub(crate) tickets: Vec<tickets::Ticket>,
    pub(crate) new_ticket_category: tickets::TicketCategory,
    pub(crate) new_ticket_title: String,
    pub(crate) new_ticket_description: String,
    pub(crate) needs_repo_setup: bool,
    pub(crate) setup_remote_url: String,
    pub(crate) setup_editor_handle: String,
    pub(crate) current_view: AppView,
    pub(crate) show_new_project_modal: bool,
    pub(crate) saved_projects: Vec<projects::SavedProject>,
    pub(crate) new_project_name: String,
    pub(crate) new_project_base_dir: String,
    pub(crate) storage_drive_letter: String,
    pub(crate) storage_remote_name: String,
    pub(crate) storage_remote_subfolder: String,
    pub(crate) rclone_remote_authenticated: bool,
    pub(crate) rclone_oauth_in_progress: bool,
    pub(crate) rclone_oauth_rx: Option<Receiver<Result<(), String>>>,
    pub(crate) github_oauth_in_progress: bool,
    pub(crate) github_oauth_rx: Option<Receiver<Result<String, String>>>,
    pub(crate) ingest_watch_folder: String,
    pub(crate) ingest_queue: Vec<app::IngestJob>,
    pub(crate) transcode_worker_handle: Option<Child>,
    pub(crate) github_auth: app::GitHubAuthState,
    #[allow(dead_code)]
    pub(crate) timeline_watcher: Option<watcher::TimelineWatcher>,
    pub(crate) event_rx: Option<Receiver<watcher::AppEvent>>,
    utility_daemon_handle: Option<Child>,
    autostart_attempted_for_session: bool,
    rclone_handle: Option<Child>,
    last_health_poll: Instant,
}

impl Drop for ResolveSidecarApp {
    fn drop(&mut self) {
        if let Some(mut child) = self.utility_daemon_handle.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.rclone_handle.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.transcode_worker_handle.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(crate) fn configure_hidden_command(cmd: &mut Command) {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
}

pub(crate) fn auto_deploy_resolve_scripts() -> Option<String> {
    let appdata = std::env::var_os("APPDATA")?;
    let target_dir = PathBuf::from(appdata)
        .join("Blackmagic Design")
        .join("DaVinci Resolve")
        .join("Support")
        .join("Fusion")
        .join("Scripts")
        .join("Utility");
    if !target_dir.is_dir() {
        return None;
    }

    let mut synced_any = false;
    for (filename, content) in [
        ("sidecar_utility.py", EMBEDDED_SIDECAR_UTILITY_PY),
        ("sidecar_import.py", EMBEDDED_SIDECAR_IMPORT_PY),
    ] {
        let dest = target_dir.join(filename);
        let needs_write = match std::fs::read_to_string(&dest) {
            Ok(existing) => existing != content,
            Err(_) => true,
        };
        if needs_write {
            let tmp = target_dir.join(format!("{filename}.tmp"));
            if std::fs::write(&tmp, content.as_bytes()).is_ok()
                && std::fs::rename(&tmp, &dest).is_ok()
            {
                synced_any = true;
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    if synced_any {
        let msg = "[Auto-Deploy] Sidecar scripts synced to DaVinci Resolve.".to_string();
        println!("{msg}");
        Some(msg)
    } else {
        None
    }
}

fn detect_active_repo_path() -> PathBuf {
    for candidate in [
        PathBuf::from(r"Z:\"),
        PathBuf::from(r"C:\Users\zanem\Resolve_Cloud_Storage"),
    ] {
        if Repository::open(&candidate).is_ok() {
            return candidate;
        }
    }
    PathBuf::from(r"C:\Users\zanem\Resolve_Cloud_Storage")
}

fn is_resolve_running() -> bool {
    let mut cmd = Command::new("tasklist");
    cmd.args(["/FI", "IMAGENAME eq Resolve.exe", "/NH"]);
    configure_hidden_command(&mut cmd);
    cmd.output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("Resolve.exe"))
        .unwrap_or(false)
}

fn detect_storage_state(rclone_active: bool) -> StorageMountState {
    if Repository::open(r"Z:\").is_ok() {
        StorageMountState::MountedZ
    } else if rclone_active {
        StorageMountState::MountingZ
    } else {
        StorageMountState::LocalProject
    }
}

pub(crate) fn is_z_drive_missing(media_path: Option<&str>) -> bool {
    let Some(raw) = media_path else {
        return false;
    };
    let lower = raw.trim().to_ascii_lowercase();
    if lower.starts_with(r"z:\") || lower.starts_with("z:/") {
        !Path::new(raw.trim()).exists()
    } else {
        false
    }
}

pub(crate) fn is_local_media_not_on_z(media_path: Option<&str>) -> bool {
    let Some(raw) = media_path else {
        return false;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !(lower.starts_with(r"z:\") || lower.starts_with("z:/"))
}

fn format_relative_time(timestamp_secs: i64) -> String {
    let now = Utc::now().timestamp();
    let elapsed = (now - timestamp_secs).max(0);
    if elapsed < 10 {
        "just now".to_string()
    } else if elapsed < 60 {
        format!("{elapsed}s ago")
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86400)
    }
}

fn format_tc_secs(seconds: f64) -> String {
    let secs = seconds.max(0.0).round() as u64;
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

fn short_track_badge(track: &str) -> (String, bool) {
    let lower = track.to_lowercase();
    if let Some(num) = lower.strip_prefix("audio ") {
        return (format!("A{}", num.trim()), true);
    }
    if let Some(num) = lower.strip_prefix("video ") {
        return (format!("V{}", num.trim()), false);
    }
    (track.to_string(), lower.contains("audio") || track.starts_with('A'))
}

fn read_commit_cut_otio(repo: &Repository, commit: &git2::Commit<'_>) -> Option<String> {
    let tree = commit.tree().ok()?;
    if let Ok(entry) = tree.get_path(Path::new("Timelines/cut.otio")) {
        if let Ok(blob) = repo.find_blob(entry.id()) {
            if let Ok(text) = std::str::from_utf8(blob.content()) {
                return Some(text.to_string());
            }
        }
    }
    let mut fallback_blob_id = None;
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        if root.starts_with("Timelines") {
            if let Some(name) = entry.name() {
                if name.ends_with(".otio") && !name.starts_with("Restore_") {
                    fallback_blob_id = Some(entry.id());
                    return git2::TreeWalkResult::Abort;
                }
            }
        }
        git2::TreeWalkResult::Ok
    });
    let blob = repo.find_blob(fallback_blob_id?).ok()?;
    std::str::from_utf8(blob.content()).ok().map(str::to_string)
}

fn find_clip_media_path(
    snap: &otio_engine::TimelineSnapshot,
    track_name: &str,
    clip_name: &str,
) -> Option<String> {
    snap.clips
        .values()
        .find(|c| c.track_name == track_name && c.name == clip_name && c.media_path.is_some())
        .and_then(|c| c.media_path.clone())
}

fn diff_snapshots_to_rows(
    curr_snap: &otio_engine::TimelineSnapshot,
    parent_snap: &otio_engine::TimelineSnapshot,
) -> Vec<CommitChangeRow> {
    let delta = curr_snap.diff(parent_snap);
    let mut rows = Vec::new();

    for trim in &delta.trimmed_clips {
        let sign = if trim.frame_delta >= 0 { "+" } else { "" };
        let (badge, is_audio) = short_track_badge(&trim.track_name);
        let media_path = find_clip_media_path(curr_snap, &trim.track_name, &trim.clip_name)
            .or_else(|| find_clip_media_path(parent_snap, &trim.track_name, &trim.clip_name));
        rows.push(CommitChangeRow {
            track: badge,
            is_audio,
            media_file: trim.clip_name.clone(),
            media_path,
            change_type: "Trimmed".to_string(),
            position: format!("{sign}{}f", trim.frame_delta),
        });
    }

    for split in &delta.split_points {
        let (badge, is_audio) = short_track_badge(&split.track_name);
        let media_path = find_clip_media_path(curr_snap, &split.track_name, &split.clip_name);
        rows.push(CommitChangeRow {
            track: badge,
            is_audio,
            media_file: split.clip_name.clone(),
            media_path,
            change_type: "Split".to_string(),
            position: format_tc_secs(split.at_seconds),
        });
    }

    for track in &delta.added_tracks {
        let (badge, is_audio) = short_track_badge(track);
        rows.push(CommitChangeRow {
            track: badge,
            is_audio,
            media_file: "—".to_string(),
            media_path: None,
            change_type: "Track Added".to_string(),
            position: "00:00:00".to_string(),
        });
    }

    for track in &delta.removed_tracks {
        let (badge, is_audio) = short_track_badge(track);
        rows.push(CommitChangeRow {
            track: badge,
            is_audio,
            media_file: "—".to_string(),
            media_path: None,
            change_type: "Track Removed".to_string(),
            position: "00:00:00".to_string(),
        });
    }

    for clip in curr_snap.clips.values() {
        if delta.added_clips.contains(&clip.name) {
            let (badge, is_audio) = short_track_badge(&clip.track_name);
            let start_tc = format_tc_secs(clip.timeline_offset_sec);
            let end_tc = format_tc_secs(clip.timeline_offset_sec + clip.duration_seconds);
            rows.push(CommitChangeRow {
                track: badge,
                is_audio,
                media_file: clip.name.clone(),
                media_path: clip.media_path.clone(),
                change_type: "Added".to_string(),
                position: format!("{start_tc} -> {end_tc}"),
            });
        }
    }

    for clip in parent_snap.clips.values() {
        if delta.removed_clips.contains(&clip.name) {
            let (badge, is_audio) = short_track_badge(&clip.track_name);
            let start_tc = format_tc_secs(clip.timeline_offset_sec);
            let end_tc = format_tc_secs(clip.timeline_offset_sec + clip.duration_seconds);
            rows.push(CommitChangeRow {
                track: badge,
                is_audio,
                media_file: clip.name.clone(),
                media_path: clip.media_path.clone(),
                change_type: "Deleted".to_string(),
                position: format!("{start_tc} -> {end_tc}"),
            });
        }
    }

    rows
}

fn summarize_commit_display(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    raw_message: &str,
) -> (String, Vec<String>) {
    let mut tracks: Vec<String> = Vec::new();
    let is_generic = raw_message.trim().is_empty()
        || raw_message.contains("Timeline snapshot")
        || raw_message.starts_with("micro-save");

    if let Some(curr_raw) = read_commit_cut_otio(repo, commit) {
        let prev_raw = commit
            .parent(0)
            .ok()
            .and_then(|p| read_commit_cut_otio(repo, &p))
            .unwrap_or_else(|| {
                r#"{"OTIO_SCHEMA":"Timeline.1","name":"Timeline","tracks":{"OTIO_SCHEMA":"Stack.1","children":[]}}"#
                    .to_string()
            });
        if let Ok(report) = otio_diff::extract_otio_delta(&prev_raw, &curr_raw) {
            for t in &report.tracks_affected {
                let (badge, _) = short_track_badge(t);
                if !tracks.contains(&badge) {
                    tracks.push(badge);
                }
            }
            if is_generic && !report.details.is_empty() {
                return (otio_diff::generate_semantic_commit_message(&report), tracks);
            }
        }
    }

    if tracks.is_empty() {
        for word in raw_message.split(|c: char| !c.is_ascii_alphanumeric()) {
            let is_track_token = (word.starts_with('V') || word.starts_with('A'))
                && word.len() >= 2
                && word[1..].chars().all(|ch| ch.is_ascii_digit());
            if is_track_token && !tracks.contains(&word.to_string()) {
                tracks.push(word.to_string());
            }
        }
    }

    let cleaned_msg = if is_generic {
        if tracks.is_empty() {
            "[Edit] Adjusted timeline cut".to_string()
        } else {
            format!("[Edit] Adjusted timeline cut on {}", tracks.join(", "))
        }
    } else {
        raw_message.to_string()
    };

    (cleaned_msg, tracks)
}

pub(crate) fn compute_commit_changes(repo_path: &Path, oid_str: &str) -> Vec<CommitChangeRow> {
    let Ok(repo) = Repository::open(repo_path) else {
        return Vec::new();
    };
    let Ok(oid) = Oid::from_str(oid_str) else {
        return Vec::new();
    };
    let Ok(commit) = repo.find_commit(oid) else {
        return Vec::new();
    };
    let Some(curr_raw) = read_commit_cut_otio(&repo, &commit) else {
        return Vec::new();
    };
    let Ok(curr_snap) = otio_engine::parse_otio(&curr_raw) else {
        return Vec::new();
    };
    let parent_snap = commit
        .parent(0)
        .ok()
        .and_then(|p| read_commit_cut_otio(&repo, &p))
        .and_then(|raw| otio_engine::parse_otio(&raw).ok())
        .unwrap_or_default();

    diff_snapshots_to_rows(&curr_snap, &parent_snap)
}

pub(crate) fn compute_burst_changes(
    repo_path: &Path,
    newest_oid_str: &str,
    oldest_oid_str: &str,
) -> Vec<CommitChangeRow> {
    let Ok(repo) = Repository::open(repo_path) else {
        return Vec::new();
    };
    let Ok(newest_oid) = Oid::from_str(newest_oid_str) else {
        return Vec::new();
    };
    let Ok(oldest_oid) = Oid::from_str(oldest_oid_str) else {
        return Vec::new();
    };
    let Ok(newest_commit) = repo.find_commit(newest_oid) else {
        return Vec::new();
    };
    let Ok(oldest_commit) = repo.find_commit(oldest_oid) else {
        return Vec::new();
    };
    let Some(curr_raw) = read_commit_cut_otio(&repo, &newest_commit) else {
        return Vec::new();
    };
    let Ok(curr_snap) = otio_engine::parse_otio(&curr_raw) else {
        return Vec::new();
    };
    let parent_snap = oldest_commit
        .parent(0)
        .ok()
        .and_then(|p| read_commit_cut_otio(&repo, &p))
        .and_then(|raw| otio_engine::parse_otio(&raw).ok())
        .unwrap_or_default();

    diff_snapshots_to_rows(&curr_snap, &parent_snap)
}

fn collect_branch_tags(repo: &Repository) -> HashMap<Oid, Vec<String>> {
    let mut map: HashMap<Oid, Vec<String>> = HashMap::new();
    let head_oid = repo.head().ok().and_then(|h| h.target());

    if let Ok(branches) = repo.branches(Some(BranchType::Local)) {
        for (branch, _) in branches.flatten() {
            let Some(name) = branch.name().ok().flatten().map(str::to_string) else {
                continue;
            };
            if name.starts_with("scratch/") {
                continue;
            }
            if let Some(oid) = branch.get().target() {
                let label = if head_oid == Some(oid)
                    && name
                        == branch_manager::current_branch_name(
                            repo.workdir().unwrap_or_else(|| Path::new(".")),
                        )
                {
                    format!("HEAD, {name}")
                } else {
                    name
                };
                map.entry(oid).or_default().push(label);
            }
        }
    }
    if let Ok(branches) = repo.branches(Some(BranchType::Remote)) {
        for (branch, _) in branches.flatten() {
            let Some(name) = branch.name().ok().flatten().map(str::to_string) else {
                continue;
            };
            if name == "origin/HEAD" || (!name.starts_with("origin/pass/") && name != "origin/main")
            {
                continue;
            }
            if let Some(oid) = branch.get().target() {
                let entry = map.entry(oid).or_default();
                if !entry.iter().any(|existing| existing == &name) {
                    entry.push(name);
                }
            }
        }
    }
    map
}

fn find_pipeline_bat() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("pipeline").join("start_pipeline.bat");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let local = PathBuf::from("pipeline").join("start_pipeline.bat");
    if local.is_file() {
        return Some(local);
    }
    None
}

impl ResolveSidecarApp {
    fn new() -> Self {
        let mut rclone_handle = None;
        let mut boot_status = "System ready".to_string();
        if !Path::new(r"Z:\").exists() {
            if let Some(pipeline_bat) = find_pipeline_bat() {
                let mut cmd = Command::new("cmd.exe");
                cmd.args(["/C", &pipeline_bat.to_string_lossy()]);
                if let Some(parent) = pipeline_bat.parent() {
                    cmd.current_dir(parent);
                }
                configure_hidden_command(&mut cmd);
                match cmd.spawn() {
                    Ok(_) => {
                        boot_status = "Starting pipeline (Z: mount + transcode worker)...".to_string();
                    }
                    Err(err) => {
                        boot_status = format!("Pipeline start error: {err}");
                    }
                }
            } else {
                match mount_engine::mount_z_drive() {
                    Ok(child_opt) => {
                        rclone_handle = child_opt;
                        if mount_engine::is_z_mounted() {
                            boot_status = "Mounted Z:\\ via rclone WebDAV".to_string();
                        }
                    }
                    Err(err) => {
                        boot_status = format!("Z:\\ mount skipped: {err}");
                    }
                }
            }
        }

        let repo_path = detect_active_repo_path();
        let needs_repo_setup = !repo_path.join(".git").exists() && !Path::new(".git").exists();
        if !needs_repo_setup {
            git_engine::ensure_gitignore_excludes_restore_otio(&repo_path);
        }
        if let Some(deploy_msg) = auto_deploy_resolve_scripts() {
            boot_status = deploy_msg;
        }
        let (timeline_watcher, event_rx) = if needs_repo_setup {
            (None, None)
        } else {
            match watcher::start_timeline_watcher(repo_path.clone()) {
                Some((w, rx)) => (Some(w), Some(rx)),
                None => (None, None),
            }
        };
        let default_editor = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "Editor".to_string());

        let saved_projects = projects::load_saved_projects(&repo_path);
        let current_view = AppView::HomeProjects;

        let mut app = Self {
            repo_path,
            commits: Vec::new(),
            status_message: boot_status,
            resolve_running: is_resolve_running(),
            storage_state: detect_storage_state(rclone_handle.is_some()),
            current_branch: "main".to_string(),
            selected_commit_oid: None,
            selected_burst_range: None,
            selected_commit_changes: Vec::new(),
            expanded_group_ids: HashSet::new(),
            show_fork_sandbox_modal: false,
            fork_sandbox_name: "cold-open".to_string(),
            ai_model_state: ai_engine::start_background_model_setup(),
            remote_sync_state: git_engine::RemoteSyncState::Checking,
            z_drive_connected: mount_engine::is_z_mounted(),
            pr_review_branch: None,
            pr_review_changes: Vec::new(),
            pr_review_meta: PrReviewMeta::default(),
            show_tickets_drawer: false,
            tickets: Vec::new(),
            new_ticket_category: tickets::TicketCategory::Nitpick,
            new_ticket_title: String::new(),
            new_ticket_description: String::new(),
            needs_repo_setup,
            setup_remote_url: "https://github.com/THEREDNECK1/resolve-project-collab".to_string(),
            setup_editor_handle: default_editor,
            current_view,
            show_new_project_modal: false,
            saved_projects,
            new_project_name: String::new(),
            new_project_base_dir: r"C:\Users\zanem".to_string(),
            storage_drive_letter: "Z:".to_string(),
            storage_remote_name: "gdrive".to_string(),
            storage_remote_subfolder: "Resolve_Cloud_Storage".to_string(),
            rclone_remote_authenticated: false,
            rclone_oauth_in_progress: false,
            rclone_oauth_rx: None,
            github_oauth_in_progress: false,
            github_oauth_rx: None,
            ingest_watch_folder: r"Z:\_Inbox".to_string(),
            ingest_queue: Vec::new(),
            transcode_worker_handle: None,
            github_auth: app::GitHubAuthState::default(),
            timeline_watcher,
            event_rx,
            utility_daemon_handle: None,
            autostart_attempted_for_session: false,
            rclone_handle,
            last_health_poll: Instant::now(),
        };
        app.load_github_collab_state();
        app.check_rclone_remote_auth();
        if !app.needs_repo_setup {
            app.reload_git_dag();
        }
        app.ensure_resolve_autostart_daemon();
        app
    }

    pub(crate) fn reload_git_dag(&mut self) {
        self.resolve_running = is_resolve_running();
        let rclone_active = self.is_rclone_running();
        self.storage_state = detect_storage_state(rclone_active);
        self.z_drive_connected = Path::new(r"Z:\").exists();
        self.current_branch = branch_manager::current_branch_name(&self.repo_path);
        self.tickets = tickets::load_tickets(&self.repo_path);
        self.refresh_git_log();
    }

    pub(crate) fn refresh_git_log(&mut self) {
        self.commits.clear();
        let Ok(repo) = Repository::open(&self.repo_path) else {
            self.status_message = format!("Repo not found at {}", self.repo_path.display());
            return;
        };

        let branch_tags = collect_branch_tags(&repo);

        let mut main_chain = HashSet::new();
        if let Ok(main_branch) = repo.find_branch("main", BranchType::Local) {
            if let Some(mut cur) = main_branch.get().target() {
                for _ in 0..250 {
                    main_chain.insert(cur);
                    let Ok(c) = repo.find_commit(cur) else { break };
                    let Some(p) = c.parent_ids().next() else { break };
                    cur = p;
                }
            }
        }

        // Map non-main branch commits into Lane 1 (primary feature/sandbox) or Lane 2 (overlapping)
        let mut branch_lanes: HashMap<Oid, (usize, CommitBranchKind, String)> = HashMap::new();
        let mut non_main_branches: Vec<(String, Oid)> = Vec::new();
        let mut remote_refs_to_walk: Vec<String> = Vec::new();
        let mut next_lane = 1usize;
        if let Ok(branches) = repo.branches(Some(BranchType::Local)) {
            for (b, _) in branches.flatten() {
                if let (Some(n), Some(oid)) = (b.name().ok().flatten(), b.get().target()) {
                    if n != "main" && !n.starts_with("scratch/") {
                        non_main_branches.push((n.to_string(), oid));
                    }
                }
            }
            non_main_branches.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, tip) in &non_main_branches {
                let assigned_lane = next_lane.min(2);
                next_lane += 1;
                let kind = if name.starts_with("experiment/") || name.starts_with("sandbox/") {
                    CommitBranchKind::Experiment
                } else {
                    CommitBranchKind::Pass
                };
                if let Ok(mut w) = repo.revwalk() {
                    let _ = w.push(*tip);
                    for oid in w.take(80).flatten() {
                        if main_chain.contains(&oid) {
                            break;
                        }
                        branch_lanes
                            .entry(oid)
                            .or_insert_with(|| (assigned_lane, kind, name.clone()));
                    }
                }
            }
        }

        if let Ok(remote_branches) = repo.branches(Some(BranchType::Remote)) {
            for (b, _) in remote_branches.flatten() {
                let Some(name) = b.name().ok().flatten().map(str::to_string) else {
                    continue;
                };
                if name != "origin/main" && !name.starts_with("origin/pass/") {
                    continue;
                }
                let ref_name = format!("refs/remotes/{name}");
                if let Some(tip) = b.get().target() {
                    remote_refs_to_walk.push(ref_name);
                    if !main_chain.contains(&tip) {
                        let assigned_lane = next_lane.min(2);
                        next_lane += 1;
                        if let Ok(mut w) = repo.revwalk() {
                            let _ = w.push(tip);
                            for oid in w.take(80).flatten() {
                                if main_chain.contains(&oid) {
                                    break;
                                }
                                branch_lanes.entry(oid).or_insert_with(|| {
                                    (assigned_lane, CommitBranchKind::Pass, name.clone())
                                });
                            }
                        }
                    }
                }
            }
        }

        if let Ok(mut revwalk) = repo.revwalk() {
            let _ = revwalk.set_sorting(Sort::TIME | Sort::TOPOLOGICAL);
            let mut pushed = false;
            if repo.find_reference("refs/heads/main").is_ok() {
                let _ = revwalk.push_ref("refs/heads/main");
                pushed = true;
            }
            for (name, _) in &non_main_branches {
                if revwalk.push_ref(&format!("refs/heads/{name}")).is_ok() {
                    pushed = true;
                }
            }
            for rref in &remote_refs_to_walk {
                if revwalk.push_ref(rref).is_ok() {
                    pushed = true;
                }
            }
            if !pushed {
                let _ = revwalk.push_head();
            }

            let mut skipped_parent_map: HashMap<String, String> = HashMap::new();
            for oid in revwalk.take(60).flatten() {
                let Ok(commit) = repo.find_commit(oid) else { continue };
                let message = commit.summary().unwrap_or("No message").to_string();
                let oid_str = oid.to_string();
                if message.starts_with("review(") {
                    if let Some(fp) = commit.parent_ids().next() {
                        skipped_parent_map.insert(oid_str, fp.to_string());
                    }
                    continue;
                }

                let (lane, branch_kind, branch_name) = if main_chain.contains(&oid) {
                    (0, CommitBranchKind::Main, None)
                } else if let Some((l, k, bname)) = branch_lanes.get(&oid) {
                    (*l, *k, Some(bname.clone()))
                } else {
                    (1, CommitBranchKind::Pass, None)
                };

                let parent_ids = commit
                    .parent_ids()
                    .map(|pid| {
                        let mut cur = pid.to_string();
                        while let Some(next) = skipped_parent_map.get(&cur) {
                            cur = next.clone();
                        }
                        cur
                    })
                    .collect();

                let (display_message, tracks) =
                    summarize_commit_display(&repo, &commit, &message);

                let ts = commit.time().seconds();
                self.commits.push(GitCommitNode {
                    full_id: oid_str,
                    short_id: format!("{:.7}", oid),
                    message: display_message,
                    author: commit.author().name().unwrap_or("Zane").to_string(),
                    time: format_relative_time(ts),
                    timestamp_secs: ts,
                    tag: branch_tags.get(&oid).map(|l| l.join(", ")).filter(|s| !s.is_empty()),
                    parent_ids,
                    lane,
                    is_merge: commit.parent_count() > 1,
                    branch_kind,
                    branch_name,
                    tracks,
                });
            }
        }

        if let Some(first_oid) = self.commits.first().map(|c| c.full_id.clone()) {
            let still_exists = self
                .selected_commit_oid
                .as_deref()
                .is_some_and(|sel| self.commits.iter().any(|c| c.full_id == sel));
            if !still_exists {
                self.select_commit(first_oid);
            } else if let Some((newest, oldest)) = self.selected_burst_range.clone() {
                self.selected_commit_changes =
                    compute_burst_changes(&self.repo_path, &newest, &oldest);
            } else if let Some(sel) = self.selected_commit_oid.clone() {
                self.selected_commit_changes = compute_commit_changes(&self.repo_path, &sel);
            }
        } else {
            self.selected_commit_oid = None;
            self.selected_burst_range = None;
            self.selected_commit_changes.clear();
        }
        self.status_message = format!("Synced ({} commits loaded)", self.commits.len());
    }

    pub(crate) fn select_commit(&mut self, full_oid: String) {
        self.selected_burst_range = None;
        self.selected_commit_changes = compute_commit_changes(&self.repo_path, &full_oid);
        self.selected_commit_oid = Some(full_oid);
    }

    pub(crate) fn select_burst_group(&mut self, newest_oid: String, oldest_oid: String) {
        self.selected_commit_changes =
            compute_burst_changes(&self.repo_path, &newest_oid, &oldest_oid);
        self.selected_burst_range = Some((newest_oid.clone(), oldest_oid));
        self.selected_commit_oid = Some(newest_oid);
    }

    pub(crate) fn toggle_group_expanded(&mut self, key: &str) {
        if !self.expanded_group_ids.remove(key) {
            self.expanded_group_ids.insert(key.to_string());
        }
    }

    pub(crate) fn sync_repo_now(&mut self) {
        let mut fetch_note = String::new();
        if let Ok(repo) = Repository::open(&self.repo_path) {
            if let Ok(remotes) = repo.remotes() {
                if !remotes.is_empty() {
                    let mut cmd = Command::new("git");
                    cmd.current_dir(&self.repo_path)
                        .args(["fetch", "--all", "--prune"]);
                    configure_hidden_command(&mut cmd);
                    if let Ok(out) = cmd.output() {
                        if out.status.success() {
                            fetch_note = " (fetched remotes)".to_string();
                        }
                    }
                }
            }
        }
        let _ = watcher::commit_micro_save_if_dirty_with_msg(&self.repo_path, None);
        self.reload_git_dag();
        self.status_message = format!(
            "Synced {} ({} commits){fetch_note}",
            self.repo_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("repo"),
            self.commits.len()
        );
    }

    pub(crate) fn on_click_fork_sandbox_cut(&mut self) {
        let name = self.fork_sandbox_name.clone();
        match git_engine::fork_sandbox_cut(&self.repo_path, &name) {
            Ok((branch, tl_name)) => {
                self.reload_git_dag();
                self.status_message = format!("Forked '{branch}' -> Resolve '{tl_name}'");
            }
            Err(err) => self.status_message = format!("Fork error: {err}"),
        }
    }

    pub(crate) fn merge_branch_to_main(&mut self, branch_name: &str) {
        match git_engine::merge_branch_into_main(&self.repo_path, branch_name) {
            Ok(_short_oid) => {
                self.pr_review_branch = None;
                self.pr_review_changes.clear();
                self.pr_review_meta = PrReviewMeta::default();
                self.reload_git_dag();
                if let Some(first_oid) = self.commits.first().map(|c| c.full_id.clone()) {
                    self.select_commit(first_oid);
                }
                self.status_message = "Merged pass and pushed to remote main!".to_string();

                let push_repo = self.repo_path.clone();
                let push_tx = self.timeline_watcher.as_ref().map(|w| w.sender());
                let _ = std::thread::Builder::new()
                    .name("resolve-sidecar-git-push".to_string())
                    .spawn(move || {
                        let res = git_engine::push_main_to_origin(&push_repo);
                        if let Some(tx) = push_tx {
                            let _ = tx.send(watcher::AppEvent::PushCompleted(res));
                        }
                    });
            }
            Err(err) => self.status_message = format!("Merge error: {err}"),
        }
    }

    pub(crate) fn resolve_branch_tip_oid(repo_path: &Path, branch_name: &str) -> Option<Oid> {
        let repo = Repository::open(repo_path).ok()?;
        let clean = branch_name.trim();
        for ref_name in [
            format!("refs/heads/{clean}"),
            format!("refs/remotes/{clean}"),
        ] {
            if let Ok(r) = repo.find_reference(&ref_name) {
                if let Some(oid) = r.target() {
                    return Some(oid);
                }
            }
        }
        None
    }

    pub(crate) fn open_pr_review(&mut self, branch_name: &str) {
        let Ok(repo) = Repository::open(&self.repo_path) else {
            return;
        };
        let Some(branch_oid) = Self::resolve_branch_tip_oid(&self.repo_path, branch_name) else {
            self.status_message = format!("Unable to locate branch '{branch_name}'");
            return;
        };
        let Ok(branch_commit) = repo.find_commit(branch_oid) else {
            return;
        };
        let author = branch_commit
            .author()
            .name()
            .unwrap_or("Unknown")
            .to_string();
        let commit_count = {
            let mut count = 0usize;
            if let Ok(mut walk) = repo.revwalk() {
                if walk.push(branch_oid).is_ok() {
                    if let Ok(main_oid) = repo.refname_to_id("refs/heads/main") {
                        let _ = walk.hide(main_oid);
                    }
                    count = walk.count();
                }
            }
            count
        };

        let Some(curr_raw) = read_commit_cut_otio(&repo, &branch_commit) else {
            self.pr_review_changes.clear();
            self.pr_review_meta = PrReviewMeta {
                author,
                commit_count,
                ..PrReviewMeta::default()
            };
            self.pr_review_branch = Some(branch_name.to_string());
            return;
        };
        let Ok(curr_snap) = otio_engine::parse_otio(&curr_raw) else {
            return;
        };

        let main_snap = repo
            .find_reference("refs/heads/main")
            .ok()
            .and_then(|r| r.peel_to_commit().ok())
            .and_then(|c| read_commit_cut_otio(&repo, &c))
            .and_then(|raw| otio_engine::parse_otio(&raw).ok())
            .unwrap_or_default();

        let rows = diff_snapshots_to_rows(&curr_snap, &main_snap);
        let total_cuts = rows
            .iter()
            .filter(|r| {
                !r.is_audio && matches!(r.change_type.as_str(), "Split" | "Added" | "Deleted" | "Trimmed")
            })
            .count();
        let audio_edits = rows.iter().filter(|r| r.is_audio).count();
        let duration_delta_sec = curr_snap.total_duration - main_snap.total_duration;

        self.pr_review_changes = rows;
        self.pr_review_meta = PrReviewMeta {
            author,
            commit_count,
            total_cuts,
            audio_edits,
            duration_delta_sec,
        };
        self.pr_review_branch = Some(branch_name.to_string());
    }

    pub(crate) fn close_pr_review(&mut self) {
        self.pr_review_branch = None;
        self.pr_review_changes.clear();
        self.pr_review_meta = PrReviewMeta::default();
    }

    pub(crate) fn toggle_tickets_drawer(&mut self) {
        self.show_tickets_drawer = !self.show_tickets_drawer;
        if self.show_tickets_drawer {
            self.current_view = AppView::Timeline;
            self.close_pr_review();
            self.tickets = tickets::load_tickets(&self.repo_path);
        }
    }

    pub(crate) fn active_head_short_hash(&self) -> String {
        self.commits
            .first()
            .map(|c| c.short_id.clone())
            .unwrap_or_else(|| "HEAD".to_string())
    }

    pub(crate) fn active_timeline_name(&self) -> String {
        let cut_path = self.repo_path.join("Timelines").join("cut.otio");
        if let Ok(raw) = std::fs::read_to_string(cut_path) {
            if let Ok(snap) = otio_engine::parse_otio(&raw) {
                return snap.name;
            }
        }
        "Master Timeline".to_string()
    }

    pub(crate) fn submit_new_ticket(&mut self) {
        let title = self.new_ticket_title.trim().to_string();
        if title.is_empty() {
            self.status_message = "Ticket title cannot be empty.".to_string();
            return;
        }
        let author = if self.setup_editor_handle.trim().is_empty() {
            "Editor".to_string()
        } else {
            self.setup_editor_handle.trim().to_string()
        };
        let ticket = tickets::Ticket {
            id: format!("t-{}", Utc::now().timestamp_millis()),
            category: self.new_ticket_category,
            title: title.clone(),
            description: self.new_ticket_description.trim().to_string(),
            author,
            created_at: Utc::now().format("%Y-%m-%d %H:%M").to_string(),
            status: tickets::TicketStatus::Open,
            commit_hash: self.active_head_short_hash(),
            timeline_name: self.active_timeline_name(),
        };
        self.tickets.insert(0, ticket);
        let msg = format!("ticket({}): {}", self.new_ticket_category.label(), title);
        match tickets::save_and_commit_tickets(&self.repo_path, &self.tickets, &msg) {
            Ok(()) => {
                self.new_ticket_title.clear();
                self.new_ticket_description.clear();
                self.reload_git_dag();
                self.status_message = format!("Submitted & committed ticket: {title}");
            }
            Err(err) => {
                self.status_message = format!("Ticket save error: {err}");
            }
        }
    }

    pub(crate) fn toggle_ticket_status(&mut self, ticket_id: &str) {
        let mut updated_title = None;
        let mut next_label = "";
        for t in &mut self.tickets {
            if t.id == ticket_id {
                t.status = match t.status {
                    tickets::TicketStatus::Open => tickets::TicketStatus::Resolved,
                    tickets::TicketStatus::Resolved => tickets::TicketStatus::Open,
                };
                updated_title = Some(t.title.clone());
                next_label = t.status.label();
                break;
            }
        }
        if let Some(title) = updated_title {
            let msg = format!("ticket: mark '{title}' {next_label}");
            match tickets::save_and_commit_tickets(&self.repo_path, &self.tickets, &msg) {
                Ok(()) => {
                    self.reload_git_dag();
                    self.status_message = format!("Ticket '{title}' -> {next_label}");
                }
                Err(err) => {
                    self.status_message = format!("Ticket update error: {err}");
                }
            }
        }
    }

    pub(crate) fn clone_and_start_editing(&mut self) {
        let url = self.setup_remote_url.trim().to_string();
        if url.is_empty() {
            self.status_message = "Git Remote URL cannot be empty.".to_string();
            return;
        }
        let target_dir = self.repo_path.clone();
        let dir_has_entries = target_dir
            .read_dir()
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);

        let clone_ok = if !target_dir.exists() || !dir_has_entries {
            if let Some(parent) = target_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut cmd = Command::new("git");
            cmd.args(["clone", &url]).arg(&target_dir);
            configure_hidden_command(&mut cmd);
            cmd.output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        } else {
            let _ = Command::new("git")
                .args(["init"])
                .current_dir(&target_dir)
                .status();
            let _ = Command::new("git")
                .args(["remote", "remove", "origin"])
                .current_dir(&target_dir)
                .status();
            let _ = Command::new("git")
                .args(["remote", "add", "origin", &url])
                .current_dir(&target_dir)
                .status();
            let fetch_ok = Command::new("git")
                .args(["fetch", "origin"])
                .current_dir(&target_dir)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if fetch_ok {
                let _ = Command::new("git")
                    .args(["checkout", "-B", "main", "origin/main"])
                    .current_dir(&target_dir)
                    .status();
            }
            fetch_ok
        };

        if !clone_ok && Repository::open(&target_dir).is_err() {
            self.status_message = format!("Failed to clone {url}");
            return;
        }

        let handle = self.setup_editor_handle.trim();
        if !handle.is_empty() {
            let mut cfg = Command::new("git");
            cfg.args(["config", "user.name", handle])
                .current_dir(&target_dir);
            configure_hidden_command(&mut cfg);
            let _ = cfg.status();
        }

        git_engine::ensure_gitignore_excludes_restore_otio(&target_dir);
        if self.timeline_watcher.is_none() {
            if let Some((w, rx)) = watcher::start_timeline_watcher(target_dir.clone()) {
                self.timeline_watcher = Some(w);
                self.event_rx = Some(rx);
            }
        }
        self.needs_repo_setup = false;
        self.reload_git_dag();
        self.status_message = format!("Connected to {url}");
    }

    pub(crate) fn consolidate_media_to_z(&mut self, local_path: &str) {
        if !Path::new(r"Z:\").exists() {
            self.status_message = "Cannot consolidate: Z:\\ drive is unmounted.".to_string();
            return;
        }
        let src = Path::new(local_path.trim());
        if !src.is_file() {
            self.status_message = format!("Source media not found on disk: {local_path}");
            return;
        }
        let Some(file_name) = src.file_name() else {
            self.status_message = format!("Invalid media path: {local_path}");
            return;
        };

        let subfolder = otio_engine::target_z_subfolder_for_media(src);
        let dest_dir = PathBuf::from(r"Z:\").join(subfolder);
        if let Err(err) = std::fs::create_dir_all(&dest_dir) {
            self.status_message = format!("Failed creating {}: {err}", dest_dir.display());
            return;
        }
        let dest_path = dest_dir.join(file_name);
        if !dest_path.exists() {
            if let Err(err) = std::fs::copy(src, &dest_path) {
                self.status_message = format!("Failed copying media to Z:\\: {err}");
                return;
            }
        }

        let cut_otio_path = self.repo_path.join("Timelines").join("cut.otio");
        let dest_str = dest_path.to_string_lossy().to_string();
        match otio_engine::rewrite_otio_media_target(&cut_otio_path, local_path, &dest_str) {
            Ok(0) => {
                self.status_message = format!(
                    "Copied {} to Z:\\{subfolder}\\ (no matching reference in active cut.otio)",
                    file_name.to_string_lossy()
                );
            }
            Ok(n) => {
                self.status_message = format!(
                    "Consolidated {} to Z:\\{subfolder}\\ ({n} ref updated; auto-committing)",
                    file_name.to_string_lossy()
                );
            }
            Err(err) => {
                self.status_message = format!("Copied to Z:\\ but failed updating cut.otio: {err}");
            }
        }
    }

    pub(crate) fn preview_branch_cut_in_resolve(&mut self, branch_name: &str) {
        let Some(oid) = Self::resolve_branch_tip_oid(&self.repo_path, branch_name) else {
            self.status_message = format!("Branch '{branch_name}' not found for preview");
            return;
        };
        ui::trigger_restore_to_resolve(self, &oid.to_string());
    }

    pub(crate) fn pull_latest_from_origin(&mut self) {
        match git_engine::pull_latest_main(&self.repo_path) {
            Ok(_) => {
                self.reload_git_dag();
                if let Some(first_oid) = self.commits.first().map(|c| c.full_id.clone()) {
                    self.select_commit(first_oid);
                }
                let behind = git_engine::check_remote_behind_count(&self.repo_path);
                self.remote_sync_state = if behind > 0 {
                    git_engine::RemoteSyncState::Behind(behind)
                } else {
                    git_engine::RemoteSyncState::Synced
                };
                self.status_message = "Pulled latest changes from origin/main".to_string();
            }
            Err(err) => {
                self.status_message = format!("Pull error: {err}");
            }
        }
    }

    pub(crate) fn discard_sandbox_branch(&mut self, branch_name: &str) {
        match git_engine::discard_branch(&self.repo_path, branch_name) {
            Ok(_) => {
                self.reload_git_dag();
                if let Some(first_oid) = self.commits.first().map(|c| c.full_id.clone()) {
                    self.select_commit(first_oid);
                }
                self.status_message = format!("Discarded '{branch_name}' -> reset to main");
            }
            Err(err) => self.status_message = format!("Discard error: {err}"),
        }
    }

    pub(crate) fn is_rclone_running(&mut self) -> bool {
        self.rclone_handle
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .map(|s| s.is_none())
            .unwrap_or(false)
    }

    pub(crate) fn mount_z_drive_now(&mut self) {
        if mount_engine::is_z_mounted() {
            self.z_drive_connected = true;
            self.status_message = "Z:\\ is already mounted.".to_string();
            return;
        }
        if let Some(mut old) = self.rclone_handle.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        match mount_engine::mount_z_drive() {
            Ok(child_opt) => {
                self.rclone_handle = child_opt;
                self.z_drive_connected = mount_engine::is_z_mounted();
                if self.z_drive_connected {
                    self.repo_path = detect_active_repo_path();
                    self.reload_git_dag();
                    self.status_message = "Mounted Z:\\ (gdrive) via rclone WebDAV".to_string();
                } else {
                    self.status_message = "rclone started, waiting for Z:\\ mount".to_string();
                }
            }
            Err(err) => {
                self.z_drive_connected = mount_engine::is_z_mounted();
                self.status_message = format!("Mount Z:\\ failed: {err}");
            }
        }
    }

    pub(crate) fn poll_watcher_events(&mut self, ctx: &egui::Context) {
        let drained: Vec<watcher::AppEvent> = if let Some(ref rx) = self.event_rx {
            let mut list = Vec::new();
            while let Ok(evt) = rx.try_recv() {
                list.push(evt);
            }
            list
        } else {
            Vec::new()
        };

        if drained.is_empty() {
            return;
        }

        let mut repo_updated = false;
        for evt in drained {
            match evt {
                watcher::AppEvent::RepoUpdated => {
                    repo_updated = true;
                }
                watcher::AppEvent::RemoteStatusChanged(state) => {
                    self.remote_sync_state = state;
                }
                watcher::AppEvent::PushCompleted(res) => match res {
                    Ok(()) => {
                        self.reload_git_dag();
                        self.remote_sync_state = git_engine::RemoteSyncState::Synced;
                        self.status_message = "Merged pass and pushed to remote main!".to_string();
                    }
                    Err(err) => {
                        self.status_message = format!("Merged locally; push warning: {err}");
                    }
                },
            }
        }

        if repo_updated {
            self.reload_git_dag();
            if let Some(first) = self.commits.first().cloned() {
                self.select_commit(first.full_id.clone());
                self.status_message = format!("Committed {}: {}", first.short_id, first.message);
            }
            let behind = git_engine::check_remote_behind_count(&self.repo_path);
            self.remote_sync_state = if behind > 0 {
                git_engine::RemoteSyncState::Behind(behind)
            } else {
                git_engine::RemoteSyncState::Synced
            };
        }

        ctx.request_repaint();
    }

    fn find_sidecar_utility_script() -> Option<PathBuf> {
        let local = PathBuf::from("sidecar_utility.py");
        if local.is_file() {
            return Some(local);
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let adjacent = dir.join("sidecar_utility.py");
                if adjacent.is_file() {
                    return Some(adjacent);
                }
            }
        }
        if let Some(appdata) = dirs::data_dir() {
            for rel in [
                ["Developer", "Scripting", "Utility"],
                ["Fusion", "Scripts", "Utility"],
            ] {
                let candidate = appdata
                    .join("Blackmagic Design")
                    .join("DaVinci Resolve")
                    .join("Support")
                    .join(rel[0])
                    .join(rel[1])
                    .join(rel[2])
                    .join("sidecar_utility.py");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    fn is_utility_daemon_listening() -> bool {
        use std::net::{SocketAddr, TcpStream};
        let addr = SocketAddr::from(([127, 0, 0, 1], 47831));
        TcpStream::connect_timeout(&addr, Duration::from_millis(60)).is_ok()
    }

    #[cfg(windows)]
    fn post_startup_utility_hotkey() {
        use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_F12;
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
        if let Some(hwnd) = found_hwnd {
            let vk_code = VK_F12.0;
            let scan_code = 0x58u32;
            let wparam = WPARAM(vk_code as usize);
            let lparam_down = LPARAM(((scan_code << 16) | 1) as isize);
            let lparam_up = LPARAM((0xC000_0001u32 | (scan_code << 16)) as i32 as isize);
            unsafe {
                let _ = PostMessageA(hwnd, WM_KEYDOWN, wparam, lparam_down);
                let _ = PostMessageA(hwnd, WM_KEYUP, wparam, lparam_up);
            }
        }
    }

    #[cfg(not(windows))]
    fn post_startup_utility_hotkey() {}

    fn ensure_resolve_autostart_daemon(&mut self) {
        if !self.resolve_running {
            self.autostart_attempted_for_session = false;
            if let Some(mut child) = self.utility_daemon_handle.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return;
        }
        let still_alive = self
            .utility_daemon_handle
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .is_some_and(|s| s.is_none());
        if still_alive || Self::is_utility_daemon_listening() || self.autostart_attempted_for_session
        {
            return;
        }
        self.autostart_attempted_for_session = true;
        if let Some(script) = Self::find_sidecar_utility_script() {
            let mut cmd = Command::new("python");
            cmd.arg(script).arg("--autostart");
            configure_hidden_command(&mut cmd);
            if let Ok(child) = cmd.spawn() {
                self.utility_daemon_handle = Some(child);
            }
        }
        // Trigger Resolve's bound Utility hotkey once per Resolve session so Free edition starts in-process.
        if !Self::is_utility_daemon_listening() {
            Self::post_startup_utility_hotkey();
        }
    }
}

impl eframe::App for ResolveSidecarApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_watcher_events(ctx);
        self.poll_rclone_oauth(ctx);
        self.poll_github_oauth(ctx);

        if self.last_health_poll.elapsed() >= Duration::from_secs(3) {
            self.last_health_poll = Instant::now();
            self.resolve_running = is_resolve_running();
            self.ensure_resolve_autostart_daemon();
            self.z_drive_connected = Path::new(r"Z:\").exists();
            let rclone_active = self.is_rclone_running();
            self.storage_state = detect_storage_state(rclone_active);
        }

        ctx.request_repaint_after(Duration::from_millis(400));
        ui::render_app(self, ctx);
    }
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([860.0, 560.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/lanya_icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Lanya",
        options,
        Box::new(|_cc| Box::new(ResolveSidecarApp::new())),
    )
}
