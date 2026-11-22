@echo off
setlocal
cd /d "%~dp0"

if not exist "UPizy.exe" (
  echo UPizy.exe not found, building first...
  call build.bat
  if not exist "UPizy.exe" exit /b 1
)

start "" "UPizy.exe" %*
