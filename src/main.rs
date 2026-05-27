mod mcp;
mod sources;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use odoo_mcp::OdooClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

// ── Config file ───────────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Default)]
struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    connections: std::collections::HashMap<String, ConnectionConfig>,
}

#[derive(Deserialize, Serialize, Default, Clone)]
struct ConnectionConfig {
    #[serde(skip_serializing_if = "Option::is_none")] url:      Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] ext_url:  Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] db:       Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] cert:     Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] key:      Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sources: Vec<sources::SourceConfig>,
}

fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("odoo-mcp").join("config.yaml"))
        .unwrap_or_else(|| PathBuf::from("odoo-mcp.yaml"))
}

fn load_config(path: Option<&PathBuf>) -> Result<Config> {
    let resolved = path.cloned().unwrap_or_else(default_config_path);
    if !resolved.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&resolved)
        .with_context(|| format!("Cannot read config: {}", resolved.display()))?;
    serde_yaml::from_str(&text)
        .with_context(|| format!("Invalid YAML in {}", resolved.display()))
}

fn save_config(config: &Config, path: Option<&PathBuf>) -> Result<()> {
    let resolved = path.cloned().unwrap_or_else(default_config_path);
    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create config dir: {}", parent.display()))?;
    }
    let text = serde_yaml::to_string(config).context("Failed to serialize config")?;
    std::fs::write(&resolved, text)
        .with_context(|| format!("Cannot write config: {}", resolved.display()))
}

// ── Session cache ─────────────────────────────────────────────────────────────

fn session_dir(config_path: Option<&PathBuf>) -> PathBuf {
    config_path
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| default_config_path().parent().unwrap().to_path_buf())
        .join("sessions")
}

fn load_session(config_path: Option<&PathBuf>, profile: &str) -> Option<String> {
    let path = session_dir(config_path).join(format!("{profile}.txt"));
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_session(config_path: Option<&PathBuf>, profile: &str, session_id: &str) {
    let dir = session_dir(config_path);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{profile}.txt")), session_id);
}

// ── CLI ───────────────────────────────────────────────────────────────────────

/// Odoo XML-RPC CLI connector.
///
/// Connection parameters are resolved in priority order:
///   1. CLI flag
///   2. Environment variable (ODOO_URL, ODOO_DB, ODOO_USERNAME, ODOO_PASSWORD, ODOO_CERT, ODOO_KEY)
///   3. Config file profile (~/.config/odoo-mcp/config.yaml)
#[derive(Parser)]
#[command(name = "odoo-mcp", version)]
struct Cli {
    /// Path to YAML config file (default: ~/.config/odoo-mcp/config.yaml)
    #[arg(long, env = "ODOO_CONFIG")]
    config: Option<PathBuf>,

    /// Connection profile name from config file (uses `default:` key if omitted)
    #[arg(long, env = "ODOO_PROFILE")]
    profile: Option<String>,

    /// Odoo base URL, e.g. https://odoo.example.com
    #[arg(long, env = "ODOO_URL")]
    url: Option<String>,

    /// Database name
    #[arg(long, env = "ODOO_DB")]
    db: Option<String>,

    /// Login username
    #[arg(long, env = "ODOO_USERNAME")]
    username: Option<String>,

    /// Password or API key
    #[arg(long, env = "ODOO_PASSWORD")]
    password: Option<String>,

    /// mTLS client certificate (.crt / .pem)
    #[arg(long, env = "ODOO_CERT")]
    cert: Option<String>,

    /// mTLS client private key (.key / .pem)
    #[arg(long, env = "ODOO_KEY")]
    key: Option<String>,

    /// Alternative public URL for unauthenticated ext-odoo endpoints
    #[arg(long, env = "ODOO_EXT_URL")]
    ext_url: Option<String>,

    /// Use ext_url as base and skip authentication
    #[arg(long)]
    ext: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate and print the uid (smoke-test)
    Auth,

    /// search_read shortcut — most common query pattern
    SearchRead {
        /// Model name, e.g. account.move
        #[arg(long)]
        model: String,

        /// Domain as JSON, e.g. '[["state","=","posted"]]'
        #[arg(long, default_value = "[]")]
        domain: String,

        /// Fields to return (comma-separated)
        #[arg(long, value_delimiter = ',', default_value = "id,name")]
        fields: Vec<String>,

        /// Maximum number of records
        #[arg(long)]
        limit: Option<usize>,

        /// Number of records to skip
        #[arg(long, default_value_t = 0)]
        offset: usize,

        /// Sort order, e.g. "id desc"
        #[arg(long)]
        order: Option<String>,
    },

    /// Return record IDs matching a domain
    Search {
        #[arg(long)] model: String,
        #[arg(long, default_value = "[]")] domain: String,
        #[arg(long)] limit: Option<usize>,
        #[arg(long, default_value_t = 0)] offset: usize,
        #[arg(long)] order: Option<String>,
    },

    /// Return count of records matching a domain
    SearchCount {
        #[arg(long)] model: String,
        #[arg(long, default_value = "[]")] domain: String,
    },

    /// Read specific records by IDs
    Read {
        #[arg(long)] model: String,
        /// Record IDs as JSON array, e.g. '[1,2,3]'
        #[arg(long)] ids: String,
        #[arg(long, value_delimiter = ',', default_value = "id,name")] fields: Vec<String>,
    },

    /// Return field definitions for a model
    FieldsGet {
        #[arg(long)] model: String,
        /// Comma-separated field names to filter (empty = all fields)
        #[arg(long, value_delimiter = ',')] fields: Vec<String>,
        /// Attributes to include, e.g. string,type,required
        #[arg(long, value_delimiter = ',', default_value = "string,type,required,readonly,relation")] attributes: Vec<String>,
    },

    /// Raw execute_kw — full flexibility for any model/method
    ExecuteKw {
        /// Model name
        #[arg(long)]
        model: String,

        /// Method name, e.g. write, create, search_read
        #[arg(long)]
        method: String,

        /// Positional args as a JSON array, e.g. '[[1,2,3]]'
        #[arg(long, default_value = "[]")]
        args: String,

        /// Keyword args as a JSON object, e.g. '{"context":{"active_test":false}}'
        #[arg(long, default_value = "{}")]
        kwargs: String,

        /// Save output to file. If the result is a base64 string, it is decoded first.
        #[arg(short = 'o', long, value_name = "FILE")]
        output: Option<PathBuf>,
    },

    /// Download a report PDF (e.g. invoice) using the Odoo web session
    PrintReport {
        /// Report technical name, e.g. gt_billing.gt_invoice
        #[arg(long)]
        report: String,

        /// Record ID(s), comma-separated for multiple, e.g. 1068747 or 1068747,1068748
        #[arg(long, value_delimiter = ',')]
        ids: Vec<u64>,

        /// Output file (default: <report_suffix>_<ids>.pdf in current directory)
        #[arg(short = 'o', long, value_name = "FILE")]
        output: Option<PathBuf>,
    },

    /// Create initial config file from template (if not already present)
    Init,

    /// Manage connection profiles in the config file
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Start MCP server (JSON-RPC over stdio) for use with Claude
    Serve,

    /// Pull / clone all configured source repositories
    UpdateSources,

    /// Direct HTTP request to any Odoo endpoint (no XML-RPC wrapping)
    Http {
        /// HTTP method: GET, POST, PUT, PATCH, DELETE, HEAD
        method: String,

        /// Path on the Odoo server, e.g. /web/dataset/call_kw
        path: String,

        /// Request body (for POST/PUT/PATCH)
        #[arg(long)]
        body: Option<String>,

        /// Content-Type header for requests with a body
        #[arg(long, default_value = "application/json")]
        content_type: String,

        /// Extra headers as KEY:VALUE (repeatable, e.g. --header X-Token:abc)
        #[arg(long = "header", value_name = "KEY:VALUE")]
        headers: Vec<String>,

        /// Save raw response bytes to file instead of printing
        #[arg(short = 'o', long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// List all connection profiles
    List,
    /// Show full config with secrets masked
    Show,
    /// Create or update a connection profile
    Set {
        /// Profile name to create or update
        #[arg(long)] profile: String,
        #[arg(long)] url:      Option<String>,
        #[arg(long)] db:       Option<String>,
        #[arg(long)] username: Option<String>,
        #[arg(long)] password: Option<String>,
        #[arg(long)] ext_url:  Option<String>,
        #[arg(long)] cert:     Option<String>,
        #[arg(long)] key:      Option<String>,
        /// Make this the default profile
        #[arg(long)] default:  bool,
    },
    /// Remove a connection profile
    Remove {
        #[arg(long)] profile: String,
    },
    /// Set the default profile
    Default {
        #[arg(long)] profile: String,
    },
}

// ── Merge helpers ─────────────────────────────────────────────────────────────

/// Merge CLI/env value with config file value. CLI wins.
fn resolve(cli: Option<String>, cfg: Option<String>) -> Option<String> {
    cli.or(cfg)
}

fn require(value: Option<String>, name: &str) -> Result<String> {
    value.with_context(|| {
        format!(
            "Missing required parameter: --{name} / ODOO_{} / config file",
            name.to_uppercase().replace('-', "_")
        )
    })
}

// ── HTTP client ───────────────────────────────────────────────────────────────

fn build_http_client(cert: Option<&str>, key: Option<&str>) -> Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::ClientBuilder::new()
        .timeout(std::time::Duration::from_secs(60));

    if let (Some(cert_path), Some(key_path)) = (cert, key) {
        let cert_pem =
            std::fs::read(cert_path).with_context(|| format!("Cannot read cert: {cert_path}"))?;
        let key_pem =
            std::fs::read(key_path).with_context(|| format!("Cannot read key: {key_path}"))?;
        // rustls expects cert + key concatenated in a single PEM buffer
        let mut pem = cert_pem;
        pem.extend_from_slice(&key_pem);
        let identity = reqwest::Identity::from_pem(&pem)
            .context("Failed to build mTLS identity from cert+key")?;
        builder = builder.identity(identity);
    }

    builder.build().context("Failed to build HTTP client")
}

/// Return a valid web session_id: use cache if present, otherwise authenticate and cache it.
fn ensure_session(
    odoo: &OdooClient,
    config_path: Option<&PathBuf>,
    profile: &str,
    db: &str,
    username: &str,
    password: &str,
) -> Result<String> {
    if let Some(sid) = load_session(config_path, profile) {
        return Ok(sid);
    }
    let sid = odoo.web_authenticate(db, username, password)?;
    save_session(config_path, profile, &sid);
    Ok(sid)
}

/// True only if the response looks like an Odoo login redirect (HTML page).
/// Used to decide whether a non-PDF response warrants a re-auth retry.
fn is_login_redirect(bytes: &[u8]) -> bool {
    bytes.starts_with(b"<")
}

/// Verify bytes start with %PDF; bail with a response preview otherwise.
fn require_pdf(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.starts_with(b"%PDF") {
        return Ok(bytes);
    }
    let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]);
    bail!("Expected PDF but got:\n{preview}");
}

/// Download a report PDF with session cookie.
/// Re-authenticates once only if the response looks like an HTML login redirect.
fn download_report(
    odoo: &OdooClient,
    path: &str,
    config_path: Option<&PathBuf>,
    profile: &str,
    db: &str,
    username: &str,
    password: &str,
) -> Result<Vec<u8>> {
    let fetch = |session_id: &str| -> Result<Vec<u8>> {
        let extra = vec![("Cookie".to_string(), format!("session_id={session_id}"))];
        odoo.http_request_bytes("GET", path, None, "application/json", &extra)
    };

    let sid = ensure_session(odoo, config_path, profile, db, username, password)?;
    let bytes = fetch(&sid)?;
    if bytes.starts_with(b"%PDF") {
        return Ok(bytes);
    }
    if !is_login_redirect(&bytes) {
        return require_pdf(bytes); // Not a session issue — fail immediately.
    }

    // Looks like a login redirect → session expired, re-authenticate once.
    let new_sid = odoo.web_authenticate(db, username, password)?;
    save_session(config_path, profile, &new_sid);
    require_pdf(fetch(&new_sid)?)
}

/// Extract binary bytes for --output:
/// - string   → base64-decode
/// - [single] → recurse into the one element
/// - {datas}  → base64-decode the `datas` field
/// - anything else → pretty-print as JSON text
fn extract_binary(v: &Json) -> Result<Vec<u8>> {
    use base64::Engine;
    if let Some(b64) = v.as_str() {
        return base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("Result is a string but not valid base64");
    }
    if let Some(arr) = v.as_array() {
        if arr.len() == 1 {
            return extract_binary(&arr[0]);
        }
    }
    if let Some(obj) = v.as_object() {
        if let Some(datas) = obj.get("datas") {
            if let Some(b64) = datas.as_str() {
                return base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .context("datas field is not valid base64");
            }
        }
    }
    Ok(serde_json::to_string_pretty(v)?.into_bytes())
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load config file (silently absent = empty config).
    let config = load_config(cli.config.as_ref())?;

    // Pick the connection profile.
    let profile_name = cli.profile
        .as_deref()
        .or(config.default.as_deref())
        .unwrap_or("default");
    let cfg_conn = config
        .connections
        .get(profile_name)
        .cloned()
        .unwrap_or_default();

    let is_source_cmd  = matches!(&cli.command, Command::UpdateSources);
    let is_no_auth_cmd = matches!(
        &cli.command,
        Command::Init | Command::Config { .. }
    );
    // Skip auth for --ext, source management, and config commands.
    let needs_auth = !cli.ext && !is_source_cmd && !is_no_auth_cmd;

    // Merge: CLI/env > config file.
    let url = if cli.ext {
        require(
            resolve(cli.ext_url, cfg_conn.ext_url),
            "ext-url",
        )?
    } else {
        require(resolve(cli.url, cfg_conn.url), "url")?
    };
    let cert = resolve(cli.cert, cfg_conn.cert);
    let key  = resolve(cli.key,  cfg_conn.key);

    let (db, username, password) = if needs_auth {
        let db       = resolve(cli.db,       cfg_conn.db)      .unwrap_or_else(|| "odoo".to_string());
        let username = resolve(cli.username, cfg_conn.username) .unwrap_or_else(|| "admin".to_string());
        let password = require(resolve(cli.password, cfg_conn.password), "password")?;
        (db, username, password)
    } else {
        let db       = resolve(cli.db,       cfg_conn.db)      .unwrap_or_default();
        let username = resolve(cli.username, cfg_conn.username) .unwrap_or_default();
        let password = resolve(cli.password, cfg_conn.password).unwrap_or_default();
        (db, username, password)
    };

    let sources = cfg_conn.sources.clone();
    let http = build_http_client(cert.as_deref(), key.as_deref())?;
    let mut odoo = OdooClient::new(&url, &db, &password, http);

    let uid = if needs_auth {
        odoo.authenticate(&username)?
    } else {
        0
    };

    match cli.command {
        Command::PrintReport { report, ids, output } => {
            let ids_str = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            let report_path = format!("/report/pdf/{report}/{ids_str}");

            let out_path = output.unwrap_or_else(|| {
                let suffix = report.split('.').last().unwrap_or(&report);
                PathBuf::from(format!("{suffix}_{ids_str}.pdf"))
            });

            let bytes = download_report(
                &odoo, &report_path,
                cli.config.as_ref(), profile_name,
                &db, &username, &password,
            )?;

            std::fs::write(&out_path, &bytes)
                .with_context(|| format!("Cannot write: {}", out_path.display()))?;
            eprintln!("Wrote {} bytes → {}", bytes.len(), out_path.display());
        }

        Command::Init => {
            let path = cli.config.clone().unwrap_or_else(default_config_path);
            if path.exists() {
                println!("Config already exists: {}", path.display());
            } else {
                let template = include_str!("../config.example.yaml");
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("Cannot create dir: {}", parent.display()))?;
                }
                std::fs::write(&path, template)
                    .with_context(|| format!("Cannot write: {}", path.display()))?;
                println!("Created: {}", path.display());
            }
            println!("Edit the config, then run: odoo-mcp auth");
        }

        Command::Config { action } => {
            let path = cli.config.clone().unwrap_or_else(default_config_path);
            let mut cfg = load_config(Some(&path))?;
            match action {
                ConfigCommand::List => {
                    if cfg.connections.is_empty() {
                        println!("No profiles configured. Run: odoo-mcp init");
                    } else {
                        let default = cfg.default.as_deref().unwrap_or("");
                        let mut names: Vec<_> = cfg.connections.keys().collect();
                        names.sort();
                        for name in names {
                            let conn = &cfg.connections[name];
                            let url  = conn.url.as_deref().unwrap_or("(no url)");
                            let mark = if name == default { "  ← default" } else { "" };
                            println!("  {name:<20} {url}{mark}");
                        }
                    }
                }
                ConfigCommand::Show => {
                    let mut masked = cfg;
                    for conn in masked.connections.values_mut() {
                        if conn.password.is_some() { conn.password = Some("***".into()); }
                        if conn.key.is_some()      { conn.key      = Some("***".into()); }
                        for src in &mut conn.sources {
                            if src.token.is_some() { src.token = Some("***".into()); }
                        }
                    }
                    print!("{}", serde_yaml::to_string(&masked).context("Serialization failed")?);
                }
                ConfigCommand::Set { profile, url, db, username, password, ext_url, cert, key, default } => {
                    let conn = cfg.connections.entry(profile.clone()).or_default();
                    if let Some(v) = url      { conn.url      = Some(v); }
                    if let Some(v) = db       { conn.db       = Some(v); }
                    if let Some(v) = username { conn.username = Some(v); }
                    if let Some(v) = password { conn.password = Some(v); }
                    if let Some(v) = ext_url  { conn.ext_url  = Some(v); }
                    if let Some(v) = cert     { conn.cert     = Some(v); }
                    if let Some(v) = key      { conn.key      = Some(v); }
                    if default { cfg.default = Some(profile.clone()); }
                    save_config(&cfg, Some(&path))?;
                    println!("Saved profile '{profile}' → {}", path.display());
                }
                ConfigCommand::Remove { profile } => {
                    if cfg.connections.remove(&profile).is_none() {
                        anyhow::bail!("Profile '{profile}' not found");
                    }
                    if cfg.default.as_deref() == Some(profile.as_str()) {
                        cfg.default = None;
                    }
                    save_config(&cfg, Some(&path))?;
                    println!("Removed profile '{profile}'");
                }
                ConfigCommand::Default { profile } => {
                    if !cfg.connections.contains_key(&profile) {
                        anyhow::bail!("Profile '{profile}' not found");
                    }
                    cfg.default = Some(profile.clone());
                    save_config(&cfg, Some(&path))?;
                    println!("Default profile set to '{profile}'");
                }
            }
        }

        Command::Serve => {
            mcp::run_server(odoo, sources)?;
        }

        Command::UpdateSources => {
            if sources.is_empty() {
                println!("No sources configured for profile '{profile_name}'.");
            }
            for (path, result) in sources::update_all(&sources) {
                match result {
                    Ok(msg) => println!("ok  {msg}"),
                    Err(e)  => eprintln!("err {path}: {e}"),
                }
            }
        }

        Command::Auth => {
            let out = serde_json::json!({
                "uid": uid,
                "db": db,
                "url": url,
                "profile": profile_name,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        }

        Command::Search { model, domain, limit, offset, order } => {
            let domain_val: Json = serde_json::from_str(&domain)
                .with_context(|| format!("Invalid domain JSON: {domain}"))?;
            let ids = odoo.search_all(&model, domain_val, order.as_deref(), offset, limit)?;
            println!("{}", serde_json::to_string_pretty(&Json::Array(ids))?);
        }

        Command::SearchCount { model, domain } => {
            let domain_val: Json = serde_json::from_str(&domain)
                .with_context(|| format!("Invalid domain JSON: {domain}"))?;
            let result = odoo.execute_kw(&model, "search_count", json!([domain_val]), json!({}))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Command::Read { model, ids, fields } => {
            let ids_val: Json = serde_json::from_str(&ids)
                .with_context(|| format!("Invalid ids JSON: {ids}"))?;
            let result = odoo.execute_kw(
                &model, "read",
                json!([ids_val]),
                json!({"fields": fields}),
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Command::FieldsGet { model, fields, attributes } => {
            let mut kwargs = serde_json::Map::new();
            if !fields.is_empty() { kwargs.insert("allfields".into(), json!(fields)); }
            kwargs.insert("attributes".into(), json!(attributes));
            let result = odoo.execute_kw(&model, "fields_get", json!([]), Json::Object(kwargs))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Command::SearchRead { model, domain, fields, limit, offset, order } => {
            let domain_val: Json = serde_json::from_str(&domain)
                .with_context(|| format!("Invalid domain JSON: {domain}"))?;
            let records = odoo.search_read_all(
                &model, domain_val, &fields, order.as_deref(), offset, limit,
            )?;
            println!("{}", serde_json::to_string_pretty(&Json::Array(records))?);
        }

        Command::ExecuteKw { model, method, args, kwargs, output } => {
            let args_val: Json = serde_json::from_str(&args)
                .with_context(|| format!("Invalid args JSON: {args}"))?;
            let kwargs_val: Json = serde_json::from_str(&kwargs)
                .with_context(|| format!("Invalid kwargs JSON: {kwargs}"))?;

            let result = odoo.execute_kw(&model, &method, args_val, kwargs_val)?;

            if let Some(out_path) = output {
                let bytes = extract_binary(&result)?;
                std::fs::write(&out_path, &bytes)
                    .with_context(|| format!("Cannot write: {}", out_path.display()))?;
                eprintln!("Wrote {} bytes → {}", bytes.len(), out_path.display());
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }

        Command::Http { method, path, body, content_type, headers, output } => {
            let mut extra: Vec<(String, String)> = headers
                .iter()
                .map(|h| {
                    let (k, v) = h.split_once(':').with_context(|| {
                        format!("Invalid header (expected KEY:VALUE): {h}")
                    })?;
                    Ok((k.trim().to_string(), v.trim().to_string()))
                })
                .collect::<Result<_>>()?;

            // Attach session cookie for authenticated (non-ext) requests.
            if !cli.ext {
                let sid = ensure_session(
                    &odoo, cli.config.as_ref(), profile_name,
                    &db, &username, &password,
                )?;
                extra.push(("Cookie".to_string(), format!("session_id={sid}")));
            }

            if let Some(out_path) = output {
                let bytes = odoo.http_request_bytes(
                    &method, &path, body.as_deref(), &content_type, &extra,
                )?;
                // If saving to a .pdf file, verify we got a real PDF.
                // If not (session expired) — re-authenticate once and retry.
                let is_pdf_ext = out_path.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("pdf"))
                    .unwrap_or(false);
                let bytes = if is_pdf_ext && !bytes.starts_with(b"%PDF") && !cli.ext {
                    if is_login_redirect(&bytes) {
                        // Session expired — re-authenticate once and retry.
                        let new_sid = odoo.web_authenticate(&db, &username, &password)?;
                        save_session(cli.config.as_ref(), profile_name, &new_sid);
                        for (k, v) in &mut extra {
                            if k == "Cookie" { *v = format!("session_id={new_sid}"); }
                        }
                        let retry = odoo.http_request_bytes(
                            &method, &path, body.as_deref(), &content_type, &extra,
                        )?;
                        require_pdf(retry)?
                    } else {
                        require_pdf(bytes)? // Not a session issue — fail immediately.
                    }
                } else if is_pdf_ext {
                    require_pdf(bytes)?
                } else {
                    bytes
                };
                std::fs::write(&out_path, &bytes)
                    .with_context(|| format!("Cannot write: {}", out_path.display()))?;
                eprintln!("Wrote {} bytes → {}", bytes.len(), out_path.display());
            } else {
                let text = odoo.http_request(
                    &method, &path, body.as_deref(), &content_type, &extra,
                )?;
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(json) => println!("{}", serde_json::to_string_pretty(&json)?),
                    Err(_) => print!("{text}"),
                }
            }
        }
    }

    Ok(())
}
