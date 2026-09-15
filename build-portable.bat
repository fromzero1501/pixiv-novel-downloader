@echo off
rem Keep this file ASCII-only: cmd.exe reads .bat as GBK, non-ASCII text turns into mojibake.
cd /d "%~dp0"
setlocal

for /f "delims=" %%v in ('node -p "require('./package.json').version"') do set VERSION=%%v
echo ==========================================
echo  Portable Build  -  version %VERSION%
echo ==========================================
echo First run may take a few minutes for Rust release compilation.
echo.

call npm run portable
if errorlevel 1 (
  echo.
  echo Build FAILED.
  pause
  exit /b 1
)

echo.
echo ==========================================
echo  Done: PixivNovelDownloader\PixivNovelDownloader-v%VERSION%.exe
echo  (in the release output folder next to this project)
echo  Portable: double-click to run, no install.
echo  App data lives in the "data" folder next to the exe.
echo ==========================================
pause
