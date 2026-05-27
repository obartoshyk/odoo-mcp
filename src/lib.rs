use anyhow::{bail, Context, Result};
use serde_json::{json, Value as Json};

// ── OdooClient ────────────────────────────────────────────────────────────────

pub struct OdooClient {
    base_url: String,
    db: String,
    password: String,
    http: reqwest::blocking::Client,
    pub uid: Option<i64>,
}

impl OdooClient {
    pub fn new(url: &str, db: &str, password: &str, http: reqwest::blocking::Client) -> Self {
        OdooClient {
            base_url: url.trim_end_matches('/').to_string(),
            db: db.to_string(),
            password: password.to_string(),
            http,
            uid: None,
        }
    }

    fn jsonrpc(&self, service: &str, method: &str, args: Json) -> Result<Json> {
        let endpoint = format!("{}/jsonrpc", self.base_url);
        let body = json!({
            "jsonrpc": "2.0",
            "method": "call",
            "id": 1,
            "params": {
                "service": service,
                "method": method,
                "args": args
            }
        });

        let resp = self
            .http
            .post(&endpoint)
            .json(&body)
            .send()
            .with_context(|| format!("POST {endpoint} failed"))?;

        let status = resp.status();
        let envelope: Json = resp
            .json()
            .context("Failed to parse JSON-RPC response")?;

        if !status.is_success() {
            bail!("HTTP {status}");
        }

        if let Some(error) = envelope.get("error") {
            let msg = error
                .pointer("/data/message")
                .and_then(Json::as_str)
                .or_else(|| error["message"].as_str())
                .unwrap_or("unknown error");
            bail!("Odoo error: {msg}");
        }

        Ok(envelope["result"].clone())
    }

    /// Authenticate and store uid. Returns the uid.
    pub fn authenticate(&mut self, username: &str) -> Result<i64> {
        let result = self.jsonrpc(
            "common",
            "authenticate",
            json!([self.db, username, self.password, {}]),
        )?;

        match &result {
            Json::Number(n) if n.as_i64().is_some_and(|v| v > 0) => {
                let uid = n.as_i64().unwrap();
                self.uid = Some(uid);
                Ok(uid)
            }
            _ => bail!("Authentication failed — check --username / --password / --db"),
        }
    }

    /// Call `execute_kw` on the object endpoint.
    pub fn execute_kw(
        &self,
        model: &str,
        method: &str,
        args: Json,
        kwargs: Json,
    ) -> Result<Json> {
        let uid = self.uid.context("Not authenticated — call authenticate() first")?;
        self.jsonrpc(
            "object",
            "execute_kw",
            json!([self.db, uid, self.password, model, method, args, kwargs]),
        )
    }

    /// Direct HTTP request to any Odoo endpoint (bypasses JSON-RPC).
    pub fn http_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        content_type: &str,
        extra_headers: &[(String, String)],
    ) -> Result<String> {
        let url = format!("{}{}", self.base_url, path);
        let mut builder = match method.to_uppercase().as_str() {
            "GET"    => self.http.get(&url),
            "POST"   => self.http.post(&url),
            "PUT"    => self.http.put(&url),
            "PATCH"  => self.http.patch(&url),
            "DELETE" => self.http.delete(&url),
            "HEAD"   => self.http.head(&url),
            other    => bail!("Unsupported HTTP method: {other}"),
        };

        for (k, v) in extra_headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        if let Some(b) = body {
            builder = builder.header("Content-Type", content_type).body(b.to_string());
        }

        let resp = builder
            .send()
            .with_context(|| format!("HTTP {method} {url} failed"))?;

        let status = resp.status();
        let text = resp.text().context("Failed to read response body")?;

        if !status.is_success() {
            bail!("HTTP {status}: {}", text.chars().take(500).collect::<String>());
        }

        Ok(text)
    }
}
