@echo off
rem Drop video files onto this .bat to open them in UPizy.
setlocal
cd /d "%~dp0"
if not exist "UPizy.exe" (
  echo Build first: build.bat
  pause
  exit /b 1
)
start "" "UPizy.exe" %*
