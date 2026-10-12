@echo off
rem Run the desktop app in dev mode (Vite + Tauri) with the CUDA build environment.
call "%~dp0env.bat" || exit /b 1
cd /d "%~dp0..\app"
call npm run tauri -- dev --features cuda
