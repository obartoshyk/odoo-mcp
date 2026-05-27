# odoo-mcp

Odoo CLI + MCP server in a single binary. Query Odoo from the terminal or expose it as tools for Claude AI.

## Features

- **CLI** — `search-read`, `search`, `read`, `fields-get`, `execute-kw`, `http`, `print-report`
- **MCP server** — JSON-RPC 2.0 over stdio; Claude can query models, read source code, call methods
- **Auto-pagination** — `search` and `search-read` fetch all pages in chunks of 300 automatically
- **Safe mode** — `execute-kw` is blocked by default per profile; enable explicitly per profile
- **Web session auth** — session cookie cached per profile (`~/.config/odoo-mcp/sessions/`)
- **PDF reports** — `print-report` downloads PDFs via Odoo web session with auto re-auth
- **mTLS** — client certificate + key for mutual TLS connections
- **No system git** — source management uses [gitoxide](https://github.com/Byron/gitoxide) (no `git` binary required on Windows)
- **No system OpenSSL** — TLS via `rustls`

## Install

### Pre-built binaries

Download from [Releases](https://github.com/obartoshyk/odoo-mcp/releases) for Linux x86_64, macOS (Intel / Apple Silicon), or Windows.

### From source

```bash
cargo install --git https://github.com/obartoshyk/odoo-mcp
```

or clone and build:

```bash
git clone https://github.com/obartoshyk/odoo-mcp
cd odoo-mcp
cargo install --path .
```

## Quick start

```bash
# Create initial config
odoo-mcp init

# Edit ~/.config/odoo-mcp/config.yaml, then smoke-test
odoo-mcp auth

# Query
odoo-mcp search-read --model account.move \
  --domain '[["state","=","posted"]]' \
  --fields id,name,amount_total \
  --limit 10 --order "id desc"

# Download an invoice PDF
odoo-mcp print-report --report account.report_invoice --ids 12345 -o invoice.pdf
```

## Config file

Default location:

| OS | Path |
|----|------|
| Linux | `~/.config/odoo-mcp/config.yaml` |
| macOS | `~/Library/Application Support/odoo-mcp/config.yaml` |
| Windows | `%APPDATA%\odoo-mcp\config.yaml` |

```yaml
default: production

connections:
  production:
    url: https://odoo.example.com
    db: mydb
    username: admin
    password: "api-key-or-password"
    # safe_mode: true  # default — execute-kw is blocked
    ext_url: https://ext-odoo.example.com  # for --ext unauthenticated requests
    cert: /path/to/client.crt              # mTLS (optional)
    key:  /path/to/client.key

  local:
    url: http://localhost:8069
    db: odoo
    username: admin
    password: admin
    safe_mode: false  # allow execute-kw on dev
```

Override via env vars: `ODOO_URL`, `ODOO_DB`, `ODOO_USERNAME`, `ODOO_PASSWORD`, `ODOO_CERT`, `ODOO_KEY`, `ODOO_PROFILE`, `ODOO_CONFIG`.

## CLI commands

| Command | Description |
|---------|-------------|
| `init` | Create config from template |
| `auth` | Smoke-test authentication |
| `search-read` | Search + read records (auto-paginated) |
| `search` | Return record IDs (auto-paginated) |
| `search-count` | Count matching records |
| `read` | Read records by IDs |
| `fields-get` | Field definitions for a model |
| `execute-kw` | Call any model method *(unsafe mode only)* |
| `http` | Raw HTTP request with session auth |
| `print-report` | Download PDF report |
| `serve` | Start MCP server over stdio |
| `update-sources` | Pull / clone git source trees |
| `config list/show/set/remove/default` | Manage connection profiles |

### Safe mode

`execute-kw` is blocked by default. Enable per profile:

```bash
odoo-mcp config set --profile local --safe-mode false
```

### Examples

```bash
# Count posted invoices
odoo-mcp search-count --model account.move --domain '[["state","=","posted"]]'

# Read specific records
odoo-mcp read --model res.partner --ids '[1,2,3]' --fields id,name,email

# Discover fields on a model
odoo-mcp fields-get --model account.move --attributes string,type,required

# Raw HTTP with session cookie
odoo-mcp http GET /web/health
odoo-mcp http POST /api/v2/endpoint --body '{"key":"value"}'

# Save binary response
odoo-mcp http GET /report/pdf/account.report_invoice/123 -o invoice.pdf

# Execute a method (safe_mode: false required)
odoo-mcp execute-kw --model account.move --method action_post --args '[[123]]'
```

## MCP server

Starts a JSON-RPC 2.0 server over **stdio** for use with Claude Desktop or Claude Code.

### Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)  
`%APPDATA%\Claude\claude_desktop_config.json` (Windows)

```json
{
  "mcpServers": {
    "odoo": {
      "command": "odoo-mcp",
      "args": ["--profile", "production", "serve"]
    }
  }
}
```

### Claude Code

`~/.claude/settings.json`:

```json
{
  "mcpServers": {
    "odoo": {
      "command": "odoo-mcp",
      "args": ["--profile", "production", "serve"]
    }
  }
}
```

### Available MCP tools

| Tool | Description |
|------|-------------|
| `odoo_search_read` | Search and read records (auto-paginated) |
| `odoo_search` | Return record IDs (auto-paginated) |
| `odoo_search_count` | Count matching records |
| `odoo_read` | Read records by IDs |
| `odoo_fields_get` | Field definitions |
| `odoo_execute_kw` | Call any model method *(hidden in safe mode)* |
| `odoo_http` | Raw HTTP request |
| `odoo_list_addons` | List all Odoo addons from source trees |
| `odoo_addon_structure` | Structural overview of an addon |
| `odoo_model_source` | Full Python source for a model |
| `odoo_search_source` | Search across Python source files |
| `odoo_update_sources` | Pull / clone configured git source trees |

## Source trees (for code tools)

Configure git repos per profile — Claude can read Python source without extra API calls:

```yaml
connections:
  production:
    sources:
      - path: /opt/odoo/addons/custom
        origin: git@github.com:your-org/odoo-addons.git
        branch: main
        ssh_key: /home/user/.ssh/id_ed25519
        update_on_serve: true   # pull on every `serve` start

      - path: /opt/odoo/src/odoo/addons
        origin: https://github.com/odoo/odoo.git
        branch: "16.0"
        update_on_serve: false  # update manually: odoo-mcp update-sources
```

SSH key auth and HTTPS token auth (`token: ghp_xxx`) are supported. No system `git` required.

## License

MIT
