@echo off
rem Install or update Facet on this PC from this folder's source: double-click this file.
rem Builds the release exe and the installer (a few minutes), then runs the installer.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-installer.ps1" -Install
if errorlevel 1 (
    echo.
    echo Facet was NOT updated - see the error above.
    pause
    exit /b 1
)
