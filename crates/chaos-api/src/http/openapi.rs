use axum::{Router, http::header::CONTENT_TYPE, response::IntoResponse, routing::get};

use super::ApiState;

const IDENTITY_V1: &str = include_str!("../../../../openapi/identity-v1.json");
const STORE_V1: &str = include_str!("../../../../openapi/store-v1.json");
const WEBHOOKS_V1: &str = include_str!("../../../../openapi/webhooks-v1.json");

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/identity-v1.json", get(identity_v1))
        .route("/store-v1.json", get(store_v1))
        .route("/webhooks-v1.json", get(webhooks_v1))
}

async fn identity_v1() -> impl IntoResponse {
    contract(IDENTITY_V1)
}

async fn store_v1() -> impl IntoResponse {
    contract(STORE_V1)
}

async fn webhooks_v1() -> impl IntoResponse {
    contract(WEBHOOKS_V1)
}

fn contract(value: &'static str) -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/vnd.oai.openapi+json")], value)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{IDENTITY_V1, STORE_V1, WEBHOOKS_V1};

    #[test]
    fn published_contracts_are_openapi_31_documents() {
        for document in [IDENTITY_V1, STORE_V1, WEBHOOKS_V1] {
            let specification: Value = serde_json::from_str(document).unwrap();
            assert_eq!(specification["openapi"], "3.1.0");
        }
    }

    #[test]
    fn identity_contract_contains_only_identity_bootstrap_operations() {
        let specification: Value = serde_json::from_str(IDENTITY_V1).unwrap();
        let paths = specification["paths"].as_object().unwrap();
        assert_eq!(paths.len(), 3);
        assert!(paths.contains_key("/auth/external"));
        assert!(paths.contains_key("/access-keys"));
        assert!(paths.contains_key("/access-keys/{access_key_id}"));
    }
}
