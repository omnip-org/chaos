use axum::{Router, extract::State, routing::get};
use chaos_application::ApplicationError;
use serde::Serialize;

use crate::http::{ApiError, ApiResponse, ApiState};

#[derive(Serialize)]
struct HealthData {
    status: &'static str,
}

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/live", get(live))
        .route("/ready", get(ready))
}

async fn live() -> ApiResponse<HealthData> {
    ApiResponse::ok(HealthData { status: "ok" })
}

async fn ready(State(state): State<ApiState>) -> Result<ApiResponse<HealthData>, ApiError> {
    if !state.lifecycle.is_accepting_traffic() {
        return Err(ApplicationError::Unavailable {
            service: "instance",
            source: anyhow::anyhow!("instance is draining"),
        }
        .into());
    }

    match tokio::time::timeout(
        state.infrastructure.dependency_timeout,
        state.infrastructure.check_dependencies(),
    )
    .await
    {
        Ok(Ok(())) => Ok(ApiResponse::ok(HealthData { status: "ok" })),
        Ok(Err(source)) => Err(ApplicationError::Unavailable {
            service: "infrastructure",
            source,
        }
        .into()),
        Err(source) => Err(ApplicationError::Unavailable {
            service: "infrastructure",
            source: source.into(),
        }
        .into()),
    }
}
