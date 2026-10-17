@echo off
rem ============================================================
rem  一键启动 AI Work 助手（TraeWorkAssistant）
rem  双击本文件即可。
rem  · 源码没改动时：直接拉起已编译好的程序（秒开，不重新编译）
rem  · 源码比程序新 / 程序不存在时：自动重新编译后再启动
rem    （前端 npm run build + Rust cargo build，一般 1-3 分钟）
rem  注意：重新编译前需先关闭正在运行的旧程序，否则 exe 被占用会链接失败。
rem ============================================================
setlocal
cd /d "%~dp0"

set "EXE=%~dp0src-tauri\target\debug\ai-work-assistant.exe"

rem ---- 运行状态检测：是否已有实例在跑 ----
set "RUNNING="
for /f "delims=" %%i in ('tasklist /fi "imagename eq ai-work-assistant.exe" /nh 2^>nul ^| find /i "ai-work-assistant.exe"') do set "RUNNING=1"

rem ---- 新鲜度检测：compare 源码时间 vs exe 时间 ----
set "STATE=stale"
for /f "delims=" %%i in ('node "%~dp0scripts\launcher_check.mjs" "%EXE%" 2^>nul') do set "STATE=%%i"
if "%STATE%"=="" set "STATE=stale"

if /i "%STATE%"=="fresh" goto :launch

rem ================= 需要（重新）编译 =================
if defined RUNNING (
  echo.
  echo [WARN] Sources are newer than the built program, so it must be rebuilt.
  echo        A running instance is holding the exe. Please close
  echo        "AI Work Assistant" first, then run this file again.
  echo.
  pause
  exit /b 1
)

echo.
echo ============================================================
echo  Rebuilding: sources changed since last build
echo ============================================================
echo.

echo [1/2] Building frontend...
call npm.cmd run build
if errorlevel 1 (
  echo.
  echo [ERROR] Frontend build failed. Check output above.
  pause
  exit /b 1
)

echo.
echo [2/2] Building Rust program ^(first run may take several minutes^)...
cargo build --manifest-path "%~dp0src-tauri\Cargo.toml"
if errorlevel 1 (
  echo.
  echo [ERROR] Rust build failed. Check output above.
  pause
  exit /b 1
)

if not exist "%EXE%" (
  echo.
  echo [ERROR] Build finished but file not found:
  echo   %EXE%
  pause
  exit /b 1
)

echo.
echo Build done.
echo.

:launch
if defined RUNNING (
  echo AI Work Assistant is already running. Nothing to do.
  ping -n 4 127.0.0.1 >nul
  exit /b 0
)

echo Starting AI Work Assistant...
powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process '%EXE%'"
exit /b 0
