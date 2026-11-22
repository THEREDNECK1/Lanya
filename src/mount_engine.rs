use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn is_z_mounted() -> bool {
    Path::new(r"Z:\").exists()
}

pub(crate) fn find_rclone_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let adjacent = dir.join("rclone.exe");
            if adjacent.is_file() {
                return Some(adjacent);
            }
        }
    }

    let local = PathBuf::from("rclone.exe");
    if local.is_file() {
        return Some(local);
    }

    let mut check = Command::new("rclone");
    check.arg("--version");
    #[cfg(windows)]
    check.creation_flags(CREATE_NO_WINDOW);
    if check.output().map(|o| o.status.success()).unwrap_or(false) {
        return Some(PathBuf::from("rclone"));
    }

    None
}

pub(crate) fn mount_z_drive() -> Result<Option<Child>, String> {
    mount_z_drive_with_remote("gdrive:Resolve_Cloud_Storage")
}

pub(crate) fn mount_z_drive_with_remote(remote_spec: &str) -> Result<Option<Child>, String> {
    if is_z_mounted() {
        return Ok(None);
    }

    let rclone_bin = find_rclone_binary()
        .ok_or_else(|| "rclone.exe not found in PATH or beside executable".to_string())?;

    let mut rclone_cmd = Command::new(&rclone_bin);
    rclone_cmd.args([
        "serve",
        "webdav",
        remote_spec,
        "--addr",
        "127.0.0.1:8080",
        "--vfs-cache-mode",
        "full",
        "--buffer-size",
        "64M",
        "--vfs-read-chunk-size",
        "32M",
        "--vfs-read-chunk-size-limit",
        "2G",
    ]);
    #[cfg(windows)]
    rclone_cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = rclone_cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn rclone: {e}"))?;

    let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid loopback addr");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bound = false;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("rclone exited early with status {status}"));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(120)).is_ok() {
            bound = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }

    if !bound {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Timed out waiting for rclone WebDAV on 127.0.0.1:8080".to_string());
    }

    let mut net_cmd = Command::new("net");
    net_cmd.args(["use", "Z:", "http://127.0.0.1:8080/", "/persistent:no"]);
    #[cfg(windows)]
    net_cmd.creation_flags(CREATE_NO_WINDOW);

    let net_status = net_cmd
        .status()
        .map_err(|e| format!("Failed to execute net use Z:: {e}"))?;

    if !net_status.success() && !is_z_mounted() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("net use Z: failed with status {net_status}"));
    }

    Ok(Some(child))
}
