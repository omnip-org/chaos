use axum::http::{HeaderMap, header::COOKIE};
use chaos_core::contracts::AnalyticsEventInput;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::response::parse_api_time;
use crate::http::ApiError;

const MAX_META_BROWSER_ID_BYTES: usize = 2_048;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnalyticsEventBody {
    pub(crate) event_id: Uuid,
    pub(crate) event_name: String,
    pub(crate) occurred_at: String,
    pub(crate) properties: Value,
}

impl AnalyticsEventBody {
    pub(crate) fn into_input(
        self,
        field_prefix: &'static str,
    ) -> Result<AnalyticsEventInput, ApiError> {
        if self.event_id.is_nil() {
            return Err(invalid_value("event_id", "must be a non-nil UUID"));
        }
        if !self.properties.is_object() {
            return Err(invalid_value("properties", "must be a JSON object"));
        }
        if self.properties.to_string().len() > 32 * 1024 {
            return Err(invalid_value("properties", "must not exceed 32768 bytes"));
        }
        Ok(AnalyticsEventInput {
            event_id: self.event_id,
            event_name: self.event_name,
            occurred_at: parse_api_time(&self.occurred_at)
                .map_err(|_| invalid_value(field_prefix, "must be an RFC 3339 timestamp"))?,
            properties: self.properties,
        })
    }
}

/// Event-captured attribution metadata stays authoritative. Cookies only fill
/// missing browser matching IDs because a queued event can be delivered after
/// the browser has landed on another campaign. Network headers from a
/// forwarding request are deliberately not copied into the event: a
/// server-side storefront SDK attaches the original edge context before the
/// request reaches Chaos.
pub(crate) fn request_meta(headers: &HeaderMap) -> Map<String, Value> {
    let mut meta = Map::new();
    if let Some(value) = cookie(headers, "_fbc").filter(|value| valid_meta_browser_id(value)) {
        meta.insert("fbc".into(), Value::String(value));
    }
    if let Some(value) = cookie(headers, "_fbp").filter(|value| valid_meta_browser_id(value)) {
        meta.insert("fbp".into(), Value::String(value));
    }
    meta
}

pub(crate) fn merge_request_meta(
    mut properties: Value,
    request_meta: &Map<String, Value>,
) -> Value {
    if request_meta.is_empty() {
        return properties;
    }
    let Some(object) = properties.as_object_mut() else {
        return properties;
    };
    let mut meta = object
        .remove("_meta")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    for (key, value) in request_meta {
        match key.as_str() {
            "fbc" | "fbp" => {
                let has_valid_event_value = meta
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(valid_meta_browser_id);
                if !has_valid_event_value {
                    meta.insert(key.clone(), value.clone());
                }
            }
            _ => {
                meta.insert(key.clone(), value.clone());
            }
        }
    }
    object.insert("_meta".into(), Value::Object(meta));
    properties
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|header| {
            header.split(';').find_map(|part| {
                let (key, value) = part.trim().split_once('=')?;
                (key == name && !value.trim().is_empty()).then(|| value.trim().to_owned())
            })
        })
        .filter(|value| value.len() <= MAX_META_BROWSER_ID_BYTES)
}

fn valid_meta_browser_id(value: &str) -> bool {
    if value.len() > MAX_META_BROWSER_ID_BYTES {
        return false;
    }
    let mut parts = value.splitn(4, '.');
    let (Some(prefix), Some(version), Some(timestamp), Some(suffix)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    prefix == "fb"
        && !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit())
        && timestamp.len() == 13
        && timestamp.bytes().all(|byte| byte.is_ascii_digit())
        && !suffix.is_empty()
        && !suffix.chars().any(char::is_whitespace)
}

fn invalid_value(field: &'static str, reason: &'static str) -> ApiError {
    chaos_core::ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::AnalyticsEventBody;
    use serde_json::json;
    use uuid::Uuid;

    fn event(properties: serde_json::Value) -> AnalyticsEventBody {
        AnalyticsEventBody {
            event_id: Uuid::now_v7(),
            event_name: "add_to_cart".into(),
            occurred_at: "2026-08-28T00:00:00Z".into(),
            properties,
        }
    }

    #[test]
    fn accepts_an_object_event_envelope() {
        assert!(
            event(json!({"_meta": {}}))
                .into_input("events.occurred_at")
                .is_ok()
        );
    }

    #[test]
    fn rejects_nil_ids_and_non_object_properties() {
        let mut nil_id = event(json!({}));
        nil_id.event_id = Uuid::nil();
        assert!(nil_id.into_input("events.occurred_at").is_err());

        assert!(event(json!([])).into_input("events.occurred_at").is_err());
    }
}
