import os
import shutil
import subprocess
import time
import tempfile
import tkinter as tk
from pathlib import Path
import ctypes

try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    try:
        ctypes.windll.user32.SetProcessDPIAware()
    except Exception:
        pass

p_z = Path(r"Z:\Media\_Inbox")
if p_z.exists():
    BASE_DIR = Path(r"Z:\Media")
else:
    BASE_DIR = Path(r"\\127.0.0.1@8080\DavWWWRoot\Media")

INBOX_DIR = BASE_DIR / "_Inbox"
ORIGINALS_DIR = BASE_DIR / "Originals"
PROXIES_DIR = BASE_DIR / "Proxies_720p"
VIDEO_EXTS = {".mp4", ".mov", ".mkv", ".m4v"}

processed_files = set()

def run_transcode(file_path, res_choice, fps_choice, use_hw):
    proxy_name = file_path.stem + "_proxy.mp4"
    target_proxy = PROXIES_DIR / proxy_name
    target_original = ORIGINALS_DIR / file_path.name

    print("\n[>>>] Staging input file locally...")
    temp_dir = Path(tempfile.gettempdir())
    local_in = temp_dir / ("ingest_in_" + file_path.name)
    local_out = temp_dir / ("ingest_out_" + proxy_name)

    try:
        shutil.copyfile(str(file_path), str(local_in))
        print("[*] Local buffer ready: " + str(local_in))
        print("[*] Res: " + res_choice + " | FPS: " + fps_choice + " | HW: " + str(use_hw))

        vf_list = []
        if res_choice == "720p":
            vf_list.append("scale=-2:720")
        elif res_choice == "1080p":
            vf_list.append("scale=-2:1080")

        if fps_choice != "Source":
            vf_list.append("fps=" + fps_choice)

        vf_list.append("format=yuv420p")
        vf_str = ",".join(vf_list)

        success = False
        if use_hw:
            cmd_amf = [
                "ffmpeg", "-y",
                "-i", str(local_in),
                "-vf", vf_str,
                "-c:v", "hevc_amf",
                "-quality", "speed",
                "-b:v", "3500k",
                "-c:a", "aac",
                "-b:a", "128k",
                str(local_out)
            ]
            res = subprocess.run(cmd_amf, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if res.returncode == 0:
                success = True
                print("[OK] AMD AMF hardware transcode succeeded.")
            else:
                print("[!] AMF unavailable. Falling back to CPU libx264...")

        if not success:
            cmd_cpu = [
                "ffmpeg", "-y",
                "-i", str(local_in),
                "-vf", vf_str,
                "-c:v", "libx264",
                "-preset", "veryfast",
                "-crf", "22",
                "-c:a", "aac",
                "-b:a", "128k",
                str(local_out)
            ]
            cpu = subprocess.run(cmd_cpu, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if cpu.returncode == 0:
                success = True
                print("[OK] CPU transcode succeeded.")
            else:
                err = cpu.stderr.decode("utf-8", errors="ignore")
                print("[ERROR] FFmpeg failed completely:\n" + err[-400:])

        if success:
            print("[*] Uploading proxy: " + target_proxy.name)
            shutil.copyfile(str(local_out), str(target_proxy))
            print("[OK] Proxy deployed.")
            try:
                shutil.move(str(file_path), str(target_original))
                print("[OK] Source archived.")
            except Exception as e:
                print("[!] Archive notice: " + str(e))
    except Exception as ex:
        print("[EXCEPTION] " + str(ex))
    finally:
        if local_in.exists():
            try:
                os.remove(str(local_in))
            except Exception:
                pass
        if local_out.exists():
            try:
                os.remove(str(local_out))
            except Exception:
                pass

def spawn_tray_card(file_path):
    root = tk.Tk()
    root.overrideredirect(True)
    root.attributes("-topmost", True)
    root.configure(bg="#18181b", highlightthickness=2, highlightbackground="#3f3f46")

   # Generous width & taskbar margin for 125%/150% Windows scaling
    w, h_col, h_exp = 540, 165, 330
    sw = root.winfo_screenwidth()
    sh = root.winfo_screenheight()
    x = sw - w - 35
    y = sh - h_col - 85
    root.geometry(str(w) + "x" + str(h_col) + "+" + str(x) + "+" + str(y))    
    
    header = tk.Frame(root, bg="#18181b")
    header.pack(fill="x", padx=18, pady=(14, 6))

    tk.Label(header, text="Cloud Ingest", fg="#10b981", bg="#18181b", font=("Segoe UI", 12, "bold")).pack(side="left")

    countdown = [10]
    timer_lbl = tk.Label(header, text="Auto-run in 10s", fg="#a1a1aa", bg="#18181b", font=("Segoe UI", 10))
    timer_lbl.pack(side="right")

    disp_name = file_path.name if len(file_path.name) <= 42 else (file_path.name[:39] + "...")
    tk.Label(root, text=disp_name, fg="#f4f4f5", bg="#18181b", font=("Segoe UI", 11, "bold"), anchor="w").pack(fill="x", padx=18, pady=(2, 8))

    settings_box = tk.Frame(root, bg="#27272a")
    res_var = tk.StringVar(value="720p")
    fps_var = tk.StringVar(value="Source")
    hw_var = tk.BooleanVar(value=True)

    row1 = tk.Frame(settings_box, bg="#27272a")
    row1.pack(fill="x", padx=14, pady=5)
    tk.Label(row1, text="Resolution:", fg="#d4d4d8", bg="#27272a", font=("Segoe UI", 10, "bold"), width=11, anchor="w").pack(side="left")
    for r in ["720p", "1080p", "Source"]:
        tk.Radiobutton(row1, text=r, variable=res_var, value=r, bg="#27272a", fg="#f4f4f5", selectcolor="#18181b", activebackground="#27272a", font=("Segoe UI", 10)).pack(side="left", padx=8)

    row2 = tk.Frame(settings_box, bg="#27272a")
    row2.pack(fill="x", padx=14, pady=5)
    tk.Label(row2, text="Frame Rate:", fg="#d4d4d8", bg="#27272a", font=("Segoe UI", 10, "bold"), width=11, anchor="w").pack(side="left")
    for f in ["Source", "24", "30", "60"]:
        tk.Radiobutton(row2, text=f, variable=fps_var, value=f, bg="#27272a", fg="#f4f4f5", selectcolor="#18181b", activebackground="#27272a", font=("Segoe UI", 10)).pack(side="left", padx=8)

    row3 = tk.Frame(settings_box, bg="#27272a")
    row3.pack(fill="x", padx=14, pady=(5, 8))
    tk.Checkbutton(row3, text="Hardware Acceleration (AMD AMF / GPU)", variable=hw_var, bg="#27272a", fg="#f4f4f5", selectcolor="#18181b", activebackground="#27272a", font=("Segoe UI", 10)).pack(anchor="w")

    btn_bar = tk.Frame(root, bg="#18181b")
    btn_bar.pack(fill="x", padx=18, pady=(4, 14))

    user_accepted = [False]
    params = {}
    timer_id = [None]

    def accept():
        if timer_id[0]:
            root.after_cancel(timer_id[0])
        params["res"] = res_var.get()
        params["fps"] = fps_var.get()
        params["hw"] = hw_var.get()
        user_accepted[0] = True
        root.destroy()

    def ignore():
        if timer_id[0]:
            root.after_cancel(timer_id[0])
        print("[*] Skipped: " + file_path.name)
        root.destroy()

    is_expanded = [False]
    def toggle_expand():
        if timer_id[0]:
            root.after_cancel(timer_id[0])
        timer_lbl.config(text="Auto-run paused")
        if not is_expanded[0]:
            settings_box.pack(fill="x", padx=18, pady=6, before=btn_bar)
            root.geometry(str(w) + "x" + str(h_exp) + "+" + str(x) + "+" + str(sh - h_exp - 75))
            opt_btn.config(text="Hide Options ▲")
            is_expanded[0] = True
        else:
            settings_box.pack_forget()
            root.geometry(str(w) + "x" + str(h_col) + "+" + str(x) + "+" + str(y))
            opt_btn.config(text="Options ▼")
            is_expanded[0] = False

    tk.Button(btn_bar, text="Transcode", bg="#2563eb", fg="#ffffff", activebackground="#1d4ed8", font=("Segoe UI", 10, "bold"), relief="flat", padx=14, pady=5, cursor="hand2", command=accept).pack(side="left")
    opt_btn = tk.Button(btn_bar, text="Options ▼", bg="#27272a", fg="#d4d4d8", activebackground="#3f3f46", font=("Segoe UI", 10), relief="flat", padx=12, pady=5, cursor="hand2", command=toggle_expand)
    opt_btn.pack(side="left", padx=10)
    tk.Button(btn_bar, text="✕ Dismiss", bg="#18181b", fg="#71717a", activebackground="#27272a", activeforeground="#ef4444", font=("Segoe UI", 10), relief="flat", padx=10, pady=5, cursor="hand2", command=ignore).pack(side="right")

    def tick():
        countdown[0] -= 1
        if countdown[0] <= 0:
            accept()
        else:
            timer_lbl.config(text="Auto-run in " + str(countdown[0]) + "s")
            timer_id[0] = root.after(1000, tick)

    timer_id[0] = root.after(1000, tick)
    root.mainloop()

    if user_accepted[0]:
        run_transcode(file_path, params["res"], params["fps"], params["hw"])

def scan_inbox():
    if not INBOX_DIR.exists():
        return
    try:
        items = list(INBOX_DIR.iterdir())
    except Exception:
        return

    for item in items:
        if item.is_file() and item.suffix.lower() in VIDEO_EXTS:
            if item.name not in processed_files:
                try:
                    if item.stat().st_size == 0:
                        continue
                except Exception:
                    continue
                processed_files.add(item.name)
                print("\n[+] Incoming clip: " + item.name)
                spawn_tray_card(item)

if __name__ == "__main__":
    print("[*-] Tray monitor active on: " + str(INBOX_DIR))
    while True:
        scan_inbox()
        time.sleep(3)