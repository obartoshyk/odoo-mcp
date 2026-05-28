# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Odoo CLI + MCP server. Two roles in one binary:

- **CLI** — query Odoo from the terminal via JSON-RPC (`search-read`, `execute-kw`) or raw HTTP
- **MCP server** — expose Odoo data and source code as tools for Claude (`serve`)

Reads credentials from a YAML config file, env vars, or CLI flags (priority: CLI > env > config). Supports mTLS.

## Build & Install

```bash
cargo build --release
# binary: target/release/odoo-mcp

# Install to ~/.cargo/bin/:
cargo install --path .
```

## Config file

Default path per OS:

| OS      | Path |
|---------|------|
| Linux   | `~/.config/odoo-mcp/config.yaml` |
| macOS   | `~/Library/Application Support/odoo-mcp/config.yaml` |
| Windows | `%APPDATA%\odoo-mcp\config.yaml` |

Override via `--config /path/to/file.yaml` or `ODOO_CONFIG` env var.

```yaml
default: sales   # profile used when --profile is omitted

connections:
  sales:
    url: https://odoo.gurtam.team
    db: gurtam
    username: user@example.com
    password: "secret"
    ext_url: https://ext-odoo.gurtam.team   # public URL, no auth
    cert: /path/to/client.crt               # mTLS (optional)
    key:  /path/to/client.key

    # Git source trees for odoo_model_source / odoo_search_source MCP tools.
    # Sources are per-profile: different profiles can point to different branches.
    sources:
      - path: /home/user/projects/odoo/git/odoo/addons
        origin: https://github.com/odoo/odoo.git
        branch: "16.0"
        update_on_serve: false      # heavy; update manually

      - path: /home/user/projects/odoo/addons/gt
        origin: git@github.com:your-org/odoo-addons.git
        branch: main
        ssh_key: /home/user/.ssh/id_ed25519
        update_on_serve: true       # pull on every `serve` start

      - path: /home/user/projects/odoo/addons/oca
        origin: https://github.com/OCA/account-financial-tools.git
        branch: "16.0"
        # token: ghp_xxxx           # GitHub PAT for private HTTPS repos
        update_on_serve: false

  local:
    url: http://localhost:8069
    db: odoo
    username: admin
    password: admin
    safe_mode: false   # allow execute-kw on dev
```

### Connection config fields

| Field | Default | Description |
|-------|---------|-------------|
| `url` | — | Odoo base URL |
| `db` | `odoo` | Database name |
| `username` | `admin` | Login |
| `password` | — | Password or API key |
| `ext_url` | — | Public URL for unauthenticated `--ext` requests |
| `cert` | — | mTLS client certificate path |
| `key` | — | mTLS private key path |
| `safe_mode` | `true` | When `true`, `execute-kw` is blocked. Set to `false` to allow write operations. |

### Source config fields

| Field | Default | Description |
|-------|---------|-------------|
| `path` | — | Local directory (will be created on clone) |
| `origin` | — | Git remote URL (SSH or HTTPS); required for auto-clone |
| `branch` | `main` | Branch to track |
| `ssh_key` | — | Path to SSH private key |
| `token` | — | Bearer token for HTTPS auth (GitHub PAT etc.) |
| `update_on_serve` | `false` | Pull automatically when `serve` starts |

---

## CLI subcommands

### `init` — create initial config

Creates the config file from a built-in template if it does not already exist. Safe to run on an existing setup — will not overwrite.

```bash
odoo-mcp init
# Created: ~/.config/odoo-mcp/config.yaml
# Edit the config, then run: odoo-mcp auth

# Already exists — no-op:
# Config already exists: ~/.config/odoo-mcp/config.yaml
```

### `config` — manage connection profiles

All subcommands read/write the config file. Passwords are never stored in `config.yaml`.

#### `config set` — create or update a profile (declarative YAML merge)

`config set` accepts a YAML patch: only the fields you provide overwrite the profile; everything else is preserved. The YAML mirrors the structure of a single connection entry from `config.yaml`.

```bash
# Create a profile from inline YAML
odoo-mcp config set --profile sales \
  --yaml 'url: https://odoo.example.com
db: gurtam
username: admin@example.com
password: secret'

# Load profile from a file, make default
odoo-mcp config set --profile sales -f profile.yaml --default

# Patch a single field
odoo-mcp config set --profile sales --yaml 'url: https://new.odoo.example.com'

# Set password via flag (equivalent to --yaml 'password: ...')
odoo-mcp config set --profile sales --password "my-api-key"

# Read stdin (useful in scripts)
echo 'db: mydb' | odoo-mcp config set --profile sales -f -

# Enable / disable execute-kw
odoo-mcp config set --profile local --safe-mode false
odoo-mcp config set --profile local --safe-mode true
```

> All `ConnectionConfig` fields go in the YAML (`url`, `db`, `username`, `password`, `ext_url`, `cert`, `key`, `sources`, …). `--password`, `--safe-mode`, and `--default` are convenience flags for the most common one-liner changes.

#### Other config subcommands

```bash
# List all profiles
odoo-mcp config list
#   local                http://localhost:8069  ← default
#   sales                https://odoo.gurtam.team

# Show full config with secrets masked
odoo-mcp config show

# Change the default profile
odoo-mcp config default --profile local

# Remove a profile
odoo-mcp config remove --profile old
```

---

### `auth` — smoke test

```bash
odoo-mcp --profile sales auth
# → {"uid": 42, "db": "gurtam", "url": "https://...", "profile": "sales"}
```

### `search` — return record IDs

```bash
odoo-mcp search --model res.partner --domain '[["is_company","=",true]]' --limit 10
# → [4426, 17534, 17537, ...]
```

### `search-count` — count matching records

```bash
odoo-mcp search-count --model res.partner --domain '[["is_company","=",true]]'
# → 7239
```

### `search-read` — query records with fields

```bash
# Last 10 posted invoices
odoo-mcp search-read \
  --model account.move \
  --domain '[["move_type","=","out_invoice"],["state","=","posted"]]' \
  --fields id,name,partner_id,amount_total,invoice_date,payment_state \
  --limit 10 \
  --order "id desc"

# Partners without email
odoo-mcp search-read \
  --model res.partner \
  --domain '[["email","=",false],["is_company","=",true]]' \
  --fields id,name,phone
```

### `read` — read records by IDs

```bash
odoo-mcp read \
  --model account.move \
  --ids '[1070023,1070024]' \
  --fields id,name,amount_total,state
```

### `fields-get` — field definitions for a model

```bash
# All fields (type, label, required, readonly, relation)
odoo-mcp fields-get --model account.move

# Specific fields only
odoo-mcp fields-get --model account.move --fields name,partner_id,amount_total

# Custom attributes
odoo-mcp fields-get --model account.move \
  --attributes string,type,required,readonly,relation,help
```

### `execute-kw` — any model method

> **Safe mode:** `execute-kw` is blocked by default. Enable with:
> ```bash
> odoo-mcp config set --profile <name> --safe-mode false
> ```

```bash
# Read specific fields of one record
odoo-mcp execute-kw \
  --model account.move \
  --method read \
  --args '[[1070023]]' \
  --kwargs '{"fields": ["name","invoice_line_ids","amount_total"]}'

# Create a record
odoo-mcp execute-kw \
  --model res.partner \
  --method create \
  --args '[{"name": "Test Partner", "email": "test@example.com"}]'

# Call a custom method
odoo-mcp execute-kw \
  --model account.move \
  --method action_post \
  --args '[[1070023]]'

# Save binary result to file (-o / --output)
# If the result is a base64 string, it is decoded to raw bytes first.
# Otherwise the pretty-printed JSON is written.
odoo-mcp execute-kw \
  --model ir.attachment \
  --method read \
  --args '[[153996]]' \
  --kwargs '{"fields":["datas"]}' \
  -o /tmp/attachment.json

# Example: save a base64-encoded file stored in ir.attachment
odoo-mcp execute-kw \
  --model ir.attachment \
  --method read \
  --args '[[153996]]' \
  --kwargs '{"fields":["datas"]}' \
  -o /tmp/file.pdf
```

### `http` — direct HTTP request

Bypasses JSON-RPC. Auth is never performed. Response is pretty-printed if JSON.

```bash
# Health check
odoo-mcp http GET /web/health

# Odoo JSON-RPC web API
odoo-mcp http POST /web/dataset/call_kw \
  --body '{"jsonrpc":"2.0","method":"call","id":1,"params":{
    "model":"res.partner","method":"search_read",
    "args":[[]],"kwargs":{"fields":["id","name"],"limit":5}}}'

# Custom Content-Type
odoo-mcp http POST /some/form/endpoint \
  --content-type "application/x-www-form-urlencoded" \
  --body "key=value"

# Extra headers (repeatable)
odoo-mcp http GET /api/v2/resource \
  --header "Authorization:Bearer $TOKEN" \
  --header "X-Custom:value"

# Save raw response bytes to file (-o / --output)
# Useful for binary downloads (PDFs, images, exports).
# Requires session cookies for Odoo report endpoints.
odoo-mcp http GET /report/pdf/account.report_invoice/961403 \
  --header "Cookie:session_id=YOUR_SESSION" \
  -o /tmp/invoice.pdf
```

### `print-report` — download report PDF

Downloads a PDF report via the Odoo web session. Authenticates automatically and caches the session cookie per-profile in `~/.config/odoo-mcp/sessions/<profile>.txt`. On subsequent calls the cache is reused; if the session has expired, re-authentication is transparent.

```bash
# Single invoice
odoo-mcp --profile sales print-report \
  --report gt_billing.gt_invoice \
  --ids 1068747 \
  -o /tmp/invoice_1068747.pdf

# Multiple records in one PDF
odoo-mcp --profile sales print-report \
  --report gt_billing.gt_invoice \
  --ids 1068747,1068748,1068749

# Default output name: <report_suffix>_<ids>.pdf in current directory
# e.g. gt_invoice_1068747.pdf
odoo-mcp --profile sales print-report --report gt_billing.gt_invoice --ids 1068747
```

Session cache: `~/.config/odoo-mcp/sessions/<profile>.txt` (plain session_id, auto-refreshed on expiry).

### `--ext` — unauthenticated public endpoints

Switches base URL to `ext_url` from config and skips auth entirely.

```bash
# Use ext_url from active profile, no auth
odoo-mcp --ext http GET /api/v2/public/ping

# With body
odoo-mcp --ext http POST /api/v2/webhook \
  --body '{"event":"test"}'

# Inline ext-url without config
odoo-mcp --ext-url https://ext-odoo.gurtam.team --ext \
  http GET /api/v2/public/status
```

### `update-sources` — pull git source trees

Pull or clone all sources configured in the active profile. Fetch + hard reset to `origin/<branch>`. Does not require Odoo credentials. Does **not** require system `git` — uses the built-in [gitoxide](https://github.com/Byron/gitoxide) library.

```bash
odoo-mcp --profile sales update-sources
# ok  /home/user/projects/odoo/addons/gt (reset to origin/main)
# ok  /home/user/projects/odoo/addons/oca (reset to origin/16.0)
```

Env vars: `ODOO_URL`, `ODOO_DB`, `ODOO_USERNAME`, `ODOO_PASSWORD`, `ODOO_CERT`, `ODOO_KEY`, `ODOO_EXT_URL`, `ODOO_PROFILE`, `ODOO_CONFIG`.

---

## MCP server (`serve`)

Starts a JSON-RPC 2.0 MCP server over **stdio**. At startup: authenticates to Odoo, pulls sources with `update_on_serve: true`, then waits for requests from Claude.

Capabilities exposed:
- **`tools`** — all Odoo data and source-code tools (see below)
- **`resources`** — one resource: `odoo-mcp://docs` (full tool reference, embedded in the binary)

```bash
odoo-mcp --profile sales serve

# Ext mode — no auth, public endpoints only
odoo-mcp --profile sales --ext serve
```

### Claude Desktop config

macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`  
Windows: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "odoo": {
      "command": "odoo-mcp",
      "args": ["--profile", "sales", "serve"]
    }
  }
}
```

### Claude Code config

`.claude/settings.json` (project) or `~/.claude/settings.json` (global):

```json
{
  "mcpServers": {
    "odoo": {
      "command": "odoo-mcp",
      "args": ["--profile", "sales", "serve"]
    }
  }
}
```

---

## MCP tools reference

### Odoo data tools

#### `odoo_search`

Return IDs of records matching a domain. Use when you only need IDs, not field values.

| Argument | Required | Description |
|----------|----------|-------------|
| `model` | yes | Model technical name |
| `domain` | no | JSON domain (default: `[]`) |
| `limit` | no | Max IDs to return |
| `offset` | no | Records to skip |
| `order` | no | Sort, e.g. `id desc` |

```
odoo_search(model="res.partner", domain='[["is_company","=",true]]', limit=10)
# → [4426, 17534, 17537, ...]
```

#### `odoo_search_count`

Return the number of records matching a domain.

| Argument | Required | Description |
|----------|----------|-------------|
| `model` | yes | Model technical name |
| `domain` | no | JSON domain (default: `[]`) |

```
odoo_search_count(model="res.partner", domain='[["is_company","=",true]]')
# → 7239
```

#### `odoo_search_read`

Search and read records from any Odoo model.

| Argument | Required | Description |
|----------|----------|-------------|
| `model` | yes | Model technical name, e.g. `account.move` |
| `domain` | no | JSON domain, e.g. `[["state","=","posted"]]` (default: `[]`) |
| `fields` | no | Comma-separated fields, e.g. `id,name,amount_total` (default: `id,name`) |
| `limit` | no | Max records to return |
| `offset` | no | Records to skip (pagination) |
| `order` | no | Sort, e.g. `id desc` |

```
# Find last 5 unpaid invoices over €100
odoo_search_read(
  model="account.move",
  domain='[["move_type","=","out_invoice"],["payment_state","=","not_paid"],["amount_total",">",100]]',
  fields="id,name,partner_id,amount_total,invoice_date",
  limit=5,
  order="id desc"
)
```

#### `odoo_read`

Read specific records by IDs. Use when you already have IDs.

| Argument | Required | Description |
|----------|----------|-------------|
| `model` | yes | Model technical name |
| `ids` | yes | Record IDs as JSON array, e.g. `[1,2,3]` |
| `fields` | no | Comma-separated fields (default: `id,name`) |

```
odoo_read(model="account.move", ids="[1070023]", fields="id,name,invoice_line_ids,amount_total")
```

#### `odoo_fields_get`

Return field definitions for a model: type, label, required, readonly, relation target. Use this to discover available fields before building queries.

| Argument | Required | Description |
|----------|----------|-------------|
| `model` | yes | Model technical name |
| `fields` | no | Comma-separated field names to filter (omit for all fields) |
| `attributes` | no | Attributes to include (default: `string,type,required,readonly,relation`) |

```
# Discover all fields on account.move
odoo_fields_get(model="account.move")

# Check specific fields
odoo_fields_get(model="account.move", fields="partner_id,invoice_line_ids,amount_total")
```

#### `odoo_execute_kw`

Call any method on an Odoo model. **Not available in safe mode** (hidden from `tools/list`). Enable with `odoo-mcp config set --profile <name> --safe-mode false`.

| Argument | Required | Description |
|----------|----------|-------------|
| `model` | yes | Model technical name |
| `method` | yes | Method name, e.g. `write`, `create`, `unlink`, `action_post` |
| `args` | no | Positional args as JSON array (default: `[]`) |
| `kwargs` | no | Keyword args as JSON object (default: `{}`) |

```
# Post an invoice
odoo_execute_kw(model="account.move", method="action_post", args="[[1070023]]")

# Reset to draft
odoo_execute_kw(model="account.move", method="button_draft", args="[[1070023]]")
```

#### `odoo_http`

Direct HTTP request to any Odoo endpoint, bypassing JSON-RPC.

| Argument | Required | Description |
|----------|----------|-------------|
| `path` | yes | Server path, e.g. `/web/health` |
| `method` | no | HTTP method (default: `GET`) |
| `body` | no | Request body string |
| `content_type` | no | Content-Type (default: `application/json`) |

```
odoo_http(path="/web/health")
odoo_http(method="POST", path="/api/v2/invoices", body='{"partner_id":123}')
```

---

### Source code tools

These tools read Python source files from the configured git trees. No Odoo API calls are made.

#### `odoo_list_addons`

List all Odoo addons found across all source trees: name, version, summary, dependencies, path. **Use this first** to understand the overall application structure.

```
odoo_list_addons()
# → Found 1112 addons:
# ## gt_account (Custom Account)
#   version:  16.0.1.0.4
#   summary:  Gurtam Account customisations
#   depends:  gt_common, gt_agreement, account, ...
#   path:     /home/user/projects/odoo/addons/gt/gt_account
# ...
```

#### `odoo_addon_structure`

Structural overview of a specific addon without reading full source: models defined, models extended, HTTP routes, data files, security.

| Argument | Required | Description |
|----------|----------|-------------|
| `addon` | yes | Technical addon name (directory name), e.g. `gt_billing` |

```
odoo_addon_structure(addon="gt_account")
# → # gt_account — Custom Account
#   version: 16.0.1.0.4
#   depends: gt_common, gt_agreement, ...
#
#   ## Models defined
#     account.reasons  (Reasons)  [Model]  — models/account_reason.py:5
#     account.payment  (AccountPayment)  [Model]  — models/account_payment.py:5
#
#   ## Models inherited / extended
#     account.move  — models/account_move.py:8
#     account.move.line  — models/account_move.py:347
#     ...
#
#   ## Data files
#     data/account_data.xml
#     views/account_move_views.xml
#     ...
```

#### `odoo_model_source`

Return the full Python source of all files that define or inherit a model. Shows fields, computed fields, constraints, onchange handlers, and business methods.

| Argument | Required | Description |
|----------|----------|-------------|
| `model` | yes | Model technical name, e.g. `account.move` |

```
odoo_model_source(model="gt.billing.order")
# → # /home/user/projects/odoo/addons/gt/gt_billing/models/billing_order.py
#   class GtBillingOrder(models.Model):
#       _name = 'gt.billing.order'
#       ...all fields, methods, etc...
```

#### `odoo_search_source`

Search for any string across all Python source files. Use to find business logic, methods, field usages, routes, cron definitions — anything in the codebase.

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `query` | yes | — | Case-sensitive substring to find |
| `path_filter` | no | — | File path must contain this substring (e.g. `gt_billing`) |
| `context` | no | 5 | Lines of context around each match |
| `max_matches` | no | 30 | Result limit |

```
# Find all places where action_post is defined
odoo_search_source(query="def action_post", path_filter="gt_")

# Find HTTP routes in a specific addon
odoo_search_source(query="@http.route", path_filter="gt_billing")

# Find all cron job definitions
odoo_search_source(query="ir.cron", path_filter="gt_")

# Find field usage across the codebase
odoo_search_source(query="agreement_currency_id", context=3)
```

#### `odoo_update_sources`

Pull / clone all configured source repos (fetch + hard reset to `origin/<branch>`). Call this when you need fresh code. Does not require system `git` — uses the built-in gitoxide library.

```
odoo_update_sources()
# → ok  /home/user/projects/odoo/addons/gt (reset to origin/main)
#   ok  /home/user/projects/odoo/addons/oca (reset to origin/16.0)
```

---

### Typical Claude workflow

```
# 1. Understand the application
odoo_list_addons()

# 2. Drill into a specific module
odoo_addon_structure("gt_billing")

# 3. Read the model definition
odoo_model_source("gt.billing.order")

# 4. Find specific business logic
odoo_search_source("def action_confirm", path_filter="gt_billing")

# 5. Query live data
odoo_search_read(model="gt.billing.order", domain='[["state","=","draft"]]',
  fields="id,name,partner_id,amount_total", limit=10)

# 6. Call a method
odoo_execute_kw(model="gt.billing.order", method="action_confirm", args="[[42]]")
```

---

## Architecture

| File | Purpose |
|------|---------|
| `src/lib.rs` | `OdooClient`: JSON-RPC auth/execute_kw, direct HTTP |
| `src/main.rs` | CLI: `auth`, `search-read`, `execute-kw`, `http`, `print-report`, `serve`, `update-sources` |
| `src/mcp.rs` | MCP server: JSON-RPC 2.0 over stdio, tool dispatch |
| `src/sources.rs` | Git source management (gitoxide), file walker, addon/model introspection |

JSON-RPC over `/jsonrpc` endpoint (no XML-RPC). TLS via `rustls` (no system OpenSSL needed, works on Windows). Git via `gix` (no system git needed, works on Windows).

## Dependencies

| Crate | Purpose |
|-------|---------|
| `reqwest` (blocking + rustls-tls + json) | HTTP with mTLS and JSON body |
| `gix` (blocking-http-transport-reqwest-rust-tls + worktree-mutation) | Pure-Rust git: clone, fetch, hard reset, SSH key + HTTPS token auth |
| `serde` + `serde_yaml` | Config deserialization |
| `serde_json` | JSON-RPC, output, and MCP protocol |
| `base64` | Decode base64 binary fields when writing to file with `-o` |
| `clap` (derive + env) | CLI |
| `dirs` | OS-appropriate config path |
| `anyhow` | Error handling |
