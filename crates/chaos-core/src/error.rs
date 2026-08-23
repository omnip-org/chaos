use chaos_domain::{DomainError, FieldViolation};

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("validation failed")]
    Validation { violations: Vec<FieldViolation> },
    #[error("authentication is required")]
    Unauthorized,
    #[error("the caller is not allowed to perform this operation")]
    Forbidden,
    #[error("{resource} was not found")]
    NotFound { resource: &'static str, id: String },
    #[error("{message}")]
    Conflict {
        code: &'static str,
        message: &'static str,
    },
    #[error("request rate limit exceeded; retry after {retry_after_seconds} seconds")]
    RateLimited { retry_after_seconds: u32 },
    #[error("dependency {service} is unavailable")]
    Unavailable {
        service: &'static str,
        #[source]
        source: anyhow::Error,
    },
    #[error("an unexpected application error occurred")]
    Unexpected(#[source] anyhow::Error),
}

impl From<DomainError> for ApplicationError {
    fn from(value: DomainError) -> Self {
        match value {
            DomainError::Validation(violations) => Self::Validation { violations },
        }
    }
}

pub(crate) fn database_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}
