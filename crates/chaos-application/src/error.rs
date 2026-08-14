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
