#!/usr/bin/env bash
set -e

REPO="obartoshyk/odoo-mcp"
INSTALL_DIR="$HOME/.local/bin"
CLAUDE_CONFIG="$HOME/.config/Claude/claude_desktop_config.json"

# ── colors ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BOLD='\033[1m'; NC='\033[0m'
info()    { echo -e "${GREEN}▶${NC} $*"; }
warn()    { echo -e "${YELLOW}⚠${NC}  $*"; }
prompt()  { echo -e "${BOLD}$*${NC}"; }
success() { echo -e "${GREEN}✓${NC} $*"; }

echo ""
echo -e "${BOLD}odoo-mcp installer${NC}"
echo "────────────────────────────────────────"

# ── 1. detect arch ──────────────────────────────────────────────────────────
ARCH=$(uname -m)
if [ "$ARCH" = "x86_64" ]; then
    ASSET="odoo-mcp-x86_64-unknown-linux-gnu"
else
    echo -e "${RED}Unsupported architecture: $ARCH (only x86_64 is available)${NC}"; exit 1
fi
info "Architecture: $ARCH → $ASSET"

# ── 2. download latest release ──────────────────────────────────────────────
info "Fetching latest release from github.com/$REPO ..."
LATEST_URL=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep "browser_download_url" \
    | grep "$ASSET" \
    | head -1 \
    | cut -d'"' -f4)

if [ -z "$LATEST_URL" ]; then
    echo -e "${RED}Could not find release asset $ASSET${NC}"; exit 1
fi

mkdir -p "$INSTALL_DIR"
info "Downloading $LATEST_URL ..."
curl -fsSL "$LATEST_URL" -o "$INSTALL_DIR/odoo-mcp"
chmod +x "$INSTALL_DIR/odoo-mcp"
success "Installed to $INSTALL_DIR/odoo-mcp"

# ── 3. ensure $INSTALL_DIR is in PATH ───────────────────────────────────────
SHELL_RC="$HOME/.bashrc"
[ "$(basename "$SHELL")" = "zsh" ] && SHELL_RC="$HOME/.zshrc"

if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
    echo "" >> "$SHELL_RC"
    echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$SHELL_RC"
    export PATH="$INSTALL_DIR:$PATH"
    warn "Added $INSTALL_DIR to PATH in $SHELL_RC"
fi

# ── 4. collect connection details ────────────────────────────────────────────
[ -t 0 ] || exec < /dev/tty

echo ""
echo -e "${BOLD}Connection setup${NC}"
echo "────────────────────────────────────────"

prompt "Profile name (default: sales): "
read -r PROFILE
PROFILE="${PROFILE:-sales}"

prompt "Odoo URL (e.g. https://odoo.gurtam.team): "
read -r ODOO_URL
while [ -z "$ODOO_URL" ]; do
    warn "URL is required."
    read -r ODOO_URL
done

prompt "Database name (default: odoo): "
read -r ODOO_DB
ODOO_DB="${ODOO_DB:-odoo}"

prompt "Username / email: "
read -r ODOO_USER
while [ -z "$ODOO_USER" ]; do
    warn "Username is required."
    read -r ODOO_USER
done

prompt "Password or API key: "
read -rs ODOO_PASS
echo ""
while [ -z "$ODOO_PASS" ]; do
    warn "Password is required."
    read -rs ODOO_PASS
    echo ""
done

prompt "External (public) URL, e.g. https://ext-odoo.gurtam.team (leave blank to skip): "
read -r ODOO_EXT_URL

prompt "Make '$PROFILE' the default profile? [Y/n]: "
read -r MAKE_DEFAULT
MAKE_DEFAULT="${MAKE_DEFAULT:-Y}"

# ── 5. write config ──────────────────────────────────────────────────────────
info "Writing config..."
EXT_LINE=""
[ -n "$ODOO_EXT_URL" ] && EXT_LINE="
ext_url: $ODOO_EXT_URL"

"$INSTALL_DIR/odoo-mcp" config set \
    --profile "$PROFILE" \
    --yaml "url: $ODOO_URL
db: $ODOO_DB
username: $ODOO_USER
password: $(printf '%s' "$ODOO_PASS" | sed "s/'/\\'\\'/g")$EXT_LINE" \
    $([[ "$MAKE_DEFAULT" =~ ^[Yy] ]] && echo "--default")

success "Config saved to ~/.config/odoo-mcp/config.yaml"

# ── 6. verify connection ──────────────────────────────────────────────────────
info "Testing connection to Odoo..."
if "$INSTALL_DIR/odoo-mcp" --profile "$PROFILE" auth; then
    success "Connection OK"
else
    echo -e "${RED}Connection failed. Check URL / credentials and re-run.${NC}"
    exit 1
fi

# ── 7. Claude Desktop config ──────────────────────────────────────────────────
echo ""
echo -e "${BOLD}Claude Desktop${NC}"
echo "────────────────────────────────────────"

if [ ! -f "$CLAUDE_CONFIG" ]; then
    prompt "Claude Desktop config not found at $CLAUDE_CONFIG"
    prompt "Create it? [Y/n]: "
    read -r CREATE_CLAUDE
    CREATE_CLAUDE="${CREATE_CLAUDE:-Y}"
    if [[ "$CREATE_CLAUDE" =~ ^[Yy] ]]; then
        mkdir -p "$(dirname "$CLAUDE_CONFIG")"
        echo '{"mcpServers": {}}' > "$CLAUDE_CONFIG"
    fi
fi

if [ -f "$CLAUDE_CONFIG" ]; then
    if grep -q "odoo-mcp" "$CLAUDE_CONFIG" 2>/dev/null; then
        warn "odoo-mcp already present in Claude Desktop config — skipping."
    else
        prompt "Add odoo-mcp to Claude Desktop? [Y/n]: "
        read -r ADD_CLAUDE
        ADD_CLAUDE="${ADD_CLAUDE:-Y}"
        if [[ "$ADD_CLAUDE" =~ ^[Yy] ]]; then
            python3 - <<PYEOF
import json
path = "$CLAUDE_CONFIG"
with open(path) as f:
    cfg = json.load(f)
cfg.setdefault("mcpServers", {})["odoo-mcp"] = {
    "command": "$INSTALL_DIR/odoo-mcp",
    "args": ["--profile", "$PROFILE", "serve"]
}
with open(path, "w") as f:
    json.dump(cfg, f, indent=2)
    f.write("\n")
print("  Written: " + path)
PYEOF
            success "Claude Desktop config updated"
            warn "Restart Claude Desktop for changes to take effect"
        fi
    fi
fi

# ── done ──────────────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}${BOLD}Done!${NC}"
echo ""
echo "Commands:"
echo "  odoo-mcp auth                        — test connection"
echo "  odoo-mcp --profile $PROFILE serve    — start MCP server"
echo "  odoo-mcp config list                 — show profiles"
echo ""
