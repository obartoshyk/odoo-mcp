use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value as Json};

use odoo_claude_mcp::{from_json, OdooClient, Value};

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run_server(odoo: OdooClient) -> Result<()> {
    eprintln!("odoo-claude-mcp: MCP server ready");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

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
                    "capabilities": {"tools": {}},
                    "serverInfo": {
                        "name": "odoo-claude-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            ),

            "tools/list" => ok_resp(id, json!({"tools": tools_schema()})),

            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = &msg["params"]["arguments"];
                match call_tool(&odoo, name, args) {
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

fn tools_schema() -> Json {
    json!([
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
        }
    ])
}

// ── Tool dispatch ─────────────────────────────────────────────────────────────

fn call_tool(odoo: &OdooClient, name: &str, args: &Json) -> Result<String> {
    match name {
        "odoo_search_read" => tool_search_read(odoo, args),
        "odoo_execute_kw" => tool_execute_kw(odoo, args),
        "odoo_http" => tool_http(odoo, args),
        other => bail!("Unknown tool: {other}"),
    }
}

fn tool_search_read(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model = args["model"].as_str().context("missing field: model")?;
    let domain_str = args["domain"].as_str().unwrap_or("[]");
    let fields_str = args["fields"].as_str().unwrap_or("id,name");
    let limit = args["limit"].as_u64().map(|v| v as usize);
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let order = args["order"].as_str();

    let domain_json: serde_json::Value = serde_json::from_str(domain_str)
        .with_context(|| format!("Invalid domain JSON: {domain_str}"))?;

    let mut kwargs: BTreeMap<String, Value> = BTreeMap::new();
    kwargs.insert("domain".into(), from_json(&domain_json));
    kwargs.insert(
        "fields".into(),
        Value::Array(
            fields_str
                .split(',')
                .map(|f| Value::String(f.trim().to_string()))
                .collect(),
        ),
    );
    if let Some(lim) = limit {
        kwargs.insert("limit".into(), Value::Int(lim as i64));
    }
    if offset > 0 {
        kwargs.insert("offset".into(), Value::Int(offset as i64));
    }
    if let Some(ord) = order {
        kwargs.insert("order".into(), Value::String(ord.to_string()));
    }

    let result = odoo.execute_kw(
        model,
        "search_read",
        Value::Array(vec![]),
        Value::Struct(kwargs),
    )?;
    Ok(serde_json::to_string_pretty(&result.to_json())?)
}

fn tool_execute_kw(odoo: &OdooClient, args: &Json) -> Result<String> {
    let model = args["model"].as_str().context("missing field: model")?;
    let method = args["method"].as_str().context("missing field: method")?;
    let args_str = args["args"].as_str().unwrap_or("[]");
    let kwargs_str = args["kwargs"].as_str().unwrap_or("{}");

    let args_json: serde_json::Value = serde_json::from_str(args_str)
        .with_context(|| format!("Invalid args JSON: {args_str}"))?;
    let kwargs_json: serde_json::Value = serde_json::from_str(kwargs_str)
        .with_context(|| format!("Invalid kwargs JSON: {kwargs_str}"))?;

    let result = odoo.execute_kw(
        model,
        method,
        from_json(&args_json),
        from_json(&kwargs_json),
    )?;
    Ok(serde_json::to_string_pretty(&result.to_json())?)
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
