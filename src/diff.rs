use crate::{db_watcher, TimelineClip, TimelineDiffItem};
use git2::{BranchType, Repository};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct OtioDocument {
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) tracks: Option<OtioStack>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct OtioStack {
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) children: Vec<OtioTrack>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct OtioTrack {
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) children: Vec<OtioClip>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct OtioClip {
    #[serde(default)]
    pub(crate) name: Option<String>,
}

pub(crate) struct ComputedPassDiff {
    pub(crate) timeline_name: String,
    pub(crate) track_summary: String,
    pub(crate) diff_items: Vec<TimelineDiffItem>,
    pub(crate) live_snapshot: Option<db_watcher::TimelineSnapshot>,
}

pub(crate) fn unwrap_timeline_name(
    otio_path: Option<&Path>,
    baseline_path: &Path,
    fallback_project_name: &str,
) -> String {
    if let Some(otio) = otio_path.filter(|p| p.is_file()) {
        if let Ok(raw) = std::fs::read_to_string(otio) {
            if let Ok(doc) = serde_json::from_str::<OtioDocument>(&raw) {
                if let Some(name) = doc.name.filter(|s| !s.trim().is_empty()) {
                    return name;
                }
                if let Some(stack_name) = doc
                    .tracks
                    .as_ref()
                    .and_then(|t| t.name.as_ref())
                    .filter(|s| !s.trim().is_empty())
                {
                    return stack_name.clone();
                }
            }
        }
    }

    if baseline_path.is_file() {
        if let Ok(raw) = std::fs::read_to_string(baseline_path) {
            if let Ok(snap) = serde_json::from_str::<db_watcher::TimelineSnapshot>(&raw) {
                if !snap.timeline_name.trim().is_empty() {
                    return snap.timeline_name;
                }
            }
        }
    }

    if !fallback_project_name.trim().is_empty() {
        format!("{} - Master Timeline", fallback_project_name.trim())
    } else {
        "Master Timeline".to_string()
    }
}

pub(crate) fn format_track_summary(items: &[TimelineDiffItem], timeline_name: &str) -> String {
    let mut tracks: Vec<String> = Vec::new();
    for item in items {
        let prefix = if item.clip.track_type == "audio" {
            "A"
        } else {
            "V"
        };
        let label = format!("{prefix}{}", item.clip.track_index);
        if !tracks.contains(&label) {
            tracks.push(label);
        }
    }

    if tracks.is_empty() {
        timeline_name.to_string()
    } else {
        tracks.join(", ")
    }
}

pub(crate) fn compute_pass_diff(
    repo_path: &Path,
    project_file: Option<&Path>,
    baseline_path: &Path,
) -> Result<ComputedPassDiff, String> {
    let baseline_clips: Vec<TimelineClip> = match std::fs::read_to_string(baseline_path) {
        Ok(raw) => serde_json::from_str::<db_watcher::TimelineSnapshot>(&raw)
            .map(|b| b.clips)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    let otio_candidate = find_otio_in_timelines(&repo_path.join("Timelines"));

    let mut live_snapshot = match project_file {
        Some(pf) if pf.is_file() => Some(db_watcher::read_timeline_snapshot(pf)?),
        _ => None,
    };

    let fallback_proj = live_snapshot
        .as_ref()
        .map(|s| s.project_name.as_str())
        .unwrap_or("Resolve Project");

    let timeline_name =
        unwrap_timeline_name(otio_candidate.as_deref(), baseline_path, fallback_proj);

    if let Some(ref mut snap) = live_snapshot {
        snap.timeline_name = timeline_name.clone();
    }

    let mut diff_items = if let Some(ref snap) = live_snapshot {
        db_watcher::compute_timeline_diff(&baseline_clips, &snap.clips)
    } else {
        Vec::new()
    };

    // If Project.db/Project.xml yielded 0 structured clip diffs, inspect Git tree diff
    // between `scratch/live-stream` and `main` so file-level timeline changes are never lost.
    if diff_items.is_empty() {
        diff_items = diff_scratch_vs_main(repo_path, &timeline_name);
    }

    let track_summary = format_track_summary(&diff_items, &timeline_name);

    Ok(ComputedPassDiff {
        timeline_name,
        track_summary,
        diff_items,
        live_snapshot,
    })
}

fn find_otio_in_timelines(timelines_dir: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(timelines_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("otio"))
            .unwrap_or(false)
        {
            return Some(path);
        }
    }
    None
}

fn diff_scratch_vs_main(repo_path: &Path, timeline_name: &str) -> Vec<TimelineDiffItem> {
    let Ok(repo) = Repository::open(repo_path) else {
        return Vec::new();
    };
    let Ok(scratch_branch) = repo.find_branch("scratch/live-stream", BranchType::Local) else {
        return Vec::new();
    };
    let Ok(scratch_commit) = scratch_branch.get().peel_to_commit() else {
        return Vec::new();
    };
    let Ok(scratch_tree) = scratch_commit.tree() else {
        return Vec::new();
    };

    let main_tree = repo
        .find_branch("main", BranchType::Local)
        .ok()
        .and_then(|b| b.get().peel_to_commit().ok())
        .and_then(|c| c.tree().ok());

    let Ok(diff) = repo.diff_tree_to_tree(main_tree.as_ref(), Some(&scratch_tree), None) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for delta in diff.deltas() {
        let change_type = match delta.status() {
            git2::Delta::Added => "ADDED",
            git2::Delta::Deleted => "DELETED",
            _ => "MODIFIED",
        };
        let rel_path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| timeline_name.to_string());

        let track_type = if rel_path.to_ascii_lowercase().contains("audio") {
            "audio".to_string()
        } else {
            "video".to_string()
        };

        items.push(TimelineDiffItem {
            change_type: change_type.to_string(),
            clip: TimelineClip {
                track_type,
                track_index: 1,
                clip_name: rel_path.clone(),
                media_path: repo_path.join(&rel_path).to_string_lossy().to_string(),
                start_frame: 0,
                end_frame: 0,
                duration: 0,
                source_in: 0,
                source_out: 0,
                speed: 100.0,
                cdl: None,
            },
            previous_clip: None,
        });
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_timeline_name_never_returns_none_literal() {
        let temp = std::env::temp_dir().join(format!("resolve_diff_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp);
        let otio = temp.join("cut.otio");
        std::fs::write(&otio, r#"{"name": "Episode 04 Cut", "tracks": {"name": "Stack"}}"#)
            .unwrap();

        let name = unwrap_timeline_name(Some(&otio), &temp.join(".timeline_base.json"), "Proj");
        assert_eq!(name, "Episode 04 Cut");
        assert_ne!(name, "none");

        let fallback = unwrap_timeline_name(None, &temp.join("missing.json"), "MyProj");
        assert_eq!(fallback, "MyProj - Master Timeline");
        assert_ne!(fallback, "none");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
