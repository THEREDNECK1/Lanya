@echo off
title Cloud Ingest Pipeline
echo [*] Starting rclone WebDAV background server...
start /b rclone serve webdav gdrive:Resolve_Cloud_Storage --addr 127.0.0.1:8080 --vfs-cache-mode full --buffer-size 64M

echo [*] Waiting 2 seconds for server loopback...
timeout /t 2 /nobreak >nul

echo [*] Mounting Z: drive...
net use Z: /delete /y >nul 2>&1
net use Z: http://127.0.0.1:8080/ /persistent:no >nul

echo [*] Launching Interactive Tray Worker...
cd /d "%~dp0"
python ai-ingestor\interactive_transcode_worker.py
pause
