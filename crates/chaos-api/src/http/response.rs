use axum::{Json, http::StatusCode, response::IntoResponse};
use chaos_application::ApplicationError;
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.envelope)).into_response()
    }
}

pub(super) fn format_time(value: OffsetDateTime) -> Result<String, ApplicationError> {
    value
        .format(&Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}
