use crate::branch_manager;
use git2::{BranchType, IndexAddOption, Oid, Repository, Signature};
use std::path::{Path, PathBuf};

pub fn ensure_gitignore_excludes_restore_otio(repo_path: &Path) {
    let gitignore_path = repo_path.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut additions = Vec::new();
    for rule in [
        "Timelines/Restore_*.otio",
        "Timelines/cut_tmp.otio",
        "Timelines/*.otio.tmp",
        "Timelines/*.signal",
        ".sidecar/",
    ] {
        if !existing.lines().any(|l| l.trim() == rule) {
            additions.push(rule);
        }
    }
    if !additions.is_empty() {
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        for rule in additions {
            updated.push_str(rule);
            updated.push('\n');
        }
        let _ = std::fs::write(&gitignore_path, updated);
    }
}

pub fn read_head_file(repo_path: &Path, rel_path: &str) -> Option<String> {
    let repo = Repository::open(repo_path).ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    let tree = commit.tree().ok()?;

    let mut candidates = vec![rel_path.to_string()];
    if let Some(base) = Path::new(rel_path).file_name().and_then(|n| n.to_str()) {
        if base != rel_path {
            candidates.push(base.to_string());
        }
    }
    for cand in candidates {
        if let Ok(entry) = tree.get_path(Path::new(&cand)) {
            if let Ok(blob) = repo.find_blob(entry.id()) {
                if let Ok(text) = std::str::from_utf8(blob.content()) {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

fn rewrite_otio_timeline_name(raw_bytes: &[u8], timeline_name: &str) -> Vec<u8> {
    if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(raw_bytes) {
        if let Some(obj) = value.as_object_mut() {
            if obj.contains_key("OTIO_SCHEMA") || obj.contains_key("name") {
                obj.insert(
                    "name".to_string(),
                    serde_json::Value::String(timeline_name.to_string()),
                );
                if let Ok(serialized) = serde_json::to_vec_pretty(&value) {
                    return serialized;
                }
            }
        }
    }
    raw_bytes.to_vec()
}

fn find_commit_otio_blob<'a>(
    repo: &'a Repository,
    tree: &git2::Tree<'a>,
) -> Result<git2::Blob<'a>, String> {
    for candidate in ["Timelines/cut.otio", "cut.otio"] {
        if let Ok(entry) = tree.get_path(Path::new(candidate)) {
            if let Ok(blob) = repo.find_blob(entry.id()) {
                return Ok(blob);
            }
        }
    }

    let mut fallback_oid = None;
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |_root, entry| {
        if let Some(name) = entry.name() {
            if name.ends_with(".otio") && !name.starts_with("Restore_") {
                fallback_oid = Some(entry.id());
                return git2::TreeWalkResult::Abort;
            }
        }
        git2::TreeWalkResult::Ok
    });

    let blob_oid =
        fallback_oid.ok_or_else(|| "No cut.otio found in selected commit tree".to_string())?;
    repo.find_blob(blob_oid)
        .map_err(|e| format!("Blob read error: {e}"))
}

fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

pub fn export_commit_otio(repo_path: &Path, commit_hash: &str) -> Result<PathBuf, String> {
    ensure_gitignore_excludes_restore_otio(repo_path);

    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let oid = match Oid::from_str(commit_hash) {
        Ok(id) => id,
        Err(_) => repo
            .revparse_single(commit_hash)
            .map_err(|e| format!("Invalid commit hash '{commit_hash}': {e}"))?
            .id(),
    };
    let commit = repo
        .find_commit(oid)
        .map_err(|e| format!("Commit lookup error: {e}"))?;
    let tree = commit
        .tree()
        .map_err(|e| format!("Tree lookup error: {e}"))?;
    let blob = find_commit_otio_blob(&repo, &tree)?;

    let timelines_dir = repo_path.join("Timelines");
    std::fs::create_dir_all(&timelines_dir)
        .map_err(|e| format!("Timelines dir error: {e}"))?;

    if let Ok(entries) = std::fs::read_dir(&timelines_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("Restore_") && name.ends_with(".otio") {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    let full_oid_str = oid.to_string();
    let short = &full_oid_str[..7.min(full_oid_str.len())];
    let restore_path = timelines_dir.join(format!("Restore_{short}.otio"));
    let exported_bytes = rewrite_otio_timeline_name(blob.content(), "Sidecar_Cut");
    std::fs::write(&restore_path, exported_bytes)
        .map_err(|e| format!("Failed writing {}: {e}", restore_path.display()))?;

    let canonical = std::fs::canonicalize(&restore_path)
        .map(strip_windows_verbatim_prefix)
        .unwrap_or_else(|_| restore_path.clone());

    Ok(canonical)
}

pub fn commit_working_tree_with_message(
    repo_path: &Path,
    message: &str,
) -> Result<Option<String>, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let statuses = repo
        .statuses(None)
        .map_err(|e| format!("Git status error: {e}"))?;
    if statuses.is_empty() {
        return Ok(None);
    }

    ensure_gitignore_excludes_restore_otio(repo_path);
    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("git add error: {e}"))?;
    let _ = index.remove_all(
        [
            "Timelines/Restore_*.otio",
            "Timelines/cut_tmp.otio",
            "Timelines/*.otio.tmp",
            "Timelines/*.signal",
            ".sidecar",
            ".sidecar/*",
        ]
        .iter(),
        None,
    );
    index
        .write()
        .map_err(|e| format!("Index write error: {e}"))?;

    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Tree write error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Tree find error: {e}"))?;

    let head_ref_name = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().map(|s| format!("refs/heads/{s}")))
        .unwrap_or_else(|| "refs/heads/main".to_string());

    let parent_commit = repo
        .find_reference(&head_ref_name)
        .ok()
        .and_then(|r| r.peel_to_commit().ok())
        .or_else(|| repo.head().ok().and_then(|h| h.peel_to_commit().ok()));

    if let Some(ref parent) = parent_commit {
        if parent.tree_id() == tree_id {
            return Ok(None);
        }
    }

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Zane", "zane@editor.local"))
        .map_err(|e| format!("Signature error: {e}"))?;

    let trimmed = message.trim();
    let msg = if trimmed.is_empty() {
        "[Edit] Adjusted timeline cut"
    } else {
        trimmed
    };
    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();

    let oid = repo
        .commit(Some(&head_ref_name), &sig, &sig, msg, &tree, &parents)
        .map_err(|e| format!("Commit error: {e}"))?;

    Ok(Some(format!("{:.7}", oid)))
}

#[allow(dead_code)]
pub fn submit_pass_branch(repo_path: &Path, title: &str, notes: &str) -> Result<String, String> {
    let slug = branch_manager::slugify_branch_title(title);
    let branch_name = format!("pass/{slug}");

    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let parent_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    ensure_gitignore_excludes_restore_otio(repo_path);
    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
    index
        .add_all(["Timelines", "Media", "_media"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("Stage pass error: {e}"))?;
    let _ = index.remove_all(["Timelines/Restore_*.otio", "Timelines/*.signal"].iter(), None);
    index.write().map_err(|e| format!("Index write error: {e}"))?;

    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Tree write error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Tree find error: {e}"))?;

    if let Some(ref base) = parent_commit {
        repo.branch(&branch_name, base, true)
            .map_err(|e| format!("Branch error: {e}"))?;
    }
    let ref_name = format!("refs/heads/{branch_name}");
    repo.set_head(&ref_name)
        .map_err(|e| format!("Set HEAD error: {e}"))?;

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Zane", "zane@editor.local"))
        .map_err(|e| format!("Signature error: {e}"))?;

    let msg = if notes.trim().is_empty() {
        title.trim().to_string()
    } else {
        format!("{}\n\n{}", title.trim(), notes.trim())
    };
    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();

    repo.commit(Some(&ref_name), &sig, &sig, &msg, &tree, &parents)
        .map_err(|e| format!("Pass commit error: {e}"))?;

    Ok(branch_name)
}

pub fn slugify_sandbox(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_start_matches("experiment/")
        .trim_start_matches("sandbox/");
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "edit-session".to_string()
    } else {
        slug
    }
}

/// Forks `experiment/<name>` from `main` (or HEAD), commits `Timelines/cut.otio`
/// onto the new branch ref so it immediately appears on Lane 1 in the DAG, and
/// writes `Timelines/sandbox.signal` for `sidecar_bridge.py` to duplicate the
/// active timeline in Resolve as `Timeline [SANDBOX: <name>]`.
pub fn fork_sandbox_cut(repo_path: &Path, raw_name: &str) -> Result<(String, String), String> {
    let slug = slugify_sandbox(raw_name);
    let branch_name = format!("experiment/{slug}");
    let resolve_timeline_name = format!("Timeline [SANDBOX: {slug}]");

    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let base_commit = repo
        .find_branch("main", BranchType::Local)
        .ok()
        .and_then(|b| b.get().peel_to_commit().ok())
        .or_else(|| repo.head().ok().and_then(|h| h.peel_to_commit().ok()))
        .ok_or_else(|| "No base commit found to fork sandbox".to_string())?;

    if repo.find_branch(&branch_name, BranchType::Local).is_err() {
        repo.branch(&branch_name, &base_commit, false)
            .map_err(|e| format!("Failed to create branch {branch_name}: {e}"))?;
    }

    let ref_name = format!("refs/heads/{branch_name}");
    repo.set_head(&ref_name)
        .map_err(|e| format!("Failed to checkout {branch_name}: {e}"))?;

    ensure_gitignore_excludes_restore_otio(repo_path);
    let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
    index
        .add_all(["Timelines"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("Stage Timelines error: {e}"))?;
    let _ = index.remove_all(["Timelines/Restore_*.otio", "Timelines/*.signal"].iter(), None);
    index
        .write()
        .map_err(|e| format!("Index write error: {e}"))?;
    let tree_id = index
        .write_tree()
        .map_err(|e| format!("Write tree error: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("Find tree error: {e}"))?;

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Zane", "zane@editor.local"))
        .map_err(|e| format!("Signature error: {e}"))?;

    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &format!("sandbox: fork {slug} from main"),
        &tree,
        &[&base_commit],
    )
    .map_err(|e| format!("Sandbox fork commit error: {e}"))?;

    // Signal sidecar_bridge.py to duplicate the active timeline in Resolve
    let timelines_dir = repo_path.join("Timelines");
    let _ = std::fs::create_dir_all(&timelines_dir);
    let signal_path = timelines_dir.join("sandbox.signal");
    let signal_payload = serde_json::json!({
        "action": "duplicate_sandbox",
        "branch": branch_name,
        "timeline_name": resolve_timeline_name,
        "otio_path": timelines_dir.join("cut.otio").to_string_lossy(),
    });
    if let Ok(text) = serde_json::to_string_pretty(&signal_payload) {
        let _ = std::fs::write(signal_path, text);
    }

    Ok((branch_name, resolve_timeline_name))
}

fn write_export_signal(repo_path: &Path, action: &str, branch: &str) {
    let timelines_dir = repo_path.join("Timelines");
    let _ = std::fs::create_dir_all(&timelines_dir);
    let signal_path = timelines_dir.join("export.signal");
    let payload = serde_json::json!({
        "action": action,
        "branch": branch,
        "otio_path": timelines_dir.join("cut.otio").to_string_lossy(),
    });
    if let Ok(text) = serde_json::to_string_pretty(&payload) {
        let _ = std::fs::write(signal_path, text);
    }
}

pub fn merge_branch_into_main(repo_path: &Path, branch_name: &str) -> Result<String, String> {
    let clean_branch = branch_name.trim();
    if clean_branch.is_empty() || clean_branch == "main" {
        return Err("Cannot merge main into itself".to_string());
    }

    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let source_branch = repo
        .find_branch(clean_branch, BranchType::Local)
        .map_err(|e| format!("Branch '{clean_branch}' lookup error: {e}"))?;
    let source_commit = source_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("Branch commit error: {e}"))?;

    let mut main_branch = repo
        .find_branch("main", BranchType::Local)
        .map_err(|e| format!("Main branch lookup error: {e}"))?;
    let main_commit = main_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("Main commit error: {e}"))?;

    repo.set_head("refs/heads/main")
        .map_err(|e| format!("Checkout main error: {e}"))?;
    let mut co = git2::build::CheckoutBuilder::new();
    co.force();
    repo.checkout_head(Some(&mut co))
        .map_err(|e| format!("Checkout main tree error: {e}"))?;

    let merged_oid = if source_commit.id() == main_commit.id() {
        main_commit.id()
    } else if repo
        .graph_descendant_of(source_commit.id(), main_commit.id())
        .unwrap_or(false)
    {
        main_branch
            .get_mut()
            .set_target(source_commit.id(), &format!("Fast-forward main to {clean_branch}"))
            .map_err(|e| format!("Fast-forward main error: {e}"))?;
        source_commit.id()
    } else {
        let sig = repo
            .signature()
            .or_else(|_| Signature::now("Zane", "zane@editor.local"))
            .map_err(|e| format!("Signature error: {e}"))?;
        let merged_tree = source_commit
            .tree()
            .map_err(|e| format!("Branch tree error: {e}"))?;
        repo.commit(
            Some("refs/heads/main"),
            &sig,
            &sig,
            &format!("Merge branch '{clean_branch}' into main"),
            &merged_tree,
            &[&main_commit, &source_commit],
        )
        .map_err(|e| format!("Merge commit error: {e}"))?
    };

    let mut co2 = git2::build::CheckoutBuilder::new();
    co2.force();
    repo.checkout_head(Some(&mut co2))
        .map_err(|e| format!("Checkout merged main error: {e}"))?;

    if let Ok(mut b) = repo.find_branch(clean_branch, BranchType::Local) {
        let _ = b.delete();
    }

    write_export_signal(repo_path, "merged_to_main", clean_branch);
    Ok(format!("{:.7}", merged_oid))
}

pub fn discard_branch(repo_path: &Path, branch_name: &str) -> Result<Option<String>, String> {
    let clean_branch = branch_name.trim();
    if clean_branch.is_empty() || clean_branch == "main" {
        return Err("Cannot discard the main branch".to_string());
    }

    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let main_branch = repo
        .find_branch("main", BranchType::Local)
        .map_err(|e| format!("main branch not found: {e}"))?;
    let main_commit = main_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("main commit error: {e}"))?;

    let head_is_target = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s == clean_branch))
        .unwrap_or(false);

    if head_is_target {
        let _ = branch_manager::discard_sandbox(repo_path)?;
    } else {
        // Still keep HEAD on main if detached
        if repo.head().ok().is_some_and(|h| !h.is_branch()) {
            let _ = repo.set_head("refs/heads/main");
        }
    }

    if let Ok(mut b) = repo.find_branch(clean_branch, BranchType::Local) {
        b.delete()
            .map_err(|e| format!("Delete branch '{clean_branch}' error: {e}"))?;
    }

    Ok(Some(format!("{:.7}", main_commit.id())))
}

#[allow(dead_code)]
pub fn merge_sandbox_to_main(repo_path: &Path, branch_name: &str) -> Result<String, String> {
    merge_branch_into_main(repo_path, branch_name)
}

#[allow(dead_code)]
pub fn discard_sandbox(repo_path: &Path, branch_name: &str) -> Result<Option<String>, String> {
    discard_branch(repo_path, branch_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_and_merge_sandbox_creates_branch_and_merges_to_main() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_git_engine_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("Timelines")).unwrap();

        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(&temp_root, &opts).unwrap();
        std::fs::write(temp_root.join("Timelines").join("cut.otio"), "{\"name\":\"Master\"}").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_all(["Timelines"].iter(), IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tid = idx.write_tree().unwrap();
        let tree = repo.find_tree(tid).unwrap();
        let sig = Signature::now("Zane", "zane@local").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "Node 0", &tree, &[]).unwrap();

        let (branch, tl_name) = fork_sandbox_cut(&temp_root, "cold-open").unwrap();
        assert_eq!(branch, "experiment/cold-open");
        assert_eq!(tl_name, "Timeline [SANDBOX: cold-open]");
        assert!(temp_root.join("Timelines").join("sandbox.signal").is_file());

        let merge_short = merge_sandbox_to_main(&temp_root, &branch).unwrap();
        assert_eq!(merge_short.len(), 7);
        assert!(repo.find_branch(&branch, BranchType::Local).is_err());

        drop(tree);
        drop(repo);
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn export_commit_otio_extracts_blob_and_writes_restore_file() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_export_otio_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("Timelines")).unwrap();

        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(&temp_root, &opts).unwrap();
        let sig = Signature::now("Zane", "zane@local").unwrap();

        let otio_file = temp_root.join("Timelines").join("cut.otio");
        std::fs::write(&otio_file, "{\"name\":\"CutV1\"}").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_all(["Timelines"].iter(), IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tid1 = idx.write_tree().unwrap();
        let tree1 = repo.find_tree(tid1).unwrap();
        let oid1 = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "Cut V1", &tree1, &[])
            .unwrap();

        std::fs::write(&otio_file, "{\"name\":\"CutV2\"}").unwrap();
        idx.add_all(["Timelines"].iter(), IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tid2 = idx.write_tree().unwrap();
        let tree2 = repo.find_tree(tid2).unwrap();
        let c1 = repo.find_commit(oid1).unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "Cut V2", &tree2, &[&c1])
            .unwrap();

        let exported = export_commit_otio(&temp_root, &oid1.to_string()).unwrap();
        assert!(exported.is_file());
        let expected_name = format!("Restore_{:.7}.otio", oid1);
        assert_eq!(
            exported.file_name().and_then(|n| n.to_str()),
            Some(expected_name.as_str())
        );
        let exported_text = std::fs::read_to_string(&exported).unwrap();
        let exported_json: serde_json::Value = serde_json::from_str(&exported_text).unwrap();
        assert_eq!(exported_json.get("name").and_then(|v| v.as_str()), Some("Sidecar_Cut"));
        // Live cut.otio remains untouched
        assert_eq!(std::fs::read_to_string(&otio_file).unwrap(), "{\"name\":\"CutV2\"}");

        let gitignore = std::fs::read_to_string(temp_root.join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|l| l.trim() == "Timelines/Restore_*.otio"));
        assert!(!temp_root.join(".sidecar").join("restore.signal").exists());

        drop(c1);
        drop(tree2);
        drop(tree1);
        drop(repo);
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn commit_working_tree_and_merge_or_discard_pass_branch() {
        let temp_root = std::env::temp_dir().join(format!(
            "resolve_pass_pr_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("Timelines")).unwrap();

        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(&temp_root, &opts).unwrap();
        let otio_file = temp_root.join("Timelines").join("cut.otio");
        std::fs::write(&otio_file, "{\"name\":\"V0\"}").unwrap();
        let init_commit = commit_working_tree_with_message(&temp_root, "[Edit] Base").unwrap();
        assert!(init_commit.is_some());

        // Create a pass/* branch and commit on it
        let base_c = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("pass/audio-mix", &base_c, false).unwrap();
        repo.set_head("refs/heads/pass/audio-mix").unwrap();
        std::fs::write(&otio_file, "{\"name\":\"V1\"}").unwrap();
        let pass_commit = commit_working_tree_with_message(&temp_root, "[Audio] Mix pass").unwrap();
        assert!(pass_commit.is_some());

        // Accept & Merge to Main (fast-forward)
        let merged = merge_branch_into_main(&temp_root, "pass/audio-mix").unwrap();
        assert_eq!(merged, pass_commit.unwrap());
        assert!(repo.find_branch("pass/audio-mix", BranchType::Local).is_err());
        assert_eq!(branch_manager::current_branch_name(&temp_root), "main");

        // Create another pass branch and discard it
        let new_base = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("pass/bad-cut", &new_base, false).unwrap();
        repo.set_head("refs/heads/pass/bad-cut").unwrap();
        std::fs::write(&otio_file, "{\"name\":\"V2_BAD\"}").unwrap();
        let _ = commit_working_tree_with_message(&temp_root, "[Edit] Bad cut").unwrap();

        discard_branch(&temp_root, "pass/bad-cut").unwrap();
        assert!(repo.find_branch("pass/bad-cut", BranchType::Local).is_err());
        assert_eq!(branch_manager::current_branch_name(&temp_root), "main");
        assert_eq!(std::fs::read_to_string(&otio_file).unwrap(), "{\"name\":\"V1\"}");

        drop(new_base);
        drop(base_c);
        drop(repo);
        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
