use crate::otio_diff::{self, OtioDeltaReport};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const MODEL_URL: &str = "https://huggingface.co/HuggingFaceTB/SmolLM2-360M-Instruct-GGUF/resolve/main/smollm2-360m-instruct.Q4_K_M.gguf";
const MODEL_FILENAME: &str = "smollm2-360m-instruct-q4_k_m.gguf";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiModelState {
    Downloading,
    Ready(PathBuf),
    Fallback,
}

pub fn model_file_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|base| base.join("ResolveSidecar").join("models").join(MODEL_FILENAME))
}

pub fn start_background_model_setup() -> Arc<Mutex<AiModelState>> {
    let Some(target_path) = model_file_path() else {
        return Arc::new(Mutex::new(AiModelState::Fallback));
    };

    if is_valid_model(&target_path) {
        return Arc::new(Mutex::new(AiModelState::Ready(target_path)));
    }

    let state = Arc::new(Mutex::new(AiModelState::Downloading));
    let state_bg = Arc::clone(&state);

    std::thread::spawn(move || {
        let result = download_model_to(&target_path);
        if let Ok(mut guard) = state_bg.lock() {
            *guard = match result {
                Ok(()) if is_valid_model(&target_path) => AiModelState::Ready(target_path),
                _ => AiModelState::Fallback,
            };
        }
    });

    state
}

fn is_valid_model(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && m.len() > 1_000_000)
        .unwrap_or(false)
}

fn download_model_to(target_path: &Path) -> Result<(), String> {
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create model dir: {e}"))?;
    }

    let part_path = target_path.with_extension("gguf.part");
    let _ = std::fs::remove_file(&part_path);

    let mut cmd = Command::new("curl.exe");
    cmd.arg("-L")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("-o")
        .arg(&part_path)
        .arg(MODEL_URL);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let status = cmd
        .status()
        .map_err(|e| format!("Failed to launch curl.exe: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&part_path);
        return Err(format!("Model download exited with {status}"));
    }

    std::fs::rename(&part_path, target_path)
        .map_err(|e| format!("Failed to finalize downloaded model: {e}"))?;
    Ok(())
}

fn find_llama_cli() -> Option<PathBuf> {
    if let Some(local) = dirs::data_local_dir() {
        let local_bin = local
            .join("ResolveSidecar")
            .join("bin")
            .join("llama-cli.exe");
        if local_bin.is_file() {
            return Some(local_bin);
        }
    }
    None
}

fn try_local_gguf_inference(report: &OtioDeltaReport) -> Option<String> {
    let model_path = model_file_path().filter(|p| is_valid_model(p))?;
    let llama_cli = find_llama_cli()?;
    let json_payload = serde_json::to_string(report).ok()?;
    let prompt = format!(
        "Convert this video edit JSON delta into ONE short commit label (under 65 chars) prefixed with [Audio], [Edit], or [Cut & Mix]: {json_payload}"
    );

    let mut cmd = Command::new(llama_cli);
    cmd.arg("-m")
        .arg(model_path)
        .arg("-p")
        .arg(prompt)
        .arg("-n")
        .arg("32")
        .arg("--temp")
        .arg("0.1")
        .arg("--no-display-prompt");

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && l.starts_with('['))?;
    Some(line.chars().take(72).collect())
}

#[allow(dead_code)]
pub fn summarize_delta_report(report: &OtioDeltaReport) -> String {
    if let Some(ai_label) = try_local_gguf_inference(report) {
        return ai_label;
    }
    otio_diff::generate_semantic_commit_message(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otio_diff::OtioDeltaDetail;

    #[test]
    fn formats_descriptive_semantic_labels_for_audio_and_pacing() {
        let audio_report = OtioDeltaReport {
            summary: "1 audio clip added".into(),
            tracks_affected: vec!["Audio 3".into()],
            details: vec![OtioDeltaDetail {
                action: "added".into(),
                track: "Audio 3".into(),
                file: Some("test_of_time.mp3".into()),
                duration_sec: Some(142),
                delta_frames: None,
                at_timecode: None,
            }],
        };
        assert_eq!(
            summarize_delta_report(&audio_report),
            "[Audio] Added test_of_time.mp3 to A3"
        );
    }
}
