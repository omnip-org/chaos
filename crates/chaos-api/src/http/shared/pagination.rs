use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_core::{ApplicationError, contracts::ReviewPageCursor};
use chaos_domain::{FieldViolation, catalog::ReviewId};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::http::{ApiError, PageMeta, ResponseMeta};

const CURSOR_VERSION: u8 = 1;
const REVIEW_CURSOR_VERSION: u8 = 2;

#[derive(Clone, Copy)]
pub(crate) enum CursorKind {
    Product = 5,
    Collection = 15,
    Review = 17,
}

pub(crate) fn page_limit(limit: Option<u16>) -> Result<u16, ApiError> {
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

pub(crate) fn encode_cursor(id: Uuid, kind: CursorKind) -> String {
    let mut payload = [0_u8; 18];
    payload[0] = CURSOR_VERSION;
    payload[1] = kind as u8;
    payload[2..].copy_from_slice(id.as_bytes());
    URL_SAFE_NO_PAD.encode(payload)
}

pub(crate) fn decode_cursor(cursor: &str, expected_kind: CursorKind) -> Result<Uuid, ApiError> {
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

pub(crate) fn encode_review_cursor(cursor: ReviewPageCursor) -> String {
    let mut payload = [0_u8; 34];
    payload[0] = REVIEW_CURSOR_VERSION;
    payload[1] = CursorKind::Review as u8;
    payload[2..18].copy_from_slice(&cursor.sort_at.unix_timestamp_nanos().to_be_bytes());
    payload[18..].copy_from_slice(cursor.id.as_uuid().as_bytes());
    URL_SAFE_NO_PAD.encode(payload)
}

pub(crate) fn decode_review_cursor(cursor: &str) -> Result<ReviewPageCursor, ApiError> {
    let decoded = URL_SAFE_NO_PAD.decode(cursor).ok();
    decoded
        .as_deref()
        .filter(|value| {
            value.len() == 34
                && value[0] == REVIEW_CURSOR_VERSION
                && value[1] == CursorKind::Review as u8
        })
        .and_then(|value| {
            let nanos = i128::from_be_bytes(value[2..18].try_into().ok()?);
            let sort_at = OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()?;
            let id = Uuid::from_slice(&value[18..]).ok()?;
            Some(ReviewPageCursor {
                sort_at,
                id: ReviewId::from_uuid(id),
            })
        })
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

pub(crate) fn page_meta(has_more: bool, next_cursor: Option<String>) -> ResponseMeta {
    ResponseMeta {
        page: Some(PageMeta {
            has_more,
            next_cursor,
        }),
    }
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

    #[test]
    fn review_cursor_keeps_timestamp_and_tie_breaker() {
        let id = ReviewId::new();
        let sort_at = OffsetDateTime::from_unix_timestamp_nanos(1_789_021_800_123_456_000).unwrap();
        let encoded = encode_review_cursor(ReviewPageCursor { sort_at, id });
        let decoded = decode_review_cursor(&encoded).unwrap();
        assert_eq!(decoded.sort_at, sort_at);
        assert_eq!(decoded.id, id);
        assert!(decode_review_cursor(&encode_cursor(id.as_uuid(), CursorKind::Review)).is_err());
        assert!(decode_cursor(&encoded, CursorKind::Product).is_err());
    }
}
