//! Contract test helpers (#275): a provider's schema fragment, extracted
//! from its official spec into `tests/contracts/<provider>/fragment.json`
//! (sources in each `SOURCES.md`), a small JSON Schema validator, and the
//! checks of one captured request against an operation of the fragment.
//!
//! Fragments come in two shapes: OpenAPI (`operations`, keyed
//! `"<METHOD> <path>"`, OpenAPI 3 or Swagger 2 inside) and Google discovery
//! (`methods`, keyed `resource.method`). Schemas live under `$defs` and
//! refer to each other as `#/$defs/<name>`.
//!
//! The validator covers the keywords the fragments use: `$ref`, `type`
//! (and OpenAPI 3.0 `nullable`), `enum`, `const`, `properties`,
//! `required`, `additionalProperties`, `items`, `anyOf`, `oneOf`,
//! `allOf`, `minimum`, `maximum`, `minLength`, `maxLength`, `pattern`.
//! A request body is checked strictly: a property the schema does not
//! name is an error unless the schema allows extra properties (a
//! misspelled field is ignored by most servers, silently). A response is
//! checked leniently: providers add fields.
use super::Captured;
use serde_json::Value;
use std::path::PathBuf;

/// How strictly unknown object properties are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Request,
    Response,
}

/// One parameter of an operation, OpenAPI or discovery.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    /// `query`, `header` or `path`.
    pub location: String,
    pub required: bool,
    pub schema: Value,
}

pub struct Fragment {
    pub name: String,
    pub root: Value,
}

/// `tests/contracts/<provider>`.
pub fn dir(provider: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("contracts")
        .join(provider)
}

impl Fragment {
    pub fn load(provider: &str) -> Fragment {
        let path = dir(provider).join("fragment.json");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        Fragment {
            name: provider.to_string(),
            root: serde_json::from_str(&text).unwrap_or_else(|e| panic!("{provider}: {e}")),
        }
    }

    /// A schema that refers to `$defs/<name>`.
    pub fn def(&self, name: &str) -> Value {
        assert!(
            self.root
                .pointer(&format!("/$defs/{}", escape(name)))
                .is_some(),
            "{}: no $defs/{name}",
            self.name
        );
        serde_json::json!({ "$ref": format!("#/$defs/{name}") })
    }

    fn op(&self, op: &str) -> &Value {
        self.root
            .get("operations")
            .and_then(|o| o.get(op))
            .or_else(|| self.root.get("methods").and_then(|m| m.get(op)))
            .unwrap_or_else(|| panic!("{}: no operation {op}", self.name))
    }

    /// The JSON request body schema of `op`: OpenAPI 3 `requestBody`,
    /// Swagger 2 `in: body`, or discovery `request`.
    pub fn request_schema(&self, op: &str) -> Value {
        let o = self.op(op);
        if let Some(r) = o.get("request") {
            return r.clone();
        }
        if let Some(c) = o.pointer("/requestBody/content") {
            let (_, media) = c.as_object().unwrap().iter().next().unwrap();
            return media["schema"].clone();
        }
        o.get("parameters")
            .and_then(Value::as_array)
            .and_then(|ps| ps.iter().find(|p| p["in"] == "body"))
            .map(|p| p["schema"].clone())
            .unwrap_or_else(|| panic!("{}: {op} has no request body", self.name))
    }

    /// The schema of `op`'s answer with `status` (discovery: `response`).
    pub fn response_schema(&self, op: &str, status: u16) -> Value {
        let o = self.op(op);
        if let Some(r) = o.get("response") {
            return r.clone();
        }
        let r = &o["responses"][status.to_string()];
        let r = self.resolve(r);
        if let Some(s) = r.get("schema") {
            return s.clone();
        }
        let c = r
            .get("content")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("{}: {op} {status} has no content", self.name));
        c.get("application/json")
            .or_else(|| c.values().next())
            .map(|m| m["schema"].clone())
            .unwrap()
    }

    /// The status codes `op` documents.
    pub fn statuses(&self, op: &str) -> Vec<u16> {
        self.op(op)
            .get("responses")
            .and_then(Value::as_object)
            .map(|r| r.keys().filter_map(|k| k.parse().ok()).collect())
            .unwrap_or_default()
    }

    /// The parameters of `op`, refs resolved.
    pub fn params(&self, op: &str) -> Vec<Param> {
        let o = self.op(op);
        match o.get("parameters") {
            Some(Value::Array(ps)) => ps
                .iter()
                .map(|p| self.resolve(p))
                .filter(|p| p["in"] != "body")
                .map(|p| Param {
                    name: p["name"].as_str().unwrap().to_string(),
                    location: p["in"].as_str().unwrap().to_string(),
                    required: p["required"].as_bool().unwrap_or(false),
                    // Swagger 2 keeps the type on the parameter itself.
                    schema: p.get("schema").cloned().unwrap_or_else(|| {
                        let mut s = p.clone();
                        for k in ["name", "in", "required", "description"] {
                            s.as_object_mut().unwrap().remove(k);
                        }
                        s
                    }),
                })
                .collect(),
            Some(Value::Object(ps)) => ps
                .iter()
                .map(|(name, p)| Param {
                    name: name.clone(),
                    location: p["location"].as_str().unwrap_or("query").to_string(),
                    required: p["required"].as_bool().unwrap_or(false),
                    schema: p.clone(),
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn resolve<'a>(&'a self, v: &'a Value) -> &'a Value {
        match v.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let name = r.strip_prefix("#/$defs/").expect("a $defs ref");
                self.root["$defs"]
                    .get(name)
                    .unwrap_or_else(|| panic!("{}: dangling {r}", self.name))
            }
            None => v,
        }
    }

    /// Errors of `instance` against `schema`, each with its JSON pointer.
    pub fn errors(&self, schema: &Value, instance: &Value, mode: Mode) -> Vec<String> {
        let mut out = Vec::new();
        self.check(schema, instance, "", mode, &mut out);
        out
    }

    /// Panics with every error of `instance` against `schema`.
    pub fn assert_valid(&self, schema: &Value, instance: &Value, mode: Mode, what: &str) {
        let errs = self.errors(schema, instance, mode);
        assert!(
            errs.is_empty(),
            "{} {what}: does not match the spec:\n  {}\ninstance: {instance}",
            self.name,
            errs.join("\n  ")
        );
    }

    fn check(&self, schema: &Value, inst: &Value, at: &str, mode: Mode, out: &mut Vec<String>) {
        let Some(s) = schema.as_object() else {
            return;
        };
        if let Some(r) = s.get("$ref") {
            let target = self.resolve(&serde_json::json!({ "$ref": r })).clone();
            self.check(&target, inst, at, mode, out);
        }
        if inst.is_null() && s.get("nullable") == Some(&Value::Bool(true)) {
            return;
        }
        let here = |m: String| format!("{}: {m}", if at.is_empty() { "/" } else { at });
        for key in ["anyOf", "oneOf"] {
            if let Some(list) = s.get(key).and_then(Value::as_array) {
                let passing = list
                    .iter()
                    .filter(|b| self.errors(b, inst, mode).is_empty())
                    .count();
                // `oneOf` is read as "at least one": published specs list
                // overlapping variants (Deepgram's sample rates per
                // encoding, answers with extra fields), which no value
                // could match exactly once. Tests check the variant that
                // applies where it matters.
                if passing == 0 {
                    let why: Vec<String> = list
                        .iter()
                        .map(|b| self.errors(b, inst, mode).join("; "))
                        .collect();
                    out.push(here(format!(
                        "{key}: {passing} of {} branches match ({})",
                        list.len(),
                        why.join(" | ")
                    )));
                }
            }
        }
        if let Some(list) = s.get("allOf").and_then(Value::as_array) {
            for b in list {
                self.check(b, inst, at, mode, out);
            }
        }
        if let Some(t) = s.get("type") {
            let types: Vec<&str> = match t {
                Value::String(t) => vec![t.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if !types.iter().any(|t| type_ok(t, inst)) {
                out.push(here(format!("expected {types:?}, got {inst}")));
                return;
            }
        }
        if let Some(e) = s.get("enum").and_then(Value::as_array) {
            if !e.contains(inst) {
                out.push(here(format!(
                    "{inst} is not one of {}",
                    Value::Array(e.clone())
                )));
            }
        }
        if let Some(c) = s.get("const") {
            if c != inst {
                out.push(here(format!("{inst} is not {c}")));
            }
        }
        if let Some(x) = inst.as_f64() {
            if let Some(min) = s.get("minimum").and_then(Value::as_f64) {
                if x < min {
                    out.push(here(format!("{x} < minimum {min}")));
                }
            }
            if let Some(max) = s.get("maximum").and_then(Value::as_f64) {
                if x > max {
                    out.push(here(format!("{x} > maximum {max}")));
                }
            }
        }
        if let Some(text) = inst.as_str() {
            let n = text.chars().count() as u64;
            if let Some(min) = s.get("minLength").and_then(Value::as_u64) {
                if n < min {
                    out.push(here(format!("length {n} < minLength {min}")));
                }
            }
            if let Some(max) = s.get("maxLength").and_then(Value::as_u64) {
                if n > max {
                    out.push(here(format!("length {n} > maxLength {max}")));
                }
            }
            if let Some(p) = s.get("pattern").and_then(Value::as_str) {
                if !regex::Regex::new(p).unwrap().is_match(text) {
                    out.push(here(format!("{text:?} does not match {p}")));
                }
            }
        }
        if let Some(items) = inst.as_array() {
            if let Some(item) = s.get("items") {
                for (i, v) in items.iter().enumerate() {
                    self.check(item, v, &format!("{at}/{i}"), mode, out);
                }
            }
        }
        if let Some(obj) = inst.as_object() {
            let props = s.get("properties").and_then(Value::as_object);
            if let Some(req) = s.get("required").and_then(Value::as_array) {
                for r in req.iter().filter_map(Value::as_str) {
                    if !obj.contains_key(r) {
                        out.push(here(format!("missing required property '{r}'")));
                    }
                }
            }
            for (k, v) in obj {
                let path = format!("{at}/{}", escape(k));
                match props.and_then(|p| p.get(k)) {
                    Some(ps) => self.check(ps, v, &path, mode, out),
                    None => match s.get("additionalProperties") {
                        Some(Value::Bool(false)) => {
                            out.push(here(format!("property '{k}' is not allowed")))
                        }
                        Some(Value::Bool(true)) => {}
                        Some(extra) => self.check(extra, v, &path, mode, out),
                        // A request may only carry what the schema names
                        // (when it names any, and is not a union).
                        None if mode == Mode::Request
                            && props.is_some()
                            && s.get("anyOf").is_none()
                            && s.get("oneOf").is_none() =>
                        {
                            out.push(here(format!("property '{k}' is not in the spec")))
                        }
                        None => {}
                    },
                }
            }
        }
    }

    /// Checks a captured request against `op`: the method, the path
    /// (`prefix` is the server's base path, such as `/v1`), every query
    /// parameter documented and valid, required query and header
    /// parameters present, header values valid, and a JSON body against
    /// the request schema (strictly). `undocumented` names query
    /// parameters the provider documents outside this spec (each with
    /// its source in the test). Returns the path parameters.
    pub fn assert_request(
        &self,
        op: &str,
        prefix: &str,
        req: &Captured,
        undocumented: &[&str],
    ) -> Vec<(String, String)> {
        let o = self.op(op);
        let (method, template) = match o.get("httpMethod") {
            Some(m) => (
                m.as_str().unwrap().to_string(),
                format!(
                    "/{}",
                    o.get("flatPath")
                        .or_else(|| o.get("path"))
                        .and_then(Value::as_str)
                        .unwrap()
                ),
            ),
            None => {
                let (m, p) = op.split_once(' ').unwrap();
                (m.to_string(), p.to_string())
            }
        };
        assert_eq!(req.method, method, "{} {op}: method", self.name);
        let (path, query) = match req.path.split_once('?') {
            Some((p, q)) => (p, q),
            None => (req.path.as_str(), ""),
        };
        let path = path
            .strip_prefix(prefix)
            .unwrap_or_else(|| panic!("{} {op}: {path} is not under {prefix}", self.name));
        let path_params = match_template(&template, path)
            .unwrap_or_else(|| panic!("{} {op}: path {path} does not match {template}", self.name));
        let params = self.params(op);
        let pairs = parse_query(query);
        for (k, v) in &pairs {
            if undocumented.contains(&k.as_str()) {
                continue;
            }
            let p = params
                .iter()
                .find(|p| p.location == "query" && &p.name == k)
                .unwrap_or_else(|| {
                    panic!(
                        "{} {op}: query parameter '{k}' is not in the spec",
                        self.name
                    )
                });
            let ok = coerced(v)
                .iter()
                .any(|c| self.errors(&p.schema, c, Mode::Request).is_empty());
            assert!(
                ok,
                "{} {op}: query {k}={v} does not match {}: {}",
                self.name,
                p.schema,
                self.errors(&p.schema, &Value::String(v.clone()), Mode::Request)
                    .join("; ")
            );
        }
        for p in params.iter().filter(|p| p.required) {
            match p.location.as_str() {
                "query" => assert!(
                    pairs.iter().any(|(k, _)| k == &p.name),
                    "{} {op}: required query parameter '{}' missing",
                    self.name,
                    p.name
                ),
                "header" => assert!(
                    req.header(&p.name).is_some(),
                    "{} {op}: required header '{}' missing",
                    self.name,
                    p.name
                ),
                _ => {}
            }
        }
        for p in params.iter().filter(|p| p.location == "header") {
            if let Some(v) = req.header(&p.name) {
                self.assert_valid(
                    &p.schema,
                    &Value::String(v.to_string()),
                    Mode::Request,
                    &format!("{op} header {}", p.name),
                );
            }
        }
        if !req.body.is_empty()
            && req
                .header("content-type")
                .is_some_and(|c| c.starts_with("application/json"))
        {
            self.assert_valid(
                &self.request_schema(op),
                &req.json(),
                Mode::Request,
                &format!("{op} body"),
            );
        }
        path_params
    }
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.as_f64().is_some_and(|x| x.fract() == 0.0),
        "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "null" => v.is_null(),
        _ => true,
    }
}

/// A JSON pointer token.
fn escape(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

/// The values a query string may stand for: itself, a number, a boolean.
fn coerced(v: &str) -> Vec<Value> {
    let mut out = vec![Value::String(v.to_string())];
    if let Ok(n) = v.parse::<i64>() {
        out.push(Value::from(n));
    }
    if let Ok(x) = v.parse::<f64>() {
        out.push(Value::from(x));
    }
    if let Ok(b) = v.parse::<bool>() {
        out.push(Value::Bool(b));
    }
    out
}

/// `a=1&b=x%20y` to pairs, percent-decoded.
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(x)) => {
                out.push(x);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).expect("UTF-8 after decoding")
}

/// `/v1/text-to-speech/{voice_id}` against a path: the parameters, or
/// `None`. A parameter is one path segment (up to `/` or `:`), decoded.
pub fn match_template(template: &str, path: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let (mut t, mut p) = (template, path);
    while !t.is_empty() {
        if let Some(rest) = t.strip_prefix('{') {
            let end = rest.find('}')?;
            let name = rest[..end].trim_start_matches('+').to_string();
            t = &rest[end + 1..];
            let stop = t.chars().next();
            let len = p
                .find(|c: char| c == '/' || Some(c) == stop)
                .unwrap_or(p.len());
            if len == 0 {
                return None;
            }
            out.push((name, decode(&p[..len])));
            p = &p[len..];
        } else {
            let lit = t.find('{').unwrap_or(t.len());
            p = p.strip_prefix(&t[..lit])?;
            t = &t[lit..];
        }
    }
    p.is_empty().then_some(out)
}

/// An engine for `profile` (its `url` set by the caller) with `key` in
/// memory under the profile's id.
pub fn engine(profile: Value, key: Option<&str>) -> sonara_engine::external::External {
    use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
    use sonara_engine::external::profile::Profile;
    let id = profile["id"].as_str().unwrap().to_string();
    let store = std::sync::Arc::new(MemoryStore::new());
    let p = Profile::from_json(&profile).unwrap_or_else(|e| panic!("{profile}: {e:?}"));
    if let Some(k) = key {
        store
            .set(&id, &Secret::new(k), &p.origin().unwrap())
            .unwrap();
    }
    let config = sonara_engine::external::ExternalConfig::new(p, KeyResolver::new(store));
    sonara_engine::external::External::new(config).unwrap()
}

/// The profile of `v` (for an adapter built directly).
pub fn profile(v: Value) -> sonara_engine::external::profile::Profile {
    sonara_engine::external::profile::Profile::from_json(&v)
        .unwrap_or_else(|e| panic!("{v}: {e:?}"))
}

/// A provider's answer with a JSON body.
pub fn reply(status: u16, body: &Value) -> sonara_engine::external::adapter::HttpReply {
    sonara_engine::external::adapter::HttpReply {
        status,
        content_type: Some("application/json".into()),
        retry_after: None,
        body: body.to_string().into_bytes(),
    }
}

/// A provider's answer with a raw body.
pub fn raw_reply(
    status: u16,
    content_type: &str,
    body: &[u8],
) -> sonara_engine::external::adapter::HttpReply {
    sonara_engine::external::adapter::HttpReply {
        status,
        content_type: Some(content_type.into()),
        retry_after: None,
        body: body.to_vec(),
    }
}

/// Little-endian 16-bit PCM bytes.
pub fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Every sample of one synthesis.
pub fn speak(
    e: &sonara_engine::external::External,
    text: &str,
    voice: &str,
    wpm: u32,
) -> Vec<sonara_engine::PcmChunk> {
    use sonara_engine::Engine;
    e.synthesize(text, voice, wpm)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// An adapter's request as the server would capture it (built directly,
/// without HTTP): the path with its query, header names in lower case.
pub fn captured(req: &sonara_engine::external::adapter::HttpRequest) -> Captured {
    use sonara_engine::external::adapter::Method;
    let after_scheme = req
        .url
        .split_once("://")
        .map_or(req.url.as_str(), |(_, r)| r);
    let path = after_scheme
        .find('/')
        .map_or("/".to_string(), |i| after_scheme[i..].to_string());
    Captured {
        method: match req.method {
            Method::Get => "GET".into(),
            Method::Post => "POST".into(),
        },
        path,
        headers: req
            .headers
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect(),
        body: req.body.clone().unwrap_or_default(),
    }
}

/// A google.rpc.Status error body (Cloud Text-to-Speech, Gemini).
pub fn rpc_error(code: u16, status: &str, message: &str, details: Value) -> Value {
    serde_json::json!({"error": {"code": code, "message": message, "status": status,
        "details": details}})
}
