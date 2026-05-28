use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value as Json};

use odoo_mcp::OdooClient;
use crate::sources::{self, SourceConfig, search_source, list_addons, addon_structure};

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run_server(odoo: OdooClient, srcs: Vec<SourceConfig>, safe_mode: bool, profile: &str, url: &str) -> Result<()> {
    // Auto-update sources marked with update_on_serve: true.
    let to_update: Vec<_> = srcs.iter().filter(|s| s.update_on_serve).collect();
    if !to_update.is_empty() {
        eprintln!("odoo-mcp: updating {} source(s)...", to_update.len());
        for src in &to_update {
            match sources::update_source(src) {
                Ok(msg) => eprintln!("  ok  {msg}"),
                Err(e)  => eprintln!("  err {}: {e}", src.resolved_path()),
            }
        }
    }

    eprintln!("odoo-mcp: MCP server ready");

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let sources = srcs;
    let docs = server_docs(profile, url, safe_mode);

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let msg: Json = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                send(&mut stdout, &err_resp(Json::Null, -32700, &format!("Parse error: {e}")))?;
                continue;
            }
        };

        let method = msg["method"].as_str().unwrap_or("");

        // Notifications have no "id" and require no response.
        if method.starts_with("notifications/") || !msg.get("id").is_some() {
            continue;
        }

        let id = msg["id"].clone();

        let resp = match method {
            "initialize" => ok_resp(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}, "resources": {}},
                    "serverInfo": {
                        "name": "odoo-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            ),

            "tools/list" => ok_resp(id, json!({"tools": tools_schema(safe_mode)})),

            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = &msg["params"]["arguments"];
                match call_tool(&odoo, &sources, safe_mode, name, args) {
                    Ok(text) => ok_resp(
                        id,
                        json!({"content": [{"type": "text", "text": text}]}),
                    ),
                    Err(e) => ok_resp(
                        id,
                        json!({
                            "content": [{"type": "text", "text": format!("Error: {e}")}],
                            "isError": true
                        }),
                    ),
                }
            }

            "resources/list" => ok_resp(id, json!({
                "resources": [{
                    "uri": "odoo-mcp://docs",
                    "name": "Server Reference",
                    "description": "Complete reference for all MCP tools with examples",
                    "mimeType": "text/markdown"
                }]
            })),

            "resources/read" => {
                let uri = msg["params"]["uri"].as_str().unwrap_or("");
                if uri == "odoo-mcp://docs" {
                    ok_resp(id, json!({
                        "contents": [{
                            "uri": "odoo-mcp://docs",
                            "mimeType": "text/markdown",
                            "text": docs
                        }]
                    }))
                } else {
                    err_resp(id, -32002, &format!("Resource not found: {uri}"))
                }
            }

            other => err_resp(id, -32601, &format!("Method not found: {other}")),
        };

        send(&mut stdout, &resp)?;
    }

    Ok(())
}

// ── Transport helpers ─────────────────────────────────────────────────────────

fn send(stdout: &mut io::Stdout, msg: &Json) -> Result<()> {
    writeln!(stdout, "{}", serde_json::to_string(msg)?)?;
    stdout.flush()?;
    Ok(())
}

fn ok_resp(id: Json, result: Json) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err_resp(id: Json, code: i32, message: &str) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

// ── Tool schema ───────────────────────────────────────────────────────────────

fn tools_schema(safe_mode: bool) -> Json {
    let mut tools = json!([
        {
            "name": "odoo_search_read",
            "description": "Search and read records from an Odoo model. Returns a JSON array of matching records.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model": {
                        "type": "string",
                        "description": "Odoo model technical name, e.g. account.move, res.partner, sale.order"
                    },
                    "domain": {
                        "type": "string",
                        "description": "Domain filter as JSON array, e.g. [[\"state\",\"=\",\"posted\"],[\"amount_total\",\">\",100]]",
                        "default": "[]"
                    },
                    "fields": {
                        "type": "string",
                        "description": "Comma-separated field names to return, e.g. id,name,amount_total,partner_id",
                        "default": "id,name"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of records to return"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Number of records to skip (for pagination)",
                        "default": 0
                    },
                    "order": {
                        "type": "string",
                        "description": "Sort order, e.g. \"id desc\" or \"invoice_date asc\""
                    }
                },
                "required": ["model"]
            }
        },
        {
            "name": "odoo_search",
            "description": "Return record IDs matching a domain filter. Lighter than odoo_search_read when you only need IDs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model":  {"type": "string", "description": "Odoo model technical name"},
                    "domain": {"type": "string", "description": "Domain filter as JSON array", "default": "[]"},
                    "limit":  {"type": "integer", "description": "Maximum number of IDs to return"},
                    "offset": {"type": "integer", "description": "Records to skip", "default": 0},
                    "order":  {"type": "string",  "description": "Sort order, e.g. \"id desc\""}
                },
                "required": ["model"]
            }
        },
        {
            "name": "odoo_search_count",
            "description": "Return the number of records matching a domain filter.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model":  {"type": "string", "description": "Odoo model technical name"},
                    "domain": {"type": "string", "description": "Domain filter as JSON array", "default": "[]"}
                },
                "required": ["model"]
            }
        },
        {
            "name": "odoo_read",
            "description": "Read specific records by their IDs. Use when you already have IDs and need field values.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model":  {"type": "string", "description": "Odoo model technical name"},
                    "ids":    {"type": "string", "description": "Record IDs as JSON array, e.g. [1,2,3]"},
                    "fields": {"type": "string", "description": "Comma-separated field names", "default": "id,name"}
                },
                "required": ["model", "ids"]
            }
        },
        {
            "name": "odoo_fields_get",
            "description": "Return field definitions for an Odoo model: type, label, required, readonly, relation target. Use this to discover available fields before building queries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model":      {"type": "string", "description": "Odoo model technical name"},
                    "fields":     {"type": "string", "description": "Comma-separated field names to filter (omit for all fields)"},
                    "attributes": {"type": "string", "description": "Comma-separated attributes to include", "default": "string,type,required,readonly,relation"}
                },
                "required": ["model"]
            }
        },
        {
            "name": "odoo_execute_kw",
            "description": "Call any method on an Odoo model via execute_kw. Use for create, write, unlink, or any custom model method.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model": {
                        "type": "string",
                        "description": "Odoo model technical name, e.g. account.move"
                    },
                    "method": {
                        "type": "string",
                        "description": "Method name, e.g. write, create, unlink, action_post, read"
                    },
                    "args": {
                        "type": "string",
                        "description": "Positional args as JSON array, e.g. [[1,2,3]] or [[{\"name\":\"Invoice\"}]]",
                        "default": "[]"
                    },
                    "kwargs": {
                        "type": "string",
                        "description": "Keyword args as JSON object, e.g. {\"fields\": [\"name\",\"amount_total\"]}",
                        "default": "{}"
                    }
                },
                "required": ["model", "method"]
            }
        },
        {
            "name": "odoo_http",
            "description": "Make a direct HTTP request to any Odoo endpoint, bypassing XML-RPC. Useful for custom controllers and public/ext endpoints.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "HTTP method: GET, POST, PUT, PATCH, DELETE",
                        "default": "GET"
                    },
                    "path": {
                        "type": "string",
                        "description": "Path on the Odoo server, e.g. /web/health or /api/v2/invoices"
                    },
                    "body": {
                        "type": "string",
                        "description": "Request body for POST/PUT/PATCH (usually JSON string)"
                    },
                    "content_type": {
                        "type": "string",
                        "description": "Content-Type header for the request body",
                        "default": "application/json"
                    }
                },
                "required": ["path"]
            }
        },
        {
            "name": "odoo_list_addons",
            "description": "List all Odoo addons found in the configured source trees with their name, version, summary, and dependencies. Use this first to understand the overall application structure before drilling into specifics.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "odoo_addon_structure",
            "description": "Return the structural overview of a specific Odoo addon: models it defines, models it extends, HTTP controllers/routes, data files, and security rules. Use after odoo_list_addons to understand what a module contains before reading source code.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "addon": {
                        "type": "string",
                        "description": "Technical addon name (directory name), e.g. gt_billing, account, sale"
                    }
                },
                "required": ["addon"]
            }
        },
        {
            "name": "odoo_model_source",
            "description": "Return the Python source code (from the local git checkout) that defines or inherits an Odoo model. Use this to understand field names, types, relations, computed fields, and business logic before building queries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model": {
                        "type": "string",
                        "description": "Odoo model technical name, e.g. account.move, res.partner, gt_billing.order"
                    }
                },
                "required": ["model"]
            }
        },
        {
            "name": "odoo_search_source",
            "description": "Search for any string across all Python source files in the configured git trees. Use to find business logic, methods, field usages, routes, cron jobs, constraints — anything in the codebase. Returns matching lines with context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Case-sensitive substring to search for, e.g. \"def action_post\", \"@http.route\", \"agreement_currency_id\", \"class GtBilling\""
                    },
                    "path_filter": {
                        "type": "string",
                        "description": "Optional substring the file path must contain to restrict search scope, e.g. \"gt_billing\" or \"account\" or \"controllers\""
                    },
                    "context": {
                        "type": "integer",
                        "description": "Number of lines of context to show before and after each match",
                        "default": 5
                    },
                    "max_matches": {
                        "type": "integer",
                        "description": "Maximum number of matches to return",
                        "default": 30
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "odoo_update_sources",
            "description": "Pull / clone all configured git source repositories (git fetch + reset --hard to origin branch). Use when you need fresh source code before inspecting models.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }
    ]); // end tools list

    if safe_mode {
        // Remove odoo_execute_kw from the list — not visible to the AI in safe mode.
            tools.as_array_mut().unwrap().retain(|t| t["name"] != "odoo_execute_kw");
    }

    tools
}

// ── Tool dispatch ─────────────────────────────────────────────────────────────

fn call_tool(odoo: &OdooClient, sources: &[SourceConfig], safe_mode: bool, name: &str, args: &Json) -> Result<String> {
    if safe_mode && name == "odoo_execute_kw" {
        bail!("odoo_execute_kw is disabled in safe mode. Set safe_mode: false in the profile config to enable it.");
    }
    match name {
        "odoo_search"         => tool_search(odoo, args),
        "odoo_search_count"   => tool_search_count(odoo, args),
        "odoo_read"           => tool_read(odoo, args),
        "odoo_fields_get"     => tool_fields_get(odoo, args),
        "odoo_search_read"    => tool_search_read(odoo, args),
        "odoo_execute_kw"     => tool_execute_kw(odoo, args),
        "odoo_http"           => tool_http(odoo, args),
        "odoo_list_addons"    => list_addons(sources).map_err(Into::into),
        "odoo_addon_structure"=> tool_addon_structure(sources, args),
        "odoo_model_source"   => tool_model_source(sources, args),
        "odoo_search_source"  => tool_search_source(sources, args),
        "odoo_update_sources" => tool_update_sources(sources),
        other => bail!("Unknown tool: {other}"),
    }
}

fn tool_search(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model      = args["model"].as_str().context("missing field: model")?;
    let domain_str = args["domain"].as_str().unwrap_or("[]");
    let limit      = args["limit"].as_u64().map(|v| v as usize);
    let offset     = args["offset"].as_u64().unwrap_or(0) as usize;
    let order      = args["order"].as_str();

    let domain: Json = serde_json::from_str(domain_str)
        .with_context(|| format!("Invalid domain JSON: {domain_str}"))?;

    let ids = odoo.search_all(model, domain, order, offset, limit)?;
    Ok(serde_json::to_string_pretty(&Json::Array(ids))?)
}

fn tool_search_count(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model      = args["model"].as_str().context("missing field: model")?;
    let domain_str = args["domain"].as_str().unwrap_or("[]");

    let domain: Json = serde_json::from_str(domain_str)
        .with_context(|| format!("Invalid domain JSON: {domain_str}"))?;

    let result = odoo.execute_kw(model, "search_count", json!([domain]), json!({}))?;
    Ok(serde_json::to_string_pretty(&result)?)
}

fn tool_read(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model      = args["model"].as_str().context("missing field: model")?;
    let ids_str    = args["ids"].as_str().context("missing field: ids")?;
    let fields_str = args["fields"].as_str().unwrap_or("id,name");

    let ids: Json = serde_json::from_str(ids_str)
        .with_context(|| format!("Invalid ids JSON: {ids_str}"))?;
    let fields: Vec<&str> = fields_str.split(',').map(str::trim).collect();

    let result = odoo.execute_kw(model, "read", json!([ids]), json!({"fields": fields}))?;
    Ok(serde_json::to_string_pretty(&result)?)
}

fn tool_fields_get(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model          = args["model"].as_str().context("missing field: model")?;
    let fields_str     = args["fields"].as_str().unwrap_or("");
    let attributes_str = args["attributes"].as_str()
        .unwrap_or("string,type,required,readonly,relation");

    let attributes: Vec<&str> = attributes_str.split(',').map(str::trim).collect();
    let mut kwargs = serde_json::Map::new();
    if !fields_str.is_empty() {
        let fields: Vec<&str> = fields_str.split(',').map(str::trim).collect();
        kwargs.insert("allfields".into(), json!(fields));
    }
    kwargs.insert("attributes".into(), json!(attributes));

    let result = odoo.execute_kw(model, "fields_get", json!([]), Json::Object(kwargs))?;
    Ok(serde_json::to_string_pretty(&result)?)
}

fn tool_search_read(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model      = args["model"].as_str().context("missing field: model")?;
    let domain_str = args["domain"].as_str().unwrap_or("[]");
    let fields_str = args["fields"].as_str().unwrap_or("id,name");
    let limit      = args["limit"].as_u64().map(|v| v as usize);
    let offset     = args["offset"].as_u64().unwrap_or(0) as usize;
    let order      = args["order"].as_str();

    let domain: Json = serde_json::from_str(domain_str)
        .with_context(|| format!("Invalid domain JSON: {domain_str}"))?;
    let fields: Vec<String> = fields_str.split(',').map(|s| s.trim().to_string()).collect();

    let records = odoo.search_read_all(model, domain, &fields, order, offset, limit)?;
    Ok(serde_json::to_string_pretty(&Json::Array(records))?)
}

fn tool_execute_kw(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model = args["model"].as_str().context("missing field: model")?;
    let method = args["method"].as_str().context("missing field: method")?;
    let args_str = args["args"].as_str().unwrap_or("[]");
    let kwargs_str = args["kwargs"].as_str().unwrap_or("{}");

    let args_val: Json = serde_json::from_str(args_str)
        .with_context(|| format!("Invalid args JSON: {args_str}"))?;
    let kwargs_val: Json = serde_json::from_str(kwargs_str)
        .with_context(|| format!("Invalid kwargs JSON: {kwargs_str}"))?;

    let result = odoo.execute_kw(model, method, args_val, kwargs_val)?;
    Ok(serde_json::to_string_pretty(&result)?)
}

fn tool_http(odoo: &OdooClient, args: &Json) -> Result<String> {
    let method = args["method"].as_str().unwrap_or("GET");
    let path = args["path"].as_str().context("missing field: path")?;
    let body = args["body"].as_str();
    let content_type = args["content_type"].as_str().unwrap_or("application/json");

    let text = odoo.http_request(method, path, body, content_type, &[])?;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(json) => Ok(serde_json::to_string_pretty(&json)?),
        Err(_) => Ok(text),
    }
}

fn tool_addon_structure(sources: &[SourceConfig], args: &Json) -> Result<String> {
    let addon = args["addon"].as_str().context("missing field: addon")?;
    addon_structure(addon, sources)
}

fn tool_model_source(sources: &[SourceConfig], args: &Json) -> Result<String> {
    let model = args["model"].as_str().context("missing field: model")?;
    sources::find_model_source(model, sources)
}

fn tool_search_source(sources: &[SourceConfig], args: &Json) -> Result<String> {
    let query       = args["query"].as_str().context("missing field: query")?;
    let path_filter = args["path_filter"].as_str();
    let context     = args["context"].as_u64().unwrap_or(5) as usize;
    let max_matches = args["max_matches"].as_u64().unwrap_or(30) as usize;
    search_source(query, path_filter, context, max_matches, sources)
}

fn tool_update_sources(sources: &[SourceConfig]) -> Result<String> {
    if sources.is_empty() {
        return Ok("No sources configured for this profile.".to_string());
    }
    let results = sources::update_all(sources);
    let lines: Vec<String> = results
        .into_iter()
        .map(|(path, res)| match res {
            Ok(msg) => format!("ok  {msg}"),
            Err(e)  => format!("err {path}: {e}"),
        })
        .collect();
    Ok(lines.join("\n"))
}

// ── Embedded server documentation ─────────────────────────────────────────────

fn server_docs(profile: &str, url: &str, safe_mode: bool) -> String {
    let safe_note = if safe_mode {
        "**Safe mode ON** — `odoo_execute_kw` is disabled (read-only access)."
    } else {
        "**Safe mode OFF** — `odoo_execute_kw` is enabled (write access allowed)."
    };

    format!(r#"# odoo-mcp MCP Server

**Profile:** `{profile}`
**Odoo URL:** {url}
{safe_note}

## Overview

odoo-mcp provides AI tools to query and interact with an Odoo ERP instance.
It exposes two categories of tools:

- **Data tools** — query live Odoo data via XML-RPC
- **Source tools** — read and search Python source code from local git checkouts

Credentials are stored server-side and **never returned by any tool**.

---

## Typical workflow

```
# 1. Discover installed modules
odoo_list_addons()

# 2. Understand a specific module
odoo_addon_structure("gt_billing")

# 3. Read model fields and methods
odoo_model_source("gt.billing.order")

# 4. Find specific logic in the codebase
odoo_search_source("def action_confirm", path_filter="gt_billing")

# 5. Query live data
odoo_search_read(
  model="gt.billing.order",
  domain='[["state","=","draft"]]',
  fields="id,name,partner_id,amount_total",
  limit=10,
  order="id desc"
)

# 6. Call a method (unsafe mode only)
odoo_execute_kw(model="gt.billing.order", method="action_confirm", args="[[42]]")
```

---

## Data tools

### odoo_search_read
Search and read records from any Odoo model. Returns a JSON array.

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `model` | yes | — | Model technical name, e.g. `account.move` |
| `domain` | no | `[]` | JSON domain, e.g. `[["state","=","posted"]]` |
| `fields` | no | `id,name` | Comma-separated field names |
| `limit` | no | all | Max records to return |
| `offset` | no | `0` | Records to skip (pagination) |
| `order` | no | — | Sort, e.g. `id desc` |

```
odoo_search_read(
  model="account.move",
  domain='[["move_type","=","out_invoice"],["payment_state","=","not_paid"]]',
  fields="id,name,partner_id,amount_total,invoice_date",
  limit=20,
  order="id desc"
)
```

---

### odoo_search
Return record IDs matching a domain. Lighter than `odoo_search_read` when only IDs are needed.

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `model` | yes | — | Model technical name |
| `domain` | no | `[]` | JSON domain |
| `limit` | no | all | Max IDs |
| `offset` | no | `0` | Records to skip |
| `order` | no | — | Sort order |

```
odoo_search(model="res.partner", domain='[["is_company","=",true]]', limit=50)
```

---

### odoo_search_count
Return the count of records matching a domain.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `model` | yes | Model technical name |
| `domain` | no | JSON domain (default: `[]`) |

```
odoo_search_count(model="account.move", domain='[["state","=","posted"]]')
```

---

### odoo_read
Read specific records by IDs. Use when you already have IDs.

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `model` | yes | — | Model technical name |
| `ids` | yes | — | JSON array of IDs, e.g. `[1,2,3]` |
| `fields` | no | `id,name` | Comma-separated field names |

```
odoo_read(model="account.move", ids="[1070023,1070024]", fields="id,name,amount_total,state")
```

---

### odoo_fields_get
Return field definitions for a model: type, label, required, readonly, relation target.
**Use this before building queries** to discover available field names.

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `model` | yes | — | Model technical name |
| `fields` | no | all | Comma-separated field names to filter |
| `attributes` | no | `string,type,required,readonly,relation` | Attributes to include |

```
# Discover all fields
odoo_fields_get(model="account.move")

# Check specific fields
odoo_fields_get(model="account.move", fields="partner_id,invoice_line_ids,amount_total")
```

---

### odoo_execute_kw
Call any method on an Odoo model. **Disabled in safe mode.**

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `model` | yes | — | Model technical name |
| `method` | yes | — | Method name, e.g. `write`, `create`, `action_post` |
| `args` | no | `[]` | Positional args as JSON array |
| `kwargs` | no | `{{}}` | Keyword args as JSON object |

```
# Post an invoice
odoo_execute_kw(model="account.move", method="action_post", args="[[1070023]]")

# Create a partner
odoo_execute_kw(
  model="res.partner",
  method="create",
  args='[{{"name":"Test","email":"test@example.com"}}]'
)
```

---

### odoo_http
Direct HTTP request to any Odoo endpoint, bypassing XML-RPC.
Useful for custom controllers, health checks, and REST APIs.

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `path` | yes | — | Path on the Odoo server, e.g. `/web/health` |
| `method` | no | `GET` | HTTP method |
| `body` | no | — | Request body string |
| `content_type` | no | `application/json` | Content-Type header |

```
odoo_http(path="/web/health")
odoo_http(method="POST", path="/api/v2/invoices", body='{{"partner_id":123}}')
```

---

## Source code tools

These tools read Python source files from local git checkouts configured in the profile.
No Odoo API calls are made.

### odoo_list_addons
List all Odoo addons found across all configured source trees.
Returns name, version, summary, dependencies, and path for each addon.
**Start here** to understand the overall application structure.

```
odoo_list_addons()
```

---

### odoo_addon_structure
Structural overview of a specific addon: models defined, models extended,
HTTP routes, data files, security rules.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `addon` | yes | Technical addon name (directory name), e.g. `gt_billing` |

```
odoo_addon_structure("gt_billing")
odoo_addon_structure("account")
```

---

### odoo_model_source
Return the full Python source of all files that define or inherit a model.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `model` | yes | Model technical name, e.g. `account.move` |

```
odoo_model_source("gt.billing.order")
odoo_model_source("account.move")
```

---

### odoo_search_source
Search for any string across all Python source files.
Use to find business logic, methods, field usages, routes, cron jobs.

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `query` | yes | — | Case-sensitive substring to find |
| `path_filter` | no | — | File path must contain this substring |
| `context` | no | `5` | Lines of context around each match |
| `max_matches` | no | `30` | Result limit |

```
# Find where a method is defined
odoo_search_source("def action_post", path_filter="gt_")

# Find all HTTP routes in an addon
odoo_search_source("@http.route", path_filter="gt_billing")

# Find all cron definitions
odoo_search_source("ir.cron", path_filter="gt_")

# Trace a field across the codebase
odoo_search_source("agreement_currency_id", context=3)
```

---

### odoo_update_sources
Pull or clone all configured source repos (fetch + hard reset to origin branch).
Call this when you need fresh source code.

```
odoo_update_sources()
```
"#)
}
