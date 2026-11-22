@echo off
setlocal
cd /d "%~dp0"

if exist "UPizy-cli.exe" (
  "UPizy-cli.exe" --install-ai
) else if exist "target\release\upizy-cli.exe" (
  "target\release\upizy-cli.exe" --install-ai
) else (
  echo UPizy-cli.exe not found. Run build.bat first.
)

pause
