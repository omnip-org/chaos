use axum::{
    Json,
    extract::rejection::{JsonRejection, QueryRejection},
    http::StatusCode,
    response::IntoResponse,
};
use chaos_application::ApplicationError;
use serde::Serialize;

#[derive(Debug)]
pub enum ApiError {
    Application(ApplicationError),
    Request {
        status: StatusCode,
        code: &'static str,
        message: &'static str,
    },
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<ErrorDetail>,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub field: &'static str,
    pub reason: String,
}

impl From<ApplicationError> for ApiError {
    fn from(value: ApplicationError) -> Self {
        Self::Application(value)
    }
}

impl From<chaos_domain::DomainError> for ApiError {
    fn from(value: chaos_domain::DomainError) -> Self {
        Self::Application(value.into())
    }
}

impl ApiError {
    pub fn from_json_rejection(rejection: JsonRejection) -> Self {
        let status = rejection.status();
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            return Self::Request {
                status,
                code: "payload_too_large",
                message: "the request body exceeds the allowed size",
            };
        }
        if status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            return Self::Request {
                status,
                code: "unsupported_media_type",
                message: "the request body must use application/json",
            };
        }
        Self::Request {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_json",
            message: "the request body is not valid JSON",
        }
    }

    pub fn from_query_rejection(_rejection: QueryRejection) -> Self {
        Self::Request {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_query",
            message: "the query parameters are invalid",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let retry_after = match &self {
            Self::Application(ApplicationError::RateLimited {
                retry_after_seconds,
            }) => Some(*retry_after_seconds),
            _ => None,
        };
        let (status, code, message, details) = match self {
            Self::Request {
                status,
                code,
                message,
            } => (status, code.into(), message.into(), vec![]),
            Self::Application(error) => match error {
                ApplicationError::Validation { violations } => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed".into(),
                    "one or more fields are invalid".into(),
                    violations
                        .into_iter()
                        .map(|item| ErrorDetail {
                            field: item.field,
                            reason: item.reason,
                        })
                        .collect(),
                ),
                ApplicationError::Unauthorized => (
                    StatusCode::UNAUTHORIZED,
                    "unauthorized".into(),
                    "authentication is required".into(),
                    vec![],
                ),
                ApplicationError::Forbidden => (
                    StatusCode::FORBIDDEN,
                    "forbidden".into(),
                    "you are not allowed to perform this operation".into(),
                    vec![],
                ),
                ApplicationError::NotFound { resource, id } => (
                    StatusCode::NOT_FOUND,
                    "not_found".into(),
                    format!("{resource} {id} was not found"),
                    vec![],
                ),
                ApplicationError::Conflict { code, message } => {
                    (StatusCode::CONFLICT, code.into(), message.into(), vec![])
                }
                ApplicationError::RateLimited { .. } => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited".into(),
                    "the request rate limit was exceeded".into(),
                    vec![],
                ),
                ApplicationError::Unavailable { service, source } => {
                    tracing::warn!(%service, error = %source, "application dependency unavailable");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "service_unavailable".into(),
                        "a required service is temporarily unavailable".into(),
                        vec![],
                    )
                }
                ApplicationError::Unexpected(source) => {
                    tracing::error!(error = %source, "unexpected application error");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal_error".into(),
                        "an unexpected error occurred".into(),
                        vec![],
                    )
                }
            },
        };

        let mut response = (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code,
                    message,
                    details,
                },
            }),
        )
            .into_response();
        if let Some(seconds) = retry_after
            && let Ok(value) = axum::http::HeaderValue::from_str(&seconds.to_string())
        {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{StatusCode, header::RETRY_AFTER};

    use super::*;

    #[test]
    fn rate_limit_errors_include_retry_after() {
        let response = ApiError::from(ApplicationError::RateLimited {
            retry_after_seconds: 37,
        })
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[RETRY_AFTER], "37");
    }
}
