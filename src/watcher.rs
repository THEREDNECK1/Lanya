use crate::resolve_db;
use chrono::Utc;
use git2::{
    build::CheckoutBuilder, BranchType, IndexAddOption, Oid, Repository, Signature, Sort,
};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const SCRATCH_BRANCH: &str = "scratch/live-stream";
const DEBOUNCE_MS: u64 = 2000;

#[derive(Clone)]
pub(crate) struct MicroCommitNode {
    pub(crate) full_id: String,
    pub(crate) short_id: String,
    pub(crate) message: String,
    pub(crate) timestamp: String,
}

pub(crate) enum WatcherEvent {
    MicroSaveCommitted {
        project_file: PathBuf,
        short_id: String,
    },
    ProjectFileChanged(PathBuf),
}

pub(crate) struct LiveWatcherHandle {
    _watcher: RecommendedWatcher,
    pub(crate) rx: Receiver<WatcherEvent>,
}

pub(crate) fn start_live_watcher(repo_path: PathBuf) -> Option<LiveWatcherHandle> {
    let watch_root = resolve_db::guest_projects_dir()
        .filter(|p| p.is_dir())
        .or_else(|| {
            let support = dirs::data_dir()?
                .join("Blackmagic Design")
                .join("DaVinci Resolve")
                .join("Support")
                .join("Resolve Project Backups");
            support.is_dir().then_some(support)
        })?;

    let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
    let (ui_tx, ui_rx) = mpsc::channel::<WatcherEvent>();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            let relevant = matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_));
            if !relevant {
                return;
            }
            for path in event.paths {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.eq_ignore_ascii_case("Project.xml")
                    || name.eq_ignore_ascii_case("Project.db")
                    || name.eq_ignore_ascii_case("Project.db-wal")
                {
                    let target = if name.eq_ignore_ascii_case("Project.db-wal") {
                        path.with_file_name("Project.db")
                    } else {
                        path
                    };
                    let _ = raw_tx.send(target);
                }
            }
        }
    })
    .ok()?;

    watcher.watch(&watch_root, RecursiveMode::Recursive).ok()?;

    thread::spawn(move || {
        let debounce = Duration::from_millis(DEBOUNCE_MS);
        let mut pending_path: Option<PathBuf> = None;
        let mut last_event_at: Option<Instant> = None;

        loop {
            match raw_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(path) => {
                    pending_path = Some(path);
                    last_event_at = Some(Instant::now());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let (Some(path), Some(ts)) = (pending_path.as_ref(), last_event_at) {
                        if ts.elapsed() >= debounce {
                            let settled_path = path.clone();
                            pending_path = None;
                            last_event_at = None;

                            let _ = sync_project_file_to_repo(&settled_path, &repo_path);
                            if let Ok(Some(short_id)) = commit_micro_save_if_dirty(&repo_path) {
                                let _ = ui_tx.send(WatcherEvent::MicroSaveCommitted {
                                    project_file: settled_path,
                                    short_id,
                                });
                            } else {
                                let _ = ui_tx.send(WatcherEvent::ProjectFileChanged(settled_path));
                            }
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    Some(LiveWatcherHandle {
        _watcher: watcher,
        rx: ui_rx,
    })
}

pub(crate) fn sync_project_file_to_repo(
    project_file: &Path,
    repo_path: &Path,
) -> Result<(), String> {
    if !project_file.is_file() {
        return Ok(());
    }
    let timelines_dir = repo_path.join("Timelines");
    std::fs::create_dir_all(&timelines_dir)
        .map_err(|e| format!("Failed to create Timelines dir: {e}"))?;
    let file_name = project_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Project.db");
    let dest = timelines_dir.join(file_name);
    std::fs::copy(project_file, dest)
        .map(|_| ())
        .map_err(|e| format!("Failed to mirror project file: {e}"))
}

pub(crate) fn commit_micro_save_if_dirty(repo_path: &Path) -> Result<Option<String>, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let statuses = repo
        .statuses(None)
        .map_err(|e| format!("Git status error: {e}"))?;

    if statuses.is_empty() {
        return Ok(None);
    }

    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("git add . error: {e}"))?;
    index
        .write()
        .map_err(|e| format!("Index write error: {e}"))?;

    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Tree write error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Tree find error: {e}"))?;

    let parent_commit = repo
        .find_branch(SCRATCH_BRANCH, BranchType::Local)
        .ok()
        .and_then(|b| b.get().peel_to_commit().ok())
        .or_else(|| repo.head().ok().and_then(|h| h.peel_to_commit().ok()));

    if let Some(ref parent) = parent_commit {
        if parent.tree_id() == tree_id {
            return Ok(None);
        }
    }

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Zane", "zane@editor.local"))
        .map_err(|e| format!("Signature error: {e}"))?;

    let iso_ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let msg = format!("micro-save: {iso_ts}");

    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();
    let scratch_ref = format!("refs/heads/{SCRATCH_BRANCH}");

    let oid = repo
        .commit(Some(&scratch_ref), &sig, &sig, &msg, &tree, &parents)
        .map_err(|e| format!("Micro-save commit error: {e}"))?;

    Ok(Some(format!("{:.7}", oid)))
}

pub(crate) fn list_micro_commits(repo_path: &Path) -> Vec<MicroCommitNode> {
    let Ok(repo) = Repository::open(repo_path) else {
        return Vec::new();
    };
    let Ok(branch) = repo.find_branch(SCRATCH_BRANCH, BranchType::Local) else {
        return Vec::new();
    };
    let Some(tip_oid) = branch.get().target() else {
        return Vec::new();
    };

    let Ok(mut revwalk) = repo.revwalk() else {
        return Vec::new();
    };
    let _ = revwalk.set_sorting(Sort::TIME);
    let _ = revwalk.push(tip_oid);

    let mut nodes = Vec::new();
    for oid in revwalk.take(25).flatten() {
        if let Ok(commit) = repo.find_commit(oid) {
            let message = commit.summary().unwrap_or("micro-save").to_string();
            let timestamp = chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
                .map(|d| d.format("%H:%M:%S").to_string())
                .unwrap_or_default();
            nodes.push(MicroCommitNode {
                full_id: oid.to_string(),
                short_id: format!("{:.7}", oid),
                message,
                timestamp,
            });
        }
    }
    nodes
}

pub(crate) fn rollback_to_commit(repo_path: &Path, full_oid: &str) -> Result<(), String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let oid = Oid::from_str(full_oid).map_err(|e| format!("Invalid OID: {e}"))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| format!("Commit lookup error: {e}"))?;
    let tree = commit
        .tree()
        .map_err(|e| format!("Tree lookup error: {e}"))?;

    let mut opts = CheckoutBuilder::new();
    opts.force();
    repo.checkout_tree(tree.as_object(), Some(&mut opts))
        .map_err(|e| format!("Checkout rollback error: {e}"))?;

    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
    index
        .read_tree(&tree)
        .map_err(|e| format!("Index tree error: {e}"))?;
    index
        .write()
        .map_err(|e| format!("Index write error: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_save_commits_to_scratch_and_rolls_back() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_watcher_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("Timelines")).unwrap();

        let repo = Repository::init(&temp_root).unwrap();
        let file_path = temp_root.join("Timelines").join("cut.otio");
        std::fs::write(&file_path, "v1").unwrap();

        let first = commit_micro_save_if_dirty(&temp_root).unwrap();
        assert!(first.is_some());

        std::fs::write(&file_path, "v2").unwrap();
        let second = commit_micro_save_if_dirty(&temp_root).unwrap();
        assert!(second.is_some());

        let list = list_micro_commits(&temp_root);
        assert_eq!(list.len(), 2);
        assert!(list[0].message.starts_with("micro-save:"));

        // Rollback to first micro-save (oldest in list)
        rollback_to_commit(&temp_root, &list[1].full_id).unwrap();
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "v1");

        drop(repo);
        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
