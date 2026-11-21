#![allow(dead_code)]

use git2::{IndexAddOption, Repository, Signature};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedProject {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) last_opened: String,
}

fn registry_file_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ResolveSidecar").join("projects.json"))
}

pub(crate) fn load_saved_projects(default_repo: &Path) -> Vec<SavedProject> {
    let mut list: Vec<SavedProject> = registry_file_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str::<Vec<SavedProject>>(&raw).ok())
        .unwrap_or_default();

    // Ensure default active repo is in the registry if it exists
    if Repository::open(default_repo).is_ok()
        && !list.iter().any(|p| p.path == default_repo)
    {
        let name = default_repo
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Resolve_Cloud_Storage")
            .to_string();
        list.insert(
            0,
            SavedProject {
                name,
                path: default_repo.to_path_buf(),
                last_opened: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            },
        );
        save_projects_list(&list);
    }

    list
}

pub(crate) fn save_projects_list(projects: &[SavedProject]) {
    let Some(path) = registry_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(projects) {
        let _ = std::fs::write(path, json);
    }
}

pub(crate) fn create_new_project(name: &str, base_dir: &Path) -> Result<SavedProject, String> {
    let clean_name = name.trim();
    if clean_name.is_empty() {
        return Err("Project name cannot be empty".to_string());
    }

    let slug: String = clean_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let project_path = base_dir.join(&slug);

    if project_path.exists() && Repository::open(&project_path).is_ok() {
        return Err(format!("Project already exists at {}", project_path.display()));
    }

    std::fs::create_dir_all(project_path.join("Timelines"))
        .map_err(|e| format!("Failed to create Timelines dir: {e}"))?;
    std::fs::create_dir_all(project_path.join("_media").join("audio"))
        .map_err(|e| format!("Failed to create _media/audio dir: {e}"))?;
    std::fs::create_dir_all(project_path.join("_media").join("video"))
        .map_err(|e| format!("Failed to create _media/video dir: {e}"))?;
    std::fs::create_dir_all(project_path.join("Media").join("Audio"))
        .map_err(|e| format!("Failed to create Media dir: {e}"))?;
    std::fs::create_dir_all(project_path.join("staging_media").join("screenshots"))
        .map_err(|e| format!("Failed to create staging_media dir: {e}"))?;

    std::fs::write(project_path.join("_media").join("audio").join(".gitkeep"), "")
        .map_err(|e| format!("Failed to write _media/audio/.gitkeep: {e}"))?;
    std::fs::write(project_path.join("_media").join("video").join(".gitkeep"), "")
        .map_err(|e| format!("Failed to write _media/video/.gitkeep: {e}"))?;
    std::fs::write(project_path.join("Media").join("Audio").join(".gitkeep"), "")
        .map_err(|e| format!("Failed to write .gitkeep: {e}"))?;
    std::fs::write(
        project_path.join("staging_media").join("screenshots").join(".gitkeep"),
        "",
    )
    .map_err(|e| format!("Failed to write screenshots .gitkeep: {e}"))?;
    std::fs::write(
        project_path.join(".gitignore"),
        "Timelines/Restore_*.otio\nTimelines/*.otio.tmp\nTimelines/*.signal\n",
    )
    .map_err(|e| format!("Failed to write .gitignore: {e}"))?;

    let default_cut = Path::new(r"C:\Users\zanem\Resolve_Cloud_Storage\Timelines\cut.otio");
    let target_cut = project_path.join("Timelines").join("cut.otio");
    if default_cut.is_file() && !target_cut.exists() {
        let _ = std::fs::copy(default_cut, &target_cut);
    }

    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = Repository::init_opts(&project_path, &opts)
        .map_err(|e| format!("Git init error: {e}"))?;

    let mut index = repo.index().map_err(|e| format!("Git index error: {e}"))?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("Git stage error: {e}"))?;
    index
        .write()
        .map_err(|e| format!("Git index write error: {e}"))?;
    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Git tree error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Git find tree error: {e}"))?;

    let sig = Signature::now("Zane", "zane@editor.local")
        .map_err(|e| format!("Git signature error: {e}"))?;
    repo.commit(
        Some("refs/heads/main"),
        &sig,
        &sig,
        &format!("chore: initialize {clean_name} (Node 0)"),
        &tree,
        &[],
    )
    .map_err(|e| format!("Git root commit error: {e}"))?;

    Ok(SavedProject {
        name: clean_name.to_string(),
        path: project_path,
        last_opened: chrono::Utc::now().format("%Y-%m-%d").to_string(),
    })
}
