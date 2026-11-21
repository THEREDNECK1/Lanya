#!/usr/bin/env python3
"""Headless DaVinci Resolve importer for resolve-sidecar.

Reads incoming_cut_delta.json and non-destructively injects clips onto
designated collaboration overlay tracks (V2 / A3) in the active timeline.
"""

from __future__ import annotations

import os
import sys

# Automatically register DaVinci Resolve binary folder for Windows DLL discovery (Python 3.8+)
bmd_dir = r"C:\Program Files\Blackmagic Design\DaVinci Resolve"
if sys.platform == "win32" and os.path.isdir(bmd_dir):
    if hasattr(os, "add_dll_directory"):
        try:
            os.add_dll_directory(bmd_dir)
        except Exception:
            pass
    os.environ["PATH"] = bmd_dir + os.pathsep + os.environ.get("PATH", "")

# Append Blackmagic scripting modules
module_dir = r"C:\ProgramData\Blackmagic Design\DaVinci Resolve\Support\Developer\Scripting\Modules"
if os.path.isdir(module_dir) and module_dir not in sys.path:
    sys.path.append(module_dir)

try:
    import DaVinciResolveScript as dvr_script
except Exception as e:
    import json
    print(json.dumps({"status": "error", "message": f"Bridge failed to import DaVinciResolveScript: {e}"}), file=sys.stderr)
    sys.exit(1)

import argparse
import json
import shutil
from pathlib import Path
from typing import Any

DEFAULT_INPUT_PATH = Path(r"C:\Users\zanem\Resolve_Cloud_Storage\Timelines\incoming_cut_delta.json")
DEFAULT_AUDIO_DEST_DIR = Path(r"Z:\Media\Audio")

VIDEO_OVERLAY_NAME = "V2 (Collab Pass)"
AUDIO_OVERLAY_NAME = "A3 (Collab Audio)"


def emit_error(message: str, code: int = 1) -> None:
    payload = json.dumps({"status": "error", "message": message, "error": message})
    print(payload, file=sys.stderr)
    sys.exit(code)


def connect_resolve() -> Any:
    resolve = None
    try:
        resolve = dvr_script.scriptapp("Resolve")
        if not resolve:
            fusion = dvr_script.scriptapp("Fusion")
            if fusion:
                resolve = fusion.GetResolve()
    except Exception as exc:  # pragma: no cover - depends on external Resolve IPC
        emit_error(f"Failed to connect to DaVinci Resolve IPC: {exc}")
        return None

    if not resolve:
        emit_error(
            "Could not connect to DaVinci Resolve instance. "
            "Is Resolve running with External Scripting enabled?"
        )
    return resolve


def ensure_overlay_track(timeline: Any, track_type: str, target_name: str, min_index: int) -> int:
    count = int(timeline.GetTrackCount(track_type) or 0)
    for idx in range(1, count + 1):
        try:
            name = str(timeline.GetTrackName(track_type, idx) or "")
            if name == target_name:
                return idx
        except Exception:
            continue

    while count < min_index:
        timeline.AddTrack(track_type)
        count = int(timeline.GetTrackCount(track_type) or 0)

    # Prefer placing on min_index if it's empty or renameable; otherwise add a fresh track
    target_idx = min_index
    try:
        existing_items = timeline.GetItemListInTrack(track_type, target_idx) or []
        if existing_items:
            timeline.AddTrack(track_type)
            target_idx = int(timeline.GetTrackCount(track_type) or min_index)
    except Exception:
        pass

    try:
        timeline.SetTrackName(track_type, target_idx, target_name)
    except Exception:
        pass

    return target_idx


def index_media_pool(folder: Any, cache: dict[str, Any]) -> None:
    if folder is None:
        return
    try:
        for item in folder.GetClipList() or []:
            path = str(item.GetClipProperty("File Path") or "")
            if path:
                cache[os.path.normcase(os.path.normpath(path))] = item
    except Exception:
        pass
    try:
        for sub in folder.GetSubFolderList() or []:
            index_media_pool(sub, cache)
    except Exception:
        pass


def get_or_import_media_item(
    media_pool: Any, cache: dict[str, Any], file_path: str
) -> Any | None:
    if not file_path:
        return None
    norm = os.path.normcase(os.path.normpath(file_path))
    if norm in cache:
        return cache[norm]

    imported = media_pool.ImportMedia([file_path]) or []
    if imported:
        cache[norm] = imported[0]
        return imported[0]
    return None


def is_repo_audio_transport_path(file_path: str, repo_root: Path) -> bool:
    if not file_path:
        return False
    norm_file = os.path.normcase(os.path.normpath(os.path.abspath(file_path)))
    audio_dir = os.path.normcase(os.path.normpath(str(repo_root / "Media" / "Audio")))
    return norm_file.startswith(audio_dir + os.sep)


def migrate_transport_audio(
    imported_items: list[tuple[str, Any]],
    repo_root: Path,
    dest_dir: Path = DEFAULT_AUDIO_DEST_DIR,
) -> list[str]:
    migrated: list[str] = []
    for src_path, mpi in imported_items:
        if not is_repo_audio_transport_path(src_path, repo_root):
            continue
        src = Path(src_path)
        if src.name == ".gitkeep" or not src.is_file():
            continue

        os.makedirs(dest_dir, exist_ok=True)
        dest = dest_dir / src.name

        if not dest.exists():
            shutil.copy2(src, dest)

        replaced = False
        try:
            replaced = bool(mpi.ReplaceClip(str(dest)))
        except Exception:
            replaced = False

        if replaced:
            if src.is_file():
                os.remove(src)
            migrated.append(str(dest))

    return migrated


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Import incoming_cut_delta.json clips onto non-destructive Resolve tracks."
    )
    parser.add_argument(
        "--input",
        type=Path,
        default=DEFAULT_INPUT_PATH,
        help=f"Input JSON path (default: {DEFAULT_INPUT_PATH})",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=None,
        help="Repository root path for Media/Audio transport detection.",
    )
    parser.add_argument(
        "--audio-dest",
        type=Path,
        default=DEFAULT_AUDIO_DEST_DIR,
        help=f"Permanent audio destination directory (default: {DEFAULT_AUDIO_DEST_DIR})",
    )
    args = parser.parse_args()

    input_path: Path = args.input
    if not input_path.is_file():
        emit_error(f"Delta JSON not found at {input_path}")

    repo_root: Path = (
        args.repo_root
        if args.repo_root is not None
        else (
            input_path.parent.parent
            if input_path.parent.name.lower() == "timelines"
            else input_path.parent
        )
    )

    try:
        delta = json.loads(input_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        emit_error(f"Failed to parse delta JSON: {exc}")

    clips = delta.get("clips", [])
    if not isinstance(clips, list):
        emit_error("Invalid delta JSON: 'clips' must be a list.")

    resolve = connect_resolve()
    project_manager = resolve.GetProjectManager()
    project = project_manager.GetCurrentProject() if project_manager else None
    if project is None:
        emit_error("No active DaVinci Resolve project is open.")

    timeline = project.GetCurrentTimeline()
    if timeline is None:
        emit_error("No active DaVinci Resolve timeline is open.")

    media_pool = project.GetMediaPool()
    if media_pool is None:
        emit_error("Could not access DaVinci Resolve Media Pool.")

    video_track_idx = ensure_overlay_track(timeline, "video", VIDEO_OVERLAY_NAME, 2)
    audio_track_idx = ensure_overlay_track(timeline, "audio", AUDIO_OVERLAY_NAME, 3)

    media_cache: dict[str, Any] = {}
    index_media_pool(media_pool.GetRootFolder(), media_cache)

    clips_inserted = 0
    imported_for_migration: list[tuple[str, Any]] = []
    for entry in clips:
        if isinstance(entry, dict) and entry.get("change_type") == "DELETED":
            continue
        clip = entry.get("clip", entry) if isinstance(entry, dict) else entry
        file_path = str(clip.get("media_path") or clip.get("file_path") or "")
        mpi = get_or_import_media_item(media_pool, media_cache, file_path)
        if mpi is None:
            continue

        track_type = str(clip.get("track_type") or "video").lower()
        target_track = video_track_idx if track_type == "video" else audio_track_idx
        media_type = 1 if track_type == "video" else 2

        source_start = int(clip.get("source_in", clip.get("source_start", 0)))
        duration = max(1, int(clip.get("duration", 1)))
        timeline_start = int(clip.get("start_frame", clip.get("timeline_start", 0)))

        clip_info = {
            "mediaPoolItem": mpi,
            "startFrame": source_start,
            "endFrame": source_start + duration,
            "trackIndex": target_track,
            "recordFrame": timeline_start,
            "mediaType": media_type,
        }

        appended = media_pool.AppendToTimeline([clip_info])
        if appended:
            clips_inserted += 1
            imported_for_migration.append((file_path, mpi))

    migrated = migrate_transport_audio(
        imported_for_migration, repo_root=repo_root, dest_dir=args.audio_dest
    )

    print(
        json.dumps(
            {
                "status": "merged",
                "clips_inserted": clips_inserted,
                "audio_migrated": len(migrated),
            }
        )
    )


if __name__ == "__main__":
    main()
