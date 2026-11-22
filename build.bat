@echo off
setlocal
cd /d "%~dp0"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

where cargo >nul 2>nul
if errorlevel 1 (
  echo [ERROR] Rust/cargo not found. Install: winget install Rustlang.Rustup
  pause
  exit /b 1
)

rem Keep personal paths out of the binary (panic locations from Rust and deps).
if "%UPIZY_NOREMAP%"=="" (
  set "RUSTFLAGS=--remap-path-prefix=%USERPROFILE%=C:\src"
)

echo Building UPizy (release)...
cargo build --release
if errorlevel 1 (
  echo [ERROR] Build failed.
  pause
  exit /b 1
)

copy /y "target\release\upizy.exe" "UPizy.exe" >nul
copy /y "target\release\upizy-cli.exe" "UPizy-cli.exe" >nul

echo.
echo Build OK:
echo   UPizy.exe      - GUI
echo   UPizy-cli.exe  - console
echo.
echo Optional: run UPizy.exe and click "Download AI" (Real-ESRGAN, ~45 MB)
echo Optional: click "Download RIFE" for frame interpolation (~410 MB)
pause
