@echo off
rem ============================================================
rem  一键启动 AI Work 助手（TraeWorkAssistant）
rem  双击本文件即可；不会重新编译，直接拉起已编译好的程序。
rem  若提示找不到程序，说明还没编译过，按提示先编译一次即可。
rem ============================================================
setlocal
cd /d "%~dp0"

set "EXE=%~dp0src-tauri\target\debug\ai-work-assistant.exe"

if not exist "%EXE%" (
  echo.
  echo [ERROR] Not found:
  echo   %EXE%
  echo.
  echo Please build it once first ^(about 1-2 minutes^):
  echo   cd /d "%~dp0src-tauri"
  echo   cargo build
  echo.
  pause
  exit /b 1
)

rem 已在运行则不重复启动（避免两个实例抢同一个数据库）
tasklist /fi "imagename eq ai-work-assistant.exe" 2>nul | find /i "ai-work-assistant.exe" >nul
if not errorlevel 1 (
  echo AI Work Assistant is already running. Nothing to do.
  ping -n 4 127.0.0.1 >nul
  exit /b 0
)

echo Starting AI Work Assistant...
powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process '%EXE%'"
exit /b 0
