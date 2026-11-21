use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn guest_projects_dir() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(
        PathBuf::from(appdata)
            .join(r"Blackmagic Design\DaVinci Resolve\Support\Resolve Project Library\Resolve Projects\Users\guest\Projects"),
    )
}

pub fn get_resolve_project_file(project_name: &str) -> Option<PathBuf> {
    let base = guest_projects_dir()?;
    let proj_dir = base.join(project_name);
    let xml = proj_dir.join("Project.xml");
    if xml.exists() {
        return Some(xml);
    }
    let db = proj_dir.join("Project.db");
    if db.exists() {
        return Some(db);
    }
    None
}

pub fn find_most_recent_project_file(configured_project: Option<&str>) -> Option<PathBuf> {
    if let Some(name) = configured_project.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(path) = get_resolve_project_file(name) {
            return Some(path);
        }
    }

    let base = guest_projects_dir()?;
    if !base.is_dir() {
        return None;
    }

    let mut best_dir: Option<(SystemTime, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let modified = dir_latest_modified(&path);
            if best_dir
                .as_ref()
                .map(|(t, _)| modified > *t)
                .unwrap_or(true)
            {
                best_dir = Some((modified, path));
            }
        }
    }

    let (_, proj_dir) = best_dir?;
    let xml = proj_dir.join("Project.xml");
    if xml.is_file() {
        return Some(xml);
    }
    let db = proj_dir.join("Project.db");
    if db.is_file() {
        return Some(db);
    }
    None
}

fn dir_latest_modified(dir: &Path) -> SystemTime {
    let mut latest = dir
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for candidate in ["Project.xml", "Project.db"] {
        let p = dir.join(candidate);
        if let Ok(meta) = p.metadata() {
            if let Ok(m) = meta.modified() {
                if m > latest {
                    latest = m;
                }
            }
        }
    }
    latest
}
