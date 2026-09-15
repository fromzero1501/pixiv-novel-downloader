@echo off
cd /d "%~dp0"
title Pixiv Novel Downloader - Dev Mode
setlocal

echo ==========================================
echo  Pixiv Novel Downloader - Dev Mode
echo ==========================================
echo.

if not exist "node_modules\vite\package.json" (
  echo [1/3] Installing npm dependencies ^(first run only^)...
  call npm install
  if errorlevel 1 (
    echo.
    echo npm install FAILED. Check your network, or run "npm ci" manually.
    pause
    exit /b 1
  )
) else (
  echo [1/3] npm dependencies OK.
)

echo [2/3] Freeing dev port 1420...
for /f "tokens=5" %%a in ('netstat -ano ^| findstr :1420 ^| findstr LISTENING') do (
  echo       killing PID %%a
  taskkill /F /PID %%a >nul 2>&1
)
timeout /t 1 /nobreak >nul

echo [3/3] Starting dev mode ^(the help doc is rebuilt automatically^).
echo.
echo   First run after a "cargo clean" recompiles all Rust dependencies and
echo   can take 5-15 minutes. Later starts take only a few seconds.
echo   Close this window or press Ctrl+C to stop.
echo.

call npm run tauri -- dev

echo.
echo Dev mode stopped.
pause
