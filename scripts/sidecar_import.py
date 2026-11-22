"""Main-thread timeline importer for DaVinci Resolve."""

from __future__ import annotations

import os
import subprocess
import sys
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
if MODULE_DIR not in sys.path:
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


def resolve_target_otio(argv: list[str]) -> str:
    args = [a for a in argv[1:] if a != "--api-only"]
    if args and args[0].strip():
        return os.path.abspath(args[0].strip())
    if os.path.isdir(EXPORT_DIR):
        for name in sorted(os.listdir(EXPORT_DIR), reverse=True):
            if name.startswith("Restore_") and name.endswith(".otio"):
                return os.path.join(EXPORT_DIR, name)
    return os.path.join(EXPORT_DIR, "cut.otio")


def find_timelines_by_name(project: object, target_name: str) -> list[object]:
    matches = []
    try:
        count = int(project.GetTimelineCount() or 0)
        for idx in range(1, count + 1):
            tl = project.GetTimelineByIndex(idx)
            if tl and str(tl.GetName() or "") == target_name:
                matches.append(tl)
    except Exception:
        pass
    return matches


def import_via_resolve_api(resolve_obj: object, otio_path: str) -> bool:
    pm = resolve_obj.GetProjectManager()
    project = pm.GetCurrentProject() if pm else None
    media_pool = project.GetMediaPool() if project else None
    if not project or not media_pool:
        print("[Sidecar Import] Error: No active project or MediaPool.", file=sys.stderr)
        return False

    # 1. Identify if Sidecar_Cut already exists before importing
    old_timelines = find_timelines_by_name(project, "Sidecar_Cut")

    # 2. Import target OTIO via MediaPool
    new_timeline = media_pool.ImportTimelineFromFile(str(otio_path))
    if new_timeline is None:
        new_timeline = media_pool.ImportTimelineFromFile(
            str(otio_path),
            {"importSourceClips": True},
        )
    if new_timeline is None:
        print(f"[Sidecar Import] Failed to import {otio_path}", file=sys.stderr)
        return False

    # 3. Switch active viewer focus to imported timeline first so old timeline can be deleted
    try:
        project.SetCurrentTimeline(new_timeline)
    except Exception:
        pass

    # 4. Delete previous Sidecar_Cut timeline entry (prevent tab bloat)
    if old_timelines:
        for old_tl in old_timelines:
            if old_tl == new_timeline:
                continue
            try:
                media_pool.DeleteTimelines([old_tl])
            except Exception:
                pass

    # 5. Rename active timeline to Sidecar_Cut
    try:
        new_timeline.SetName("Sidecar_Cut")
    except Exception:
        pass

    print(f"Restored {os.path.basename(otio_path)} -> active tab 'Sidecar_Cut'")
    return True


def try_external_resolve_api(otio_path: str) -> str | None:
    creationflags = 0x08000000 if sys.platform == "win32" else 0
    try:
        res = subprocess.run(
            [sys.executable, os.path.abspath(__file__), "--api-only", str(otio_path)],
            capture_output=True,
            text=True,
            timeout=5,
            creationflags=creationflags,
        )
        if res.returncode == 0 and res.stdout.strip():
            return res.stdout.strip()
    except Exception:
        pass
    return None


def import_via_windows_gui(otio_path: str) -> bool:
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    ULONG_PTR = ctypes.c_uint64 if ctypes.sizeof(ctypes.c_void_p) == 8 else ctypes.c_uint32

    class MOUSEINPUT(ctypes.Structure):
        _fields_ = [
            ("dx", wintypes.LONG),
            ("dy", wintypes.LONG),
            ("mouseData", wintypes.DWORD),
            ("dwFlags", wintypes.DWORD),
            ("time", wintypes.DWORD),
            ("dwExtraInfo", ULONG_PTR),
        ]

    class KEYBDINPUT(ctypes.Structure):
        _fields_ = [
            ("wVk", wintypes.WORD),
            ("wScan", wintypes.WORD),
            ("dwFlags", wintypes.DWORD),
            ("time", wintypes.DWORD),
            ("dwExtraInfo", ULONG_PTR),
        ]

    class HARDWAREINPUT(ctypes.Structure):
        _fields_ = [
            ("uMsg", wintypes.DWORD),
            ("wParamL", wintypes.WORD),
            ("wParamH", wintypes.WORD),
        ]

    class INPUT_UNION(ctypes.Union):
        _fields_ = [
            ("mi", MOUSEINPUT),
            ("ki", KEYBDINPUT),
            ("hi", HARDWAREINPUT),
        ]

    class INPUT(ctypes.Structure):
        _fields_ = [
            ("type", wintypes.DWORD),
            ("union", INPUT_UNION),
        ]

    INPUT_KEYBOARD = 1
    KEYEVENTF_KEYUP = 0x0002
    KEYEVENTF_UNICODE = 0x0004
    VK_RETURN = 0x0D
    VK_SHIFT = 0x10
    VK_CONTROL = 0x11
    VK_I = 0x49
    SW_RESTORE = 9

    EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    found_hwnd = [None]

    def enum_cb(hwnd: int, _lparam: int) -> bool:
        if not user32.IsWindowVisible(hwnd):
            return True
        buf = ctypes.create_unicode_buffer(256)
        length = user32.GetWindowTextW(hwnd, buf, 256)
        if length > 0 and "DaVinci Resolve" in buf.value:
            found_hwnd[0] = hwnd
            return False
        class_buf = ctypes.create_unicode_buffer(128)
        clen = user32.GetClassNameW(hwnd, class_buf, 128)
        if clen > 0 and class_buf.value == "Qt653QWindowIcon":
            found_hwnd[0] = hwnd
            return False
        return True

    user32.EnumWindows(EnumWindowsProc(enum_cb), 0)
    hwnd = found_hwnd[0]
    if not hwnd:
        print("DaVinci Resolve window not found", file=sys.stderr)
        return False

    if user32.IsIconic(hwnd):
        user32.ShowWindow(hwnd, SW_RESTORE)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.15)

    def make_vk(vk: int, up: bool = False) -> INPUT:
        return INPUT(
            type=INPUT_KEYBOARD,
            union=INPUT_UNION(
                ki=KEYBDINPUT(
                    wVk=vk,
                    wScan=0,
                    dwFlags=KEYEVENTF_KEYUP if up else 0,
                    time=0,
                    dwExtraInfo=0,
                )
            ),
        )

    def make_char(code: int, up: bool = False) -> INPUT:
        return INPUT(
            type=INPUT_KEYBOARD,
            union=INPUT_UNION(
                ki=KEYBDINPUT(
                    wVk=0,
                    wScan=code,
                    dwFlags=KEYEVENTF_UNICODE | (KEYEVENTF_KEYUP if up else 0),
                    time=0,
                    dwExtraInfo=0,
                )
            ),
        )

    def send(items: list[INPUT]) -> None:
        arr = (INPUT * len(items))(*items)
        user32.SendInput(len(items), arr, ctypes.sizeof(INPUT))

    send([
        make_vk(VK_CONTROL, False),
        make_vk(VK_SHIFT, False),
        make_vk(VK_I, False),
        make_vk(VK_I, True),
        make_vk(VK_SHIFT, True),
        make_vk(VK_CONTROL, True),
    ])
    time.sleep(0.35)

    text_inputs: list[INPUT] = []
    utf16_bytes = str(otio_path).encode("utf-16-le")
    for idx in range(0, len(utf16_bytes), 2):
        code_unit = int.from_bytes(utf16_bytes[idx : idx + 2], "little")
        text_inputs.append(make_char(code_unit, False))
        text_inputs.append(make_char(code_unit, True))
    text_inputs.append(make_vk(VK_RETURN, False))
    text_inputs.append(make_vk(VK_RETURN, True))
    send(text_inputs)
    time.sleep(0.35)

    send([make_vk(VK_RETURN, False), make_vk(VK_RETURN, True)])
    print(f"Restored {os.path.basename(otio_path)} -> opened in Resolve tab")
    return True


def main() -> int:
    otio_path = resolve_target_otio(sys.argv)
    if not os.path.isfile(otio_path):
        print(f"Restore OTIO not found: {otio_path}", file=sys.stderr)
        return 1

    if "--api-only" in sys.argv:
        try:
            import DaVinciResolveScript as dvr_script

            resolve_obj = dvr_script.scriptapp("Resolve")
            if resolve_obj is None:
                return 1
            return 0 if import_via_resolve_api(resolve_obj, otio_path) else 1
        except Exception as exc:
            print(f"[Sidecar Import] Resolve API error: {exc}", file=sys.stderr)
            return 1

    resolve_obj = globals().get("resolve")
    if resolve_obj is not None:
        return 0 if import_via_resolve_api(resolve_obj, otio_path) else 1

    api_msg = try_external_resolve_api(otio_path)
    if api_msg:
        print(api_msg)
        return 0

    if sys.platform == "win32":
        return 0 if import_via_windows_gui(otio_path) else 1

    print("Unsupported platform for headless Resolve import", file=sys.stderr)
    return 1


if __name__ == "__main__":
    exit_code = main()
    if globals().get("resolve") is None:
        sys.exit(exit_code)
