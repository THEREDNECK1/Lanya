use crate::otio_engine::{self, TimelineDelta, TimelineSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OtioDeltaDetail {
    pub action: String,
    pub track: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_sec: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_frames: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_timecode: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OtioDeltaReport {
    pub summary: String,
    pub tracks_affected: Vec<String>,
    pub details: Vec<OtioDeltaDetail>,
}

fn format_tc(seconds: f64) -> String {
    let secs = seconds.max(0.0).round() as u64;
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    format!("{hh:02}:{mm:02}:{ss:02}")
}

pub fn build_delta_report(
    prev_snap: &TimelineSnapshot,
    curr_snap: &TimelineSnapshot,
    delta: &TimelineDelta,
) -> OtioDeltaReport {
    let mut details = Vec::new();
    let mut tracks = BTreeSet::new();

    for track in &delta.added_tracks {
        tracks.insert(track.clone());
        details.push(OtioDeltaDetail {
            action: "track_added".to_string(),
            track: track.clone(),
            file: None,
            duration_sec: None,
            delta_frames: None,
            at_timecode: None,
        });
    }

    for track in &delta.removed_tracks {
        tracks.insert(track.clone());
        details.push(OtioDeltaDetail {
            action: "track_removed".to_string(),
            track: track.clone(),
            file: None,
            duration_sec: None,
            delta_frames: None,
            at_timecode: None,
        });
    }

    for trim in &delta.trimmed_clips {
        tracks.insert(trim.track_name.clone());
        details.push(OtioDeltaDetail {
            action: "trimmed".to_string(),
            track: trim.track_name.clone(),
            file: Some(trim.clip_name.clone()),
            duration_sec: None,
            delta_frames: Some(trim.frame_delta),
            at_timecode: None,
        });
    }

    for split in &delta.split_points {
        tracks.insert(split.track_name.clone());
        details.push(OtioDeltaDetail {
            action: "split".to_string(),
            track: split.track_name.clone(),
            file: Some(split.clip_name.clone()),
            duration_sec: None,
            delta_frames: None,
            at_timecode: Some(format_tc(split.at_seconds)),
        });
    }

    // Collect added clips with track name and duration
    for clip in curr_snap.clips.values() {
        if delta.added_clips.contains(&clip.name) {
            tracks.insert(clip.track_name.clone());
            details.push(OtioDeltaDetail {
                action: "added".to_string(),
                track: clip.track_name.clone(),
                file: Some(clip.name.clone()),
                duration_sec: Some(clip.duration_seconds.round() as i64),
                delta_frames: None,
                at_timecode: None,
            });
        }
    }

    // Collect removed clips with track name and duration
    for clip in prev_snap.clips.values() {
        if delta.removed_clips.contains(&clip.name) {
            tracks.insert(clip.track_name.clone());
            details.push(OtioDeltaDetail {
                action: "removed".to_string(),
                track: clip.track_name.clone(),
                file: Some(clip.name.clone()),
                duration_sec: Some(clip.duration_seconds.round() as i64),
                delta_frames: None,
                at_timecode: None,
            });
        }
    }

    let tracks_affected: Vec<String> = tracks.into_iter().collect();
    let summary = build_summary_line(&details, &tracks_affected);

    OtioDeltaReport {
        summary,
        tracks_affected,
        details,
    }
}

fn build_summary_line(details: &[OtioDeltaDetail], tracks_affected: &[String]) -> String {
    if details.is_empty() {
        return "No timeline changes".to_string();
    }
    if details.len() == 1 {
        let d = &details[0];
        let is_audio = d.track.to_lowercase().contains("audio") || d.track.starts_with('A');
        let kind = if is_audio { "audio clip" } else { "video clip" };
        return match d.action.as_str() {
            "added" => format!("1 {kind} added"),
            "removed" => format!("1 {kind} removed"),
            "trimmed" => format!("1 {kind} trimmed"),
            "split" => format!("1 split on {}", d.track),
            "track_added" => format!("Track {} added", d.track),
            "track_removed" => format!("Track {} removed", d.track),
            _ => format!("1 edit on {}", d.track),
        };
    }
    format!(
        "{} changes across {} track(s)",
        details.len(),
        tracks_affected.len().max(1)
    )
}

pub fn extract_otio_delta(prev_json: &str, curr_json: &str) -> Result<OtioDeltaReport, String> {
    let prev_snap = otio_engine::parse_otio(prev_json)
        .map_err(|e| format!("Failed to parse prev OTIO: {e}"))?;
    let curr_snap = otio_engine::parse_otio(curr_json)
        .map_err(|e| format!("Failed to parse curr OTIO: {e}"))?;
    let delta = curr_snap.diff(&prev_snap);
    Ok(build_delta_report(&prev_snap, &curr_snap, &delta))
}

#[cfg(test)]
pub fn extract_otio_delta_json(prev_json: &str, curr_json: &str) -> Result<String, String> {
    let report = extract_otio_delta(prev_json, curr_json)?;
    serde_json::to_string_pretty(&report).map_err(|e| format!("Serialize delta JSON error: {e}"))
}

fn short_track(track: &str) -> String {
    let lower = track.to_lowercase();
    if let Some(num) = lower.strip_prefix("audio ") {
        return format!("A{}", num.trim());
    }
    if let Some(num) = lower.strip_prefix("video ") {
        return format!("V{}", num.trim());
    }
    track.to_string()
}

pub fn generate_semantic_commit_message(report: &OtioDeltaReport) -> String {
    if report.details.is_empty() {
        return "[Edit] Adjusted timeline cut".to_string();
    }

    let mut has_audio = false;
    let mut has_video = false;
    let mut short_tracks = Vec::new();
    for trk in &report.tracks_affected {
        let lower = trk.to_lowercase();
        if lower.contains("audio") || trk.starts_with('A') {
            has_audio = true;
        } else {
            has_video = true;
        }
        let st = short_track(trk);
        if !short_tracks.contains(&st) {
            short_tracks.push(st);
        }
    }

    let prefix = match (has_video, has_audio) {
        (true, true) => "[Cut & Mix]",
        (false, true) => "[Audio]",
        _ => "[Edit]",
    };

    if report.details.len() == 1 {
        let d = &report.details[0];
        let st = short_track(&d.track);
        let clip = d.file.as_deref().unwrap_or("clip");
        return match d.action.as_str() {
            "added" => format!("{prefix} Added {clip} to {st}"),
            "removed" => format!("{prefix} Removed {clip} from {st}"),
            "trimmed" => format!("{prefix} Trimmed {clip} on {st}"),
            "split" => format!("{prefix} Split {clip} on {st}"),
            "track_added" => format!("{prefix} Added {} track", d.track),
            "track_removed" => format!("{prefix} Removed {} track", d.track),
            _ => format!("{prefix} Edited {st}"),
        };
    }

    let tracks_label = short_tracks.join(", ");
    let first_action = report.details[0].action.as_str();
    let uniform = report.details.iter().all(|d| d.action == first_action);
    let verb = if uniform {
        match first_action {
            "split" => "Split",
            "trimmed" => "Trimmed",
            "added" => "Added",
            "removed" => "Removed",
            _ => "Modified",
        }
    } else {
        "Modified"
    };

    format!(
        "{prefix} {verb} {} clips on {tracks_label}",
        report.details.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_compact_json_delta_for_added_audio_clip() {
        let prev = r#"{
            "OTIO_SCHEMA": "Timeline.1",
            "name": "Master",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [
                    { "OTIO_SCHEMA": "Track.1", "name": "Audio 3", "kind": "Audio", "children": [] }
                ]
            }
        }"#;

        let curr = r#"{
            "OTIO_SCHEMA": "Timeline.1",
            "name": "Master",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [
                    {
                        "OTIO_SCHEMA": "Track.1",
                        "name": "Audio 3",
                        "kind": "Audio",
                        "children": [
                            {
                                "OTIO_SCHEMA": "Clip.2",
                                "name": "test_of_time.mp3",
                                "source_range": {
                                    "start_time": { "value": 0.0, "rate": 24.0 },
                                    "duration": { "value": 3408.0, "rate": 24.0 }
                                }
                            }
                        ]
                    }
                ]
            }
        }"#;

        let json_str = extract_otio_delta_json(prev, curr).unwrap();
        let parsed: OtioDeltaReport = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.summary, "1 audio clip added");
        assert_eq!(parsed.tracks_affected, vec!["Audio 3".to_string()]);
        assert_eq!(parsed.details.len(), 1);
        assert_eq!(parsed.details[0].action, "added");
        assert_eq!(parsed.details[0].track, "Audio 3");
        assert_eq!(parsed.details[0].file.as_deref(), Some("test_of_time.mp3"));
        assert_eq!(parsed.details[0].duration_sec, Some(142));
    }

    #[test]
    fn generates_semantic_commit_messages_for_audio_edit_and_cut_mix() {
        let audio = OtioDeltaReport {
            summary: "1 audio clip added".into(),
            tracks_affected: vec!["Audio 1".into()],
            details: vec![OtioDeltaDetail {
                action: "added".into(),
                track: "Audio 1".into(),
                file: Some("test_of_time.mp3".into()),
                duration_sec: Some(142),
                delta_frames: None,
                at_timecode: None,
            }],
        };
        assert_eq!(
            generate_semantic_commit_message(&audio),
            "[Audio] Added test_of_time.mp3 to A1"
        );

        let cut_mix = OtioDeltaReport {
            summary: "2 changes across 2 track(s)".into(),
            tracks_affected: vec!["Video 1".into(), "Audio 1".into()],
            details: vec![
                OtioDeltaDetail {
                    action: "trimmed".into(),
                    track: "Video 1".into(),
                    file: Some("A001.mp4".into()),
                    duration_sec: None,
                    delta_frames: Some(-12),
                    at_timecode: None,
                },
                OtioDeltaDetail {
                    action: "added".into(),
                    track: "Audio 1".into(),
                    file: Some("sfx.wav".into()),
                    duration_sec: Some(3),
                    delta_frames: None,
                    at_timecode: None,
                },
            ],
        };
        assert_eq!(
            generate_semantic_commit_message(&cut_mix),
            "[Cut & Mix] Modified 2 clips on V1, A1"
        );
    }
}
