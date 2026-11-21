use crate::{resolve_db, TimelineClip, TimelineDiffItem};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct TimelineSnapshot {
    pub(crate) project_name: String,
    pub(crate) timeline_name: String,
    pub(crate) fps: String,
    pub(crate) tracks: Vec<String>,
    pub(crate) clips: Vec<TimelineClip>,
}

pub(crate) fn resolve_projects_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(roaming) = dirs::data_dir() {
        let support = roaming.join("Blackmagic Design").join("DaVinci Resolve").join("Support");
        roots.push(support.join("Resolve Project Library").join("Resolve Projects"));
        roots.push(support.join("Resolve Disk Database").join("Resolve Projects"));
    }
    if let Some(local) = dirs::data_local_dir() {
        let support = local.join("Blackmagic Design").join("DaVinci Resolve").join("Support");
        roots.push(support.join("Resolve Project Library").join("Resolve Projects"));
        roots.push(support.join("Resolve Disk Database").join("Resolve Projects"));
    }
    roots.push(PathBuf::from(
        r"C:\ProgramData\Blackmagic Design\DaVinci Resolve\Support\Resolve Project Library\Resolve Projects",
    ));
    roots.push(PathBuf::from(
        r"C:\ProgramData\Blackmagic Design\DaVinci Resolve\Support\Resolve Disk Database\Resolve Projects",
    ));
    roots
}

pub(crate) fn find_latest_project_file(configured_project: Option<&str>) -> Option<PathBuf> {
    if let Some(found) = resolve_db::find_most_recent_project_file(configured_project) {
        return Some(found);
    }

    let mut best: Option<(SystemTime, PathBuf)> = None;

    for root in resolve_projects_roots() {
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&root)
            .max_depth(6)
            .into_iter()
            .filter_map(Result::ok)
        {
            let name = entry.file_name();
            if entry.file_type().is_file() && (name == "Project.xml" || name == "Project.db") {
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                if best
                    .as_ref()
                    .map(|(t, _)| modified > *t)
                    .unwrap_or(true)
                {
                    best = Some((modified, entry.path().to_path_buf()));
                }
            }
        }
    }

    best.map(|(_, path)| path)
}

fn copy_db_snapshot_for_read(db_path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let temp_dir = std::env::temp_dir().join(format!(
        "resolve_sidecar_db_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temp DB dir: {e}"))?;

    let temp_db = temp_dir.join("Project.db");
    std::fs::copy(db_path, &temp_db)
        .map_err(|e| format!("Failed to copy Project.db for lock-free read: {e}"))?;

    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        if side.is_file() {
            let _ = std::fs::copy(&side, temp_dir.join(format!("Project.db{suffix}")));
        }
    }

    Ok((temp_dir, temp_db))
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

fn read_sm2_clips(conn: &Connection) -> Result<Vec<TimelineClip>, String> {
    // Resolve's internal Sm2TiItem table stores timeline items; column names vary slightly across versions.
    let mut col_stmt = conn
        .prepare("PRAGMA table_info(Sm2TiItem)")
        .map_err(|e| format!("PRAGMA Sm2TiItem error: {e}"))?;
    let cols: Vec<String> = col_stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("PRAGMA read error: {e}"))?
        .filter_map(Result::ok)
        .collect();

    let has = |c: &str| cols.iter().any(|x| x.eq_ignore_ascii_case(c));
    let name_col = if has("Name") { "Name" } else { "''" };
    let start_col = if has("Start") { "Start" } else { "0" };
    let dur_col = if has("Duration") { "Duration" } else { "0" };
    let offset_col = if has("MediaStartOffset") {
        "MediaStartOffset"
    } else if has("StartOffset") {
        "StartOffset"
    } else {
        "0"
    };
    let track_type_col = if has("TrackType") { "TrackType" } else { "'video'" };
    let track_idx_col = if has("TrackIndex") { "TrackIndex" } else { "1" };
    let media_col = if has("MediaFilePath") {
        "MediaFilePath"
    } else if has("FilePath") {
        "FilePath"
    } else {
        "''"
    };

    let sql = format!(
        "SELECT {name_col}, {start_col}, {dur_col}, {offset_col}, {track_type_col}, {track_idx_col}, {media_col} FROM Sm2TiItem"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Sm2TiItem query error: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            let clip_name: String = row.get::<_, Option<String>>(0)?.unwrap_or_default();
            let start_frame: i64 = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
            let duration: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let source_in: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
            let track_raw: String = row
                .get::<_, Option<String>>(4)?
                .unwrap_or_else(|| "video".to_string());
            let track_index: usize = row.get::<_, Option<i64>>(5)?.unwrap_or(1).max(1) as usize;
            let media_path: String = row.get::<_, Option<String>>(6)?.unwrap_or_default();

            let track_type = if track_raw.to_ascii_lowercase().contains("audio") {
                "audio".to_string()
            } else {
                "video".to_string()
            };

            Ok(TimelineClip {
                track_type,
                track_index,
                clip_name: if clip_name.is_empty() {
                    "Unnamed Clip".to_string()
                } else {
                    clip_name
                },
                media_path,
                start_frame,
                end_frame: start_frame + duration,
                duration,
                source_in,
                source_out: source_in + duration,
                speed: 100.0,
                cdl: None,
            })
        })
        .map_err(|e| format!("Sm2TiItem map error: {e}"))?;

    Ok(rows.filter_map(Result::ok).collect())
}

fn read_generic_clips(conn: &Connection) -> Result<Vec<TimelineClip>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT track_type, track_index, clip_name, media_path, start_frame, end_frame, duration, source_in, source_out, speed \
             FROM clips ORDER BY track_type, track_index, start_frame",
        )
        .map_err(|e| format!("clips query error: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(TimelineClip {
                track_type: row.get::<_, String>(0)?,
                track_index: row.get::<_, i64>(1)?.max(1) as usize,
                clip_name: row.get::<_, String>(2)?,
                media_path: row.get::<_, String>(3)?,
                start_frame: row.get::<_, i64>(4)?,
                end_frame: row.get::<_, i64>(5)?,
                duration: row.get::<_, i64>(6)?,
                source_in: row.get::<_, i64>(7)?,
                source_out: row.get::<_, i64>(8)?,
                speed: row.get::<_, f64>(9)?,
                cdl: None,
            })
        })
        .map_err(|e| format!("clips map error: {e}"))?;

    Ok(rows.filter_map(Result::ok).collect())
}

fn extract_xml_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let rest = &block[start..];
    let end = rest.find(&close)?;
    let val = rest[..end].trim();
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

pub(crate) fn read_timeline_snapshot_from_xml(xml_path: &Path) -> Result<TimelineSnapshot, String> {
    let raw = std::fs::read_to_string(xml_path)
        .map_err(|e| format!("Failed to read {}: {e}", xml_path.display()))?;

    let mut clips = Vec::new();
    for tag in ["Sm2TiItem", "Clip", "VideoClip", "AudioClip", "clipitem"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let mut cursor = 0usize;
        while let Some(rel_start) = raw[cursor..].find(&open) {
            let abs_start = cursor + rel_start;
            let after_open = &raw[abs_start..];
            let Some(rel_end) = after_open.find(&close) else {
                break;
            };
            let block = &after_open[..rel_end + close.len()];
            cursor = abs_start + rel_end + close.len();

            let clip_name = extract_xml_tag(block, "Name")
                .or_else(|| extract_xml_tag(block, "name"))
                .unwrap_or_else(|| "Unnamed Clip".to_string());
            let start_frame = extract_xml_tag(block, "Start")
                .or_else(|| extract_xml_tag(block, "start"))
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            let duration = extract_xml_tag(block, "Duration")
                .or_else(|| extract_xml_tag(block, "duration"))
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            let source_in = extract_xml_tag(block, "MediaStartOffset")
                .or_else(|| extract_xml_tag(block, "in"))
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            let track_raw = extract_xml_tag(block, "TrackType")
                .unwrap_or_else(|| {
                    if tag.eq_ignore_ascii_case("AudioClip") {
                        "audio".to_string()
                    } else {
                        "video".to_string()
                    }
                });
            let track_type = if track_raw.to_ascii_lowercase().contains("audio") {
                "audio".to_string()
            } else {
                "video".to_string()
            };
            let track_index = extract_xml_tag(block, "TrackIndex")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let media_path = extract_xml_tag(block, "MediaFilePath")
                .or_else(|| extract_xml_tag(block, "FilePath"))
                .or_else(|| extract_xml_tag(block, "pathurl"))
                .unwrap_or_default();

            clips.push(TimelineClip {
                track_type,
                track_index,
                clip_name,
                media_path,
                start_frame,
                end_frame: start_frame + duration,
                duration,
                source_in,
                source_out: source_in + duration,
                speed: 100.0,
                cdl: None,
            });
        }
    }

    let mut tracks = Vec::new();
    for clip in &clips {
        let prefix = if clip.track_type == "audio" { "A" } else { "V" };
        let label = format!("{prefix}{}", clip.track_index);
        if !tracks.contains(&label) {
            tracks.push(label);
        }
    }

    let project_name = xml_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("Resolve Project")
        .to_string();

    Ok(TimelineSnapshot {
        project_name,
        timeline_name: "Active Timeline".to_string(),
        fps: "24".to_string(),
        tracks,
        clips,
    })
}

pub(crate) fn read_timeline_snapshot(project_file: &Path) -> Result<TimelineSnapshot, String> {
    let is_xml = project_file
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("xml"))
        .unwrap_or(false);

    if is_xml {
        read_timeline_snapshot_from_xml(project_file)
    } else {
        read_timeline_snapshot_from_db(project_file)
    }
}

pub(crate) fn read_timeline_snapshot_from_db(db_path: &Path) -> Result<TimelineSnapshot, String> {
    if !db_path.is_file() {
        return Err(format!("Project.db not found at {}", db_path.display()));
    }

    let (temp_dir, temp_db) = copy_db_snapshot_for_read(db_path)?;
    let conn = Connection::open_with_flags(
        &temp_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("SQLite open error: {e}"))?;

    let clips = if table_exists(&conn, "Sm2TiItem") {
        read_sm2_clips(&conn)?
    } else if table_exists(&conn, "clips") {
        read_generic_clips(&conn)?
    } else {
        Vec::new()
    };

    drop(conn);
    let _ = std::fs::remove_dir_all(&temp_dir);

    let mut tracks = Vec::new();
    for clip in &clips {
        let prefix = if clip.track_type == "audio" { "A" } else { "V" };
        let label = format!("{prefix}{}", clip.track_index);
        if !tracks.contains(&label) {
            tracks.push(label);
        }
    }

    let project_name = db_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("Resolve Project")
        .to_string();

    Ok(TimelineSnapshot {
        project_name,
        timeline_name: "Active Timeline".to_string(),
        fps: "24".to_string(),
        tracks,
        clips,
    })
}

fn clip_identity_keys(clips: &[TimelineClip]) -> HashMap<(String, usize, String, usize), TimelineClip> {
    let mut counts: HashMap<(String, usize, String), usize> = HashMap::new();
    let mut keyed = HashMap::new();

    for clip in clips {
        let identity = if !clip.media_path.is_empty() {
            clip.media_path.clone()
        } else {
            clip.clip_name.clone()
        };
        let base = (clip.track_type.clone(), clip.track_index, identity);
        let occ = *counts.get(&base).unwrap_or(&0);
        counts.insert(base.clone(), occ + 1);
        keyed.insert((base.0, base.1, base.2, occ), clip.clone());
    }

    keyed
}

fn clips_equal(a: &TimelineClip, b: &TimelineClip) -> bool {
    a.clip_name == b.clip_name
        && a.media_path == b.media_path
        && a.start_frame == b.start_frame
        && a.end_frame == b.end_frame
        && a.duration == b.duration
        && a.source_in == b.source_in
        && a.source_out == b.source_out
        && (a.speed - b.speed).abs() < f64::EPSILON
        && a.cdl == b.cdl
}

pub(crate) fn compute_timeline_diff(
    baseline_clips: &[TimelineClip],
    live_clips: &[TimelineClip],
) -> Vec<TimelineDiffItem> {
    let base_map = clip_identity_keys(baseline_clips);
    let live_map = clip_identity_keys(live_clips);
    let mut diff_items = Vec::new();

    for (key, live_clip) in &live_map {
        match base_map.get(key) {
            None => diff_items.push(TimelineDiffItem {
                change_type: "ADDED".to_string(),
                clip: live_clip.clone(),
                previous_clip: None,
            }),
            Some(base_clip) if !clips_equal(base_clip, live_clip) => {
                diff_items.push(TimelineDiffItem {
                    change_type: "MODIFIED".to_string(),
                    clip: live_clip.clone(),
                    previous_clip: Some(base_clip.clone()),
                });
            }
            _ => {}
        }
    }

    for (key, base_clip) in &base_map {
        if !live_map.contains_key(key) {
            diff_items.push(TimelineDiffItem {
                change_type: "DELETED".to_string(),
                clip: base_clip.clone(),
                previous_clip: None,
            });
        }
    }

    diff_items.sort_by_key(|item| {
        (
            item.clip.track_type.clone(),
            item.clip.track_index,
            item.clip.start_frame,
        )
    });

    diff_items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_project_db_snapshot_and_diff_engine() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_sidecar_db_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        let proj_dir = temp_root.join("DemoProject");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let db_path = proj_dir.join("Project.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE Sm2TiItem (
                    Name TEXT,
                    Start INTEGER,
                    Duration INTEGER,
                    MediaStartOffset INTEGER,
                    TrackType TEXT,
                    TrackIndex INTEGER,
                    MediaFilePath TEXT
                )",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO Sm2TiItem VALUES ('Clip_A', 0, 100, 10, 'video', 1, 'C:\\Media\\A.mov')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO Sm2TiItem VALUES ('Music_1', 0, 200, 0, 'audio', 2, 'C:\\Media\\M.wav')",
                [],
            )
            .unwrap();
        }

        let base_snap = read_timeline_snapshot_from_db(&db_path).unwrap();
        assert_eq!(base_snap.clips.len(), 2);
        assert_eq!(base_snap.project_name, "DemoProject");

        // Mutate Project.db: modify Clip_A duration, delete Music_1, add Clip_B
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "UPDATE Sm2TiItem SET Duration = 150 WHERE Name = 'Clip_A'",
                [],
            )
            .unwrap();
            conn.execute("DELETE FROM Sm2TiItem WHERE Name = 'Music_1'", [])
                .unwrap();
            conn.execute(
                "INSERT INTO Sm2TiItem VALUES ('Clip_B', 150, 80, 0, 'video', 1, 'C:\\Media\\B.mov')",
                [],
            )
            .unwrap();
        }

        let live_snap = read_timeline_snapshot_from_db(&db_path).unwrap();
        let diff = compute_timeline_diff(&base_snap.clips, &live_snap.clips);

        assert_eq!(diff.len(), 3);
        assert!(diff.iter().any(|d| d.change_type == "MODIFIED" && d.clip.clip_name == "Clip_A"));
        assert!(diff.iter().any(|d| d.change_type == "ADDED" && d.clip.clip_name == "Clip_B"));
        assert!(diff.iter().any(|d| d.change_type == "DELETED" && d.clip.clip_name == "Music_1"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
