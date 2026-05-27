# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Odoo XML-RPC CLI connector. Outputs JSON to stdout. Reads credentials from a YAML config file, env vars, or CLI flags (in that priority order, CLI wins). Supports mTLS (client cert+key).

## Build & Install

```bash
cargo build --release
# binary: target/release/odoo-connector

# Install to ~/.cargo/bin/ (then available as `odoo-connector` anywhere):
cargo install --path .
```

## Config file

Default path (per OS):

| OS | Path |
|----|------|
| Linux | `~/.config/odoo-connector/config.yaml` |
| macOS | `~/Library/Application Support/odoo-connector/config.yaml` |
| Windows | `%APPDATA%\odoo-connector\config.yaml` |

```yaml
default: production   # profile used when --profile is not specified

connections:
  production:
    url: https://odoo.gurtam.team
    db: odoo
    username: admin
    password: "secret"
    ext_url: https://ext-odoo.gurtam.team  # optional: public URL, no auth required
    cert: /tmp/crt/client.crt   # optional, for mTLS
    key: /tmp/crt/client.key

  dev:
    url: http://localhost:8069
    db: odoo
    username: admin
    password: admin
```

Override path via `--config /path/to/custom.yaml` or `ODOO_CONFIG` env var.

## Usage

Priority: **CLI flag > env var > config file**.

```bash
# Smoke test — prints uid, db, url
odoo-connector auth

# Use a specific profile
odoo-connector --profile dev auth

# search_read
odoo-connector search-read \
  --model account.move \
  --domain '[["state","=","posted"],["partner_id","=",1234]]' \
  --fields id,name,amount_total,payment_state \
  --limit 10 \
  --order "id desc"

# execute_kw — any model/method
odoo-connector execute-kw \
  --model account.move \
  --method read \
  --args '[[7074]]' \
  --kwargs '{"fields": ["name","credit","agreement_currency_id"]}'

# All flags explicit (no config file)
odoo-connector \
  --url https://odoo.gurtam.team --db odoo \
  --username admin --password $PASS \
  --cert /tmp/crt/client.crt --key /tmp/crt/client.key \
  search-read --model res.partner --fields id,name,email --limit 5
```

### Direct HTTP (`http` subcommand)

Bypasses XML-RPC entirely — sends a raw HTTP request to any Odoo path. Response is pretty-printed if JSON, raw text otherwise. Auth is never performed.

```bash
# GET any endpoint (uses mTLS client from the active profile)
odoo-connector http GET /web/health

# POST with a JSON body (Content-Type: application/json by default)
odoo-connector http POST /web/dataset/call_kw \
  --body '{"jsonrpc":"2.0","method":"call","id":1,"params":{"model":"res.partner","method":"search_read","args":[[]],"kwargs":{"fields":["id","name"],"limit":5}}}'

# Custom Content-Type and extra headers
odoo-connector http POST /some/form/endpoint \
  --content-type "application/x-www-form-urlencoded" \
  --body "param1=value1&param2=value2" \
  --header "X-Custom-Token:abc123"

# Multiple extra headers
odoo-connector http GET /api/v2/resource \
  --header "Authorization:Bearer $TOKEN" \
  --header "Accept:application/json"
```

### ext-odoo — unauthenticated public endpoints

`--ext` switches the base URL to `ext_url` from the config and skips authentication entirely. Intended for endpoints exposed on the public `ext-odoo` address that require no credentials.

```bash
# GET a public endpoint using ext_url from the active config profile
odoo-connector --ext http GET /api/v2/public/ping

# POST to a public endpoint
odoo-connector --ext http POST /api/v2/webhook \
  --body '{"event":"test"}'

# Pass ext-url inline without a config file
odoo-connector --ext-url https://ext-odoo.gurtam.team --ext \
  http GET /api/v2/public/status

# Use a non-default profile that has ext_url configured
odoo-connector --profile staging --ext http GET /api/v2/public/info

# With extra headers (e.g. a shared secret for semi-public endpoints)
odoo-connector --ext http GET /api/v2/internal/report \
  --header "X-Api-Key:$EXT_KEY"
```

Env vars: `ODOO_URL`, `ODOO_DB`, `ODOO_USERNAME`, `ODOO_PASSWORD`, `ODOO_CERT`, `ODOO_KEY`, `ODOO_PROFILE`, `ODOO_CONFIG`, `ODOO_EXT_URL`.

## Architecture

- `src/lib.rs` — `Value` enum (XML-RPC types), XML serialization/deserialization (via `roxmltree`), base64 codec, `OdooClient` struct (with `http_request()` for direct HTTP)
- `src/main.rs` — CLI (`clap` derive) with subcommands: `auth`, `search-read`, `execute-kw`, `http`

XML-RPC protocol is implemented directly (no external xmlrpc crate). TLS via `rustls` (pure Rust, no system OpenSSL dependency).

## Odoo XML-RPC endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/xmlrpc/2/common` | `authenticate` | returns uid |
| `/xmlrpc/2/object` | `execute_kw` | all ORM calls |

## Dependencies

| Crate | Purpose |
|-------|---------|
| `reqwest` (blocking + rustls-tls) | HTTP with mTLS support |
| `roxmltree` | XML response parsing |
| `serde` + `serde_yaml` | Config file deserialization |
| `serde_json` | JSON output |
| `clap` (derive + env) | CLI |
| `anyhow` | Error handling |
