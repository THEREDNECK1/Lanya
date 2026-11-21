import hashlib
import os
import socket
import subprocess
import sys
import threading
import time
import traceback
from pathlib import Path

TIMELINES_DIR = Path(r"C:\Users\zanem\Resolve_Cloud_Storage\Timelines")
SIDECAR_DIR = Path(r"C:\Users\zanem\Resolve_Cloud_Storage\.sidecar")
LOG_PATH = SIDECAR_DIR / "sidecar_daemon.log"
EXPORT_PATH = TIMELINES_DIR / "cut.otio"
TMP_PATH = TIMELINES_DIR / "cut_tmp.otio"
POLL_INTERVAL_SECS = 1.2
DAEMON_LOCK_PORT = 47831

try:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(line_buffering=True)
except Exception:
    pass


def log(msg: str) -> None:
    try:
        SIDECAR_DIR.mkdir(parents=True, exist_ok=True)
        ts = time.strftime("%H:%M:%S")
        with LOG_PATH.open("a", encoding="utf-8") as lf:
            lf.write(f"[{ts}] {msg}\n")
    except Exception:
        pass

    try:
        if hasattr(sys.stdout, "flush"):
            print(msg, flush=True)
        else:
            print(msg)
    except Exception:
        pass


log("=== [Sidecar] Script Invoked ===")

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


def is_resolve_host_process() -> bool:
    exe_name = os.path.basename(sys.executable or "").lower()
    stdout_name = str(getattr(sys.stdout, "__name__", ""))
    return (
        ("fu_stdout" in sys.modules)
        or (stdout_name == "fu_stdout")
        or ("resolve" in globals())
        or ("fusion" in globals())
        or ("bmd" in globals())
        or exe_name in ("resolve.exe", "fuscript.exe")
    )


def acquire_singleton_lock() -> bool:
    if getattr(sys, "_sidecar_lock_socket", None) is not None:
        return True
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", DAEMON_LOCK_PORT))
        sock.listen(1)
        sys._sidecar_lock_socket = sock
        return True
    except OSError:
        log("[Sidecar] Port 47831 busy")
        try:
            sock.close()
        except OSError:
            pass
        return False


def release_singleton_lock() -> None:
    sock = getattr(sys, "_sidecar_lock_socket", None)
    if sock is not None:
        try:
            sock.close()
        except OSError:
            pass
        sys._sidecar_lock_socket = None


def can_connect_resolve_externally() -> bool:
    argv = getattr(sys, "argv", [])
    if "--probe" in argv:
        return False
    if is_resolve_host_process():
        return True
    creationflags = 0x08000000 if sys.platform == "win32" else 0
    try:
        res = subprocess.run(
            [sys.executable, os.path.abspath(__file__), "--probe"],
            capture_output=True,
            text=True,
            timeout=5,
            creationflags=creationflags,
        )
        return res.returncode == 0
    except Exception:
        return False


def get_resolve_instance():
    resolve_obj = globals().get("resolve")
    if resolve_obj is not None:
        return resolve_obj

    bmd_mod = globals().get("bmd")
    if bmd_mod is not None and hasattr(bmd_mod, "scriptapp"):
        try:
            resolve_obj = bmd_mod.scriptapp("Resolve")
            if resolve_obj is not None:
                return resolve_obj
        except Exception:
            pass

    if "fusion" in globals():
        try:
            fus = globals().get("fusion")
            if fus is not None and hasattr(fus, "GetResolve"):
                resolve_obj = fus.GetResolve()
                if resolve_obj is not None:
                    return resolve_obj
        except Exception:
            pass

    try:
        import DaVinciResolveScript as bmd

        return bmd.scriptapp("Resolve")
    except Exception as exc:
        log(f"[Sidecar] Resolve import failed: {exc}")
        return None


def compute_sha256(path: Path) -> str:
    if not path.is_file():
        return ""
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_unlink(path: Path) -> None:
    try:
        path.unlink(missing_ok=True)
    except OSError:
        pass


def export_timeline_otio(resolve_obj, timeline, target_path: Path, verbose: bool = False) -> bool:
    export_otio = getattr(resolve_obj, "EXPORT_OTIO", None)
    export_none = getattr(resolve_obj, "EXPORT_NONE", 0)
    if export_otio is None:
        export_otio = 17

    ok = False
    try:
        ok = bool(timeline.Export(str(target_path), export_otio))
    except TypeError:
        ok = False
    except Exception as exc:
        if verbose:
            log(f"[Sidecar Auto-Sync] timeline.Export(2-arg) exception: {exc}")
        ok = False

    if not ok or not target_path.is_file():
        try:
            ok = bool(timeline.Export(str(target_path), export_otio, export_none))
        except Exception as exc:
            if verbose:
                log(f"[Sidecar Auto-Sync] timeline.Export(3-arg) exception: {exc}")
            ok = False

    if verbose:
        log(
            f"[Sidecar Auto-Sync] Export call: type={export_otio!r}, ok={ok}, tmp_exists={target_path.is_file()}"
        )
    return ok and target_path.is_file()


def poll_once(resolve_obj, pm, last_hash: str, verbose: bool = False) -> str:
    proj = pm.GetCurrentProject() if pm else None
    timeline = proj.GetCurrentTimeline() if proj else None
    if timeline is None:
        if verbose:
            pname = proj.GetName() if proj else None
            log(f"[Sidecar Auto-Sync] No active timeline (project={pname!r}).")
        return last_hash

    TIMELINES_DIR.mkdir(parents=True, exist_ok=True)
    safe_unlink(TMP_PATH)

    ok = export_timeline_otio(resolve_obj, timeline, TMP_PATH, verbose=verbose)
    if ok:
        new_hash = compute_sha256(TMP_PATH)
        if new_hash and new_hash != last_hash:
            os.replace(TMP_PATH, EXPORT_PATH)
            tl_name = str(timeline.GetName() or "Timeline")
            log(f"[Sidecar Auto-Sync] >> Change detected on '{tl_name}'. Exported cut.otio")
            return new_hash
        safe_unlink(TMP_PATH)
        if verbose:
            tl_name = str(timeline.GetName() or "Timeline")
            log(f"[Sidecar Auto-Sync] '{tl_name}' hash unchanged ({new_hash[:12]}).")
    else:
        safe_unlink(TMP_PATH)

    return last_hash


def get_process_cmdline() -> str:
    if sys.platform != "win32":
        return " ".join(getattr(sys, "argv", []))
    try:
        import ctypes

        GetCommandLineW = ctypes.windll.kernel32.GetCommandLineW
        GetCommandLineW.restype = ctypes.c_wchar_p
        return str(GetCommandLineW() or "")
    except Exception:
        return " ".join(getattr(sys, "argv", []))


def daemon_loop(initial_resolve_obj, initial_last_hash: str) -> None:
    last_hash = initial_last_hash
    resolve_obj = initial_resolve_obj
    pm = resolve_obj.GetProjectManager()

    log("[Sidecar Auto-Sync] 1.2s polling daemon active.")
    try:
        while True:
            try:
                last_hash = poll_once(resolve_obj, pm, last_hash, verbose=False)
            except Exception as exc:
                log(f"[Sidecar Auto-Sync] Warning: {exc}\n{traceback.format_exc()}")
                safe_unlink(TMP_PATH)

            time.sleep(POLL_INTERVAL_SECS)
    finally:
        log("[Sidecar Auto-Sync] Polling daemon stopped; releasing port 47831.")
        release_singleton_lock()


def start_daemon() -> int:
    in_resolve_host = is_resolve_host_process()
    exe_name = os.path.basename(sys.executable or "").lower()
    log(
        f"[Sidecar Auto-Sync] Host check: in_resolve_host={in_resolve_host}, "
        f"exe={exe_name}, stdout={getattr(sys.stdout, '__name__', type(sys.stdout).__name__)}, "
        f"cmdline={get_process_cmdline()}"
    )

    if not in_resolve_host:
        if not can_connect_resolve_externally():
            log(
                "[Sidecar] External Resolve API unavailable "
                "(Resolve Free blocks external scripting; run from Workspace -> Scripts -> Utility -> sidecar_utility)."
            )
            return 1

    if not acquire_singleton_lock():
        return 0

    resolve_obj = get_resolve_instance()
    if resolve_obj is None:
        release_singleton_lock()
        log("[Sidecar] Resolve API unavailable.")
        return 1

    log("[Sidecar Auto-Sync] Connected to Resolve. Running initial export check...")
    pm = resolve_obj.GetProjectManager()
    last_hash = compute_sha256(EXPORT_PATH)
    try:
        last_hash = poll_once(resolve_obj, pm, last_hash, verbose=True)
    except Exception as exc:
        log(f"[Sidecar Auto-Sync] Initial poll error: {exc}\n{traceback.format_exc()}")

    if exe_name == "resolve.exe":
        t = threading.Thread(
            target=daemon_loop,
            args=(resolve_obj, last_hash),
            name="SidecarAutoSyncDaemon",
            daemon=True,
        )
        sys._sidecar_daemon_thread = t
        t.start()
    else:
        try:
            daemon_loop(resolve_obj, last_hash)
        except KeyboardInterrupt:
            pass
    return 0


def main() -> int:
    argv = getattr(sys, "argv", [])
    if "--probe" in argv:
        try:
            import DaVinciResolveScript as bmd

            return 0 if bmd.scriptapp("Resolve") is not None else 1
        except Exception:
            return 1

    return start_daemon()


if __name__ == "__main__":
    exit_code = main()
    if not is_resolve_host_process():
        sys.exit(exit_code)
