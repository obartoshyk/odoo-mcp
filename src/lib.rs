use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use roxmltree::Document;

// ── Value ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Double(f64),
    String(String),
    Array(Vec<Value>),
    Struct(BTreeMap<String, Value>),
    Nil,
    Base64(Vec<u8>),
    DateTime(String),
}

impl Value {
    /// Serialize to an XML-RPC `<value>…</value>` element.
    pub fn to_xml(&self) -> String {
        match self {
            Value::Int(i) => format!("<value><int>{i}</int></value>"),
            Value::Bool(b) => format!("<value><boolean>{}</boolean></value>", *b as u8),
            Value::Double(d) => format!("<value><double>{d}</double></value>"),
            Value::String(s) => format!("<value><string>{}</string></value>", xml_escape(s)),
            Value::Nil => "<value><nil/></value>".to_string(),
            Value::Base64(b) => format!("<value><base64>{}</base64></value>", b64_encode(b)),
            Value::DateTime(dt) => {
                format!("<value><dateTime.iso8601>{dt}</dateTime.iso8601></value>")
            }
            Value::Array(arr) => {
                let data: String = arr.iter().map(|v| v.to_xml()).collect();
                format!("<value><array><data>{data}</data></array></value>")
            }
            Value::Struct(map) => {
                let members: String = map
                    .iter()
                    .map(|(k, v)| {
                        format!("<member><name>{}</name>{}</member>", xml_escape(k), v.to_xml())
                    })
                    .collect();
                format!("<value><struct>{members}</struct></value>")
            }
        }
    }

    /// Convert to `serde_json::Value` for CLI output.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Int(i) => serde_json::json!(i),
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Double(d) => serde_json::Number::from_f64(*d)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Nil => serde_json::Value::Null,
            Value::Base64(b) => serde_json::Value::String(b64_encode(b)),
            Value::DateTime(dt) => serde_json::Value::String(dt.clone()),
            Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(|v| v.to_json()).collect())
            }
            Value::Struct(map) => {
                let obj = map.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
                serde_json::Value::Object(obj)
            }
        }
    }
}

/// Convert `serde_json::Value` → `Value` for encoding CLI arguments.
pub fn from_json(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(arr) => Value::Array(arr.iter().map(from_json).collect()),
        serde_json::Value::Object(obj) => {
            Value::Struct(obj.iter().map(|(k, v)| (k.clone(), from_json(v))).collect())
        }
    }
}

// ── XML helpers ───────────────────────────────────────────────────────────────

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn build_call(method: &str, params: &[Value]) -> String {
    let params_xml: String = params
        .iter()
        .map(|p| format!("<param>{}</param>", p.to_xml()))
        .collect();
    format!(
        r#"<?xml version="1.0"?><methodCall><methodName>{method}</methodName><params>{params_xml}</params></methodCall>"#
    )
}

fn parse_response(xml: &str) -> Result<Value> {
    let doc = Document::parse(xml).context("Failed to parse XML-RPC response")?;
    let root = doc.root_element();

    if root.tag_name().name() != "methodResponse" {
        bail!("Expected <methodResponse>, got <{}>", root.tag_name().name());
    }

    if let Some(params) = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "params")
    {
        let param = params
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "param")
            .context("<params> is empty")?;
        let value_node = param
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "value")
            .context("<param> missing <value>")?;
        return parse_value(value_node);
    }

    if let Some(fault) = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "fault")
    {
        let value_node = fault
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "value")
            .context("<fault> missing <value>")?;
        let val = parse_value(value_node)?;
        if let Value::Struct(map) = val {
            let code = map
                .get("faultCode")
                .map(|v| match v {
                    Value::Int(i) => i.to_string(),
                    _ => "?".to_string(),
                })
                .unwrap_or_default();
            let msg = map
                .get("faultString")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => format!("{other:?}"),
                })
                .unwrap_or_default();
            // Odoo can't serialize None over XML-RPC — the method succeeded but returned null.
            if msg.contains("cannot marshal None") {
                return Ok(Value::Nil);
            }
            bail!("XML-RPC fault {code}: {msg}");
        }
        bail!("XML-RPC fault (unrecognized structure)");
    }

    bail!("Invalid XML-RPC response: neither <params> nor <fault> found");
}

fn parse_value(node: roxmltree::Node) -> Result<Value> {
    if let Some(type_node) = node.children().find(|n| n.is_element()) {
        match type_node.tag_name().name() {
            "int" | "i4" | "i8" => {
                let t = type_node.text().unwrap_or("0");
                Ok(Value::Int(t.trim().parse().with_context(|| format!("invalid int: {t}"))?))
            }
            "boolean" => Ok(Value::Bool(type_node.text().unwrap_or("0").trim() == "1")),
            "double" => {
                let t = type_node.text().unwrap_or("0.0");
                Ok(Value::Double(
                    t.trim().parse().with_context(|| format!("invalid double: {t}"))?,
                ))
            }
            "string" => Ok(Value::String(type_node.text().unwrap_or("").to_string())),
            "nil" => Ok(Value::Nil),
            "base64" => Ok(Value::Base64(b64_decode(type_node.text().unwrap_or("")))),
            "dateTime.iso8601" => {
                Ok(Value::DateTime(type_node.text().unwrap_or("").to_string()))
            }
            "array" => {
                let data = type_node
                    .children()
                    .find(|n| n.is_element() && n.tag_name().name() == "data")
                    .context("<array> missing <data>")?;
                let values = data
                    .children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "value")
                    .map(parse_value)
                    .collect::<Result<Vec<_>>>()?;
                Ok(Value::Array(values))
            }
            "struct" => {
                let mut map = BTreeMap::new();
                for member in type_node
                    .children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "member")
                {
                    let name = member
                        .children()
                        .find(|n| n.is_element() && n.tag_name().name() == "name")
                        .and_then(|n| n.text())
                        .unwrap_or("")
                        .to_string();
                    let val_node = member
                        .children()
                        .find(|n| n.is_element() && n.tag_name().name() == "value")
                        .with_context(|| format!("struct member '{name}' missing <value>"))?;
                    map.insert(name, parse_value(val_node)?);
                }
                Ok(Value::Struct(map))
            }
            other => bail!("Unknown XML-RPC type tag: <{other}>"),
        }
    } else {
        // Bare text inside <value> is treated as string per the spec.
        Ok(Value::String(node.text().unwrap_or("").to_string()))
    }
}

// ── Base64 ────────────────────────────────────────────────────────────────────

const B64_CHARS: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(((data.len() + 2) / 3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(B64_CHARS[b0 >> 2] as char);
        out.push(B64_CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(if chunk.len() > 1 {
            B64_CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_CHARS[b2 & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

fn b64_decode(s: &str) -> Vec<u8> {
    let mut table = [0xffu8; 128];
    for (i, &c) in B64_CHARS.iter().enumerate() {
        table[c as usize] = i as u8;
    }
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|b| !b" \t\r\n".contains(b))
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let a = table.get(chunk[0] as usize).copied().unwrap_or(0);
        let b = table.get(chunk[1] as usize).copied().unwrap_or(0);
        out.push((a << 2) | (b >> 4));
        if chunk.len() > 2 && chunk[2] != b'=' {
            let c = table.get(chunk[2] as usize).copied().unwrap_or(0);
            out.push((b << 4) | (c >> 2));
            if chunk.len() > 3 && chunk[3] != b'=' {
                let d = table.get(chunk[3] as usize).copied().unwrap_or(0);
                out.push((c << 6) | d);
            }
        }
    }
    out
}

// ── OdooClient ────────────────────────────────────────────────────────────────

pub struct OdooClient {
    base_url: String,
    common_url: String,
    object_url: String,
    db: String,
    password: String,
    http: reqwest::blocking::Client,
    pub uid: Option<i64>,
}

impl OdooClient {
    pub fn new(url: &str, db: &str, password: &str, http: reqwest::blocking::Client) -> Self {
        let base = url.trim_end_matches('/');
        OdooClient {
            base_url: base.to_string(),
            common_url: format!("{base}/xmlrpc/2/common"),
            object_url: format!("{base}/xmlrpc/2/object"),
            db: db.to_string(),
            password: password.to_string(),
            http,
            uid: None,
        }
    }

    fn rpc(&self, endpoint: &str, method: &str, params: &[Value]) -> Result<Value> {
        let body = build_call(method, params);
        let resp = self
            .http
            .post(endpoint)
            .header("Content-Type", "text/xml; charset=utf-8")
            .body(body)
            .send()
            .with_context(|| format!("HTTP POST to {endpoint} failed"))?;

        let status = resp.status();
        let text = resp.text().context("Failed to read response body")?;

        if !status.is_success() {
            bail!("HTTP {status}: {}", text.chars().take(300).collect::<String>());
        }

        parse_response(&text)
    }

    /// Authenticate and store uid. Returns the uid.
    pub fn authenticate(&mut self, username: &str) -> Result<i64> {
        let result = self.rpc(
            &self.common_url.clone(),
            "authenticate",
            &[
                Value::String(self.db.clone()),
                Value::String(username.to_string()),
                Value::String(self.password.clone()),
                Value::Struct(BTreeMap::new()),
            ],
        )?;

        match result {
            Value::Int(uid) => {
                self.uid = Some(uid);
                Ok(uid)
            }
            Value::Bool(false) => {
                bail!("Authentication failed — check --username / --password / --db")
            }
            other => bail!("Unexpected authenticate response: {other:?}"),
        }
    }

    /// Call `execute_kw` on the object endpoint.
    pub fn execute_kw(
        &self,
        model: &str,
        method: &str,
        args: Value,
        kwargs: Value,
    ) -> Result<Value> {
        let uid = self.uid.context("Not authenticated — call authenticate() first")?;
        self.rpc(
            &self.object_url,
            "execute_kw",
            &[
                Value::String(self.db.clone()),
                Value::Int(uid),
                Value::String(self.password.clone()),
                Value::String(model.to_string()),
                Value::String(method.to_string()),
                args,
                kwargs,
            ],
        )
    }

    /// Direct HTTP request to any Odoo endpoint (bypasses XML-RPC).
    ///
    /// Returns the raw response body as a string.
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
            "GET" => self.http.get(&url),
            "POST" => self.http.post(&url),
            "PUT" => self.http.put(&url),
            "PATCH" => self.http.patch(&url),
            "DELETE" => self.http.delete(&url),
            "HEAD" => self.http.head(&url),
            other => bail!("Unsupported HTTP method: {other}"),
        };

        for (k, v) in extra_headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        if let Some(b) = body {
            builder = builder
                .header("Content-Type", content_type)
                .body(b.to_string());
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
