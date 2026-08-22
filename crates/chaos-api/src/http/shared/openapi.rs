use axum::{Router, http::header::CONTENT_TYPE, response::IntoResponse, routing::get};

use crate::http::ApiState;

const IDENTITY_V1: &str = include_str!("../../../../../openapi/identity-v1.json");
const STORE_V1: &str = include_str!("../../../../../openapi/store-v1.json");
const WEBHOOKS_V1: &str = include_str!("../../../../../openapi/webhooks-v1.json");

pub(crate) fn routes() -> Router<ApiState> {
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

    #[test]
    fn analytics_contract_matches_dynamic_event_ingestion() {
        let specification: Value = serde_json::from_str(STORE_V1).unwrap();
        let operation = &specification["paths"]["/analytics/events"]["post"];
        assert_eq!(
            operation["summary"],
            "Collect a bounded batch of first-party behavior events"
        );

        let event = &specification["components"]["schemas"]["BrowserAnalyticsEvent"];
        assert_eq!(
            event["required"],
            serde_json::json!(["event_id", "event_name", "occurred_at", "properties"])
        );
        assert_eq!(
            event["properties"]["event_name"]["pattern"],
            "^[a-z][a-z0-9_]{0,63}$"
        );
        assert_eq!(
            event["properties"]["properties"]["additionalProperties"],
            true
        );
        assert!(event["properties"].get("consent").is_none());

        assert_eq!(
            specification["components"]["schemas"]["AnalyticsCollectionResult"]["required"],
            serde_json::json!(["received", "stored", "duplicates"])
        );
    }

    #[test]
    fn webhook_contract_matches_the_provider_account_route() {
        let specification: Value = serde_json::from_str(WEBHOOKS_V1).unwrap();
        let paths = specification["paths"].as_object().unwrap();
        let path = "/payments/{provider}/{provider_account_id}";
        assert!(paths.contains_key(path));
        assert!(!paths.contains_key("/payments/{provider}"));

        let operation = &paths[path]["post"];
        assert_eq!(
            operation["security"],
            serde_json::json!([{ "stripeSignature": [] }])
        );
        assert_eq!(
            operation["parameters"][0]["schema"]["enum"],
            serde_json::json!(["stripe_checkout"])
        );
        assert_eq!(operation["parameters"][1]["name"], "provider_account_id");
        assert_eq!(operation["parameters"][1]["schema"]["format"], "uuid");
        assert_eq!(
            operation["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/StripeWebhookEvent"
        );
    }
}
