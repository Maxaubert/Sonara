//! Contract tests (#275) of kind `azure` against Microsoft's Speech REST
//! docs (`tests/contracts/azure`): the required headers (SSML content
//! type, an `X-Microsoft-OutputFormat` from the documented list for every
//! format Sonara offers, a `User-Agent` under 255 characters, the
//! subscription key), the SSML body (version, language, voice, a prosody
//! rate within 0.5 to 2), the voice list on a regional and on a resource
//! host, and the documented status codes. No real provider is called.
mod common;

use common::contract::{self, engine, pcm_bytes, speak, Fragment, Mode};
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::adapter::{Adapter, VoiceSource};
use sonara_engine::external::azure::{Azure, AZURE_FORMATS};
use sonara_engine::external::keys::Secret;
use sonara_engine::{Engine, Reason};

const KEY: &str = "0123456789abcdef0123456789abcdef";
const VOICE: &str = "en-US-AvaMultilingualNeural";
const SPEAK: &str = "POST /cognitiveservices/v1";

fn profile(server: &ScriptServer, options: Value) -> Value {
    let mut v = json!({"id": "az", "kind": "azure", "url": server.base, "voice": VOICE,
        "send_mode": "sentence", "options": {"timeout_ms": 5000}});
    for (k, x) in options.as_object().unwrap() {
        v["options"][k] = x.clone();
    }
    v
}

/// The SSML body against the documented structure (`$defs/Ssml`).
fn ssml_ok(spec: &Fragment, body: &str, voice: &str, text: &str) {
    let rules = &spec.root["$defs"]["Ssml"]["x-ssml"];
    let attr = |tag: &str, name: &str| -> Option<String> {
        let start = body.find(&format!("<{tag} "))?;
        let open = &body[start..start + body[start..].find('>')?];
        let at = open.find(&format!("{name}="))? + name.len() + 1;
        let quote = open[at..].chars().next()?;
        let rest = &open[at + 1..];
        Some(rest[..rest.find(quote)?].to_string())
    };
    assert!(body.starts_with("<speak "), "{body}");
    assert!(body.ends_with("</speak>"), "{body}");
    assert_eq!(
        attr("speak", "version").as_deref(),
        rules["version"].as_str()
    );
    for a in rules["root_attributes"].as_array().unwrap() {
        assert!(
            attr("speak", a.as_str().unwrap()).is_some(),
            "{a} in {body}"
        );
    }
    assert_eq!(attr("voice", "name").as_deref(), Some(voice));
    let rate: f64 = attr("prosody", "rate").unwrap().parse().unwrap();
    let (lo, hi) = (
        rules["prosody_rate"]["minimum"].as_f64().unwrap(),
        rules["prosody_rate"]["maximum"].as_f64().unwrap(),
    );
    assert!((lo..=hi).contains(&rate), "rate {rate} in {body}");
    assert!(body.contains(text), "the text, escaped: {body}");
    for tag in ["speak", "voice", "prosody"] {
        assert_eq!(
            body.matches(&format!("<{tag} ")).count(),
            body.matches(&format!("</{tag}>")).count(),
            "{tag} closed in {body}"
        );
    }
}

#[test]
fn speak_requests_match_the_docs_for_every_offered_format() {
    let spec = Fragment::load("azure");
    for (format, rate) in AZURE_FORMATS {
        let server = ScriptServer::start();
        server.on(
            "/cognitiveservices/v1",
            Route::new(200, "audio/x-wav", pcm_bytes(&[3, 4])),
        );
        let e = engine(
            profile(&server, json!({"output_format": format})),
            Some(KEY),
        );
        for wpm in [100, 200, 250, 400] {
            let got = speak(&e, "Tom & Jerry <say> \"hi\".", "", wpm);
            assert_eq!(got[0].sample_rate, *rate);
        }
        for req in server.requests() {
            spec.assert_request(SPEAK, "", &req, &[]);
            assert_eq!(req.header("ocp-apim-subscription-key"), Some(KEY));
            assert_eq!(req.header("x-microsoft-outputformat"), Some(*format));
            let body = String::from_utf8(req.body.clone()).unwrap();
            ssml_ok(
                &spec,
                &body,
                VOICE,
                "Tom &amp; Jerry &lt;say&gt; &quot;hi&quot;.",
            );
        }
    }
}

#[test]
fn voice_lists_on_regional_and_resource_hosts() {
    let spec = Fragment::load("azure");
    let op = "GET /cognitiveservices/voices/list";
    let list = json!([
        {"Name": "Microsoft Server Speech Text to Speech Voice (en-US, AvaMultilingualNeural)",
            "DisplayName": "Ava Multilingual", "LocalName": "Ava Multilingual",
            "ShortName": VOICE, "Gender": "Female", "Locale": "en-US",
            "LocaleName": "English (United States)", "SecondaryLocaleList": ["de-DE"],
            "SampleRateHertz": "48000", "VoiceType": "Neural", "Status": "GA",
            "WordsPerMinute": "150"},
        {"Name": "Microsoft Server Speech Text to Speech Voice (zh-CN, YunxiNeural)",
            "DisplayName": "Yunxi", "LocalName": "云希", "ShortName": "zh-CN-YunxiNeural",
            "Gender": "Male", "Locale": "zh-CN", "LocaleName": "Chinese (Mandarin, Simplified)",
            "StyleList": ["narration-relaxed"], "SampleRateHertz": "24000",
            "VoiceType": "Neural", "Status": "GA"}
    ]);
    spec.assert_valid(
        &spec.response_schema(op, 200),
        &list,
        Mode::Response,
        "voices",
    );
    let server = ScriptServer::start();
    server.on(
        "/cognitiveservices/voices/list",
        Route::json(200, &list.to_string()),
    );
    let e = engine(profile(&server, json!({})), Some(KEY));
    let got: Vec<(String, String, String)> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.name, v.language))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                VOICE.into(),
                "Ava Multilingual (en-US)".into(),
                "en-US".into()
            ),
            (
                "zh-CN-YunxiNeural".into(),
                "云希 (zh-CN)".into(),
                "zh-CN".into()
            )
        ]
    );
    let req = &server.requests()[0];
    spec.assert_request(op, "", req, &[]);
    assert_eq!(req.header("ocp-apim-subscription-key"), Some(KEY));
    // A resource host: `/tts/cognitiveservices/voices/list` for the list,
    // `/cognitiveservices/v1` for speech (the docs' sample request).
    let a = Azure::new(&contract::profile(
        json!({"id": "az", "kind": "azure", "voice": VOICE,
        "url": "https://myspeech.cognitiveservices.azure.com"}),
    ));
    let key = Secret::new(KEY);
    match a.voices(Some(&key)) {
        VoiceSource::Fetch { request, .. } => assert_eq!(
            request.url,
            "https://myspeech.cognitiveservices.azure.com/tts/cognitiveservices/voices/list"
        ),
        VoiceSource::Fixed(_) => panic!("a fetched list"),
    }
    let speak = contract::captured(&a.synth_request("Hi.", VOICE, 200, Some(&key)));
    spec.assert_request(SPEAK, "", &speak, &[]);
}

#[test]
fn documented_status_codes_map_to_reasons() {
    let spec = Fragment::load("azure");
    let a = Azure::new(&contract::profile(
        json!({"id": "az", "kind": "azure", "voice": VOICE, "options": {"region": "westeurope"}}),
    ));
    let cases = [
        (400, Some(false), Reason::BadVoice),
        (400, None, Reason::BadConfig),
        (401, None, Reason::Auth),
        (415, None, Reason::BadConfig),
        (429, None, Reason::RateLimited),
        (502, None, Reason::Server),
        (503, None, Reason::Server),
    ];
    for (status, listed, want) in cases {
        assert!(
            spec.statuses(SPEAK).contains(&status),
            "{status} documented"
        );
        let e = a.map_error(
            &contract::raw_reply(status, "text/plain", b""),
            "en-US-Nobody",
            listed,
        );
        assert_eq!(e.reason, want, "{status} {listed:?}");
    }
}
