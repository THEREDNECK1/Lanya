use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClipMeta {
    pub name: String,
    pub track_name: String,
    pub track_type: String, // "Video" or "Audio"
    pub start_seconds: f64,
    pub duration_seconds: f64,
    pub timeline_offset_sec: f64,
    pub rate: f64,
}

#[derive(Default, Clone, Debug)]
pub struct TimelineSnapshot {
    pub name: String,
    pub clips: HashMap<String, ClipMeta>,
    pub track_names: Vec<String>,
    pub total_duration: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrimDelta {
    pub clip_name: String,
    pub track_name: String,
    pub frame_delta: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitPoint {
    pub track_name: String,
    pub clip_name: String,
    pub at_seconds: f64,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct TimelineDelta {
    pub modified_tracks: Vec<String>,
    pub added_tracks: Vec<String>,
    pub removed_tracks: Vec<String>,
    pub added_clips: Vec<String>,
    pub removed_clips: Vec<String>,
    pub trimmed_clips: Vec<TrimDelta>,
    pub split_points: Vec<SplitPoint>,
    pub duration_delta_sec: f64,
}

pub fn parse_otio(json_str: &str) -> Result<TimelineSnapshot, Box<dyn std::error::Error>> {
    let root: Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("[OTIO Parser Error] Failed to parse: {}", err);
            return Err(Box::new(err));
        }
    };
    let mut snapshot = TimelineSnapshot::default();

    snapshot.name = root
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Master Timeline")
        .to_string();

    let tracks = match root.pointer("/tracks/children").and_then(|c| c.as_array()) {
        Some(t) => t,
        None => return Ok(snapshot),
    };

    fn collect_items(
        items: &[Value],
        track_name: &str,
        track_type: &str,
        snapshot: &mut TimelineSnapshot,
        track_duration: &mut f64,
        occ_counts: &mut HashMap<String, usize>,
    ) {
        for item in items {
            let schema = item
                .get("schema")
                .or_else(|| item.get("OTIO_SCHEMA"))
                .and_then(|s| s.as_str())
                .unwrap_or("");

            match schema {
                "Clip.1" | "Clip.2" => {
                    let clip_name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("Untitled")
                        .to_string();
                    let start_val = item
                        .pointer("/source_range/start_time/value")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let start_rate = item
                        .pointer("/source_range/start_time/rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(24.0)
                        .max(1.0);
                    let dur_val = item
                        .pointer("/source_range/duration/value")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let dur_rate = item
                        .pointer("/source_range/duration/rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(start_rate)
                        .max(1.0);

                    let start_seconds = start_val / start_rate;
                    let duration_seconds = dur_val / dur_rate;
                    let timeline_offset_sec = *track_duration;
                    *track_duration += duration_seconds;

                    let base_key = format!("{}_{}", track_name, clip_name);
                    let occ = *occ_counts.get(&base_key).unwrap_or(&0);
                    occ_counts.insert(base_key.clone(), occ + 1);
                    let key = if occ == 0 {
                        base_key
                    } else {
                        format!("{base_key}#{occ}")
                    };

                    snapshot.clips.insert(
                        key,
                        ClipMeta {
                            name: clip_name,
                            track_name: track_name.to_string(),
                            track_type: track_type.to_string(),
                            start_seconds,
                            duration_seconds,
                            timeline_offset_sec,
                            rate: dur_rate,
                        },
                    );
                }
                "Gap.1" => {
                    let dur_val = item
                        .pointer("/source_range/duration/value")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let dur_rate = item
                        .pointer("/source_range/duration/rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(24.0)
                        .max(1.0);
                    *track_duration += dur_val / dur_rate;
                }
                "Stack.1" | "Track.1" => {
                    if let Some(nested) = item.get("children").and_then(|c| c.as_array()) {
                        collect_items(
                            nested,
                            track_name,
                            track_type,
                            snapshot,
                            track_duration,
                            occ_counts,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    for track in tracks {
        let track_name = track
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let track_type = track
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("Video")
            .to_string();

        if !snapshot.track_names.contains(&track_name) {
            snapshot.track_names.push(track_name.clone());
        }

        let mut track_duration = 0.0_f64;
        let mut occ_counts = HashMap::new();

        if let Some(children) = track.get("children").and_then(|c| c.as_array()) {
            collect_items(
                children,
                &track_name,
                &track_type,
                &mut snapshot,
                &mut track_duration,
                &mut occ_counts,
            );
        }

        if track_duration > snapshot.total_duration {
            snapshot.total_duration = track_duration;
        }
    }

    Ok(snapshot)
}

impl TimelineSnapshot {
    pub fn diff(&self, prev: &TimelineSnapshot) -> TimelineDelta {
        let mut added_clips = Vec::new();
        let mut removed_clips = Vec::new();
        let mut modified_tracks = Vec::new();
        let mut trimmed_clips = Vec::new();
        let mut split_points = Vec::new();

        let added_tracks: Vec<String> = self
            .track_names
            .iter()
            .filter(|t| !prev.track_names.contains(t))
            .cloned()
            .collect();
        let removed_tracks: Vec<String> = prev
            .track_names
            .iter()
            .filter(|t| !self.track_names.contains(t))
            .cloned()
            .collect();

        for t in added_tracks.iter().chain(removed_tracks.iter()) {
            if !modified_tracks.contains(t) {
                modified_tracks.push(t.clone());
            }
        }

        let mut prev_counts: HashMap<(String, String), usize> = HashMap::new();
        for meta in prev.clips.values() {
            *prev_counts
                .entry((meta.track_name.clone(), meta.name.clone()))
                .or_insert(0) += 1;
        }
        let mut curr_counts: HashMap<(String, String), Vec<&ClipMeta>> = HashMap::new();
        for meta in self.clips.values() {
            curr_counts
                .entry((meta.track_name.clone(), meta.name.clone()))
                .or_default()
                .push(meta);
        }

        for ((track_name, clip_name), list) in &curr_counts {
            let p_count = *prev_counts
                .get(&(track_name.clone(), clip_name.clone()))
                .unwrap_or(&0);
            if p_count >= 1 && list.len() > p_count {
                let mut sorted = list.clone();
                sorted.sort_by(|a, b| {
                    a.timeline_offset_sec
                        .partial_cmp(&b.timeline_offset_sec)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let split_sec = sorted
                    .get(1)
                    .map(|c| c.timeline_offset_sec)
                    .unwrap_or(0.0);
                split_points.push(SplitPoint {
                    track_name: track_name.clone(),
                    clip_name: clip_name.clone(),
                    at_seconds: split_sec,
                });
                if !modified_tracks.contains(track_name) {
                    modified_tracks.push(track_name.clone());
                }
            }
        }

        for (key, meta) in &self.clips {
            if let Some(prev_meta) = prev.clips.get(key) {
                let dur_delta = meta.duration_seconds - prev_meta.duration_seconds;
                let start_delta = meta.start_seconds - prev_meta.start_seconds;
                let offset_delta = meta.timeline_offset_sec - prev_meta.timeline_offset_sec;
                let frame_delta = (dur_delta * meta.rate).round() as i64;
                let shift_frames = (offset_delta * meta.rate).round() as i64;
                if frame_delta != 0
                    || (start_delta * meta.rate).abs() >= 1.0
                    || shift_frames.abs() >= 1
                {
                    let effective_frames = if frame_delta != 0 {
                        frame_delta
                    } else if (start_delta * meta.rate).abs() >= 1.0 {
                        (start_delta * meta.rate).round() as i64
                    } else {
                        shift_frames
                    };
                    let already_split = split_points.iter().any(|sp| {
                        sp.track_name == meta.track_name && sp.clip_name == meta.name
                    });
                    if !already_split {
                        trimmed_clips.push(TrimDelta {
                            clip_name: meta.name.clone(),
                            track_name: meta.track_name.clone(),
                            frame_delta: effective_frames,
                        });
                        if !modified_tracks.contains(&meta.track_name) {
                            modified_tracks.push(meta.track_name.clone());
                        }
                    }
                }
            } else {
                let is_split_piece = split_points
                    .iter()
                    .any(|sp| sp.track_name == meta.track_name && sp.clip_name == meta.name);
                if !is_split_piece {
                    added_clips.push(meta.name.clone());
                    if !modified_tracks.contains(&meta.track_name) {
                        modified_tracks.push(meta.track_name.clone());
                    }
                }
            }
        }

        for (key, meta) in &prev.clips {
            if !self.clips.contains_key(key) {
                removed_clips.push(meta.name.clone());
                if !modified_tracks.contains(&meta.track_name) {
                    modified_tracks.push(meta.track_name.clone());
                }
            }
        }

        TimelineDelta {
            modified_tracks,
            added_tracks,
            removed_tracks,
            added_clips,
            removed_clips,
            trimmed_clips,
            split_points,
            duration_delta_sec: self.total_duration - prev.total_duration,
        }
    }
}

#[allow(dead_code)]
fn format_timecode(seconds: f64) -> String {
    let total_secs = seconds.max(0.0).round() as u64;
    let hrs = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    format!("{hrs:02}:{mins:02}:{secs:02}")
}

#[allow(dead_code)]
pub(crate) fn load_snapshot_from_timelines(timelines_dir: &Path) -> Option<TimelineSnapshot> {
    let preferred = timelines_dir.join("cut.otio");
    let otio_path = if preferred.is_file() {
        Some(preferred)
    } else {
        std::fs::read_dir(timelines_dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("otio"))
                        .unwrap_or(false)
            })
    }?;

    let raw = std::fs::read_to_string(otio_path).ok()?;
    parse_otio(&raw).ok()
}

#[allow(dead_code)]
pub(crate) fn format_delta_commit_message(delta: &TimelineDelta) -> String {
    if let Some(split) = delta.split_points.first() {
        return format!(
            "split: {} at {}",
            split.track_name,
            format_timecode(split.at_seconds)
        );
    }

    if let Some(trim) = delta.trimmed_clips.first() {
        let sign = if trim.frame_delta >= 0 { "+" } else { "" };
        return format!(
            "trim: {} ({sign}{} frames on {})",
            trim.clip_name, trim.frame_delta, trim.track_name
        );
    }

    let tracks_str = if delta.modified_tracks.is_empty() {
        "timeline".to_string()
    } else {
        delta.modified_tracks.join(", ")
    };

    if let Some(first_added) = delta.added_clips.first() {
        let extra = if delta.added_clips.len() > 1 {
            format!(" (+{} more)", delta.added_clips.len() - 1)
        } else {
            String::new()
        };
        return format!(
            "add: {first_added}{extra} ({:+.1}s on {tracks_str})",
            delta.duration_delta_sec
        );
    }

    if let Some(first_removed) = delta.removed_clips.first() {
        return format!(
            "remove: {first_removed} ({:+.1}s on {tracks_str})",
            delta.duration_delta_sec
        );
    }

    if let Some(added_trk) = delta.added_tracks.first() {
        return format!("track: added {added_trk} (0 clips)");
    }

    if let Some(removed_trk) = delta.removed_tracks.first() {
        return format!("track: removed {removed_trk}");
    }

    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    format!("micro-save: {ts}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_otio_and_diff_detects_added_and_removed_clips() {
        let otio_v1 = r#"{
            "name": "Episode 1",
            "tracks": {
                "children": [
                    {
                        "name": "V1",
                        "kind": "Video",
                        "children": [
                            {
                                "schema": "Clip.1",
                                "name": "intro_shot.mov",
                                "source_range": {
                                    "start_time": {"value": 0.0, "rate": 24.0},
                                    "duration": {"value": 48.0, "rate": 24.0}
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;

        let otio_v2 = r#"{
            "name": "Episode 1",
            "tracks": {
                "children": [
                    {
                        "name": "V1",
                        "kind": "Video",
                        "children": [
                            {
                                "schema": "Clip.1",
                                "name": "intro_shot.mov",
                                "source_range": {
                                    "start_time": {"value": 0.0, "rate": 24.0},
                                    "duration": {"value": 48.0, "rate": 24.0}
                                }
                            }
                        ]
                    },
                    {
                        "name": "A3",
                        "kind": "Audio",
                        "children": [
                            {
                                "schema": "Clip.1",
                                "name": "dyno_audio_clean.wav",
                                "source_range": {
                                    "start_time": {"value": 0.0, "rate": 24.0},
                                    "duration": {"value": 72.0, "rate": 24.0}
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;

        let snap1 = parse_otio(otio_v1).unwrap();
        let snap2 = parse_otio(otio_v2).unwrap();
        let delta = snap2.diff(&snap1);

        assert_eq!(delta.added_clips, vec!["dyno_audio_clean.wav"]);
        assert_eq!(delta.modified_tracks, vec!["A3"]);
        assert!((delta.duration_delta_sec - 1.0).abs() < 0.01);

        let msg = format_delta_commit_message(&delta);
        assert!(msg.starts_with("add: dyno_audio_clean.wav"));
        assert!(msg.contains("A3"));
    }

    #[test]
    fn parse_otio_detects_trim_and_split() {
        let base = r#"{
            "name": "Cut",
            "tracks": {
                "children": [
                    {
                        "name": "V1",
                        "kind": "Video",
                        "children": [
                            {
                                "OTIO_SCHEMA": "Clip.2",
                                "name": "A002_C004.mov",
                                "source_range": {
                                    "start_time": {"value": 0.0, "rate": 24.0},
                                    "duration": {"value": 120.0, "rate": 24.0}
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;

        let trimmed = r#"{
            "name": "Cut",
            "tracks": {
                "children": [
                    {
                        "name": "V1",
                        "kind": "Video",
                        "children": [
                            {
                                "OTIO_SCHEMA": "Clip.2",
                                "name": "A002_C004.mov",
                                "source_range": {
                                    "start_time": {"value": 0.0, "rate": 24.0},
                                    "duration": {"value": 102.0, "rate": 24.0}
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;

        let s_base = parse_otio(base).unwrap();
        let s_trim = parse_otio(trimmed).unwrap();
        let d_trim = s_trim.diff(&s_base);
        let trim_msg = format_delta_commit_message(&d_trim);
        assert_eq!(trim_msg, "trim: A002_C004.mov (-18 frames on V1)");

        let split = r#"{
            "name": "Cut",
            "tracks": {
                "children": [
                    {
                        "name": "V1",
                        "kind": "Video",
                        "children": [
                            {
                                "OTIO_SCHEMA": "Clip.2",
                                "name": "A002_C004.mov",
                                "source_range": {
                                    "start_time": {"value": 0.0, "rate": 24.0},
                                    "duration": {"value": 48.0, "rate": 24.0}
                                }
                            },
                            {
                                "OTIO_SCHEMA": "Clip.2",
                                "name": "A002_C004.mov",
                                "source_range": {
                                    "start_time": {"value": 48.0, "rate": 24.0},
                                    "duration": {"value": 54.0, "rate": 24.0}
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;

        let s_split = parse_otio(split).unwrap();
        let d_split = s_split.diff(&s_trim);
        let split_msg = format_delta_commit_message(&d_split);
        assert_eq!(split_msg, "split: V1 at 00:00:02");
    }
}
