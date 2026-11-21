use crate::configure_hidden_command;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TrackSummary {
    pub name: String,
    pub track_type: String, // "video" or "audio"
    pub clip_names: Vec<String>,
    pub total_clips: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TimelineSnapshot {
    pub timeline_name: String,
    pub duration_seconds: f64,
    pub tracks: Vec<TrackSummary>,
}

#[derive(Debug)]
pub struct SemanticDiff {
    pub modified_tracks: Vec<String>,
    pub added_clips: Vec<String>,
    pub removed_clips: Vec<String>,
    pub duration_delta_sec: f64,
}

impl TimelineSnapshot {
    pub fn diff(&self, previous: &TimelineSnapshot) -> SemanticDiff {
        let mut modified_tracks = Vec::new();
        let mut added_clips = Vec::new();
        let mut removed_clips = Vec::new();

        let prev_clips: HashSet<String> = previous
            .tracks
            .iter()
            .flat_map(|t| t.clip_names.clone())
            .collect();

        let curr_clips: HashSet<String> = self
            .tracks
            .iter()
            .flat_map(|t| t.clip_names.clone())
            .collect();

        for clip in curr_clips.difference(&prev_clips) {
            added_clips.push(clip.clone());
        }
        for clip in prev_clips.difference(&curr_clips) {
            removed_clips.push(clip.clone());
        }

        for curr_track in &self.tracks {
            if let Some(prev_track) = previous.tracks.iter().find(|t| t.name == curr_track.name) {
                if curr_track.clip_names != prev_track.clip_names {
                    modified_tracks.push(curr_track.name.clone());
                }
            } else {
                modified_tracks.push(curr_track.name.clone());
            }
        }

        SemanticDiff {
            modified_tracks,
            added_clips,
            removed_clips,
            duration_delta_sec: self.duration_seconds - previous.duration_seconds,
        }
    }
}

#[derive(Deserialize)]
struct RawOtio {
    name: Option<String>,
    tracks: Option<RawOtioStack>,
}

#[derive(Deserialize)]
struct RawOtioStack {
    children: Option<Vec<RawOtioTrack>>,
}

#[derive(Deserialize)]
struct RawOtioTrack {
    name: Option<String>,
    kind: Option<String>,
    children: Option<Vec<RawOtioItem>>,
}

#[derive(Deserialize)]
struct RawOtioItem {
    name: Option<String>,
    source_range: Option<RawOtioRange>,
}

#[derive(Deserialize)]
struct RawOtioRange {
    duration: Option<RawOtioTime>,
}

#[derive(Deserialize)]
struct RawOtioTime {
    value: Option<f64>,
    rate: Option<f64>,
}

pub(crate) fn snapshot_from_timelines_dir(timelines_dir: &Path) -> Option<TimelineSnapshot> {
    let otio_path = find_otio_file(timelines_dir)?;
    let raw = std::fs::read_to_string(&otio_path).ok()?;
    let doc = serde_json::from_str::<RawOtio>(&raw).ok()?;

    let timeline_name = doc
        .name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Master Timeline".to_string());

    let mut tracks = Vec::new();
    let mut max_duration_sec = 0.0_f64;

    if let Some(stack) = doc.tracks {
        for (idx, trk) in stack.children.unwrap_or_default().into_iter().enumerate() {
            let kind_raw = trk.kind.unwrap_or_else(|| "Video".to_string());
            let track_type = if kind_raw.to_ascii_lowercase().contains("audio") {
                "audio".to_string()
            } else {
                "video".to_string()
            };
            let prefix = if track_type == "audio" { "A" } else { "V" };
            let name = trk
                .name
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("{prefix}{}", idx + 1));

            let mut clip_names = Vec::new();
            let mut track_dur_sec = 0.0_f64;
            for item in trk.children.unwrap_or_default() {
                if let Some(cname) = item.name.filter(|s| !s.trim().is_empty()) {
                    clip_names.push(cname);
                }
                if let Some(dur) = item.source_range.and_then(|r| r.duration) {
                    let val = dur.value.unwrap_or(0.0);
                    let rate = dur.rate.unwrap_or(24.0).max(1.0);
                    track_dur_sec += val / rate;
                }
            }
            if track_dur_sec > max_duration_sec {
                max_duration_sec = track_dur_sec;
            }

            let total_clips = clip_names.len();
            tracks.push(TrackSummary {
                name,
                track_type,
                clip_names,
                total_clips,
            });
        }
    }

    Some(TimelineSnapshot {
        timeline_name,
        duration_seconds: max_duration_sec,
        tracks,
    })
}

fn find_otio_file(timelines_dir: &Path) -> Option<std::path::PathBuf> {
    let preferred = timelines_dir.join("cut.otio");
    if preferred.is_file() {
        return Some(preferred);
    }
    let entries = std::fs::read_dir(timelines_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file()
            && p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("otio"))
                .unwrap_or(false)
        {
            return Some(p);
        }
    }
    None
}

#[derive(Deserialize)]
struct LlmChatResp {
    choices: Option<Vec<LlmChoice>>,
}

#[derive(Deserialize)]
struct LlmChoice {
    message: Option<LlmMsg>,
}

#[derive(Deserialize)]
struct LlmMsg {
    content: Option<String>,
}

pub(crate) fn generate_semantic_commit_message(
    diff: &SemanticDiff,
    ai_endpoint: Option<&str>,
    ai_model: Option<&str>,
) -> String {
    let tracks_str = if diff.modified_tracks.is_empty() {
        "timeline".to_string()
    } else {
        diff.modified_tracks.join(", ")
    };
    let added_str = if diff.added_clips.is_empty() {
        "none".to_string()
    } else {
        diff.added_clips.join(", ")
    };
    let removed_str = if diff.removed_clips.is_empty() {
        "none".to_string()
    } else {
        diff.removed_clips.join(", ")
    };

    let compact_diff = format!(
        "Tracks: {tracks_str} | Duration delta: {:+.1}s\nAdded: {added_str}\nRemoved: {removed_str}",
        diff.duration_delta_sec
    );

    if !diff.added_clips.is_empty() || !diff.removed_clips.is_empty() || !diff.modified_tracks.is_empty() {
        let url = ai_endpoint
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("http://127.0.0.1:11434/v1/chat/completions");
        let model_name = ai_model
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("qwen2.5-coder:7b");

        let body = serde_json::json!({
            "model": model_name,
            "temperature": 0.1,
            "messages": [
                {
                    "role": "system",
                    "content": "Return ONLY a single-line conventional commit message (e.g. feat(audio): added clip.wav on A3). No quotes or markdown."
                },
                {
                    "role": "user",
                    "content": compact_diff
                }
            ]
        });

        if let Ok(payload) = serde_json::to_string(&body) {
            let mut cmd = Command::new("curl.exe");
            cmd.args([
                "-sS",
                "--max-time",
                "2",
                "-H",
                "Content-Type: application/json",
                "-d",
                &payload,
                url,
            ]);
            configure_hidden_command(&mut cmd);

            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    let raw = String::from_utf8_lossy(&output.stdout);
                    if let Ok(resp) = serde_json::from_str::<LlmChatResp>(&raw) {
                        if let Some(line) = resp
                            .choices
                            .and_then(|c| c.into_iter().next())
                            .and_then(|c| c.message)
                            .and_then(|m| m.content)
                        {
                            let clean = line.lines().next().unwrap_or("").trim();
                            if !clean.is_empty() && clean.len() <= 100 {
                                return clean.to_string();
                            }
                        }
                    }
                }
            }
        }
    }

    // Deterministic semantic fallback
    let scope = if tracks_str.to_ascii_lowercase().contains('a') {
        "audio"
    } else if tracks_str.to_ascii_lowercase().contains('v') {
        "video"
    } else {
        "timeline"
    };

    if let Some(first_added) = diff.added_clips.first() {
        let extra = if diff.added_clips.len() > 1 {
            format!(" (+{} more)", diff.added_clips.len() - 1)
        } else {
            String::new()
        };
        format!("feat({scope}): added {first_added}{extra} on {tracks_str}")
    } else if let Some(first_removed) = diff.removed_clips.first() {
        format!("refactor({scope}): removed {first_removed} on {tracks_str}")
    } else if !diff.modified_tracks.is_empty() {
        format!("chore({scope}): updated {tracks_str} ({:+.1}s)", diff.duration_delta_sec)
    } else {
        let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        format!("micro-save: {ts}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_diff_detects_added_audio_and_formats_message() {
        let prev = TimelineSnapshot {
            timeline_name: "Main".to_string(),
            duration_seconds: 10.0,
            tracks: vec![TrackSummary {
                name: "A3".to_string(),
                track_type: "audio".to_string(),
                clip_names: vec![],
                total_clips: 0,
            }],
        };

        let curr = TimelineSnapshot {
            timeline_name: "Main".to_string(),
            duration_seconds: 22.5,
            tracks: vec![TrackSummary {
                name: "A3".to_string(),
                track_type: "audio".to_string(),
                clip_names: vec!["dyno_audio_clean.wav".to_string()],
                total_clips: 1,
            }],
        };

        let diff = curr.diff(&prev);
        assert_eq!(diff.modified_tracks, vec!["A3"]);
        assert_eq!(diff.added_clips, vec!["dyno_audio_clean.wav"]);
        assert!((diff.duration_delta_sec - 12.5).abs() < 0.01);

        let msg = generate_semantic_commit_message(&diff, Some("http://127.0.0.1:1"), None);
        assert_eq!(msg, "feat(audio): added dyno_audio_clean.wav on A3");
    }
}
