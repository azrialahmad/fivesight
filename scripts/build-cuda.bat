@echo off
call "%~dp0env.bat" || exit /b 1
cd /d "%~dp0.."
cargo %*
