use axum::{Router, http::header::CONTENT_TYPE, response::IntoResponse, routing::get};

use super::ApiState;

const ADMIN_V1: &str = include_str!("../../../../openapi/admin-v1.json");

pub(super) fn routes() -> Router<ApiState> {
    Router::new().route("/admin-v1.json", get(admin_v1))
}

async fn admin_v1() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/vnd.oai.openapi+json")],
        ADMIN_V1,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::Value;

    use super::ADMIN_V1;

    const HTTP_METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];
    const PUBLIC_OPERATIONS: [&str; 4] = [
        "requestEmailLink",
        "verifyEmailLink",
        "startPasskeyAuthentication",
        "finishPasskeyAuthentication",
    ];

    fn specification() -> Value {
        serde_json::from_str(ADMIN_V1).expect("the embedded OpenAPI contract must be valid JSON")
    }

    #[test]
    fn contract_is_openapi_31_with_unique_operation_ids() {
        let specification = specification();
        assert_eq!(specification["openapi"], "3.1.0");

        let mut operation_ids = HashSet::new();
        for path_item in specification["paths"]
            .as_object()
            .expect("paths must be an object")
            .values()
        {
            for method in HTTP_METHODS {
                let Some(operation) = path_item.get(method) else {
                    continue;
                };
                let operation_id = operation["operationId"]
                    .as_str()
                    .expect("every operation must define operationId");
                assert!(
                    operation_ids.insert(operation_id),
                    "operationId {operation_id} must be unique"
                );
            }
        }
    }

    #[test]
    fn protected_operations_declare_bearer_session_security() {
        let specification = specification();

        for path_item in specification["paths"].as_object().unwrap().values() {
            for method in HTTP_METHODS {
                let Some(operation) = path_item.get(method) else {
                    continue;
                };
                let operation_id = operation["operationId"].as_str().unwrap();
                let security = operation.get("security");

                if PUBLIC_OPERATIONS.contains(&operation_id) {
                    assert!(security.is_none(), "{operation_id} must remain public");
                } else {
                    assert_eq!(
                        security,
                        Some(&serde_json::json!([{ "bearerSession": [] }])),
                        "{operation_id} must require a bearer session"
                    );
                }
            }
        }
    }

    #[test]
    fn contract_defines_shared_response_envelopes() {
        let specification = specification();
        let schemas = specification["components"]["schemas"].as_object().unwrap();

        assert!(schemas.contains_key("ErrorEnvelope"));
        assert!(schemas.contains_key("MerchantAccountCollectionEnvelope"));
        assert!(schemas.contains_key("StoreCollectionEnvelope"));
        assert!(schemas.contains_key("ApiKeyCreatedEnvelope"));
        assert!(schemas.contains_key("ApiKeyCollectionEnvelope"));
        assert!(schemas.contains_key("PageMeta"));
    }

    #[test]
    fn every_local_reference_resolves() {
        fn visit(value: &Value, root: &Value) {
            match value {
                Value::Object(object) => {
                    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                        let pointer = reference
                            .strip_prefix('#')
                            .expect("only local OpenAPI references are allowed");
                        assert!(
                            root.pointer(pointer).is_some(),
                            "OpenAPI reference {reference} must resolve"
                        );
                    }
                    object.values().for_each(|child| visit(child, root));
                }
                Value::Array(array) => array.iter().for_each(|child| visit(child, root)),
                _ => {}
            }
        }

        let specification = specification();
        visit(&specification, &specification);
    }

    #[test]
    fn api_key_secret_is_declared_as_one_time_sensitive_response_data() {
        let specification = specification();
        let secret =
            &specification["components"]["schemas"]["ApiKeyCreated"]["properties"]["secret"];

        assert_eq!(secret["readOnly"], true);
        assert_eq!(secret["x-sensitive"], true);
        assert!(
            specification["components"]["schemas"]["ApiKey"]["properties"]
                .get("secret")
                .is_none()
        );
    }
}
