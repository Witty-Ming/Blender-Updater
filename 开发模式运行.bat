@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0blender-updater-rs"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

echo.
echo   正在编译并启动开发版（debug 构建）
echo   - 改完 Rust 或 ui/ 里的文件后，关掉窗口再双击本脚本即可
echo   - debug 构建比 release 快很多，界面表现完全一样
echo   - 窗口里按 F12 可以打开开发者工具看日志
echo.

cargo run

echo.
echo   程序已退出。按任意键关闭。
pause >nul
