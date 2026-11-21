use crate::{git_engine, otio_diff};
use chrono::Utc;
use git2::{BranchType, IndexAddOption, Repository, Signature, Sort};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, DebouncedEventKind, Debouncer};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub(crate) const SCRATCH_BRANCH: &str = "scratch/live-stream";
const EMPTY_OTIO_TIMELINE: &str =
    r#"{"OTIO_SCHEMA":"Timeline.1","name":"Timeline","tracks":{"OTIO_SCHEMA":"Stack.1","children":[]}}"#;

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct MicroCommitNode {
    pub(crate) full_id: String,
    pub(crate) short_id: String,
    pub(crate) message: String,
    pub(crate) timestamp: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppEvent {
    RepoUpdated,
}

pub(crate) struct TimelineWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn start_timeline_watcher(
    repo_path: PathBuf,
) -> Option<(TimelineWatcher, Receiver<AppEvent>)> {
    let timelines_dir = repo_path.join("Timelines");
    let _ = std::fs::create_dir_all(&timelines_dir);
    let target_cut_path = timelines_dir.join("cut.otio");

    let (tx, rx) = mpsc::channel::<AppEvent>();
    let watch_repo = repo_path.clone();

    let mut last_cut_hash: Option<u64> = std::fs::read(&target_cut_path)
        .ok()
        .map(|b| hash_bytes(&b));

    let mut debouncer = new_debouncer(
        Duration::from_millis(400),
        move |res: DebounceEventResult| {
            let Ok(events) = res else {
                return;
            };

            let cut_changed = events.iter().any(|ev| {
                matches!(
                    ev.kind,
                    DebouncedEventKind::Any | DebouncedEventKind::AnyContinuous
                ) && ev
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("cut.otio"))
            });

            if !cut_changed {
                return;
            }

            let Ok(bytes) = std::fs::read(&target_cut_path) else {
                return;
            };
            let new_hash = hash_bytes(&bytes);
            if last_cut_hash == Some(new_hash) {
                return;
            }
            last_cut_hash = Some(new_hash);

            if let Ok(Some(_commit_info)) = auto_commit_cut_change(&watch_repo) {
                let _ = tx.send(AppEvent::RepoUpdated);
            }
        },
    )
    .ok()?;

    debouncer
        .watcher()
        .watch(&timelines_dir, RecursiveMode::NonRecursive)
        .ok()?;

    Some((
        TimelineWatcher {
            _debouncer: debouncer,
        },
        rx,
    ))
}

fn find_disk_cut_otio(repo_path: &Path) -> Option<PathBuf> {
    let timelines_cut = repo_path.join("Timelines").join("cut.otio");
    if timelines_cut.is_file() {
        return Some(timelines_cut);
    }
    let root_cut = repo_path.join("cut.otio");
    if root_cut.is_file() {
        return Some(root_cut);
    }
    None
}

pub(crate) fn build_semantic_msg_against_head(repo_path: &Path) -> Option<String> {
    let cut_otio_path = find_disk_cut_otio(repo_path)?;
    let curr_raw = std::fs::read_to_string(&cut_otio_path).ok()?;
    let head_raw = git_engine::read_head_file(repo_path, "Timelines/cut.otio")
        .unwrap_or_else(|| EMPTY_OTIO_TIMELINE.to_string());
    let report = otio_diff::extract_otio_delta(&head_raw, &curr_raw).ok()?;
    if let Ok(pretty) = serde_json::to_string_pretty(&report) {
        let timelines_dir = repo_path.join("Timelines");
        let _ = std::fs::create_dir_all(&timelines_dir);
        let _ = std::fs::write(timelines_dir.join("incoming_cut_delta.json"), pretty);
    }
    Some(otio_diff::generate_semantic_commit_message(&report))
}

pub(crate) fn auto_commit_cut_change(
    repo_path: &Path,
) -> Result<Option<(String, String)>, String> {
    let semantic_msg = build_semantic_msg_against_head(repo_path);
    let label = semantic_msg
        .clone()
        .unwrap_or_else(|| "[Edit] Adjusted timeline cut".to_string());
    match git_engine::commit_working_tree_with_message(repo_path, &label)? {
        Some(short_id) => Ok(Some((short_id, label))),
        None => Ok(None),
    }
}

#[cfg(test)]
pub(crate) fn commit_micro_save_if_dirty(repo_path: &Path) -> Result<Option<String>, String> {
    commit_micro_save_if_dirty_with_msg(repo_path, None)
}

pub(crate) fn commit_micro_save_if_dirty_with_msg(
    repo_path: &Path,
    custom_message: Option<&str>,
) -> Result<Option<String>, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let statuses = repo
        .statuses(None)
        .map_err(|e| format!("Git status error: {e}"))?;

    if statuses.is_empty() {
        return Ok(None);
    }

    git_engine::ensure_gitignore_excludes_restore_otio(repo_path);
    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("git add . error: {e}"))?;
    let _ = index.remove_all(
        [
            "Timelines/Restore_*.otio",
            "Timelines/cut_tmp.otio",
            "Timelines/*.otio.tmp",
            "Timelines/*.signal",
            ".sidecar",
            ".sidecar/*",
        ]
        .iter(),
        None,
    );
    index
        .write()
        .map_err(|e| format!("Index write error: {e}"))?;

    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Tree write error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Tree find error: {e}"))?;

    let custom_present = custom_message
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());

    let head_branch = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().map(str::to_string));

    let target_ref = if custom_present {
        match head_branch.as_deref() {
            Some(active)
                if active == "main"
                    || active.starts_with("experiment/")
                    || active.starts_with("pass/") =>
            {
                format!("refs/heads/{active}")
            }
            _ => format!("refs/heads/{SCRATCH_BRANCH}"),
        }
    } else {
        match head_branch.as_deref() {
            Some(active) if active.starts_with("experiment/") || active.starts_with("pass/") => {
                format!("refs/heads/{active}")
            }
            _ => format!("refs/heads/{SCRATCH_BRANCH}"),
        }
    };

    let parent_commit = repo
        .find_reference(&target_ref)
        .ok()
        .and_then(|r| r.peel_to_commit().ok())
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
    let msg = custom_message
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("micro-save: {iso_ts}"));

    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();

    let oid = repo
        .commit(Some(&target_ref), &sig, &sig, &msg, &tree, &parents)
        .map_err(|e| format!("Micro-save commit error: {e}"))?;

    let scratch_ref = format!("refs/heads/{SCRATCH_BRANCH}");
    if target_ref != scratch_ref {
        let _ = repo.reference(&scratch_ref, oid, true, "micro-save sync");
    }

    Ok(Some(format!("{:.7}", oid)))
}

#[allow(dead_code)]
fn format_relative_time(commit_secs: i64) -> String {
    let now_secs = chrono::Utc::now().timestamp();
    let elapsed = (now_secs - commit_secs).max(0);
    if elapsed < 5 {
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

#[allow(dead_code)]
fn clean_micro_message(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with("micro-save:") || trimmed == "micro-save" {
        "Adjusted timeline cut".to_string()
    } else {
        trimmed.to_string()
    }
}

#[allow(dead_code)]
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
            let raw_message = commit.summary().unwrap_or("micro-save");
            let message = clean_micro_message(raw_message);
            let timestamp = format_relative_time(commit.time().seconds());
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

#[cfg(test)]
pub(crate) fn rollback_to_commit(repo_path: &Path, full_oid: &str) -> Result<(), String> {
    use git2::{build::CheckoutBuilder, Oid};
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
        assert_eq!(list[0].message, "Adjusted timeline cut");

        rollback_to_commit(&temp_root, &list[1].full_id).unwrap();
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "v1");

        drop(repo);
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn otio_diff_against_head_commits_with_semantic_prefix() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_watcher_head_diff_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("Timelines")).unwrap();

        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(&temp_root, &opts).unwrap();
        let cut_path = temp_root.join("Timelines").join("cut.otio");
        std::fs::write(&cut_path, EMPTY_OTIO_TIMELINE).unwrap();

        let mut idx = repo.index().unwrap();
        idx.add_all(["Timelines"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tid = idx.write_tree().unwrap();
        let tree = repo.find_tree(tid).unwrap();
        let sig = Signature::now("Zane", "zane@local").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let v2 = r#"{
            "OTIO_SCHEMA": "Timeline.1",
            "name": "Master",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [
                    {
                        "OTIO_SCHEMA": "Track.1",
                        "name": "Audio 1",
                        "kind": "Audio",
                        "children": [
                            {
                                "OTIO_SCHEMA": "Clip.2",
                                "name": "score.wav",
                                "source_range": {
                                    "start_time": { "value": 0.0, "rate": 24.0 },
                                    "duration": { "value": 120.0, "rate": 24.0 }
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;
        std::fs::write(&cut_path, v2).unwrap();

        let (short_id, msg) = auto_commit_cut_change(&temp_root)
            .unwrap()
            .unwrap();
        assert!(!short_id.is_empty());
        assert!(msg.starts_with("[Audio]"));
        assert_eq!(repo.head().unwrap().shorthand(), Some("main"));

        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head_commit.summary().unwrap(), msg);

        drop(head_commit);
        drop(tree);
        drop(repo);
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn debouncer_detects_cut_otio_change_and_emits_event() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_debouncer_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("Timelines")).unwrap();

        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(&temp_root, &opts).unwrap();
        let cut_path = temp_root.join("Timelines").join("cut.otio");
        std::fs::write(&cut_path, EMPTY_OTIO_TIMELINE).unwrap();
        git_engine::commit_working_tree_with_message(&temp_root, "init").unwrap();

        let (_watcher, rx) = start_timeline_watcher(temp_root.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(150));

        let v2 = r#"{
            "OTIO_SCHEMA": "Timeline.1",
            "name": "Master",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [
                    {
                        "OTIO_SCHEMA": "Track.1",
                        "name": "Video 1",
                        "kind": "Video",
                        "children": [
                            {
                                "OTIO_SCHEMA": "Clip.2",
                                "name": "intro.mov",
                                "source_range": {
                                    "start_time": { "value": 0.0, "rate": 24.0 },
                                    "duration": { "value": 48.0, "rate": 24.0 }
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;
        std::fs::write(&cut_path, v2).unwrap();

        let evt = rx.recv_timeout(Duration::from_secs(4)).unwrap();
        assert_eq!(evt, AppEvent::RepoUpdated);

        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert!(head_commit.summary().unwrap().starts_with("[Edit]"));

        drop(head_commit);
        drop(repo);
        drop(_watcher);
        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
