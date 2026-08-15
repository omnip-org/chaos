use axum::{Router, http::header::CONTENT_TYPE, response::IntoResponse, routing::get};

use super::ApiState;

const ADMIN_V1: &str = include_str!("../../../../openapi/admin-v1.json");
const STORE_V1: &str = include_str!("../../../../openapi/store-v1.json");
const WEBHOOKS_V1: &str = include_str!("../../../../openapi/webhooks-v1.json");

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/admin-v1.json", get(admin_v1))
        .route("/store-v1.json", get(store_v1))
        .route("/webhooks-v1.json", get(webhooks_v1))
}

async fn webhooks_v1() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/vnd.oai.openapi+json")],
        WEBHOOKS_V1,
    )
}

async fn store_v1() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/vnd.oai.openapi+json")],
        STORE_V1,
    )
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

    use super::{ADMIN_V1, STORE_V1, WEBHOOKS_V1};

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
    fn store_contract_is_separate_valid_openapi_with_resolved_references() {
        let specification: Value = serde_json::from_str(STORE_V1).unwrap();
        assert_eq!(specification["openapi"], "3.1.0");
        assert_eq!(specification["servers"][0]["url"], "/store/v1");
        assert_eq!(
            specification["paths"]["/products"]["get"]["security"],
            serde_json::json!([{ "publishableKey": ["catalog:read"] }])
        );
        assert_eq!(
            specification["paths"]["/carts"]["post"]["security"],
            serde_json::json!([{ "publishableKey": ["carts:write"] }])
        );
        assert_eq!(
            specification["paths"]["/carts/{cart_id}/checkout"]["post"]["security"],
            serde_json::json!([{ "publishableKey": ["checkout:write"] }])
        );
        assert_eq!(
            specification["paths"]["/checkouts/{checkout_id}"]["get"]["responses"]["200"]["content"]
                ["application/json"]["schema"]["$ref"],
            "#/components/schemas/CheckoutEnvelope"
        );
        for (path, method) in [
            ("/carts", "post"),
            ("/carts/{cart_id}", "get"),
            ("/carts/{cart_id}/lines/{product_variant_id}", "put"),
            ("/carts/{cart_id}/lines/{product_variant_id}", "delete"),
            ("/carts/{cart_id}/shipping-options", "post"),
            ("/carts/{cart_id}/checkout", "post"),
            ("/checkouts/{checkout_id}", "get"),
            ("/checkouts/{checkout_id}/order", "post"),
            ("/orders/{order_id}", "get"),
            ("/orders/{order_id}/payment-attempts", "post"),
            ("/payment-attempts/{payment_attempt_id}", "get"),
        ] {
            let parameters = specification["paths"][path][method]["parameters"]
                .as_array()
                .unwrap();
            assert!(
                parameters.iter().any(|parameter| {
                    parameter["$ref"] == "#/components/parameters/ShopperToken"
                })
            );
        }
        assert!(
            specification["components"]["schemas"]["Cart"]["properties"]
                .get("shopper_token")
                .is_some()
        );
        for schema in [
            "ShopperSession",
            "ShopperSessionEnvelope",
            "CreateCart",
            "SetCartLine",
            "CartLine",
            "Cart",
            "CartEnvelope",
            "CheckoutLine",
            "ShippingOption",
            "ShippingOptionCollectionEnvelope",
            "TaxCalculation",
            "PromotionCalculation",
            "Checkout",
            "CheckoutEnvelope",
            "OrderLine",
            "OrderTransition",
            "Order",
            "OrderEnvelope",
            "CreatePaymentAttempt",
            "PaymentAttempt",
            "PaymentAttemptEnvelope",
        ] {
            assert!(
                specification["components"]["schemas"].get(schema).is_some(),
                "Store contract must define {schema}"
            );
        }

        fn visit(value: &Value, root: &Value) {
            match value {
                Value::Object(object) => {
                    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                        let pointer = reference.strip_prefix('#').unwrap();
                        assert!(root.pointer(pointer).is_some(), "{reference} must resolve");
                    }
                    object.values().for_each(|child| visit(child, root));
                }
                Value::Array(array) => array.iter().for_each(|child| visit(child, root)),
                _ => {}
            }
        }
        visit(&specification, &specification);
    }

    #[test]
    fn webhook_contract_requires_signature_and_has_resolved_references() {
        let specification: Value = serde_json::from_str(WEBHOOKS_V1).unwrap();
        assert_eq!(specification["openapi"], "3.1.0");
        assert_eq!(specification["servers"][0]["url"], "/webhooks/v1");
        assert_eq!(
            specification["paths"]["/payments/{provider}"]["post"]["security"],
            serde_json::json!([{ "paymentSignature": [] }])
        );

        fn visit(value: &Value, root: &Value) {
            match value {
                Value::Object(object) => {
                    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                        let pointer = reference.strip_prefix('#').unwrap();
                        assert!(root.pointer(pointer).is_some(), "{reference} must resolve");
                    }
                    object.values().for_each(|child| visit(child, root));
                }
                Value::Array(array) => array.iter().for_each(|child| visit(child, root)),
                _ => {}
            }
        }
        visit(&specification, &specification);
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
        assert!(schemas.contains_key("StoreEnvelope"));
        assert!(schemas.contains_key("SalesChannelCollectionEnvelope"));
        assert!(schemas.contains_key("SalesChannelEnvelope"));
        assert!(schemas.contains_key("CreateInventoryLocation"));
        assert!(schemas.contains_key("AdjustInventory"));
        assert!(schemas.contains_key("InventoryLocationCollectionEnvelope"));
        assert!(schemas.contains_key("InventoryItemCollectionEnvelope"));
        assert!(schemas.contains_key("InventoryItemEnvelope"));
        assert!(schemas.contains_key("OrderEnvelope"));
        assert!(schemas.contains_key("RefundEnvelope"));
        assert!(schemas.contains_key("ProductCollectionEnvelope"));
        assert!(schemas.contains_key("ProductEnvelope"));
        assert!(schemas.contains_key("CreatePriceList"));
        assert!(schemas.contains_key("UpdatePriceList"));
        assert!(schemas.contains_key("PriceListCollectionEnvelope"));
        assert!(schemas.contains_key("PriceListEnvelope"));
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
