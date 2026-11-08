@echo off
REM Double-click to open the editor (builds first if code changed).
cd /d "%~dp0"
"%USERPROFILE%\.cargo\bin\cargo.exe" run --release -- %*
if errorlevel 1 pause
