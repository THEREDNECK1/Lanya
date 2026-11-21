use chrono::Utc;
use git2::{BranchType, Repository, Signature};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ReviewComment {
    pub(crate) id: String,
    pub(crate) branch: String,
    pub(crate) author: String,
    pub(crate) timestamp: String,
    pub(crate) status: String,
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) screenshot: Option<String>,
}

pub(crate) fn load_reviews_for_branch(repo: &Repository, branch_name: &str) -> Vec<ReviewComment> {
    let branch_type = if branch_name.contains('/')
        && !branch_name.starts_with("pass/")
        && !branch_name.starts_with("zane/")
    {
        BranchType::Remote
    } else {
        BranchType::Local
    };

    if let Ok(branch) = repo.find_branch(branch_name, branch_type) {
        if let Ok(commit) = branch.get().peel_to_commit() {
            if let Ok(tree) = commit.tree() {
                if let Some(entry) = tree.get_name("reviews.json") {
                    if let Ok(blob) = repo.find_blob(entry.id()) {
                        if let Ok(text) = std::str::from_utf8(blob.content()) {
                            if let Ok(comments) = serde_json::from_str::<Vec<ReviewComment>>(text) {
                                return comments
                                    .into_iter()
                                    .filter(|c| c.branch == branch_name)
                                    .collect();
                            }
                        }
                    }
                }
            }
        }
    }

    Vec::new()
}

pub(crate) fn attach_screenshot_to_staging(
    repo_path: &Path,
    src_image: &Path,
) -> Result<String, String> {
    if !src_image.is_file() {
        return Err(format!("Screenshot file not found: {}", src_image.display()));
    }
    let file_name = src_image
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid screenshot filename".to_string())?;

    let screenshots_dir = repo_path.join("staging_media").join("screenshots");
    std::fs::create_dir_all(&screenshots_dir)
        .map_err(|e| format!("Failed to create staging_media/screenshots: {e}"))?;

    let dest = screenshots_dir.join(file_name);
    std::fs::copy(src_image, &dest)
        .map_err(|e| format!("Failed to copy screenshot: {e}"))?;

    Ok(format!("staging_media/screenshots/{file_name}"))
}

pub(crate) fn append_review_comment(
    repo_path: &Path,
    branch_name: &str,
    author: &str,
    status: &str,
    body: &str,
    screenshot: Option<String>,
) -> Result<Vec<ReviewComment>, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err("Review comment cannot be empty".to_string());
    }

    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|e| format!("Branch lookup error: {e}"))?;
    let parent_commit = branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("Branch commit error: {e}"))?;
    let base_tree = parent_commit
        .tree()
        .map_err(|e| format!("Tree error: {e}"))?;

    let mut all_comments: Vec<ReviewComment> = match base_tree.get_name("reviews.json") {
        Some(entry) => repo
            .find_blob(entry.id())
            .ok()
            .and_then(|b| std::str::from_utf8(b.content()).ok().map(str::to_string))
            .and_then(|s| serde_json::from_str::<Vec<ReviewComment>>(&s).ok())
            .unwrap_or_default(),
        None => Vec::new(),
    };

    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let comment = ReviewComment {
        id: format!("rev-{}", Utc::now().timestamp_millis()),
        branch: branch_name.to_string(),
        author: author.to_string(),
        timestamp: ts,
        status: status.to_string(),
        body: trimmed.to_string(),
        screenshot,
    };
    all_comments.push(comment);

    let json_bytes = serde_json::to_string_pretty(&all_comments)
        .map_err(|e| format!("Serialize reviews.json error: {e}"))?;

    let blob_oid = repo
        .blob(json_bytes.as_bytes())
        .map_err(|e| format!("Blob error: {e}"))?;

    let mut builder = repo
        .treebuilder(Some(&base_tree))
        .map_err(|e| format!("Treebuilder error: {e}"))?;
    builder
        .insert("reviews.json", blob_oid, 0o100644)
        .map_err(|e| format!("Insert reviews.json error: {e}"))?;
    let new_tree_id = builder
        .write()
        .map_err(|e| format!("Write tree error: {e}"))?;
    let new_tree = repo
        .find_tree(new_tree_id)
        .map_err(|e| format!("Find tree error: {e}"))?;

    let sig = repo
        .signature()
        .or_else(|_| Signature::now(author, "reviewer@editor.local"))
        .map_err(|e| format!("Signature error: {e}"))?;

    let ref_name = format!("refs/heads/{branch_name}");
    repo.commit(
        Some(&ref_name),
        &sig,
        &sig,
        &format!("review({status}): comment on {branch_name}"),
        &new_tree,
        &[&parent_commit],
    )
    .map_err(|e| format!("Review commit error: {e}"))?;

    Ok(all_comments
        .into_iter()
        .filter(|c| c.branch == branch_name)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviews_json_appends_cleanly_to_branch() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_reviews_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).unwrap();

        let repo = Repository::init(&temp_root).unwrap();
        std::fs::write(temp_root.join("README.md"), "init").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("Zane", "zane@local").unwrap();
        let oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "root", &tree, &[])
            .unwrap();
        let commit = repo.find_commit(oid).unwrap();
        repo.branch("pass/test-cut", &commit, true).unwrap();

        let comments = append_review_comment(
            &temp_root,
            "pass/test-cut",
            "LeadEditor",
            "changes_requested",
            "Trim 12 frames off intro",
            None,
        )
        .unwrap();

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].status, "changes_requested");

        let loaded = load_reviews_for_branch(&repo, "pass/test-cut");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].body, "Trim 12 frames off intro");

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
