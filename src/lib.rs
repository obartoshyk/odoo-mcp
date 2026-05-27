use anyhow::{bail, Context, Result};
use serde_json::{json, Value as Json};

pub const PAGE_SIZE: usize = 300;

/// Fetch pages via `fetch(offset, limit) -> Vec<Json>` until exhausted or `max` reached.
pub fn paginate<F>(start: usize, max: Option<usize>, fetch: F) -> Result<Vec<Json>>
where
    F: Fn(usize, usize) -> Result<Vec<Json>>,
{
    let mut all: Vec<Json> = Vec::new();
    let mut offset = start;
    loop {
        let limit = match max {
            Some(m) => PAGE_SIZE.min(m.saturating_sub(all.len())),
            None    => PAGE_SIZE,
        };
        if limit == 0 { break; }
        let page = fetch(offset, limit)?;
        let n = page.len();
        all.extend(page);
        offset += n;
        if n < limit { break; } // last page returned fewer than requested
        if max.is_some_and(|m| all.len() >= m) { break; }
    }
    Ok(all)
}

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

    /// Fetch all pages of a `search_read` query in chunks of `PAGE_SIZE`.
    /// If `max` is Some(n), stops after n total records.
    /// `offset` is the starting offset (from CLI `--offset`).
    pub fn search_read_all(
        &self,
        model: &str,
        domain: Json,
        fields: &[String],
        order: Option<&str>,
        offset: usize,
        max: Option<usize>,
    ) -> Result<Vec<Json>> {
        paginate(offset, max, |off, lim| {
            let mut kw = serde_json::Map::new();
            kw.insert("domain".into(), domain.clone());
            kw.insert("fields".into(), json!(fields));
            kw.insert("limit".into(),  json!(lim));
            kw.insert("offset".into(), json!(off));
            if let Some(ord) = order { kw.insert("order".into(), json!(ord)); }
            let result = self.execute_kw(model, "search_read", json!([]), Json::Object(kw))?;
            result.as_array().cloned().context("search_read returned non-array")
        })
    }

    /// Fetch all IDs of a `search` query in chunks of `PAGE_SIZE`.
    /// If `max` is Some(n), stops after n total IDs.
    /// `offset` is the starting offset.
    pub fn search_all(
        &self,
        model: &str,
        domain: Json,
        order: Option<&str>,
        offset: usize,
        max: Option<usize>,
    ) -> Result<Vec<Json>> {
        paginate(offset, max, |off, lim| {
            let mut kw = serde_json::Map::new();
            kw.insert("domain".into(), domain.clone());
            kw.insert("limit".into(),  json!(lim));
            kw.insert("offset".into(), json!(off));
            if let Some(ord) = order { kw.insert("order".into(), json!(ord)); }
            let result = self.execute_kw(model, "search", json!([]), Json::Object(kw))?;
            result.as_array().cloned().context("search returned non-array")
        })
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

    /// Web-session login via /web/session/authenticate. Returns the session_id cookie value.
    pub fn web_authenticate(&self, db: &str, username: &str, password: &str) -> Result<String> {
        let endpoint = format!("{}/web/session/authenticate", self.base_url);
        let body = json!({
            "jsonrpc": "2.0", "method": "call", "id": 1,
            "params": {"db": db, "login": username, "password": password}
        });
        let resp = self.http.post(&endpoint)
            .json(&body)
            .send()
            .context("web/session/authenticate request failed")?;

        let session_id = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                s.split(';').next()?.strip_prefix("session_id=").map(str::to_string)
            })
            .context("No session_id cookie in /web/session/authenticate response")?;

        Ok(session_id)
    }

    /// Like `http_request` but returns raw bytes. Useful for binary responses (PDF, etc.).
    pub fn http_request_bytes(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        content_type: &str,
        extra_headers: &[(String, String)],
    ) -> Result<Vec<u8>> {
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
        let bytes = resp.bytes().context("Failed to read response body")?;

        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes).chars().take(500).collect::<String>();
            bail!("HTTP {status}: {preview}");
        }

        Ok(bytes.to_vec())
    }
}
