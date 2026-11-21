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
from collections import defaultdict
from pathlib import Path
from typing import Any

DEFAULT_BASELINE_PATH = Path(".timeline_base.json")


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


def extract_cdl(item: Any) -> dict[str, Any] | None:
    try:
        cdl = item.GetCDL()
        if isinstance(cdl, dict) and cdl:
            return cdl
    except Exception:
        pass
    return None


def serialize_clip(item: Any, track_type: str, track_idx: int) -> dict[str, Any]:
    start_frame = int(item.GetStart())
    end_frame = int(item.GetEnd())
    duration = int(item.GetDuration())

    try:
        source_in = int(item.GetLeftOffset())
    except Exception:
        source_in = 0
    source_out = source_in + duration

    speed = 100.0
    try:
        props = item.GetProperty()
        if isinstance(props, dict) and "Speed" in props:
            speed = float(props["Speed"])
    except Exception:
        pass

    media_path = ""
    try:
        mpi = item.GetMediaPoolItem()
        if mpi is not None:
            raw_path = str(mpi.GetClipProperty("File Path") or "").strip()
            if raw_path:
                media_path = os.path.abspath(raw_path)
    except Exception:
        media_path = ""

    return {
        "track_type": track_type,
        "track_index": track_idx,
        "clip_name": str(item.GetName() or "Unnamed Clip"),
        "media_path": media_path,
        "start_frame": start_frame,
        "end_frame": end_frame,
        "duration": duration,
        "source_in": source_in,
        "source_out": source_out,
        "speed": speed,
        "cdl": extract_cdl(item),
    }


def capture_timeline_snapshot(project: Any, timeline: Any) -> dict[str, Any]:
    clips: list[dict[str, Any]] = []
    tracks_seen: list[str] = []

    for track_type in ("video", "audio"):
        prefix = "V" if track_type == "video" else "A"
        try:
            track_count = int(timeline.GetTrackCount(track_type) or 0)
        except Exception:
            track_count = 0

        for track_idx in range(1, track_count + 1):
            try:
                items = timeline.GetItemListInTrack(track_type, track_idx) or []
            except Exception:
                items = []

            if items:
                label = f"{prefix}{track_idx}"
                if label not in tracks_seen:
                    tracks_seen.append(label)

            for item in items:
                clips.append(serialize_clip(item, track_type, track_idx))

    return {
        "project_name": str(project.GetName() or "Untitled Project"),
        "timeline_name": str(timeline.GetName() or "Untitled Timeline"),
        "fps": str(timeline.GetSetting("timelineFrameRate") or "24"),
        "tracks": tracks_seen,
        "clips": clips,
    }


def clip_identity_keys(clips: list[dict[str, Any]]) -> dict[tuple[Any, ...], dict[str, Any]]:
    counts: dict[tuple[Any, ...], int] = defaultdict(int)
    keyed: dict[tuple[Any, ...], dict[str, Any]] = {}
    for clip in clips:
        base = (
            clip.get("track_type", ""),
            int(clip.get("track_index", 0)),
            clip.get("media_path") or clip.get("clip_name", ""),
        )
        occ = counts[base]
        counts[base] += 1
        keyed[(*base, occ)] = clip
    return keyed


def clips_equal(a: dict[str, Any], b: dict[str, Any]) -> bool:
    fields = (
        "clip_name",
        "media_path",
        "start_frame",
        "end_frame",
        "duration",
        "source_in",
        "source_out",
        "speed",
        "cdl",
    )
    return all(a.get(k) == b.get(k) for k in fields)


def compute_diff(
    baseline_clips: list[dict[str, Any]], live_clips: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    base_map = clip_identity_keys(baseline_clips)
    live_map = clip_identity_keys(live_clips)

    diff_items: list[dict[str, Any]] = []

    for key, live_clip in live_map.items():
        if key not in base_map:
            diff_items.append({"change_type": "ADDED", "clip": live_clip})
        elif not clips_equal(base_map[key], live_clip):
            diff_items.append(
                {
                    "change_type": "MODIFIED",
                    "clip": live_clip,
                    "previous_clip": base_map[key],
                }
            )

    for key, base_clip in base_map.items():
        if key not in live_map:
            diff_items.append({"change_type": "DELETED", "clip": base_clip})

    return diff_items


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Deterministic DaVinci Resolve Timeline Diff Engine."
    )
    parser.add_argument(
        "--baseline",
        action="store_true",
        help="Capture full timeline snapshot and save to .timeline_base.json.",
    )
    parser.add_argument(
        "--baseline-file",
        type=Path,
        default=DEFAULT_BASELINE_PATH,
        help=f"Path to baseline snapshot JSON (default: {DEFAULT_BASELINE_PATH})",
    )
    args = parser.parse_args()

    resolve = connect_resolve()
    project_manager = resolve.GetProjectManager()
    if not project_manager:
        emit_error("ProjectManager not accessible")

    project = project_manager.GetCurrentProject()
    if not project:
        emit_error("No project currently open in Resolve")

    timeline = project.GetCurrentTimeline()
    if not timeline:
        emit_error("No active timeline selected")

    snapshot = capture_timeline_snapshot(project, timeline)
    baseline_path: Path = args.baseline_file

    if args.baseline:
        try:
            baseline_path.parent.mkdir(parents=True, exist_ok=True)
            baseline_path.write_text(json.dumps(snapshot, indent=2), encoding="utf-8")
        except OSError as exc:
            emit_error(f"Failed to write baseline snapshot to {baseline_path}: {exc}")

        print(
            json.dumps(
                {
                    "status": "ok",
                    "mode": "baseline",
                    "clip_count": len(snapshot["clips"]),
                    "tracks": snapshot["tracks"],
                    "baseline_path": str(baseline_path),
                }
            )
        )
        return

    baseline_clips: list[dict[str, Any]] = []
    if baseline_path.is_file():
        try:
            baseline_data = json.loads(baseline_path.read_text(encoding="utf-8"))
            if isinstance(baseline_data, dict) and isinstance(baseline_data.get("clips"), list):
                baseline_clips = baseline_data["clips"]
        except (OSError, json.JSONDecodeError) as exc:
            emit_error(f"Failed to read baseline snapshot {baseline_path}: {exc}")

    diff_items = compute_diff(baseline_clips, snapshot["clips"])

    print(
        json.dumps(
            {
                "status": "ok",
                "mode": "diff",
                "project_name": snapshot["project_name"],
                "timeline_name": snapshot["timeline_name"],
                "fps": snapshot["fps"],
                "tracks": snapshot["tracks"],
                "diff_items": diff_items,
                "live_snapshot": snapshot,
            }
        )
    )


if __name__ == "__main__":
    main()
