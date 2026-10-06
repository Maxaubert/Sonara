//! Scrubbing a JSON value for the log, shared by `sonarad` (`in` lines)
//! and the hook (`hook.log` payloads), so both mask and clip the same way.
use serde_json::Value;

/// A JSON string longer than this (bytes) is clipped in the log.
pub const FIELD_MAX: usize = 4096;

/// Make `v` fit for the log, anywhere in it: the value of a field whose
/// name says it holds a secret is masked, credential-looking words in text
/// are masked (`mask`), and every string over `FIELD_MAX` bytes is clipped.
pub fn scrub(v: &mut Value) {
    match v {
        Value::String(s) => {
            if let std::borrow::Cow::Owned(m) = crate::mask(s) {
                *s = m;
            }
            if s.len() > FIELD_MAX {
                *s = crate::clip(s, FIELD_MAX).into_owned();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        Value::Object(o) => {
            for (k, v) in o.iter_mut() {
                if crate::secret_key(k) && !v.is_null() {
                    *v = Value::String(crate::MASK.into());
                } else {
                    scrub(v);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scrubbing_masks_secret_fields_anywhere_and_keeps_null_ones() {
        let mut v = json!({
            "api_key": "abc", "password": null,
            "nested": [{"Authorization": 7}, {"name": "x"}],
        });
        scrub(&mut v);
        assert_eq!(
            v,
            json!({
                "api_key": "[redacted]", "password": null,
                "nested": [{"Authorization": "[redacted]"}, {"name": "x"}],
            })
        );
    }

    #[test]
    fn scrubbing_masks_credentials_in_text_and_clips_long_strings() {
        let mut v = json!({"command": "curl -H 'Bearer abcdefgh123'", "n": 3, "ok": true});
        scrub(&mut v);
        let cmd = v["command"].as_str().unwrap();
        assert!(
            !cmd.contains("abcdefgh123") && cmd.contains(crate::MASK),
            "{cmd}"
        );
        assert_eq!((v["n"].clone(), v["ok"].clone()), (json!(3), json!(true)));

        let mut long = json!(["a".repeat(FIELD_MAX + 10)]);
        scrub(&mut long);
        assert_eq!(
            long[0].as_str().unwrap(),
            format!("{}...[+10 bytes]", "a".repeat(FIELD_MAX))
        );
        let mut exact = json!("a".repeat(FIELD_MAX));
        scrub(&mut exact);
        assert_eq!(exact.as_str().unwrap().len(), FIELD_MAX);
    }
}
