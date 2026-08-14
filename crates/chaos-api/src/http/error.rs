use axum::{Json, http::StatusCode, response::IntoResponse};
use chaos_application::ApplicationError;
use serde::Serialize;

#[derive(Debug)]
pub struct ApiError(pub ApplicationError);

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
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, code, message, details) = match self.0 {
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
        };

        (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code,
                    message,
                    details,
                },
            }),
        )
            .into_response()
    }
}
