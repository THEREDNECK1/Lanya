use git2::{IndexAddOption, Repository, Signature};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) enum TicketCategory {
    Nitpick,
    FeatureIdea,
    BugReport,
}

impl TicketCategory {
    pub(crate) fn label(self) -> &'static str {
        match self {
            TicketCategory::Nitpick => "Nitpick",
            TicketCategory::FeatureIdea => "Feature Idea",
            TicketCategory::BugReport => "Bug Report",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) enum TicketStatus {
    Open,
    Resolved,
}

impl TicketStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            TicketStatus::Open => "Open",
            TicketStatus::Resolved => "Resolved",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Ticket {
    pub(crate) id: String,
    pub(crate) category: TicketCategory,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) author: String,
    pub(crate) created_at: String,
    pub(crate) status: TicketStatus,
    pub(crate) commit_hash: String,
    pub(crate) timeline_name: String,
}

pub(crate) fn tickets_file_path(repo_path: &Path) -> std::path::PathBuf {
    repo_path.join("tickets.json")
}

pub(crate) fn load_tickets(repo_path: &Path) -> Vec<Ticket> {
    let path = tickets_file_path(repo_path);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub(crate) fn save_and_commit_tickets(
    repo_path: &Path,
    tickets: &[Ticket],
    commit_msg: &str,
) -> Result<(), String> {
    let path = tickets_file_path(repo_path);
    let tmp_path = repo_path.join("tickets.json.tmp");
    let json = serde_json::to_string_pretty(tickets)
        .map_err(|e| format!("Failed to serialize tickets: {e}"))?;

    std::fs::write(&tmp_path, json.as_bytes())
        .map_err(|e| format!("Failed to write temp tickets.json: {e}"))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to save tickets.json: {e}"))?;

    let Ok(repo) = Repository::open(repo_path) else {
        return Ok(());
    };
    let mut index = repo
        .index()
        .map_err(|e| format!("Git index error: {e}"))?;
    index
        .add_all(["tickets.json"], IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("Git add error: {e}"))?;
    index.write().map_err(|e| format!("Git index write error: {e}"))?;

    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Git write_tree error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Git find_tree error: {e}"))?;

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Sidecar", "sidecar@local"))
        .map_err(|e| format!("Git signature error: {e}"))?;

    let parent_commit = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|oid| repo.find_commit(oid).ok());

    if let Some(parent) = parent_commit.as_ref() {
        repo.commit(Some("HEAD"), &sig, &sig, commit_msg, &tree, &[parent])
            .map_err(|e| format!("Git commit error: {e}"))?;
    } else {
        repo.commit(Some("HEAD"), &sig, &sig, commit_msg, &tree, &[])
            .map_err(|e| format!("Git commit error: {e}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_loads_and_commits_tickets_json() {
        let dir = std::env::temp_dir().join("resolve_sidecar_tickets_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "TestEditor").unwrap();
        cfg.set_str("user.email", "test@local").unwrap();

        let tickets = vec![Ticket {
            id: "t-1".to_string(),
            category: TicketCategory::Nitpick,
            title: "Tighten cut at 00:01:12".to_string(),
            description: "Trim 4 frames on V1".to_string(),
            author: "TestEditor".to_string(),
            created_at: "2026-11-21 19:00".to_string(),
            status: TicketStatus::Open,
            commit_hash: "abc1234".to_string(),
            timeline_name: "Cut 1".to_string(),
        }];

        save_and_commit_tickets(&dir, &tickets, "ticket: Tighten cut at 00:01:12").unwrap();
        let loaded = load_tickets(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "Tighten cut at 00:01:12");
        assert_eq!(loaded[0].status, TicketStatus::Open);

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.message().unwrap(), "ticket: Tighten cut at 00:01:12");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
