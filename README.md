# Lanya — Collaborative Editing Suite for DaVinci Resolve

**Lanya** is a lightweight desktop companion designed to streamline multi-editor remote collaboration in DaVinci Resolve without requiring shared on-premise hardware or expensive dedicated server setups.

---

## Key Features

- **Virtual Media Mount (`Z:\`):** 1-click Google Drive authentication via rclone to mount shared high-speed cached media assets locally.
- **Git Timeline & Marker Sync:** Track timeline cuts, marker notes, and project snapshots using standard Git version control.
- **Background Ingest & Proxy Automation:** Integrates background pipeline scripts to transcode proxy files and normalize ingest footage.
- **Zero-Friction Windows Deployment:** Packaged into a standalone per-machine `.msi` built with WiX v3.11.

<img width="960" height="564" alt="image" src="https://github.com/user-attachments/assets/d393dbb2-5e7f-44b4-b66b-82018f83c041" />

<img width="960" height="564" alt="image" src="https://github.com/user-attachments/assets/8698fa85-888d-4902-b88d-6a45a15569b1" />

<img width="960" height="564" alt="image" src="https://github.com/user-attachments/assets/ce75a700-a532-4e23-9b93-9a0d67e6ebb9" />



---

## Installation

### Using the Installer (.msi)
1. Grab the latest `lanya-0.1.0.msi` from the releases page or your local build output (`target\wix\lanya-0.1.0.msi`).
2. Run the installer to deploy the suite to `C:\Program Files\Lanya\`.
3. Launch **Lanya** directly from the Windows Start Menu.

### Building from Source
Ensure you have the Rust toolchain and WiX Toolset v3.11 installed.

```powershell
# 1. Compile release binary
cargo build --release

# 2. Compile and link MSI installer
& "C:\Tools\wix311\candle.exe" -arch x64 -out "target\wix\main.wixobj" "wix\main.wxs"
& "C:\Tools\wix311\light.exe" -ext WixUIExtension -sice:ICE69 -out "target\wix\lanya-0.1.0.msi" "target\wix\main.wixobj"
