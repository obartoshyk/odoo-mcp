# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Odoo XML-RPC CLI connector. Outputs JSON to stdout. Reads credentials from a YAML config file, env vars, or CLI flags (in that priority order, CLI wins). Supports mTLS (client cert+key).

## Build & Install

```bash
cargo build --release
# binary: target/release/odoo-xml-rpc

# Install to ~/.cargo/bin/ (then available as `odoo-xml-rpc` anywhere):
cargo install --path .
```

## Config file

Default path: `~/.config/odoo-xml-rpc/config.yaml`

```yaml
default: production   # profile used when --profile is not specified

connections:
  production:
    url: https://odoo.gurtam.team
    db: odoo
    username: admin
    password: "secret"
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
odoo-xml-rpc auth

# Use a specific profile
odoo-xml-rpc --profile dev auth

# search_read
odoo-xml-rpc search-read \
  --model account.move \
  --domain '[["state","=","posted"],["partner_id","=",1234]]' \
  --fields id,name,amount_total,payment_state \
  --limit 10 \
  --order "id desc"

# execute_kw — any model/method
odoo-xml-rpc execute-kw \
  --model account.move \
  --method read \
  --args '[[7074]]' \
  --kwargs '{"fields": ["name","credit","agreement_currency_id"]}'

# All flags explicit (no config file)
odoo-xml-rpc \
  --url https://odoo.gurtam.team --db odoo \
  --username admin --password $PASS \
  --cert /tmp/crt/client.crt --key /tmp/crt/client.key \
  search-read --model res.partner --fields id,name,email --limit 5
```

Env vars: `ODOO_URL`, `ODOO_DB`, `ODOO_USERNAME`, `ODOO_PASSWORD`, `ODOO_CERT`, `ODOO_KEY`, `ODOO_PROFILE`, `ODOO_CONFIG`.

## Architecture

- `src/lib.rs` — `Value` enum (XML-RPC types), XML serialization/deserialization (via `roxmltree`), base64 codec, `OdooClient` struct
- `src/main.rs` — CLI (`clap` derive) with subcommands: `auth`, `search-read`, `execute-kw`

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
