#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai_assistant;
mod branch_manager;
mod db_watcher;
mod diff;
mod interchange;
#[allow(dead_code)]
mod resolve_db;
mod reviews;
mod ui;
mod watcher;

use chrono::Utc;
use eframe::egui;
use git2::{BranchType, IndexAddOption, Oid, Repository, Signature, Sort};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone)]
struct GitCommitNode {
    short_id: String,
    message: String,
    author: String,
    time: String,
    tag: Option<String>,
}

#[derive(Clone)]
struct PullRequest {
    title: String,
    author: String,
    branch_name: String,
    is_remote: bool,
    time_ago: String,
    notes: String,
    tracks: Vec<String>,
    media_count: usize,
    reviews: Vec<reviews::ReviewComment>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct TimelineClip {
    pub(crate) track_type: String,
    pub(crate) track_index: usize,
    pub(crate) clip_name: String,
    pub(crate) media_path: String,
    pub(crate) start_frame: i64,
    pub(crate) end_frame: i64,
    pub(crate) duration: i64,
    pub(crate) source_in: i64,
    pub(crate) source_out: i64,
    pub(crate) speed: f64,
    pub(crate) cdl: Option<serde_json::Value>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct TimelineDiffItem {
    pub(crate) change_type: String,
    pub(crate) clip: TimelineClip,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) previous_clip: Option<TimelineClip>,
}

#[derive(Clone)]
pub(crate) struct StagedDiffItem {
    pub(crate) selected: bool,
    pub(crate) item: TimelineDiffItem,
}

#[derive(Deserialize)]
struct PipelineConfig {
    default_repo_path: Option<String>,
    fallback_repo_path: Option<String>,
    active_project: Option<String>,
    ai_endpoint: Option<String>,
    ai_model: Option<String>,
}

fn load_pipeline_config() -> Option<PipelineConfig> {
    let mut config_paths = vec![PathBuf::from("config").join("pipeline_config.json")];

    if let Ok(exe_path) = std::env::current_exe() {
        for ancestor in exe_path.ancestors().skip(1).take(4) {
            config_paths.push(ancestor.join("config").join("pipeline_config.json"));
        }
    }

    for cfg_path in config_paths {
        if let Ok(raw) = std::fs::read_to_string(&cfg_path) {
            if let Ok(cfg) = serde_json::from_str::<PipelineConfig>(&raw) {
                return Some(cfg);
            }
        }
    }
    None
}

fn load_pipeline_repo_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(cfg) = load_pipeline_config() {
        if let Some(p) = cfg.default_repo_path.filter(|s| !s.trim().is_empty()) {
            candidates.push(PathBuf::from(p));
        }
        if let Some(p) = cfg.fallback_repo_path.filter(|s| !s.trim().is_empty()) {
            candidates.push(PathBuf::from(p));
        }
    }

    let defaults = [
        PathBuf::from(r"C:\Users\zanem\Resolve_Cloud_Storage"),
        PathBuf::from(r"Z:\"),
    ];
    for p in defaults {
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    }
    candidates
}

fn detect_active_repo_path() -> PathBuf {
    if Repository::open(r"Z:\").is_ok() {
        return PathBuf::from(r"Z:\");
    }

    if Repository::open(".").is_ok() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }

    for candidate in load_pipeline_repo_candidates() {
        if candidate != Path::new(r"Z:\") && Repository::open(&candidate).is_ok() {
            return candidate;
        }
    }

    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageMountState {
    MountedZ,
    MountingZ,
    LocalProject,
}

#[derive(Clone)]
pub(crate) struct ScopeChecklist {
    pub(crate) video_cuts: bool,
    pub(crate) audio_music: bool,
    pub(crate) color_grade: bool,
    pub(crate) titles_graphics: bool,
    pub(crate) sfx_foley: bool,
}

impl Default for ScopeChecklist {
    fn default() -> Self {
        Self {
            video_cuts: true,
            audio_music: true,
            color_grade: false,
            titles_graphics: false,
            sfx_foley: false,
        }
    }
}

impl ScopeChecklist {
    pub(crate) fn selected_labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.video_cuts {
            labels.push("Video Cuts (V1/V2)");
        }
        if self.audio_music {
            labels.push("Audio / Music (A1-A3)");
        }
        if self.color_grade {
            labels.push("Color Grade");
        }
        if self.titles_graphics {
            labels.push("Titles / Graphics");
        }
        if self.sfx_foley {
            labels.push("SFX / Foley");
        }
        labels
    }
}

pub(crate) struct ResolveSidecarApp {
    pub(crate) repo_path: PathBuf,
    pub(crate) commits: Vec<GitCommitNode>,
    pub(crate) pending_prs: Vec<PullRequest>,
    pub(crate) status_message: String,
    pub(crate) new_pr_title: String,
    pub(crate) new_pr_notes: String,
    pub(crate) scope_checklist: ScopeChecklist,
    pub(crate) active_tab: usize,
    pub(crate) resolve_running: bool,
    pub(crate) storage_state: StorageMountState,
    pub(crate) staged_diffs: Vec<StagedDiffItem>,
    pub(crate) micro_commits: Vec<watcher::MicroCommitNode>,
    pub(crate) screenshot_path_input: String,
    pub(crate) attached_screenshot_rel: Option<String>,
    pub(crate) review_draft_body: String,
    pub(crate) sandbox_slug_input: String,
    pub(crate) current_branch: String,
    pending_snapshot: Option<serde_json::Value>,
    diff_project_name: String,
    diff_timeline_name: String,
    diff_fps: String,
    ai_endpoint: Option<String>,
    ai_model: Option<String>,
    rclone_handle: Option<Child>,
    transcode_worker: Option<Child>,
    last_mount_poll: Instant,
    live_watcher: Option<watcher::LiveWatcherHandle>,
    active_project_db: Option<PathBuf>,
}

impl Drop for ResolveSidecarApp {
    fn drop(&mut self) {
        if let Some(mut child) = self.transcode_worker.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.rclone_handle.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Default for ResolveSidecarApp {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolveSidecarApp {
    fn new() -> Self {
        let mut rclone_handle = None;
        if !Path::new(r"Z:\").exists() {
            rclone_handle = spawn_rclone_mount();
        }

        let transcode_worker = spawn_transcode_worker();
        let storage_state = detect_storage_state(rclone_handle.is_some());
        let repo_path = detect_active_repo_path();
        let live_watcher = watcher::start_live_watcher(repo_path.clone());
        let cfg = load_pipeline_config();
        let configured_project = cfg.as_ref().and_then(|c| c.active_project.clone());
        let ai_endpoint = cfg.as_ref().and_then(|c| c.ai_endpoint.clone());
        let ai_model = cfg.as_ref().and_then(|c| c.ai_model.clone());
        let active_project_db = db_watcher::find_latest_project_file(configured_project.as_deref());

        let mut app = Self {
            repo_path,
            commits: Vec::new(),
            pending_prs: Vec::new(),
            status_message: "System idle".to_string(),
            new_pr_title: "Music Pass v1".to_string(),
            new_pr_notes: "Added background music track on A3.".to_string(),
            scope_checklist: ScopeChecklist::default(),
            active_tab: 0,
            resolve_running: false,
            storage_state,
            staged_diffs: Vec::new(),
            micro_commits: Vec::new(),
            screenshot_path_input: String::new(),
            attached_screenshot_rel: None,
            review_draft_body: String::new(),
            sandbox_slug_input: "cold-open".to_string(),
            current_branch: "main".to_string(),
            pending_snapshot: None,
            diff_project_name: String::new(),
            diff_timeline_name: String::new(),
            diff_fps: "24".to_string(),
            ai_endpoint,
            ai_model,
            rclone_handle,
            transcode_worker,
            last_mount_poll: Instant::now(),
            live_watcher,
            active_project_db,
        };

        app.refresh_all();
        if app.commits.is_empty() && !app.baseline_path().exists() {
            app.initialize_root_baseline_if_needed();
        }
        app
    }
}

fn configure_hidden_command(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

fn find_rclone_binary() -> Option<PathBuf> {
    let mut local_candidates = vec![PathBuf::from(r"bin\rclone.exe")];
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            local_candidates.push(exe_dir.join("rclone.exe"));
            local_candidates.push(exe_dir.join("bin").join("rclone.exe"));
        }
    }

    for candidate in local_candidates {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let exe = dir.join("rclone.exe");
            if exe.is_file() {
                return Some(exe);
            }
            let bin = dir.join("rclone");
            if bin.is_file() {
                return Some(bin);
            }
        }
    }

    None
}

fn spawn_rclone_mount() -> Option<Child> {
    let rclone_bin = find_rclone_binary()?;
    let mut cmd = Command::new(rclone_bin);
    cmd.args([
        "serve",
        "webdav",
        "gdrive:Resolve_Cloud_Storage",
        "--addr",
        "127.0.0.1:8080",
    ]);
    configure_hidden_command(&mut cmd);
    cmd.spawn().ok()
}

fn resolve_python_command() -> (String, Vec<String>) {
    for ver in ["-3.11", "-3.12"] {
        let mut probe = Command::new("py");
        probe.args([ver, "-V"]);
        configure_hidden_command(&mut probe);
        if probe.output().map(|o| o.status.success()).unwrap_or(false) {
            return ("py".to_string(), vec![ver.to_string()]);
        }
    }

    let mut direct_paths = Vec::new();
    if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(&local_app).join("Programs").join("Python");
        direct_paths.push(base.join("Python311").join("python.exe"));
        direct_paths.push(PathBuf::from(r"C:\Program Files\Python311\python.exe"));
        direct_paths.push(base.join("Python312").join("python.exe"));
        direct_paths.push(PathBuf::from(r"C:\Program Files\Python312\python.exe"));
    } else {
        direct_paths.push(PathBuf::from(r"C:\Program Files\Python311\python.exe"));
        direct_paths.push(PathBuf::from(r"C:\Program Files\Python312\python.exe"));
    }

    for path in direct_paths {
        if path.is_file() {
            return (path.to_string_lossy().to_string(), Vec::new());
        }
    }

    let mut py3_probe = Command::new("py");
    py3_probe.args(["-3", "-V"]);
    configure_hidden_command(&mut py3_probe);
    if py3_probe.output().map(|o| o.status.success()).unwrap_or(false) {
        return ("py".to_string(), vec!["-3".to_string()]);
    }

    ("python".to_string(), Vec::new())
}

fn spawn_transcode_worker() -> Option<Child> {
    let worker_path = resolve_script_path("transcode_worker.py");
    if !worker_path.is_file() {
        return None;
    }
    let (cmd_bin, initial_args) = resolve_python_command();
    let mut cmd = Command::new(cmd_bin);
    cmd.args(initial_args).arg(&worker_path);
    configure_hidden_command(&mut cmd);
    cmd.spawn().ok()
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

fn is_resolve_running() -> bool {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("tasklist");
        cmd.args(["/FI", "IMAGENAME eq Resolve.exe", "/NH"]);
        configure_hidden_command(&mut cmd);
        if let Ok(output) = cmd.output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return stdout.to_ascii_lowercase().contains("resolve.exe");
        }
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn resolve_script_path(script_name: &str) -> PathBuf {
    let cwd_candidate = PathBuf::from("scripts").join(script_name);
    if cwd_candidate.is_file() {
        return cwd_candidate;
    }

    if let Ok(exe_path) = std::env::current_exe() {
        for ancestor in exe_path.ancestors().skip(1).take(4) {
            let candidate = ancestor.join("scripts").join(script_name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    cwd_candidate
}

fn format_relative_time(timestamp_secs: i64) -> String {
    let now = Utc::now().timestamp();
    let delta = now.saturating_sub(timestamp_secs);

    if delta < 60 {
        "just now".to_string()
    } else if delta < 3_600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3_600)
    } else if delta < 604_800 {
        format!("{}d ago", delta / 86_400)
    } else {
        chrono::DateTime::from_timestamp(timestamp_secs, 0)
            .map(|d| d.format("%b %d").to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

fn slugify_branch_title(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_dash = false;

    for ch in title.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "untitled-pass".to_string()
    } else {
        slug
    }
}

fn is_pr_branch_name(name: &str, is_remote: bool) -> bool {
    let short_name = if is_remote {
        name.split_once('/').map(|(_, rest)| rest).unwrap_or(name)
    } else {
        name
    };
    short_name.starts_with("pass/") || short_name.starts_with("zane/")
}

fn collect_branch_tags(repo: &Repository) -> HashMap<Oid, Vec<String>> {
    let mut tags: HashMap<Oid, Vec<String>> = HashMap::new();

    if let Ok(head) = repo.head() {
        if let Some(oid) = head.target() {
            tags.entry(oid).or_default().push("HEAD".to_string());
        }
    }

    if let Ok(branches) = repo.branches(None) {
        for item in branches.flatten() {
            let (branch, _) = item;
            if let Ok(Some(name)) = branch.name() {
                if let Some(oid) = branch.get().target() {
                    let entry = tags.entry(oid).or_default();
                    if !entry.iter().any(|existing| existing == name) {
                        entry.push(name.to_string());
                    }
                }
            }
        }
    }

    tags
}

fn inspect_tree_summary(commit: &git2::Commit<'_>) -> (Vec<String>, usize) {
    let mut tracks = Vec::new();
    let mut media_count = 0usize;

    if let Ok(tree) = commit.tree() {
        let _ = tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            let name = entry.name().unwrap_or("");
            if entry.kind() == Some(git2::ObjectType::Blob) {
                if root.starts_with("Timelines") && !tracks.iter().any(|t| t == "Timelines/") {
                    tracks.push("Timelines/".to_string());
                } else if root.starts_with("Media") {
                    media_count += 1;
                } else if !name.is_empty() && tracks.len() < 3 && !root.starts_with("Media") {
                    let label = if root.is_empty() {
                        name.to_string()
                    } else {
                        format!("{root}{name}")
                    };
                    if !tracks.iter().any(|t| t == &label) {
                        tracks.push(label);
                    }
                }
            }
            git2::TreeWalkResult::Ok
        });
    }

    if tracks.is_empty() {
        tracks.push("Timelines/".to_string());
    }

    (tracks, media_count)
}

fn resolve_signature(repo: &Repository) -> Result<Signature<'static>, git2::Error> {
    repo.signature()
        .or_else(|_| Signature::now("Zane", "zane@editor.local"))
}

impl ResolveSidecarApp {
    pub(crate) fn refresh_all(&mut self) {
        self.resolve_running = is_resolve_running();
        let rclone_active = self
            .rclone_handle
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .map(|status| status.is_none())
            .unwrap_or(false);
        self.storage_state = detect_storage_state(rclone_active);
        self.repo_path = detect_active_repo_path();
        self.current_branch = branch_manager::current_branch_name(&self.repo_path);
        self.refresh_git_log();
        self.scan_for_prs();
        self.micro_commits = watcher::list_micro_commits(&self.repo_path);
    }

    pub(crate) fn on_click_suggest_sandbox_slug(&mut self) {
        let timeline_label = if self.diff_timeline_name.trim().is_empty() {
            diff::unwrap_timeline_name(None, &self.baseline_path(), &self.diff_project_name)
        } else {
            self.diff_timeline_name.clone()
        };
        self.sandbox_slug_input = branch_manager::suggest_sandbox_slug(
            self.ai_endpoint.as_deref(),
            self.ai_model.as_deref(),
            &self.scope_checklist,
            &timeline_label,
        );
        self.status_message = format!(
            "Suggested sandbox branch: experiment/{}",
            self.sandbox_slug_input
        );
    }

    pub(crate) fn on_click_sandbox(&mut self) {
        let project_file = self.resolve_active_db_path();
        match branch_manager::start_sandbox(
            &self.repo_path,
            &self.sandbox_slug_input,
            project_file.as_deref(),
        ) {
            Ok(branch) => {
                self.refresh_all();
                self.status_message = format!("Started sandbox branch '{branch}'");
            }
            Err(err) => {
                self.status_message = format!("Sandbox error: {err}");
            }
        }
    }

    pub(crate) fn on_click_discard_sandbox(&mut self) {
        match branch_manager::discard_sandbox(&self.repo_path) {
            Ok(Some(reloaded)) => {
                self.refresh_all();
                self.status_message =
                    format!("Discarded sandbox -> reset to main & reloaded '{reloaded}' in Resolve");
            }
            Ok(None) => {
                self.refresh_all();
                self.status_message =
                    "Discarded sandbox -> reset working tree to main".to_string();
            }
            Err(err) => {
                self.status_message = format!("Discard sandbox error: {err}");
            }
        }
    }

    pub(crate) fn rollback_micro_save(&mut self, full_oid: &str) {
        if let Err(err) = watcher::rollback_to_commit(&self.repo_path, full_oid) {
            self.status_message = format!("Rollback error: {err}");
            return;
        }
        self.refresh_all();
        self.scan_timeline_diff();
        let short = &full_oid[..7.min(full_oid.len())];
        self.status_message = format!("Restored working tree to micro-save {short}");
    }

    fn refresh_git_log(&mut self) {
        self.commits.clear();
        let repo = match Repository::open(&self.repo_path) {
            Ok(r) => r,
            Err(_) => {
                self.status_message = format!("Repo not found at {}", self.repo_path.display());
                return;
            }
        };

        let branch_tags = collect_branch_tags(&repo);

        if let Ok(mut revwalk) = repo.revwalk() {
            let _ = revwalk.set_sorting(Sort::TIME);
            let _ = revwalk.push_head();
            let _ = revwalk.push_glob("refs/heads/*");

            for oid in revwalk.take(40).flatten() {
                if let Ok(commit) = repo.find_commit(oid) {
                    let short_id = format!("{:.7}", oid);
                    let message = commit.summary().unwrap_or("No message").to_string();
                    let author = commit.author().name().unwrap_or("Zane").to_string();
                    let time = format_relative_time(commit.time().seconds());
                    let tag = branch_tags.get(&oid).map(|list| list.join(", "));

                    self.commits.push(GitCommitNode {
                        short_id,
                        message,
                        author,
                        time,
                        tag,
                    });
                }
            }
        }

        self.status_message = format!("Synced ({} commits loaded)", self.commits.len());
    }

    fn scan_for_prs(&mut self) {
        self.pending_prs.clear();
        let repo = match Repository::open(&self.repo_path) {
            Ok(r) => r,
            Err(_) => return,
        };

        let main_oid = repo
            .find_branch("main", BranchType::Local)
            .ok()
            .and_then(|b| b.get().target());

        let mut collected = Vec::new();
        if let Ok(branch_iter) = repo.branches(None) {
            for item in branch_iter.flatten() {
                let (branch, branch_type) = item;
                let is_remote = branch_type == BranchType::Remote;
                let name = match branch.name() {
                    Ok(Some(n)) => n.to_string(),
                    _ => continue,
                };

                if !is_pr_branch_name(&name, is_remote) {
                    continue;
                }

                if let Ok(commit) = branch.get().peel_to_commit() {
                    if let Some(m_oid) = main_oid {
                        if commit.id() == m_oid
                            || repo.graph_descendant_of(m_oid, commit.id()).unwrap_or(false)
                        {
                            continue;
                        }
                    }

                    let msg = commit.message().unwrap_or("No details").to_string();
                    let author = commit.author().name().unwrap_or("Zane").to_string();
                    let summary = commit.summary().unwrap_or(&name).to_string();
                    let time_ago = format_relative_time(commit.time().seconds());
                    let (tracks, media_count) = inspect_tree_summary(&commit);
                    let branch_reviews = reviews::load_reviews_for_branch(&repo, &name);

                    collected.push(PullRequest {
                        title: summary,
                        author,
                        branch_name: name,
                        is_remote,
                        time_ago,
                        notes: msg,
                        tracks,
                        media_count,
                        reviews: branch_reviews,
                    });
                }
            }
        }

        self.pending_prs = collected;
    }

    fn baseline_path(&self) -> PathBuf {
        self.repo_path.join(".timeline_base.json")
    }

    fn resolve_active_db_path(&mut self) -> Option<PathBuf> {
        if let Some(ref db) = self.active_project_db {
            if db.is_file() {
                return Some(db.clone());
            }
        }
        let configured_project = load_pipeline_config().and_then(|c| c.active_project);
        let found = db_watcher::find_latest_project_file(configured_project.as_deref());
        self.active_project_db = found.clone();
        found
    }

    fn initialize_root_baseline_if_needed(&mut self) {
        if Repository::open(&self.repo_path).is_err() {
            return;
        }
        if self.resolve_active_db_path().is_some() {
            self.capture_timeline_baseline();
        }
    }

    fn commit_root_baseline_node(&self) -> Result<(), String> {
        let repo = Repository::open(&self.repo_path).map_err(|e| format!("Git open error: {e}"))?;
        if repo.head().is_ok() {
            return Ok(());
        }

        let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
        let _ = index.add_path(Path::new(".timeline_base.json"));
        index.write().map_err(|e| format!("Index write error: {e}"))?;
        let tree_id = index
            .write_tree()
            .map_err(|e| format!("Tree write error: {e}"))?;
        let tree = repo
            .find_tree(tree_id)
            .map_err(|e| format!("Tree find error: {e}"))?;
        let sig = resolve_signature(&repo).map_err(|e| format!("Signature error: {e}"))?;

        repo.commit(
            Some("refs/heads/main"),
            &sig,
            &sig,
            "chore: initial timeline baseline (Node 0)",
            &tree,
            &[],
        )
        .map_err(|e| format!("Root commit error: {e}"))?;
        let _ = repo.set_head("refs/heads/main");
        Ok(())
    }

    pub(crate) fn scan_timeline_diff(&mut self) {
        let project_file = self.resolve_active_db_path();
        let computed = match diff::compute_pass_diff(
            &self.repo_path,
            project_file.as_deref(),
            &self.baseline_path(),
        ) {
            Ok(c) => c,
            Err(err) => {
                self.status_message = format!("Timeline diff error: {err}");
                return;
            }
        };

        self.diff_timeline_name = computed.timeline_name.clone();
        if let Some(ref snap) = computed.live_snapshot {
            self.diff_project_name = snap.project_name.clone();
            self.diff_fps = snap.fps.clone();
            self.pending_snapshot = serde_json::to_value(snap).ok();
        }
        self.staged_diffs = computed
            .diff_items
            .into_iter()
            .map(|item| StagedDiffItem {
                selected: true,
                item,
            })
            .collect();

        self.status_message = format!(
            "Detected {} changes on {}",
            self.staged_diffs.len(),
            computed.track_summary
        );
    }

    pub(crate) fn capture_timeline_baseline(&mut self) {
        let project_file = match self.resolve_active_db_path() {
            Some(p) => p,
            None => {
                self.status_message =
                    "No Project.xml or Project.db found in Resolve Project Library".to_string();
                return;
            }
        };

        let snapshot = match db_watcher::read_timeline_snapshot(&project_file) {
            Ok(s) => s,
            Err(err) => {
                self.status_message = format!("Baseline read error: {err}");
                return;
            }
        };

        let clip_count = snapshot.clips.len();
        let pretty = match serde_json::to_string_pretty(&snapshot) {
            Ok(p) => p,
            Err(err) => {
                self.status_message = format!("Baseline serialize error: {err}");
                return;
            }
        };

        if let Err(err) = std::fs::write(self.baseline_path(), pretty) {
            self.status_message = format!("Baseline write error: {err}");
            return;
        }

        if let Err(err) = self.commit_root_baseline_node() {
            self.status_message = format!("Baseline saved, root commit error: {err}");
            return;
        }

        self.staged_diffs.clear();
        self.pending_snapshot = None;
        self.refresh_all();
        self.status_message = format!(
            "Captured root baseline (.timeline_base.json) with {} clips",
            clip_count
        );
    }

    pub(crate) fn generate_ai_draft(&mut self) {
        let timeline_label = if self.diff_timeline_name.trim().is_empty() {
            diff::unwrap_timeline_name(None, &self.baseline_path(), &self.diff_project_name)
        } else {
            self.diff_timeline_name.clone()
        };
        let draft = ai_assistant::generate_advisory_pr_draft(
            self.ai_endpoint.as_deref(),
            self.ai_model.as_deref(),
            &self.scope_checklist,
            &timeline_label,
            &self.new_pr_notes,
        );
        self.new_pr_title = draft.title;
        self.new_pr_notes = draft.notes;
        self.status_message = if draft.used_llm {
            "Advisory AI draft generated via local LLM (review before committing)".to_string()
        } else {
            "Advisory AI draft generated (offline template fallback)".to_string()
        };
    }

    pub(crate) fn attach_screenshot(&mut self) {
        let path_str = self.screenshot_path_input.trim();
        if path_str.is_empty() {
            self.status_message = "Error: Enter a screenshot/frame path first".to_string();
            return;
        }
        match reviews::attach_screenshot_to_staging(&self.repo_path, Path::new(path_str)) {
            Ok(rel_path) => {
                self.attached_screenshot_rel = Some(rel_path.clone());
                self.status_message = format!("Attached screenshot: {rel_path}");
            }
            Err(err) => {
                self.status_message = format!("Screenshot error: {err}");
            }
        }
    }

    pub(crate) fn submit_review_comment(&mut self, branch_name: &str, status: &str) {
        let body = self.review_draft_body.trim().to_string();
        if body.is_empty() {
            self.status_message = "Error: Review comment cannot be empty".to_string();
            return;
        }
        let author = Repository::open(&self.repo_path)
            .ok()
            .and_then(|r| resolve_signature(&r).ok())
            .and_then(|s| s.name().map(str::to_string))
            .unwrap_or_else(|| "LeadEditor".to_string());

        match reviews::append_review_comment(
            &self.repo_path,
            branch_name,
            &author,
            status,
            &body,
            self.attached_screenshot_rel.clone(),
        ) {
            Ok(_) => {
                self.review_draft_body.clear();
                self.attached_screenshot_rel = None;
                self.refresh_all();
                self.status_message =
                    format!("Added '{status}' review to '{branch_name}' in reviews.json");
            }
            Err(err) => {
                self.status_message = format!("Review error: {err}");
            }
        }
    }

    fn reset_baseline_snapshot(&mut self) {
        if let Some(snapshot) = self.pending_snapshot.take() {
            if let Ok(pretty) = serde_json::to_string_pretty(&snapshot) {
                let _ = std::fs::write(self.baseline_path(), pretty);
            }
        } else if let Some(project_file) = self.resolve_active_db_path() {
            if let Ok(snapshot) = db_watcher::read_timeline_snapshot(&project_file) {
                if let Ok(pretty) = serde_json::to_string_pretty(&snapshot) {
                    let _ = std::fs::write(self.baseline_path(), pretty);
                }
            }
        }
        self.staged_diffs.clear();
    }

    pub(crate) fn create_and_package_pr(&mut self) {
        let title = self.new_pr_title.trim().to_string();
        if title.is_empty() {
            self.status_message = "Error: Pass Title cannot be empty".to_string();
            return;
        }

        let timelines_dir = self.repo_path.join("Timelines");
        let staging_dir = self.repo_path.join("staging_media");
        if let Some(interchange_file) = interchange::find_latest_interchange_file(&timelines_dir) {
            if let Err(err) =
                interchange::unpack_otioz_if_needed(&interchange_file, &timelines_dir, &staging_dir)
            {
                self.status_message = format!("OTIOZ unpack error: {err}");
                return;
            }
        }
        if let Err(err) = interchange::stage_local_assets_for_pass(&self.repo_path) {
            self.status_message = format!("Staging media error: {err}");
            return;
        }

        let scope_labels = self.scope_checklist.selected_labels();
        if scope_labels.is_empty() {
            self.status_message = "Error: Select at least one scope item".to_string();
            return;
        }

        let scope_summary = scope_labels.join(", ");
        let delta_path = timelines_dir.join("incoming_cut_delta.json");
        if let Some(parent) = delta_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.status_message = format!("Delta dir error: {e}");
                return;
            }
        }

        let selected_items: Vec<TimelineDiffItem> = self
            .staged_diffs
            .iter()
            .filter(|d| d.selected)
            .map(|d| d.item.clone())
            .collect();
        let clips_for_import: Vec<&TimelineClip> = selected_items
            .iter()
            .filter(|item| item.change_type != "DELETED")
            .map(|item| &item.clip)
            .collect();

        let timeline_label = if self.diff_timeline_name.trim().is_empty() {
            diff::unwrap_timeline_name(None, &self.baseline_path(), &self.diff_project_name)
        } else {
            self.diff_timeline_name.clone()
        };

        let delta_payload = serde_json::json!({
            "project_name": self.diff_project_name,
            "timeline_name": timeline_label,
            "fps": self.diff_fps,
            "scope": scope_labels,
            "notes": self.new_pr_notes.trim(),
            "diff_items": selected_items,
            "clips": clips_for_import,
        });

        let serialized = match serde_json::to_string_pretty(&delta_payload) {
            Ok(s) => s,
            Err(e) => {
                self.status_message = format!("Delta serialize error: {e}");
                return;
            }
        };
        if let Err(e) = std::fs::write(&delta_path, serialized) {
            self.status_message = format!("Delta write error: {e}");
            return;
        }

        let repo = match Repository::open(&self.repo_path) {
            Ok(r) => r,
            Err(e) => {
                self.status_message = format!("Git error: {e}");
                return;
            }
        };

        let branch_name = format!("pass/{}", slugify_branch_title(&title));

        // Stage Timelines/, staging_media/, and Media/
        let mut index = match repo.index() {
            Ok(idx) => idx,
            Err(e) => {
                self.status_message = format!("Index error: {e}");
                return;
            }
        };

        if let Err(e) = index.add_all(
            ["Timelines", "staging_media", "Media"].iter(),
            IndexAddOption::DEFAULT,
            None,
        ) {
            self.status_message = format!("Stage error: {e}");
            return;
        }
        if delta_path.is_file() {
            let _ = index.add_path(Path::new("Timelines/incoming_cut_delta.json"));
        }
        if let Err(e) = index.write() {
            self.status_message = format!("Index write error: {e}");
            return;
        }

        let tree_id = match index.write_tree() {
            Ok(id) => id,
            Err(e) => {
                self.status_message = format!("Tree error: {e}");
                return;
            }
        };

        let tree = match repo.find_tree(tree_id) {
            Ok(t) => t,
            Err(e) => {
                self.status_message = format!("Tree find error: {e}");
                return;
            }
        };

        let sig = match resolve_signature(&repo) {
            Ok(s) => s,
            Err(e) => {
                self.status_message = format!("Signature error: {e}");
                return;
            }
        };

        let scope_tag = format!("[Scope: {scope_summary} | Timeline: {timeline_label}]");
        let base_notes = self.new_pr_notes.trim();
        let screenshot_note = self
            .attached_screenshot_rel
            .as_ref()
            .map(|p| format!("\nScreenshot: `{p}`"))
            .unwrap_or_default();
        let full_notes = if base_notes.is_empty() {
            format!("{scope_tag}{screenshot_note}")
        } else {
            format!("{base_notes}{screenshot_note}\n\n{scope_tag}")
        };

        let head_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit<'_>> = head_commit.iter().collect();
        let commit_msg = format!("{title}\n\n{full_notes}");

        let commit_oid = match repo.commit(None, &sig, &sig, &commit_msg, &tree, &parents) {
            Ok(id) => id,
            Err(e) => {
                self.status_message = format!("Commit error: {e}");
                return;
            }
        };

        match repo.find_commit(commit_oid) {
            Ok(commit) => {
                if let Err(e) = repo.branch(&branch_name, &commit, true) {
                    self.status_message = format!("Branch ref error: {e}");
                    return;
                }
            }
            Err(e) => {
                self.status_message = format!("Commit lookup error: {e}");
                return;
            }
        }

        let mut pushed_note = String::new();
        if let Ok(mut remote) = repo.find_remote("origin") {
            let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
            if remote.push(&[refspec.as_str()], None).is_ok() {
                pushed_note = " (pushed to origin)".to_string();
            }
        }

        // Auto-reset baseline snapshot on PR creation
        self.reset_baseline_snapshot();
        self.attached_screenshot_rel = None;

        self.active_tab = 0;
        self.refresh_all();
        self.status_message = format!(
            "Packaged [{}] onto branch '{}'{}",
            scope_summary, branch_name, pushed_note
        );
    }

    pub(crate) fn accept_and_merge_pr(&mut self, pr_index: usize) {
        let pr = match self.pending_prs.get(pr_index) {
            Some(p) => p.clone(),
            None => {
                self.status_message = "Error: Selected PR no longer exists".to_string();
                return;
            }
        };

        let repo = match Repository::open(&self.repo_path) {
            Ok(r) => r,
            Err(e) => {
                self.status_message = format!("Git error: {e}");
                return;
            }
        };

        let branch_type = if pr.is_remote {
            BranchType::Remote
        } else {
            BranchType::Local
        };

        let target_oid = match repo.find_branch(&pr.branch_name, branch_type) {
            Ok(b) => match b.get().peel_to_commit() {
                Ok(c) => c.id(),
                Err(e) => {
                    self.status_message = format!("Branch commit error: {e}");
                    return;
                }
            },
            Err(e) => {
                self.status_message = format!("Branch lookup error: {e}");
                return;
            }
        };

        let target_commit = match repo.find_commit(target_oid) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Commit lookup error: {e}");
                return;
            }
        };

        // 1. Verify safe fast-forward into `main`
        if let Ok(mut main_branch) = repo.find_branch("main", BranchType::Local) {
            if let Some(main_oid) = main_branch.get().target() {
                let is_ff = main_oid == target_oid
                    || repo.graph_descendant_of(target_oid, main_oid).unwrap_or(false);
                if !is_ff {
                    match interchange::launch_dual_tab_conflict_import(
                        &repo,
                        &self.repo_path,
                        &pr.branch_name,
                        &target_commit,
                    ) {
                        Ok(files) => {
                            self.status_message = format!(
                                "Conflict on '{}': launched Dual-Tab Sidecar Import ({} cuts) in Resolve!",
                                pr.branch_name,
                                files.len()
                            );
                        }
                        Err(err) => {
                            self.status_message = format!(
                                "Conflict on '{}' (no textual merge): {err}",
                                pr.branch_name
                            );
                        }
                    }
                    return;
                }
                let reflog_msg = format!("Fast-forward merge {}", pr.branch_name);
                if let Err(e) = main_branch.get_mut().set_target(target_oid, &reflog_msg) {
                    self.status_message = format!("Fast-forward ref error: {e}");
                    return;
                }
            } else if let Err(e) = repo.branch("main", &target_commit, true) {
                self.status_message = format!("Main branch update error: {e}");
                return;
            }
        } else if let Err(e) = repo.branch("main", &target_commit, true) {
            self.status_message = format!("Main branch creation error: {e}");
            return;
        }

        // 2. Point HEAD to main and force-checkout working directory
        if let Err(e) = repo.set_head("refs/heads/main") {
            self.status_message = format!("HEAD error: {e}");
            return;
        }

        let mut checkout_opts = git2::build::CheckoutBuilder::default();
        checkout_opts.force();
        if let Err(e) = repo.checkout_head(Some(&mut checkout_opts)) {
            self.status_message = format!("Checkout error: {e}");
            return;
        }

        // 3. Auto-reset baseline snapshot on PR merge
        self.reset_baseline_snapshot();

        // 4. Migrate transient staging_media/ and Media/Audio/* to Z:\Media\ and purge from Git
        let migrated_staging =
            interchange::migrate_staging_media_to_dir(&self.repo_path, Path::new(r"Z:\Media"))
                .unwrap_or(0);
        let pruned_audio = prune_harvested_audio(&self.repo_path).unwrap_or(0);
        let total_migrated = migrated_staging + pruned_audio;

        // 5. Hands-free Win32 import of .drt/.otio/.otioz into running Resolve.exe (no restarts, no dialogs)
        let timelines_dir = self.repo_path.join("Timelines");
        let staging_dir = self.repo_path.join("staging_media");
        let import_result = match interchange::find_latest_interchange_file(&timelines_dir) {
            Some(file) => {
                match interchange::unpack_otioz_if_needed(&file, &timelines_dir, &staging_dir) {
                    Ok(target_timeline) => {
                        interchange::import_timeline_into_running_resolve(&target_timeline)
                            .map(|_| Some(target_timeline))
                    }
                    Err(e) => Err(e),
                }
            }
            None => Ok(None),
        };

        self.refresh_all();

        match import_result {
            Ok(Some(imported_path)) => {
                let file_label = imported_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("timeline");
                if total_migrated > 0 {
                    self.status_message = format!(
                        "Merged '{}', migrated {} media files to Z:\\ & imported '{}' into Resolve!",
                        pr.branch_name, total_migrated, file_label
                    );
                } else {
                    self.status_message = format!(
                        "Merged '{}' & imported '{}' into Resolve!",
                        pr.branch_name, file_label
                    );
                }
            }
            Ok(None) => {
                self.status_message = if total_migrated > 0 {
                    format!(
                        "Merged '{}' into main & migrated {} media files to Z:\\!",
                        pr.branch_name, total_migrated
                    )
                } else {
                    format!("Merged '{}' into main cut!", pr.branch_name)
                };
            }
            Err(err) => {
                self.status_message = format!(
                    "Merged '{}' into main (Resolve Win32 import note: {err})",
                    pr.branch_name
                );
            }
        }
    }
}

fn prune_harvested_audio(repo_path: &Path) -> Result<usize, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;

    let audio_dir = repo_path.join("Media").join("Audio");
    std::fs::create_dir_all(&audio_dir).map_err(|e| format!("Audio dir error: {e}"))?;
    let gitkeep_path = audio_dir.join(".gitkeep");
    if !gitkeep_path.exists() {
        std::fs::write(&gitkeep_path, b"").map_err(|e| format!(".gitkeep write error: {e}"))?;
    }

    let targets: Vec<String> = index
        .iter()
        .filter_map(|entry| std::str::from_utf8(&entry.path).ok().map(str::to_string))
        .filter(|p| p.starts_with("Media/Audio/") && p != "Media/Audio/.gitkeep")
        .collect();

    let mut removed_count = 0usize;
    for rel in &targets {
        if index.remove_path(Path::new(rel)).is_ok() {
            removed_count += 1;
        }
        let disk_path = repo_path.join(rel);
        if disk_path.is_file() {
            let _ = std::fs::remove_file(&disk_path);
        }
    }

    if let Ok(entries) = std::fs::read_dir(&audio_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.file_name().and_then(|n| n.to_str()) != Some(".gitkeep") {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    let had_gitkeep = index.get_path(Path::new("Media/Audio/.gitkeep"), 0).is_some();
    index
        .add_path(Path::new("Media/Audio/.gitkeep"))
        .map_err(|e| format!(".gitkeep stage error: {e}"))?;

    if removed_count == 0 && had_gitkeep {
        return Ok(0);
    }

    index.write().map_err(|e| format!("Index write error: {e}"))?;
    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Tree write error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Tree find error: {e}"))?;
    let sig = resolve_signature(&repo).map_err(|e| format!("Signature error: {e}"))?;
    let head_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = head_commit.iter().collect();

    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "chore: auto-migrated harvested audio to Z:\\ and cleaned repo",
        &tree,
        &parents,
    )
    .map_err(|e| format!("Prune commit error: {e}"))?;

    Ok(removed_count)
}

impl eframe::App for ResolveSidecarApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut updated_db: Option<PathBuf> = None;
        let mut micro_saved_id: Option<String> = None;
        if let Some(ref watcher) = self.live_watcher {
            while let Ok(evt) = watcher.rx.try_recv() {
                match evt {
                    watcher::WatcherEvent::MicroSaveCommitted {
                        project_file,
                        short_id,
                    } => {
                        updated_db = Some(project_file);
                        micro_saved_id = Some(short_id);
                    }
                    watcher::WatcherEvent::ProjectFileChanged(project_file) => {
                        updated_db = Some(project_file);
                    }
                }
            }
        }
        if let Some(db_path) = updated_db {
            self.active_project_db = Some(db_path);
            self.micro_commits = watcher::list_micro_commits(&self.repo_path);
            self.scan_timeline_diff();
            if let Some(short_id) = micro_saved_id {
                self.status_message =
                    format!("Auto-saved micro-commit {short_id} to scratch/live-stream");
            }
        }

        if self.storage_state == StorageMountState::MountingZ {
            if self.last_mount_poll.elapsed() >= Duration::from_secs(1) {
                self.last_mount_poll = Instant::now();
                if Repository::open(r"Z:\").is_ok() {
                    self.storage_state = StorageMountState::MountedZ;
                    self.repo_path = PathBuf::from(r"Z:\");
                    self.refresh_all();
                } else {
                    let rclone_active = self
                        .rclone_handle
                        .as_mut()
                        .and_then(|c| c.try_wait().ok())
                        .map(|s| s.is_none())
                        .unwrap_or(false);
                    if !rclone_active {
                        self.storage_state = StorageMountState::LocalProject;
                    }
                }
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        ui::render_app(self, ctx);
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0_f32, 560.0_f32])
            .with_min_inner_size([720.0_f32, 420.0_f32])
            .with_title("Resolve Sidecar"),
        ..Default::default()
    };
    eframe::run_native(
        "Resolve Sidecar",
        options,
        Box::new(|_cc| Box::new(ResolveSidecarApp::default())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_harvested_audio_removes_wav_and_keeps_gitkeep() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_sidecar_prune_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("Media").join("Audio")).unwrap();

        let repo = Repository::init(&temp_root).unwrap();
        let wav_path = temp_root.join("Media").join("Audio").join("dummy_music.wav");
        std::fs::write(&wav_path, b"RIFF_DUMMY_WAV").unwrap();

        let mut index = repo.index().unwrap();
        index
            .add_path(Path::new("Media/Audio/dummy_music.wav"))
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("Test", "test@local").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial audio import", &tree, &[])
            .unwrap();

        let pruned = prune_harvested_audio(&temp_root).unwrap();
        assert_eq!(pruned, 1);
        assert!(!wav_path.exists());
        assert!(temp_root.join("Media").join("Audio").join(".gitkeep").exists());

        let statuses = repo.statuses(None).unwrap();
        assert_eq!(statuses.len(), 0);

        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(
            head_commit.summary().unwrap(),
            "chore: auto-migrated harvested audio to Z:\\ and cleaned repo"
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
