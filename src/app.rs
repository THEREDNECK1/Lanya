use crate::{git_engine, projects, watcher, ResolveSidecarApp};
use git2::Repository;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppView {
    HomeProjects,
    MediaPipeline,
    Timeline,
}

impl AppView {
    pub const ALL: [AppView; 3] = [
        AppView::HomeProjects,
        AppView::MediaPipeline,
        AppView::Timeline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AppView::HomeProjects => "Home / Projects",
            AppView::MediaPipeline => "Media Pipeline",
            AppView::Timeline => "Timeline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IngestStatus {
    Queued,
    Staged,
    Error(String),
}

#[derive(Debug, Clone)]
pub(crate) struct IngestJob {
    pub(crate) filename: String,
    pub(crate) target_path: String,
    pub(crate) profile: String,
    pub(crate) status: IngestStatus,
}

const DEFAULT_GITHUB_OAUTH_CLIENT_ID: &str = "Ov23liLanyaCollabClient";

#[derive(Debug, Clone, Default)]
pub(crate) struct GitHubAuthState {
    pub(crate) editor_name: String,
    pub(crate) editor_email: String,
    pub(crate) remote_url: String,
    pub(crate) pat_token: String,
    pub(crate) oauth_client_id: String,
    pub(crate) oauth_client_secret: String,
    pub(crate) show_advanced: bool,
    pub(crate) auth_verified: bool,
    pub(crate) auth_status_note: String,
}

fn github_pat_file() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ResolveSidecar").join("github_pat.txt"))
}

fn github_oauth_app_file() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ResolveSidecar").join("github_oauth_app.json"))
}

impl ResolveSidecarApp {
    pub(crate) fn switch_project(&mut self, project_path: PathBuf) {
        if !project_path.exists() {
            self.status_message = format!("Project path missing: {}", project_path.display());
            return;
        }
        self.repo_path = project_path.clone();
        self.needs_repo_setup = !self.repo_path.join(".git").exists();
        if !self.needs_repo_setup {
            git_engine::ensure_gitignore_excludes_restore_otio(&self.repo_path);
            if let Some((w, rx)) = watcher::start_timeline_watcher(self.repo_path.clone()) {
                self.timeline_watcher = Some(w);
                self.event_rx = Some(rx);
            }
            self.reload_git_dag();
            self.load_github_collab_state();
            if let Some(first_oid) = self.commits.first().map(|c| c.full_id.clone()) {
                self.select_commit(first_oid);
            }
        }
        let now_str = chrono::Utc::now().format("%Y-%m-%d").to_string();
        for p in &mut self.saved_projects {
            if p.path == project_path {
                p.last_opened = now_str.clone();
            }
        }
        projects::save_projects_list(&self.saved_projects);
        self.current_view = AppView::Timeline;
        self.status_message = format!("Opened project: {}", project_path.display());
    }

    pub(crate) fn scan_z_projects(&self) -> Vec<projects::SavedProject> {
        let mut list = self.saved_projects.clone();
        let z_projects_dir = Path::new(r"Z:\Projects");
        if let Ok(entries) = std::fs::read_dir(z_projects_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !list.iter().any(|p| p.path == path) {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Project")
                        .to_string();
                    list.push(projects::SavedProject {
                        name,
                        path,
                        last_opened: "Z:\\Projects".to_string(),
                    });
                }
            }
        }
        list
    }

    pub(crate) fn create_and_open_project(&mut self) {
        let base = PathBuf::from(self.new_project_base_dir.trim());
        match projects::create_new_project(&self.new_project_name, &base) {
            Ok(proj) => {
                let p_path = proj.path.clone();
                self.saved_projects.retain(|existing| existing.path != p_path);
                self.saved_projects.insert(0, proj);
                projects::save_projects_list(&self.saved_projects);
                self.new_project_name.clear();
                self.show_new_project_modal = false;
                self.switch_project(p_path);
            }
            Err(err) => {
                self.status_message = format!("Create project error: {err}");
            }
        }
    }

    pub(crate) fn disconnect_z_drive(&mut self) {
        let drive = self.storage_drive_letter.trim();
        let mut net_cmd = Command::new("net");
        net_cmd.args(["use", drive, "/delete", "/y"]);
        crate::configure_hidden_command(&mut net_cmd);
        let _ = net_cmd.status();

        if let Some(mut child) = self.rclone_handle.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.z_drive_connected = Path::new(r"Z:\").exists();
        self.status_message = format!("Disconnected {drive}");
    }

    pub(crate) fn remount_z_drive(&mut self) {
        self.disconnect_z_drive();
        if let Some(mut old) = self.rclone_handle.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        let remote = self.storage_remote_name.trim();
        let subfolder = self.storage_remote_subfolder.trim().trim_start_matches('/');
        let spec = if subfolder.is_empty() {
            format!("{remote}:")
        } else {
            format!("{remote}:{subfolder}")
        };
        match crate::mount_engine::mount_z_drive_with_remote(&spec) {
            Ok(child_opt) => {
                self.rclone_handle = child_opt;
                self.z_drive_connected = crate::mount_engine::is_z_mounted();
                if self.z_drive_connected {
                    self.reload_git_dag();
                    self.status_message = format!("Mounted {} ({spec})", self.storage_drive_letter);
                } else {
                    self.status_message = "rclone started, waiting for mount".to_string();
                }
            }
            Err(err) => {
                self.z_drive_connected = crate::mount_engine::is_z_mounted();
                self.status_message = format!("Remount failed: {err}");
            }
        }
    }

    pub(crate) fn check_rclone_remote_auth(&mut self) {
        let target_remote = self.storage_remote_name.trim().trim_end_matches(':');
        if target_remote.is_empty() {
            self.rclone_remote_authenticated = false;
            return;
        }
        // Check %APPDATA%\rclone\rclone.conf directly first
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let conf_path = PathBuf::from(appdata).join("rclone").join("rclone.conf");
            if let Ok(raw) = std::fs::read_to_string(&conf_path) {
                let header = format!("[{target_remote}]");
                let mut in_section = false;
                let mut has_token = false;
                for line in raw.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('[') && trimmed.ends_with(']') {
                        if in_section {
                            break;
                        }
                        in_section = trimmed.eq_ignore_ascii_case(&header);
                    } else if in_section && trimmed.to_ascii_lowercase().starts_with("token") {
                        has_token = true;
                        break;
                    }
                }
                if in_section && has_token {
                    self.rclone_remote_authenticated = true;
                    return;
                }
            }
        }

        let rclone_bin = crate::mount_engine::find_rclone_binary()
            .unwrap_or_else(|| PathBuf::from("rclone"));
        let mut cmd = Command::new(rclone_bin);
        cmd.arg("listremotes");
        crate::configure_hidden_command(&mut cmd);
        let found = cmd
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                let text = String::from_utf8_lossy(&o.stdout);
                text.lines()
                    .map(|l| l.trim().trim_end_matches(':'))
                    .any(|r| r.eq_ignore_ascii_case(target_remote))
            })
            .unwrap_or(false);
        self.rclone_remote_authenticated = found;
    }

    pub(crate) fn start_google_drive_oauth(&mut self) {
        if self.rclone_oauth_in_progress {
            return;
        }
        let remote_name = self.storage_remote_name.trim().trim_end_matches(':').to_string();
        if remote_name.is_empty() {
            self.status_message = "Remote Name cannot be empty.".to_string();
            return;
        }
        let rclone_bin = match crate::mount_engine::find_rclone_binary() {
            Some(bin) => bin,
            None => {
                self.status_message = "rclone.exe not found in PATH or beside executable.".to_string();
                return;
            }
        };

        let (tx, rx) = std::sync::mpsc::channel();
        self.rclone_oauth_rx = Some(rx);
        self.rclone_oauth_in_progress = true;
        self.status_message = "Waiting for browser authorization...".to_string();

        let _ = std::thread::Builder::new()
            .name("rclone-google-oauth".to_string())
            .spawn(move || {
                // Clean up any stale rclone auth/config process holding port 53682 (without touching `serve webdav`)
                let mut cleanup = Command::new("powershell");
                cleanup.args([
                    "-NoProfile",
                    "-Command",
                    "Get-CimInstance Win32_Process -Filter \"Name='rclone.exe'\" | Where-Object { $_.CommandLine -match 'config|authorize' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }",
                ]);
                crate::configure_hidden_command(&mut cleanup);
                let _ = cleanup.status();

                let mut cmd = Command::new(rclone_bin);
                cmd.args([
                    "config",
                    "create",
                    &remote_name,
                    "drive",
                    "config_is_local=true",
                ]);
                crate::configure_hidden_command(&mut cmd);
                let res = match cmd.output() {
                    Ok(out) if out.status.success() => Ok(()),
                    Ok(out) => {
                        let raw_err = String::from_utf8_lossy(&out.stderr);
                        let concise = raw_err
                            .lines()
                            .find(|l| l.contains("Fatal error:") || l.starts_with("Error:"))
                            .map(|l| l.trim().to_string())
                            .unwrap_or_else(|| {
                                let t = raw_err.trim();
                                if t.is_empty() {
                                    format!("rclone config exited with {}", out.status)
                                } else {
                                    t.lines().next().unwrap_or("rclone auth failed").to_string()
                                }
                            });
                        Err(concise)
                    }
                    Err(e) => Err(format!("Failed to start rclone OAuth: {e}")),
                };
                let _ = tx.send(res);
            });
    }

    pub(crate) fn poll_rclone_oauth(&mut self, ctx: &eframe::egui::Context) {
        let Some(ref rx) = self.rclone_oauth_rx else {
            return;
        };
        let mut rclone_email: Option<String> = None;
        if let Ok(res) = rx.try_recv() {
            self.rclone_oauth_in_progress = false;
            self.rclone_oauth_rx = None;
            self.check_rclone_remote_auth();
            match res {
                Ok(()) => {
                    if self.rclone_remote_authenticated {
                        let remote = self.storage_remote_name.trim().trim_end_matches(':');
                        let rclone_bin = crate::mount_engine::find_rclone_binary()
                            .unwrap_or_else(|| PathBuf::from("rclone"));
                        let mut about_cmd = Command::new(rclone_bin);
                        about_cmd.args(["backend", "userinfo", &format!("{remote}:")]);
                        crate::configure_hidden_command(&mut about_cmd);
                        if let Ok(out) = about_cmd.output() {
                            if out.status.success() {
                                let text = String::from_utf8_lossy(&out.stdout);
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                    rclone_email = v
                                        .get("email")
                                        .and_then(|e| e.as_str())
                                        .map(|s| s.trim().to_string())
                                        .filter(|s| !s.is_empty());
                                }
                            }
                        }
                        if let Some(email) = rclone_email {
                            self.github_auth.editor_email = email;
                        }
                        self.remount_z_drive();
                    } else {
                        self.status_message = format!(
                            "Google Drive authenticated ({})",
                            self.storage_remote_name.trim()
                        );
                    }
                }
                Err(err) => {
                    self.status_message = format!("Google Auth failed: {err}");
                }
            }
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    pub(crate) fn verify_pipeline_folders(&self) -> Vec<(&'static str, bool)> {
        [
            r"Z:\_Inbox",
            r"Z:\Proxies_720p",
            r"Z:\Audio",
            r"Z:\Timelines",
        ]
        .into_iter()
        .map(|p| (p, Path::new(p).is_dir()))
        .collect()
    }

    pub(crate) fn scaffold_repair_pipeline_folders(&mut self) {
        if !Path::new(r"Z:\").exists() {
            self.status_message = "Cannot scaffold folders: Z:\\ is disconnected.".to_string();
            return;
        }
        let mut created = 0usize;
        for folder in [
            r"Z:\_Inbox",
            r"Z:\Proxies_720p",
            r"Z:\Audio",
            r"Z:\Timelines",
        ] {
            let p = Path::new(folder);
            if !p.is_dir() && std::fs::create_dir_all(p).is_ok() {
                created += 1;
            }
        }
        self.status_message = if created == 0 {
            "All Z:\\ pipeline folders verified.".to_string()
        } else {
            format!("Repaired/created {created} pipeline folder(s) on Z:\\.")
        };
    }

    pub(crate) fn browse_and_stage_clips(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("Select Clips or Audio to Ingest")
            .pick_files()
        {
            for path in files {
                let s = path.to_string_lossy().to_string();
                self.queue_media_for_ingest(&s);
            }
        }
    }

    pub(crate) fn queue_media_for_ingest(&mut self, raw_path: &str) {
        let trimmed = raw_path.trim().trim_matches('"');
        if trimmed.is_empty() {
            return;
        }
        let src = Path::new(trimmed);
        if !src.is_file() {
            self.status_message = format!("Media file not found: {trimmed}");
            return;
        }
        let filename = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("clip")
            .to_string();
        let subfolder = crate::otio_engine::target_z_subfolder_for_media(src);
        let is_audio = subfolder == "Audio";
        let profile = if is_audio {
            "Audio Passthrough".to_string()
        } else {
            "hevc_amf 720p 60fps".to_string()
        };

        let dest_root = if Path::new(r"Z:\").exists() {
            if is_audio {
                PathBuf::from(r"Z:\Audio")
            } else {
                PathBuf::from(self.ingest_watch_folder.trim())
            }
        } else {
            self.repo_path
                .join("_media")
                .join(if is_audio { "audio" } else { "video" })
        };
        let dest_path = dest_root.join(&filename);
        let mut job = IngestJob {
            filename: filename.clone(),
            target_path: dest_path.to_string_lossy().to_string(),
            profile,
            status: IngestStatus::Queued,
        };

        if let Err(err) = std::fs::create_dir_all(&dest_root) {
            job.status = IngestStatus::Error(err.to_string());
            self.ingest_queue.insert(0, job);
            self.status_message = format!("Ingest directory error: {err}");
            return;
        }

        match std::fs::copy(src, &dest_path) {
            Ok(_) => {
                job.status = IngestStatus::Staged;
                self.status_message = format!("Staged {filename} -> {}", dest_path.display());
            }
            Err(err) => {
                job.status = IngestStatus::Error(err.to_string());
                self.status_message = format!("Ingest copy failed: {err}");
            }
        }
        self.ingest_queue.insert(0, job);
    }

    pub(crate) fn is_transcode_worker_running(&mut self) -> bool {
        self.transcode_worker_handle
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .map(|s| s.is_none())
            .unwrap_or(false)
    }

    pub(crate) fn launch_transcode_worker(&mut self) {
        if self.is_transcode_worker_running() {
            self.status_message = "Transcode worker is already running.".to_string();
            return;
        }
        let mut candidates = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(
                    dir.join("pipeline")
                        .join("ai-ingestor")
                        .join("interactive_transcode_worker.py"),
                );
            }
        }
        candidates.push(
            PathBuf::from("pipeline")
                .join("ai-ingestor")
                .join("interactive_transcode_worker.py"),
        );

        let Some(script) = candidates.into_iter().find(|p| p.is_file()) else {
            self.status_message = "interactive_transcode_worker.py not found.".to_string();
            return;
        };

        let mut cmd = Command::new("python");
        cmd.arg(&script);
        if let Some(parent) = script.parent() {
            cmd.current_dir(parent);
        }
        crate::configure_hidden_command(&mut cmd);
        match cmd.spawn() {
            Ok(child) => {
                self.transcode_worker_handle = Some(child);
                self.status_message = "Launched interactive_transcode_worker.py".to_string();
            }
            Err(err) => {
                self.status_message = format!("Failed to launch transcode worker: {err}");
            }
        }
    }

    pub(crate) fn redeploy_resolve_scripts(&mut self) {
        match crate::auto_deploy_resolve_scripts() {
            Some(msg) => self.status_message = msg,
            None => {
                self.status_message =
                    "Resolve scripts already up to date (or Resolve Utility folder missing)."
                        .to_string()
            }
        }
    }

    pub(crate) fn load_github_collab_state(&mut self) {
        let mut state = GitHubAuthState::default();
        if let Ok(repo) = Repository::open(&self.repo_path) {
            if let Ok(cfg) = repo.config() {
                state.editor_name = cfg.get_string("user.name").unwrap_or_default();
                state.editor_email = cfg.get_string("user.email").unwrap_or_default();
            }
            if let Ok(remote) = repo.find_remote("origin") {
                state.remote_url = remote.url().unwrap_or_default().to_string();
            }
        }
        if state.editor_name.is_empty() {
            state.editor_name = self.setup_editor_handle.clone();
        }
        if state.remote_url.is_empty() {
            state.remote_url = self.setup_remote_url.clone();
        }
        if let Some(pat_path) = github_pat_file() {
            if let Ok(raw) = std::fs::read_to_string(pat_path) {
                state.pat_token = raw.trim().to_string();
            }
        }
        if let Some(app_path) = github_oauth_app_file() {
            if let Ok(raw) = std::fs::read_to_string(app_path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    state.oauth_client_id = v
                        .get("client_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string();
                    state.oauth_client_secret = v
                        .get("client_secret")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string();
                }
            }
        }
        if state.oauth_client_id.is_empty() {
            if let Ok(env_id) = std::env::var("LANYA_GITHUB_CLIENT_ID") {
                state.oauth_client_id = env_id;
            }
        }
        if state.oauth_client_id.is_empty() {
            state.oauth_client_id = DEFAULT_GITHUB_OAUTH_CLIENT_ID.to_string();
        }
        if state.oauth_client_secret.is_empty() {
            if let Ok(env_sec) = std::env::var("LANYA_GITHUB_CLIENT_SECRET") {
                state.oauth_client_secret = env_sec;
            }
        }
        if !state.pat_token.is_empty() {
            state.auth_verified = true;
            state.auth_status_note = "Token saved locally".to_string();
        }
        self.github_auth = state;
    }

    pub(crate) fn save_github_collab_settings(&mut self) {
        let name = self.github_auth.editor_name.trim().to_string();
        let email = self.github_auth.editor_email.trim().to_string();
        let url = self.github_auth.remote_url.trim().to_string();
        let pat = self.github_auth.pat_token.trim().to_string();

        if let Ok(repo) = Repository::open(&self.repo_path) {
            if let Ok(mut cfg) = repo.config() {
                if !name.is_empty() {
                    let _ = cfg.set_str("user.name", &name);
                }
                if !email.is_empty() {
                    let _ = cfg.set_str("user.email", &email);
                }
            }
            if !url.is_empty() {
                if repo.find_remote("origin").is_ok() {
                    let _ = repo.remote_set_url("origin", &url);
                } else {
                    let _ = repo.remote("origin", &url);
                }
            }
        }

        if let Some(pat_path) = github_pat_file() {
            if let Some(parent) = pat_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if pat.is_empty() {
                let _ = std::fs::remove_file(&pat_path);
            } else {
                let _ = std::fs::write(&pat_path, pat.as_bytes());
            }
        }

        if let Some(app_path) = github_oauth_app_file() {
            if let Some(parent) = app_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let obj = serde_json::json!({
                "client_id": self.github_auth.oauth_client_id.trim(),
                "client_secret": self.github_auth.oauth_client_secret.trim(),
            });
            if let Ok(json) = serde_json::to_string_pretty(&obj) {
                let _ = std::fs::write(app_path, json);
            }
        }

        self.github_auth.auth_verified = !pat.is_empty();
        self.github_auth.auth_status_note = if pat.is_empty() {
            "Saved git identity & remote".to_string()
        } else {
            "Saved git identity, remote & PAT".to_string()
        };
        self.status_message = self.github_auth.auth_status_note.clone();
    }

    pub(crate) fn verify_github_auth(&mut self) {
        let mut cmd = Command::new("gh");
        cmd.args(["auth", "status"]);
        crate::configure_hidden_command(&mut cmd);
        match cmd.output() {
            Ok(out) if out.status.success() => {
                self.github_auth.auth_verified = true;
                self.github_auth.auth_status_note = "Verified via GitHub CLI (gh)".to_string();
                self.status_message = self.github_auth.auth_status_note.clone();
                return;
            }
            _ => {}
        }

        if !self.github_auth.pat_token.trim().is_empty() {
            self.github_auth.auth_verified = true;
            self.github_auth.auth_status_note = "PAT configured locally".to_string();
            self.status_message = self.github_auth.auth_status_note.clone();
        } else {
            self.github_auth.auth_verified = false;
            self.github_auth.auth_status_note =
                "No gh session or PAT found. Click 'Connect GitHub Account' or enter a PAT."
                    .to_string();
            self.status_message = self.github_auth.auth_status_note.clone();
        }
    }

    pub(crate) fn start_github_oauth(&mut self) {
        if self.github_oauth_in_progress {
            return;
        }
        let client_id = self.github_auth.oauth_client_id.trim().to_string();
        let client_secret = self.github_auth.oauth_client_secret.trim().to_string();
        if client_id.is_empty() {
            self.github_auth.auth_verified = false;
            self.github_auth.auth_status_note =
                "Enter GitHub OAuth Client ID (Callback: http://127.0.0.1:8088/callback)."
                    .to_string();
            self.status_message = self.github_auth.auth_status_note.clone();
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.github_oauth_rx = Some(rx);
        self.github_oauth_in_progress = true;
        self.github_auth.auth_verified = false;
        self.github_auth.auth_status_note =
            "Listening on http://127.0.0.1:8088/callback — complete sign-in in browser..."
                .to_string();
        self.status_message = self.github_auth.auth_status_note.clone();

        let _ = std::thread::Builder::new()
            .name("github-oauth-callback".to_string())
            .spawn(move || {
                use std::io::{Read, Write};
                use std::net::TcpListener;
                use std::time::{Duration, Instant};

                let listener = match TcpListener::bind("127.0.0.1:8088") {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = tx.send(Err(format!("Port 8088 bind failed: {e}")));
                        return;
                    }
                };
                let _ = listener.set_nonblocking(true);

                let redirect_uri = "http://127.0.0.1:8088/callback";
                let auth_url = format!(
                    "https://github.com/login/oauth/authorize?client_id={client_id}&redirect_uri={redirect_uri}&scope=repo"
                );
                let mut open_cmd = Command::new("cmd");
                open_cmd.args(["/C", "start", "", &auth_url]);
                crate::configure_hidden_command(&mut open_cmd);
                if let Err(e) = open_cmd.spawn() {
                    let _ = tx.send(Err(format!("Failed to open browser: {e}")));
                    return;
                }

                let deadline = Instant::now() + Duration::from_secs(120);
                let mut auth_code: Option<String> = None;

                while Instant::now() < deadline {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let mut buf = [0u8; 4096];
                            let n = stream.read(&mut buf).unwrap_or(0);
                            let req = String::from_utf8_lossy(&buf[..n]);
                            let first_line = req.lines().next().unwrap_or("");
                            let path_part = first_line.split_whitespace().nth(1).unwrap_or("");

                            if let Some(query) = path_part.strip_prefix("/callback?") {
                                for pair in query.split('&') {
                                    if let Some(val) = pair.strip_prefix("code=") {
                                        auth_code = Some(val.to_string());
                                        break;
                                    }
                                }
                                let body = "<html><body style='font-family:sans-serif;background:#0d0d0f;color:#f4f4f6;padding:40px;'><h2>Authentication successful!</h2><p>You can close this window and return to Lanya.</p></body></html>";
                                let resp = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.flush();
                                if auth_code.is_some() {
                                    break;
                                }
                            } else {
                                let _ = stream.write_all(
                                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                );
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(120));
                        }
                        Err(_) => break,
                    }
                }
                drop(listener);

                let Some(code) = auth_code else {
                    let _ = tx.send(Err("Timed out waiting for GitHub callback on port 8088.".to_string()));
                    return;
                };

                let mut curl = Command::new("curl.exe");
                curl.args([
                    "-s",
                    "-X",
                    "POST",
                    "-H",
                    "Accept: application/json",
                    "-d",
                    &format!("client_id={client_id}"),
                    "-d",
                    &format!("code={code}"),
                    "-d",
                    &format!("redirect_uri={redirect_uri}"),
                ]);
                if !client_secret.is_empty() {
                    curl.args(["-d", &format!("client_secret={client_secret}")]);
                }
                curl.arg("https://github.com/login/oauth/access_token");
                crate::configure_hidden_command(&mut curl);

                let res = match curl.output() {
                    Ok(out) if out.status.success() => {
                        let raw = String::from_utf8_lossy(&out.stdout);
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                            if let Some(tok) = json.get("access_token").and_then(|v| v.as_str()) {
                                Ok(tok.to_string())
                            } else {
                                let err_desc = json
                                    .get("error_description")
                                    .or_else(|| json.get("error"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("No access_token in GitHub response");
                                Err(err_desc.to_string())
                            }
                        } else {
                            Err("Invalid JSON from GitHub access_token endpoint".to_string())
                        }
                    }
                    Ok(out) => Err(format!("curl exited with {}", out.status)),
                    Err(e) => Err(format!("Failed to exchange code via curl.exe: {e}")),
                };
                let _ = tx.send(res);
            });
    }

    pub(crate) fn poll_github_oauth(&mut self, ctx: &eframe::egui::Context) {
        let Some(ref rx) = self.github_oauth_rx else {
            return;
        };
        if let Ok(res) = rx.try_recv() {
            self.github_oauth_in_progress = false;
            self.github_oauth_rx = None;
            match res {
                Ok(token) => {
                    self.github_auth.pat_token = token.clone();
                    if let Some(pat_path) = github_pat_file() {
                        if let Some(parent) = pat_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(pat_path, token.as_bytes());
                    }
                    self.github_auth.auth_verified = true;
                    self.github_auth.auth_status_note =
                        "GitHub OAuth token received & saved!".to_string();
                    self.status_message = self.github_auth.auth_status_note.clone();
                }
                Err(err) => {
                    self.github_auth.auth_verified = false;
                    self.github_auth.auth_status_note = err.clone();
                    self.status_message = format!("GitHub Auth: {err}");
                }
            }
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
