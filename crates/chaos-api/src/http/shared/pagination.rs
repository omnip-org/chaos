use axum::http::HeaderMap;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::ApplicationError;
use chaos_domain::FieldViolation;
use uuid::Uuid;

use super::{ApiError, PageMeta, ResponseMeta};

const IDEMPOTENCY_KEY: &str = "idempotency-key";
const CURSOR_VERSION: u8 = 1;

#[derive(Clone, Copy)]
pub(super) enum CursorKind {
    Product = 5,
    Collection = 15,
    Review = 17,
}

pub(super) fn page_limit(limit: Option<u16>) -> Result<u16, ApiError> {
    match limit.unwrap_or(20) {
        limit @ 1..=100 => Ok(limit),
        _ => Err(ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "limit",
                reason: "must be between 1 and 100".into(),
            }],
        }
        .into()),
    }
}

pub(super) fn encode_cursor(id: Uuid, kind: CursorKind) -> String {
    let mut payload = [0_u8; 18];
    payload[0] = CURSOR_VERSION;
    payload[1] = kind as u8;
    payload[2..].copy_from_slice(id.as_bytes());
    URL_SAFE_NO_PAD.encode(payload)
}

pub(super) fn decode_cursor(cursor: &str, expected_kind: CursorKind) -> Result<Uuid, ApiError> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok();
    bytes
        .as_deref()
        .filter(|value| {
            value.len() == 18 && value[0] == CURSOR_VERSION && value[1] == expected_kind as u8
        })
        .and_then(|value| Uuid::from_slice(&value[2..]).ok())
        .ok_or_else(|| {
            ApplicationError::Validation {
                violations: vec![FieldViolation {
                    field: "cursor",
                    reason: "must be a valid opaque cursor".into(),
                }],
            }
            .into()
        })
}

pub(super) fn page_meta(has_more: bool, next_cursor: Option<String>) -> ResponseMeta {
    ResponseMeta {
        page: Some(PageMeta {
            has_more,
            next_cursor,
        }),
    }
}

pub(super) fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get(IDEMPOTENCY_KEY)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .map(str::to_owned)
        .ok_or_else(|| {
            ApplicationError::Validation {
                violations: vec![FieldViolation {
                    field: "idempotency_key",
                    reason: "must be a non-empty Idempotency-Key header of at most 255 bytes"
                        .into(),
                }],
            }
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_is_bound_to_its_resource_kind() {
        let id = Uuid::now_v7();
        let cursor = encode_cursor(id, CursorKind::Product);
        assert_eq!(decode_cursor(&cursor, CursorKind::Product).unwrap(), id);
        assert!(decode_cursor(&cursor, CursorKind::Collection).is_err());
    }
}
