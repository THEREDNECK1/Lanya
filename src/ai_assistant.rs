use crate::{configure_hidden_command, ScopeChecklist};
use serde::Deserialize;
use std::process::Command;

pub(crate) struct AiDraftSuggestion {
    pub(crate) title: String,
    pub(crate) notes: String,
    pub(crate) used_llm: bool,
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Option<Vec<OpenAiChoice>>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiMessage>,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ParsedAiJson {
    title: Option<String>,
    description: Option<String>,
}

pub(crate) fn generate_advisory_pr_draft(
    endpoint: Option<&str>,
    model: Option<&str>,
    scope: &ScopeChecklist,
    timeline_name: &str,
    existing_notes: &str,
) -> AiDraftSuggestion {
    let scopes = scope.selected_labels();
    let scope_list = if scopes.is_empty() {
        "Timeline Edit".to_string()
    } else {
        scopes.join(", ")
    };

    let url = endpoint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("http://127.0.0.1:11434/v1/chat/completions");
    let model_name = model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("qwen2.5-coder:7b");

    let prompt = format!(
        "You are an assistant for a DaVinci Resolve collaborative video editing pipeline. \
         Respond ONLY with valid JSON containing keys \"title\" (short 3-5 word pass title for a git branch slug) \
         and \"description\" (concise markdown bullet list of changes). \
         Timeline: {timeline_name}. Checked scopes: {scope_list}. Editor rough notes: {existing_notes}"
    );

    let body = serde_json::json!({
        "model": model_name,
        "temperature": 0.2,
        "messages": [
            {"role": "system", "content": "Output strict JSON with keys title and description."},
            {"role": "user", "content": prompt}
        ]
    });

    if let Ok(payload) = serde_json::to_string(&body) {
        let mut cmd = Command::new("curl.exe");
        cmd.args([
            "-sS",
            "--max-time",
            "4",
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
                if let Ok(resp) = serde_json::from_str::<OpenAiChatResponse>(&raw) {
                    if let Some(content) = resp
                        .choices
                        .and_then(|c| c.into_iter().next())
                        .and_then(|c| c.message)
                        .and_then(|m| m.content)
                    {
                        let cleaned = content
                            .trim()
                            .trim_start_matches("```json")
                            .trim_start_matches("```")
                            .trim_end_matches("```")
                            .trim();
                        if let Ok(parsed) = serde_json::from_str::<ParsedAiJson>(cleaned) {
                            if let (Some(title), Some(desc)) = (parsed.title, parsed.description) {
                                if !title.trim().is_empty() && !desc.trim().is_empty() {
                                    return AiDraftSuggestion {
                                        title: title.trim().to_string(),
                                        notes: desc.trim().to_string(),
                                        used_llm: true,
                                    };
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Deterministic offline fallback when local LLM endpoint is not running
    let title = if scope.audio_music && !scope.video_cuts {
        "Audio Music Mix Pass"
    } else if scope.color_grade && !scope.video_cuts {
        "Color Grade Pass"
    } else if scope.titles_graphics {
        "Titles and Graphics Pass"
    } else if scope.sfx_foley {
        "SFX Foley Polish Pass"
    } else {
        "Timeline Assembly Cut Pass"
    }
    .to_string();

    let mut bullets = Vec::new();
    for s in &scopes {
        bullets.push(format!("- Updated **{s}**"));
    }
    if !existing_notes.trim().is_empty() {
        bullets.push(format!("- Editor notes: {}", existing_notes.trim()));
    }

    AiDraftSuggestion {
        title,
        notes: format!("### Edit Pass Summary ({timeline_name})\n{}", bullets.join("\n")),
        used_llm: false,
    }
}
