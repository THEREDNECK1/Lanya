use crate::interchange;
use git2::{build::CheckoutBuilder, Oid, Repository};
use std::path::{Path, PathBuf};

pub(crate) fn restore_micro_save_headless(
    repo_path: &Path,
    full_oid: &str,
) -> Result<Option<PathBuf>, String> {
    let repo = Repository::open(repo_path).map_err(|e| format!("Git open error: {e}"))?;
    let oid = Oid::from_str(full_oid).map_err(|e| format!("Invalid OID: {e}"))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| format!("Commit lookup error: {e}"))?;
    let tree = commit
        .tree()
        .map_err(|e| format!("Tree lookup error: {e}"))?;

    let cut_otio_rel = Path::new("Timelines/cut.otio");
    let has_cut_otio = tree.get_path(cut_otio_rel).is_ok();

    let mut opts = CheckoutBuilder::new();
    opts.force();
    if has_cut_otio {
        opts.path(cut_otio_rel);
    }

    repo.checkout_tree(tree.as_object(), Some(&mut opts))
        .map_err(|e| format!("Checkout restore error: {e}"))?;

    if !has_cut_otio {
        let mut index = repo.index().map_err(|e| format!("Index error: {e}"))?;
        index
            .read_tree(&tree)
            .map_err(|e| format!("Index tree error: {e}"))?;
        index
            .write()
            .map_err(|e| format!("Index write error: {e}"))?;
    }

    let restored_path = if has_cut_otio {
        Some(repo_path.join(cut_otio_rel))
    } else {
        interchange::find_latest_interchange_file(&repo_path.join("Timelines"))
    };

    if let Some(ref target) = restored_path {
        if target.is_file() {
            // Best-effort headless import as an active tab in running Resolve
            let _ = interchange::import_timeline_into_running_resolve(target);
        }
    }

    Ok(restored_path)
}
