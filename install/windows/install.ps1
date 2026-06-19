#Requires -Version 5.1
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$REPO       = "obartoshyk/odoo-mcp"
$INSTALL_DIR = "$env:LOCALAPPDATA\Programs\odoo-mcp"
$CLAUDE_CONFIG = "$env:APPDATA\Claude\claude_desktop_config.json"

function Write-Step  { param($msg) Write-Host "▶ $msg" -ForegroundColor Green }
function Write-Warn  { param($msg) Write-Host "⚠  $msg" -ForegroundColor Yellow }
function Write-Ok    { param($msg) Write-Host "✓ $msg"  -ForegroundColor Green }
function Ask         { param($prompt) Write-Host $prompt -ForegroundColor White -NoNewline; Read-Host " " }
function AskSecret   { param($prompt) Read-Host -Prompt $prompt -AsSecureString }

Write-Host ""
Write-Host "odoo-mcp installer" -ForegroundColor White
Write-Host "────────────────────────────────────────"

# ── 1. download latest release ──────────────────────────────────────────────
$ASSET = "odoo-mcp-x86_64-pc-windows-msvc.exe"
Write-Step "Fetching latest release from github.com/$REPO ..."

$release = Invoke-RestMethod "https://api.github.com/repos/$REPO/releases/latest"
$asset_url = ($release.assets | Where-Object { $_.name -eq $ASSET } | Select-Object -First 1).browser_download_url

if (-not $asset_url) {
    Write-Host "Could not find release asset $ASSET" -ForegroundColor Red; exit 1
}

New-Item -ItemType Directory -Force -Path $INSTALL_DIR | Out-Null
$exe = "$INSTALL_DIR\odoo-mcp.exe"

Write-Step "Downloading $asset_url ..."
Invoke-WebRequest -Uri $asset_url -OutFile $exe
Write-Ok "Installed to $exe"

# ── 2. add to user PATH ──────────────────────────────────────────────────────
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($userPath -notlike "*$INSTALL_DIR*") {
    [Environment]::SetEnvironmentVariable("PATH", "$INSTALL_DIR;$userPath", "User")
    $env:PATH = "$INSTALL_DIR;$env:PATH"
    Write-Warn "Added $INSTALL_DIR to user PATH (restart terminal to pick up)"
}

# ── 3. collect connection details ────────────────────────────────────────────
Write-Host ""
Write-Host "Connection setup" -ForegroundColor White
Write-Host "────────────────────────────────────────"

$PROFILE = Ask "Profile name (default: sales)"
if (-not $PROFILE) { $PROFILE = "sales" }

$ODOO_URL = ""
while (-not $ODOO_URL) {
    $ODOO_URL = Ask "Odoo URL (e.g. https://odoo.gurtam.team)"
    if (-not $ODOO_URL) { Write-Warn "URL is required." }
}

$ODOO_DB = Ask "Database name (default: odoo)"
if (-not $ODOO_DB) { $ODOO_DB = "odoo" }

$ODOO_USER = ""
while (-not $ODOO_USER) {
    $ODOO_USER = Ask "Username / email"
    if (-not $ODOO_USER) { Write-Warn "Username is required." }
}

$secPass = AskSecret "Password or API key"
$ODOO_PASS = [Runtime.InteropServices.Marshal]::PtrToStringAuto(
    [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secPass))

$ODOO_EXT_URL = Ask "External (public) URL, e.g. https://ext-odoo.gurtam.team (leave blank to skip)"

$makeDefault = Ask "Make '$PROFILE' the default profile? [Y/n]"
$makeDefault = if (-not $makeDefault) { "Y" } else { $makeDefault }

# ── 4. write config ──────────────────────────────────────────────────────────
Write-Step "Writing config..."

$yaml = "url: $ODOO_URL`ndb: $ODOO_DB`nusername: $ODOO_USER`npassword: '$ODOO_PASS'"
if ($ODOO_EXT_URL) { $yaml += "`next_url: $ODOO_EXT_URL" }

$configArgs = @("config", "set", "--profile", $PROFILE, "--yaml", $yaml)
if ($makeDefault -match "^[Yy]") { $configArgs += "--default" }

& $exe @configArgs
Write-Ok "Config saved to $env:APPDATA\odoo-mcp\config.yaml"

# ── 5. verify connection ──────────────────────────────────────────────────────
Write-Step "Testing connection to Odoo..."
& $exe --profile $PROFILE auth
if ($LASTEXITCODE -ne 0) {
    Write-Host "Connection failed. Check URL / credentials and re-run." -ForegroundColor Red; exit 1
}
Write-Ok "Connection OK"

# ── 6. Claude Desktop config ──────────────────────────────────────────────────
Write-Host ""
Write-Host "Claude Desktop" -ForegroundColor White
Write-Host "────────────────────────────────────────"

if (-not (Test-Path $CLAUDE_CONFIG)) {
    $create = Ask "Claude Desktop config not found. Create it? [Y/n]"
    if (-not $create -or $create -match "^[Yy]") {
        New-Item -ItemType Directory -Force -Path (Split-Path $CLAUDE_CONFIG) | Out-Null
        '{"mcpServers": {}}' | Set-Content $CLAUDE_CONFIG -Encoding UTF8
    }
}

if (Test-Path $CLAUDE_CONFIG) {
    $content = Get-Content $CLAUDE_CONFIG -Raw
    if ($content -like "*odoo-mcp*") {
        Write-Warn "odoo-mcp already present in Claude Desktop config — skipping."
    } else {
        $add = Ask "Add odoo-mcp to Claude Desktop? [Y/n]"
        if (-not $add -or $add -match "^[Yy]") {
            $cfg = $content | ConvertFrom-Json
            if (-not $cfg.mcpServers) {
                $cfg | Add-Member -NotePropertyName "mcpServers" -NotePropertyValue ([PSCustomObject]@{})
            }
            $cfg.mcpServers | Add-Member -NotePropertyName "odoo-mcp" -NotePropertyValue ([PSCustomObject]@{
                command = $exe
                args    = @("--profile", $PROFILE, "serve")
            }) -Force
            $cfg | ConvertTo-Json -Depth 10 | Set-Content $CLAUDE_CONFIG -Encoding UTF8
            Write-Ok "Claude Desktop config updated"
            Write-Warn "Restart Claude Desktop for changes to take effect"
        }
    }
}

# ── done ──────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Done!" -ForegroundColor Green
Write-Host ""
Write-Host "Commands:"
Write-Host "  odoo-mcp auth                      — test connection"
Write-Host "  odoo-mcp --profile $PROFILE serve  — start MCP server"
Write-Host "  odoo-mcp config list               — show profiles"
Write-Host ""
