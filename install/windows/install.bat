@echo off
chcp 65001 >nul
echo.
echo  odoo-mcp installer
echo  ──────────────────
echo.

where powershell >nul 2>&1
if errorlevel 1 (
    echo  ERROR: PowerShell not found. Please install PowerShell.
    pause
    exit /b 1
)

powershell -ExecutionPolicy Bypass -File "%~dp0install.ps1"

pause
