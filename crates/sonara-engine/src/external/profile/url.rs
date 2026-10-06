//! The addresses a profile sends to: `Url`, the kinds' default base
//! URLs and the origins a stored key is bound to (spec 6.4).
use super::{Kind, Preset};
use serde_json::{Map, Value};

/// The parts of an `http(s)` URL Sonara uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    /// Lower case, IPv6 without brackets.
    pub host: String,
    pub port: Option<u16>,
    /// Starts with `/` or is empty.
    pub path: String,
}

impl Url {
    /// Absolute `http`/`https` only; no userinfo, query or fragment.
    pub fn parse(s: &str) -> Result<Url, String> {
        let bad = |why: &str| format!("invalid url '{s}': {why}");
        let (https, rest) = if let Some(r) = s.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(bad("use an absolute http:// or https:// address"));
        };
        if s.contains('?') || s.contains('#') {
            return Err(bad("no query or fragment"));
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(bad("no user name or password in the address"));
        }
        if s.chars().any(|c| c.is_control() || c == ' ') {
            return Err(bad("no spaces or control characters"));
        }
        // The host checked here must be exactly the one the HTTP client
        // connects to (spec 6.4): an IPv6 literal in brackets, else ASCII
        // letters, digits, '.', '-' and '_' only.
        let (host, port) = if let Some(r) = authority.strip_prefix('[') {
            let end = r.find(']').ok_or_else(|| bad("unclosed '['"))?;
            let port = match &r[end + 1..] {
                "" => None,
                rest => Some(
                    rest.strip_prefix(':')
                        .ok_or_else(|| bad("only a port may follow ']'"))?,
                ),
            };
            if r[..end].parse::<std::net::Ipv6Addr>().is_err() {
                return Err(bad("'[...]' must hold an IPv6 address"));
            }
            (r[..end].to_string(), port)
        } else {
            let (h, p) = match authority.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            };
            if !h
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            {
                return Err(bad(
                    "the host may hold only letters, digits, '.', '-' and '_'",
                ));
            }
            (h.to_string(), p)
        };
        if host.is_empty() {
            return Err(bad("no host"));
        }
        let port = match port {
            None => None,
            Some(p) => Some(p.parse::<u16>().map_err(|_| bad("bad port"))?),
        };
        Ok(Url {
            https,
            host: host.to_ascii_lowercase(),
            port,
            path: path.to_string(),
        })
    }

    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }

    /// `scheme://host:port`, the port always written (443 or 80 when the
    /// URL has none), an IPv6 host in brackets: what a stored key is bound
    /// to (spec 6.4).
    pub fn origin(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        let port = self.port.unwrap_or(if self.https { 443 } else { 80 });
        if self.host.contains(':') {
            format!("{scheme}://[{}]:{port}", self.host)
        } else {
            format!("{scheme}://{}:{port}", self.host)
        }
    }
}

/// An Azure region as it goes into a host name (`westeurope`).
fn region_ok(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 40
        && r.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The base URL a kind uses when the profile names no `url` (spec 5.4):
/// the provider's address, Azure's from `options.region`, the
/// `openai-compatible` preset's (only `openai` has one). `None` for a kind
/// without one (`command`, a local server's preset).
pub fn default_base(kind: Kind, options: &Map<String, Value>) -> Option<String> {
    match kind {
        Kind::OpenAiCompatible => options
            .get("preset")
            .and_then(Value::as_str)
            .and_then(Preset::parse)
            .unwrap_or(Preset::Generic)
            .default_url()
            .map(str::to_string),
        Kind::ElevenLabs => Some("https://api.elevenlabs.io".into()),
        Kind::Azure => options
            .get("region")
            .and_then(Value::as_str)
            .filter(|r| region_ok(r))
            .map(|r| format!("https://{r}.tts.speech.microsoft.com")),
        Kind::Google => Some("https://texttospeech.googleapis.com".into()),
        Kind::Gemini => Some("https://generativelanguage.googleapis.com".into()),
        Kind::Cartesia => Some("https://api.cartesia.ai".into()),
        Kind::Deepgram => Some("https://api.deepgram.com".into()),
        Kind::Command => None,
    }
}

/// The origin an `engines.json` entry sends to, for any kind, supported by
/// this build or not: its `url`, else the kind's default (`default_base`).
/// `None` when it has no valid address.
pub fn origin_of(raw: &Value) -> Option<String> {
    if raw.get("kind").and_then(Value::as_str) == Some(Kind::Command.as_str()) {
        return raw
            .get("options")
            .and_then(|o| o.get("argv"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(command_origin);
    }
    let url = raw
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let kind = Kind::parse(raw.get("kind").and_then(Value::as_str)?)?;
            let empty = Map::new();
            let options = raw
                .get("options")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            default_base(kind, options)
        })?;
    Url::parse(url.trim_end_matches('/'))
        .ok()
        .map(|u| u.origin())
}

/// What a `command` profile's key is bound to: its program, in lower case
/// (Windows paths ignore case), not its arguments. The key goes only into
/// that program's environment (spec 6.4).
pub fn command_origin(program: &str) -> String {
    format!("command:{}", program.to_lowercase())
}

/// `localhost`, `127.0.0.0/8` or `::1`.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") || h == "::1" {
        return true;
    }
    match h.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => h
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.is_loopback()),
    }
}
