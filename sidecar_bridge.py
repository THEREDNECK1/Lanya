"""Conditional 5-second background bridge for DaVinci Resolve Studio/Free Python 3."""

from __future__ import annotations

import hashlib
import json
import os
import sys
import threading
import time

PROGRAM_DATA = os.environ.get("PROGRAMDATA", r"C:\ProgramData")
MODULE_DIR = os.path.join(
    PROGRAM_DATA,
    "Blackmagic Design",
    "DaVinci Resolve",
    "Support",
    "Developer",
    "Scripting",
    "Modules",
)
if os.path.isdir(MODULE_DIR) and MODULE_DIR not in sys.path:
    sys.path.insert(0, MODULE_DIR)

BMD_DIR = r"C:\Program Files\Blackmagic Design\DaVinci Resolve"
if sys.platform == "win32" and os.path.isdir(BMD_DIR):
    if hasattr(os, "add_dll_directory"):
        try:
            os.add_dll_directory(BMD_DIR)
        except Exception:
            pass
    os.environ.setdefault("RESOLVE_SCRIPT_LIB", os.path.join(BMD_DIR, "fusionscript.dll"))
    os.environ["PATH"] = BMD_DIR + os.pathsep + os.environ.get("PATH", "")

EXPORT_DIR = r"C:\Users\zanem\Resolve_Cloud_Storage\Timelines"
EXPORT_PATH = os.path.join(EXPORT_DIR, "cut.otio")
TEMP_EXPORT_PATH = os.path.join(EXPORT_DIR, "cut_tmp.otio")
RESTORE_SIGNAL_PATH = os.path.join(EXPORT_DIR, "restore.signal")
SANDBOX_SIGNAL_PATH = os.path.join(EXPORT_DIR, "sandbox.signal")
POLL_INTERVAL_SEC = 5.0


def get_resolve_instance():
    resolve_obj = globals().get("resolve")
    if resolve_obj is not None:
        return resolve_obj

    bmd_mod = globals().get("bmd")
    if bmd_mod is not None and hasattr(bmd_mod, "scriptapp"):
        resolve_obj = bmd_mod.scriptapp("Resolve")
        if resolve_obj is not None:
            return resolve_obj

    try:
        import DaVinciResolveScript as dvr_script

        return dvr_script.scriptapp("Resolve")
    except Exception as exc:
        print(f"[Sidecar Bridge] Could not import DaVinciResolveScript: {exc}")
        return None


def file_sha256(path: str) -> str:
    if not os.path.isfile(path):
        return ""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def remove_if_exists(path: str) -> None:
    try:
        if os.path.isfile(path):
            os.remove(path)
    except OSError:
        pass


def handle_sandbox_signal(project, timeline) -> bool:
    if not (project and os.path.isfile(SANDBOX_SIGNAL_PATH)):
        return False
    payload = {}
    try:
        with open(SANDBOX_SIGNAL_PATH, "r", encoding="utf-8") as fh:
            payload = json.load(fh)
    except Exception:
        payload = {}
    remove_if_exists(SANDBOX_SIGNAL_PATH)

    sb_name = payload.get("timeline_name") or "Timeline [SANDBOX: cut]"
    duplicated = None
    if timeline:
        try:
            duplicated = timeline.DuplicateTimeline(sb_name)
        except Exception:
            duplicated = None
    if not duplicated:
        otio_src = payload.get("otio_path") or EXPORT_PATH
        media_pool = project.GetMediaPool()
        if media_pool and os.path.isfile(otio_src):
            try:
                duplicated = media_pool.ImportTimelineFromFile(
                    otio_src, {"timelineName": sb_name}
                )
            except Exception:
                duplicated = None
    if duplicated:
        try:
            project.SetCurrentTimeline(duplicated)
        except Exception:
            pass
    print(f"[Sidecar Bridge] Sandbox timeline ready: {sb_name}")
    return True


def bridge_loop() -> None:
    os.makedirs(EXPORT_DIR, exist_ok=True)
    resolve_obj = get_resolve_instance()
    if resolve_obj is None:
        print("[Sidecar Bridge] Error: Resolve API not accessible.")
        return

    project_manager = resolve_obj.GetProjectManager()
    export_otio_const = getattr(resolve_obj, "EXPORT_OTIO", 7)
    last_hash = file_sha256(EXPORT_PATH)
    print(f"[Sidecar Bridge] Conditional 5s exporter active on {EXPORT_DIR}")

    while True:
        try:
            project = project_manager.GetCurrentProject() if project_manager else None
            timeline = project.GetCurrentTimeline() if project else None

            if handle_sandbox_signal(project, timeline):
                time.sleep(2.0)
                continue

            if project and os.path.isfile(RESTORE_SIGNAL_PATH):
                remove_if_exists(RESTORE_SIGNAL_PATH)
                last_hash = file_sha256(EXPORT_PATH)
                print("[Sidecar Bridge] Restore signal observed; pausing auto-export briefly.")
                time.sleep(10.0)
                continue

            if timeline:
                remove_if_exists(TEMP_EXPORT_PATH)
                try:
                    ok = timeline.Export(str(TEMP_EXPORT_PATH), export_otio_const, 0)
                except TypeError:
                    ok = timeline.Export(str(TEMP_EXPORT_PATH), export_otio_const)

                if ok and os.path.isfile(TEMP_EXPORT_PATH):
                    new_hash = file_sha256(TEMP_EXPORT_PATH)
                    if new_hash and new_hash != last_hash:
                        os.replace(TEMP_EXPORT_PATH, EXPORT_PATH)
                        last_hash = new_hash
                        print(f"[Sidecar Bridge] Snapshot updated: {EXPORT_PATH}")
                    else:
                        remove_if_exists(TEMP_EXPORT_PATH)
                else:
                    remove_if_exists(TEMP_EXPORT_PATH)
        except Exception as exc:
            print(f"[Sidecar Bridge] Loop warning: {exc}")
            remove_if_exists(TEMP_EXPORT_PATH)

        time.sleep(POLL_INTERVAL_SEC)


def start_bridge() -> None:
    t = threading.Thread(target=bridge_loop, name="ResolveSidecarBridge", daemon=False)
    t.start()


if __name__ == "__main__":
    start_bridge()
