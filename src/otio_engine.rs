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
}

#[derive(Default, Clone, Debug)]
pub struct TimelineSnapshot {
    pub name: String,
    pub clips: HashMap<String, ClipMeta>,
    pub total_duration: f64,
}

#[derive(Debug)]
pub struct TimelineDelta {
    pub modified_tracks: Vec<String>,
    pub added_clips: Vec<String>,
    pub removed_clips: Vec<String>,
    pub duration_delta_sec: f64,
}

pub fn parse_otio(json_str: &str) -> Result<TimelineSnapshot, Box<dyn std::error::Error>> {
    let root: Value = serde_json::from_str(json_str)?;
    let mut snapshot = TimelineSnapshot::default();

    snapshot.name = root
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Master")
        .to_string();

    let tracks = match root.pointer("/tracks/children").and_then(|c| c.as_array()) {
        Some(t) => t,
        None => return Ok(snapshot),
    };

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

        let mut track_duration = 0.0_f64;

        if let Some(children) = track.get("children").and_then(|c| c.as_array()) {
            for item in children {
                let schema = item
                    .get("schema")
                    .or_else(|| item.get("OTIO_SCHEMA"))
                    .and_then(|s| s.as_str());
                if schema == Some("Clip.1") {
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
                        .unwrap_or(1.0)
                        .max(1.0);
                    let dur_val = item
                        .pointer("/source_range/duration/value")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let dur_rate = item
                        .pointer("/source_range/duration/rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(1.0)
                        .max(1.0);

                    let start_seconds = start_val / start_rate;
                    let duration_seconds = dur_val / dur_rate;
                    track_duration += duration_seconds;

                    snapshot.clips.insert(
                        format!("{}_{}", track_name, clip_name),
                        ClipMeta {
                            name: clip_name,
                            track_name: track_name.clone(),
                            track_type: track_type.clone(),
                            start_seconds,
                            duration_seconds,
                        },
                    );
                }
            }
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

        for (key, meta) in &self.clips {
            if !prev.clips.contains_key(key) {
                added_clips.push(meta.name.clone());
                if !modified_tracks.contains(&meta.track_name) {
                    modified_tracks.push(meta.track_name.clone());
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
            added_clips,
            removed_clips,
            duration_delta_sec: self.total_duration - prev.total_duration,
        }
    }
}

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

pub(crate) fn format_delta_commit_message(delta: &TimelineDelta) -> String {
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
        format!(
            "feat({tracks_str}): added {first_added}{extra} ({:+.1}s)",
            delta.duration_delta_sec
        )
    } else if let Some(first_removed) = delta.removed_clips.first() {
        format!(
            "refactor({tracks_str}): removed {first_removed} ({:+.1}s)",
            delta.duration_delta_sec
        )
    } else {
        let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        format!("micro-save: {ts}")
    }
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
        assert!(msg.contains("added dyno_audio_clean.wav"));
        assert!(msg.contains("A3"));
    }
}
