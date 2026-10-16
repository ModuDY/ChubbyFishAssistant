@echo off
rem ============================================================
rem  一键打包（生成安装包 setup.exe / msi）
rem  双击本文件即可，无需手敲命令、不受 PowerShell 脚本策略影响。
rem  产物输出到项目根目录的 release\ 文件夹。
rem ============================================================
setlocal
cd /d "%~dp0"

echo [1/3] Building frontend...
call npm.cmd run build
if errorlevel 1 (
  echo.
  echo [ERROR] Frontend build failed.
  pause
  exit /b 1
)

echo.
echo [2/3] Building installers (this takes several minutes)...
call npm.cmd run tauri build
if errorlevel 1 (
  echo.
  echo [ERROR] Installer build failed. Check output above.
  pause
  exit /b 1
)

echo.
echo [3/3] Collecting installers into release\ ...
node scripts\rename_release.mjs

echo.
echo ============================================================
echo Done. Send the setup.exe from this folder to others:
echo   %~dp0release
echo ============================================================
echo.
dir /b "%~dp0release"
pause
