mod mcp;
mod sources;

use std::path::PathBuf;

use anyhow::{Context, Result};
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

    let is_http_cmd    = matches!(&cli.command, Command::Http { .. });
    let is_source_cmd  = matches!(&cli.command, Command::UpdateSources);
    let is_no_auth_cmd = matches!(
        &cli.command,
        Command::Init | Command::Config { .. }
    );
    // Skip auth for --ext, direct HTTP, source management, and config commands.
    let needs_auth = !cli.ext && !is_http_cmd && !is_source_cmd && !is_no_auth_cmd;

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

    // For HTTP-only commands we don't need credentials.
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
            let mut kwargs = serde_json::Map::new();
            kwargs.insert("domain".into(), domain_val);
            if let Some(lim) = limit { kwargs.insert("limit".into(), json!(lim)); }
            if offset > 0 { kwargs.insert("offset".into(), json!(offset)); }
            if let Some(ord) = order { kwargs.insert("order".into(), json!(ord)); }
            let result = odoo.execute_kw(&model, "search", json!([]), Json::Object(kwargs))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
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

            let mut kwargs = serde_json::Map::new();
            kwargs.insert("domain".into(), domain_val);
            kwargs.insert("fields".into(), json!(fields));
            if let Some(lim) = limit {
                kwargs.insert("limit".into(), json!(lim));
            }
            if offset > 0 {
                kwargs.insert("offset".into(), json!(offset));
            }
            if let Some(ord) = order {
                kwargs.insert("order".into(), json!(ord));
            }

            let result = odoo.execute_kw(&model, "search_read", json!([]), Json::Object(kwargs))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Command::ExecuteKw { model, method, args, kwargs } => {
            let args_val: Json = serde_json::from_str(&args)
                .with_context(|| format!("Invalid args JSON: {args}"))?;
            let kwargs_val: Json = serde_json::from_str(&kwargs)
                .with_context(|| format!("Invalid kwargs JSON: {kwargs}"))?;

            let result = odoo.execute_kw(&model, &method, args_val, kwargs_val)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Command::Http { method, path, body, content_type, headers } => {
            let extra: Vec<(String, String)> = headers
                .iter()
                .map(|h| {
                    let (k, v) = h.split_once(':').with_context(|| {
                        format!("Invalid header (expected KEY:VALUE): {h}")
                    })?;
                    Ok((k.trim().to_string(), v.trim().to_string()))
                })
                .collect::<Result<_>>()?;

            let text = odoo.http_request(
                &method,
                &path,
                body.as_deref(),
                &content_type,
                &extra,
            )?;

            // Pretty-print JSON responses; fall back to raw text.
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => println!("{}", serde_json::to_string_pretty(&json)?),
                Err(_) => print!("{text}"),
            }
        }
    }

    Ok(())
}
