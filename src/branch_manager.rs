use crate::{ai_assistant, db_watcher, interchange, slugify_branch_title, ScopeChecklist};
use chrono::Utc;
use git2::{
    build::CheckoutBuilder, BranchType, IndexAddOption, Repository, ResetType, Signature,
};
use std::path::Path;

pub(crate) fn current_branch_name(repo_path: &Path) -> String {
    let Ok(repo) = Repository::open(repo_path) else {
        return "main".to_string();
    };
    let Ok(head) = repo.head() else {
        return "main".to_string();
    };
    head.shorthand().unwrap_or("main").to_string()
}

pub(crate) fn suggest_sandbox_slug(
    ai_endpoint: Option<&str>,
    ai_model: Option<&str>,
    scope: &ScopeChecklist,
    timeline_name: &str,
) -> String {
    let draft = ai_assistant::generate_advisory_pr_draft(
        ai_endpoint,
        ai_model,
        scope,
        timeline_name,
        "2-word experimental sandbox branch slug",
    );
    let slug = slugify_branch_title(&draft.title);
    let parts: Vec<&str> = slug.split('-').filter(|p| !p.is_empty()).take(2).collect();
    if parts.is_empty() {
        "cold-open".to_string()
    } else {
        parts.join("-")
    }
}

fn commit_lingering_changes_if_dirty(repo: &Repository) -> Result<(), String> {
    let statuses = repo
        .statuses(None)
        .map_err(|e| format!("Git status error: {e}"))?;
    if statuses.is_empty() {
        return Ok(());
    }

    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("Stage error: {e}"))?;
    index.write().map_err(|e| format!("Index write error: {e}"))?;

    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Tree write error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Tree find error: {e}"))?;

    let parent_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    if let Some(ref parent) = parent_commit {
        if parent.tree_id() == tree_id {
            return Ok(());
        }
    }

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Zane", "zane@editor.local"))
        .map_err(|e| format!("Signature error: {e}"))?;
    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();

    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &format!("micro-save: {ts}"),
        &tree,
        &parents,
    )
    .map_err(|e| format!("Lingering commit error: {e}"))?;

    Ok(())
}

pub(crate) fn start_sandbox(
    repo_path: &Path,
    raw_slug: &str,
    project_file: Option<&Path>,
) -> Result<String, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;

    // 1. Ensure working tree is clean / auto-commit lingering micro-saves to current branch
    commit_lingering_changes_if_dirty(&repo)?;

    // 2. Snapshot current timeline to .timeline_base.json
    if let Some(pf) = project_file.filter(|p| p.is_file()) {
        if let Ok(snapshot) = db_watcher::read_timeline_snapshot(pf) {
            if let Ok(pretty) = serde_json::to_string_pretty(&snapshot) {
                let _ = std::fs::write(repo_path.join(".timeline_base.json"), pretty);
            }
        }
    }

    // 3. Create & checkout `experiment/<slug>`
    let slug = slugify_branch_title(raw_slug);
    let branch_name = format!("experiment/{slug}");
    let head_commit = repo
        .head()
        .map_err(|e| format!("HEAD error: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("HEAD commit error: {e}"))?;

    repo.branch(&branch_name, &head_commit, true)
        .map_err(|e| format!("Sandbox branch error: {e}"))?;

    let ref_name = format!("refs/heads/{branch_name}");
    repo.set_head(&ref_name)
        .map_err(|e| format!("Set HEAD error: {e}"))?;

    let mut checkout_opts = CheckoutBuilder::default();
    checkout_opts.force();
    repo.checkout_head(Some(&mut checkout_opts))
        .map_err(|e| format!("Checkout error: {e}"))?;

    Ok(branch_name)
}

pub(crate) fn discard_sandbox(repo_path: &Path) -> Result<Option<String>, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;

    let main_branch = repo
        .find_branch("main", BranchType::Local)
        .map_err(|e| format!("main branch not found: {e}"))?;
    let main_commit = main_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("main commit error: {e}"))?;

    // 1. Reset working tree and HEAD back to `main`
    repo.set_head("refs/heads/main")
        .map_err(|e| format!("Set HEAD main error: {e}"))?;

    let mut checkout_opts = CheckoutBuilder::default();
    checkout_opts.force();
    repo.reset(main_commit.as_object(), ResetType::Hard, Some(&mut checkout_opts))
        .map_err(|e| format!("Hard reset to main error: {e}"))?;

    // 2. Hands-free reload clean baseline timeline into running Resolve (if interchange exists)
    let timelines_dir = repo_path.join("Timelines");
    let staging_dir = repo_path.join("staging_media");
    if let Some(interchange_file) = interchange::find_latest_interchange_file(&timelines_dir) {
        let target =
            interchange::unpack_otioz_if_needed(&interchange_file, &timelines_dir, &staging_dir)?;
        let label = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("baseline")
            .to_string();
        interchange::import_timeline_into_running_resolve(&target)?;
        return Ok(Some(label));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_discard_sandbox_branches_cleanly() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_sandbox_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).unwrap();

        let repo = Repository::init(&temp_root).unwrap();
        let file = temp_root.join("cut.otio");
        std::fs::write(&file, "main_cut").unwrap();

        let mut index = repo.index().unwrap();
        index.add_path(Path::new("cut.otio")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("Zane", "zane@local").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "main root", &tree, &[])
            .unwrap();
        repo.set_head("refs/heads/main").unwrap();

        let branch = start_sandbox(&temp_root, "Cold Open", None).unwrap();
        assert_eq!(branch, "experiment/cold-open");
        assert_eq!(current_branch_name(&temp_root), "experiment/cold-open");

        // Make experimental edit on sandbox
        std::fs::write(&file, "experimental_cut").unwrap();

        let _ = discard_sandbox(&temp_root).unwrap();
        assert_eq!(current_branch_name(&temp_root), "main");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "main_cut");

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
