use axum::{
    Json,
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::{Serialize, Serializer};
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiDateTime(OffsetDateTime);

impl From<OffsetDateTime> for ApiDateTime {
    fn from(value: OffsetDateTime) -> Self {
        Self(value)
    }
}

impl Serialize for ApiDateTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        time::serde::rfc3339::serialize(&self.0, serializer)
    }
}

#[derive(Debug, Serialize)]
pub struct ResponseEnvelope<T> {
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ResponseMeta>,
}

#[derive(Debug, Serialize)]
pub struct ResponseMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<PageMeta>,
}

#[derive(Debug, Serialize)]
pub struct PageMeta {
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug)]
pub struct ApiResponse<T> {
    status: StatusCode,
    envelope: ResponseEnvelope<T>,
}

#[derive(Debug)]
pub struct PrivateApiResponse<T>(ApiResponse<T>);

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self::new(StatusCode::OK, data)
    }

    pub fn created(data: T) -> Self {
        Self::new(StatusCode::CREATED, data)
    }

    pub fn new(status: StatusCode, data: T) -> Self {
        Self {
            status,
            envelope: ResponseEnvelope { data, meta: None },
        }
    }

    pub fn with_meta(mut self, meta: ResponseMeta) -> Self {
        self.envelope.meta = Some(meta);
        self
    }

    /// Marks shopper-owned or credential-bearing data as unsuitable for any
    /// browser or intermediary cache.
    pub fn private(self) -> PrivateApiResponse<T> {
        PrivateApiResponse(self)
    }
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.envelope)).into_response()
    }
}

impl<T: Serialize> IntoResponse for PrivateApiResponse<T> {
    fn into_response(self) -> axum::response::Response {
        let mut response = self.0.into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    #[test]
    fn api_time_has_one_rfc3339_representation() {
        let value = OffsetDateTime::parse("2026-08-15T12:34:56.123456Z", &Rfc3339).unwrap();

        assert_eq!(
            serde_json::to_string(&ApiDateTime::from(value)).unwrap(),
            "\"2026-08-15T12:34:56.123456Z\""
        );
    }

    #[test]
    fn private_responses_disable_browser_and_intermediary_caching() {
        let response = ApiResponse::ok("secret").private().into_response();

        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
    }
}
