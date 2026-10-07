use chaos_core::contracts::{
    PaymentClientAction, PaymentClientActionKind as CorePaymentClientActionKind,
    StorefrontMediaAsset, StorefrontMediaScope,
};
use chaos_domain::{
    catalog::{MediaKind as DomainMediaKind, ReviewStatus as DomainReviewStatus},
    fulfillment::FulfillmentStatus as DomainFulfillmentStatus,
    integration::PaymentProvider as DomainPaymentProvider,
    sales::{
        CartStatus as DomainCartStatus, OrderPaymentStatus as DomainOrderPaymentStatus,
        OrderStatus as DomainOrderStatus,
    },
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CartStatus {
    Active,
    Locked,
    Completed,
    Abandoned,
}

impl From<DomainCartStatus> for CartStatus {
    fn from(value: DomainCartStatus) -> Self {
        match value {
            DomainCartStatus::Active => Self::Active,
            DomainCartStatus::Locked => Self::Locked,
            DomainCartStatus::Completed => Self::Completed,
            DomainCartStatus::Abandoned => Self::Abandoned,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MediaKind {
    Image,
    Video,
}

impl From<DomainMediaKind> for MediaKind {
    fn from(value: DomainMediaKind) -> Self {
        match value {
            DomainMediaKind::Image => Self::Image,
            DomainMediaKind::Video => Self::Video,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
enum MediaScope {
    Product,
    OptionValue {
        option_id: Uuid,
        option_value_id: Uuid,
    },
    Variant {
        product_variant_id: Uuid,
    },
}

impl From<StorefrontMediaScope> for MediaScope {
    fn from(value: StorefrontMediaScope) -> Self {
        match value {
            StorefrontMediaScope::Product => Self::Product,
            StorefrontMediaScope::OptionValue {
                option_id,
                option_value_id,
            } => Self::OptionValue {
                option_id: option_id.as_uuid(),
                option_value_id: option_value_id.as_uuid(),
            },
            StorefrontMediaScope::Variant { product_variant_id } => Self::Variant {
                product_variant_id: product_variant_id.as_uuid(),
            },
        }
    }
}

#[derive(Serialize)]
pub(super) struct MediaResponse {
    id: Uuid,
    #[serde(flatten)]
    scope: MediaScope,
    media_type: String,
    kind: MediaKind,
    alt_text: String,
    position: u16,
    url: String,
}

impl From<StorefrontMediaAsset> for MediaResponse {
    fn from(value: StorefrontMediaAsset) -> Self {
        Self {
            id: value.id.as_uuid(),
            scope: value.scope.into(),
            media_type: value.media_type,
            kind: value.kind.into(),
            alt_text: value.alt_text,
            position: value.position,
            url: value.url,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PaymentProvider {
    Stripe,
}

impl From<PaymentProvider> for DomainPaymentProvider {
    fn from(value: PaymentProvider) -> Self {
        match value {
            PaymentProvider::Stripe => Self::Stripe,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PaymentClientActionType {
    StripeCheckoutEmbedded,
}

impl From<CorePaymentClientActionKind> for PaymentClientActionType {
    fn from(value: CorePaymentClientActionKind) -> Self {
        match value {
            CorePaymentClientActionKind::StripeCheckoutEmbedded => Self::StripeCheckoutEmbedded,
        }
    }
}

#[derive(Serialize)]
pub(super) struct PaymentClientActionResponse {
    r#type: PaymentClientActionType,
    public_key: String,
    client_token: String,
}

impl From<PaymentClientAction> for PaymentClientActionResponse {
    fn from(value: PaymentClientAction) -> Self {
        Self {
            r#type: value.kind.into(),
            public_key: value.public_key.expose_secret().to_owned(),
            client_token: value.client_token.expose_secret().to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OrderStatus {
    Pending,
    Confirmed,
    Cancelled,
}

impl From<DomainOrderStatus> for OrderStatus {
    fn from(value: DomainOrderStatus) -> Self {
        match value {
            DomainOrderStatus::Pending => Self::Pending,
            DomainOrderStatus::Confirmed => Self::Confirmed,
            DomainOrderStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OrderPaymentStatus {
    Pending,
    Paid,
    Failed,
    Expired,
    PartiallyRefunded,
    Refunded,
}

impl From<DomainOrderPaymentStatus> for OrderPaymentStatus {
    fn from(value: DomainOrderPaymentStatus) -> Self {
        match value {
            DomainOrderPaymentStatus::Pending => Self::Pending,
            DomainOrderPaymentStatus::Paid => Self::Paid,
            DomainOrderPaymentStatus::Failed => Self::Failed,
            DomainOrderPaymentStatus::Expired => Self::Expired,
            DomainOrderPaymentStatus::PartiallyRefunded => Self::PartiallyRefunded,
            DomainOrderPaymentStatus::Refunded => Self::Refunded,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FulfillmentStatus {
    Pending,
    Shipped,
    Delivered,
    Cancelled,
}

impl From<DomainFulfillmentStatus> for FulfillmentStatus {
    fn from(value: DomainFulfillmentStatus) -> Self {
        match value {
            DomainFulfillmentStatus::Pending => Self::Pending,
            DomainFulfillmentStatus::Shipped => Self::Shipped,
            DomainFulfillmentStatus::Delivered => Self::Delivered,
            DomainFulfillmentStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
}

impl From<DomainReviewStatus> for ReviewStatus {
    fn from(value: DomainReviewStatus) -> Self {
        match value {
            DomainReviewStatus::Pending => Self::Pending,
            DomainReviewStatus::Approved => Self::Approved,
            DomainReviewStatus::Rejected => Self::Rejected,
        }
    }
}

#[cfg(test)]
mod tests {
    use chaos_core::contracts::StorefrontMediaScope;
    use chaos_domain::catalog::{
        MediaAssetId, MediaKind as DomainMediaKind, ProductOptionId, ProductOptionValueId,
        ProductVariantId,
    };
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_enums_serialize_to_the_existing_values() {
        assert_eq!(
            serde_json::to_value([
                CartStatus::Active,
                CartStatus::Locked,
                CartStatus::Completed,
                CartStatus::Abandoned,
            ])
            .unwrap(),
            json!(["active", "locked", "completed", "abandoned"])
        );
        assert_eq!(
            serde_json::to_value([MediaKind::Image, MediaKind::Video]).unwrap(),
            json!(["image", "video"])
        );
        assert_eq!(
            serde_json::to_value(PaymentClientActionType::StripeCheckoutEmbedded).unwrap(),
            json!("stripe_checkout_embedded")
        );
        assert_eq!(
            serde_json::to_value([
                OrderStatus::Pending,
                OrderStatus::Confirmed,
                OrderStatus::Cancelled,
            ])
            .unwrap(),
            json!(["pending", "confirmed", "cancelled"])
        );
        assert_eq!(
            serde_json::to_value([
                OrderPaymentStatus::Pending,
                OrderPaymentStatus::Paid,
                OrderPaymentStatus::Failed,
                OrderPaymentStatus::Expired,
                OrderPaymentStatus::PartiallyRefunded,
                OrderPaymentStatus::Refunded,
            ])
            .unwrap(),
            json!([
                "pending",
                "paid",
                "failed",
                "expired",
                "partially_refunded",
                "refunded"
            ])
        );
        assert_eq!(
            serde_json::to_value([
                FulfillmentStatus::Pending,
                FulfillmentStatus::Shipped,
                FulfillmentStatus::Delivered,
                FulfillmentStatus::Cancelled,
            ])
            .unwrap(),
            json!(["pending", "shipped", "delivered", "cancelled"])
        );
        assert_eq!(
            serde_json::to_value([
                ReviewStatus::Pending,
                ReviewStatus::Approved,
                ReviewStatus::Rejected,
            ])
            .unwrap(),
            json!(["pending", "approved", "rejected"])
        );
    }

    #[test]
    fn payment_provider_rejects_unknown_values() {
        assert_eq!(
            serde_json::from_value::<PaymentProvider>(json!("stripe")).unwrap(),
            PaymentProvider::Stripe
        );
        assert!(serde_json::from_value::<PaymentProvider>(json!("unknown")).is_err());
    }

    #[test]
    fn media_scope_serializes_only_its_valid_identifiers() {
        let option_id = Uuid::from_u128(1);
        let option_value_id = Uuid::from_u128(2);
        let variant_id = Uuid::from_u128(3);
        let media = |scope| {
            MediaResponse::from(StorefrontMediaAsset {
                id: MediaAssetId::from_uuid(Uuid::from_u128(4)),
                scope,
                media_type: "image/webp".into(),
                kind: DomainMediaKind::Image,
                alt_text: "Product image".into(),
                position: 0,
                url: "https://assets.example.test/product.webp".into(),
            })
        };

        let product = serde_json::to_value(media(StorefrontMediaScope::Product)).unwrap();
        assert_eq!(product["scope"], "product");
        assert!(product.get("option_id").is_none());
        assert!(product.get("product_variant_id").is_none());

        let option_value = serde_json::to_value(media(StorefrontMediaScope::OptionValue {
            option_id: ProductOptionId::from_uuid(option_id),
            option_value_id: ProductOptionValueId::from_uuid(option_value_id),
        }))
        .unwrap();
        assert_eq!(option_value["scope"], "option_value");
        assert_eq!(option_value["option_id"], option_id.to_string());
        assert_eq!(option_value["option_value_id"], option_value_id.to_string());
        assert!(option_value.get("product_variant_id").is_none());

        let variant = serde_json::to_value(media(StorefrontMediaScope::Variant {
            product_variant_id: ProductVariantId::from_uuid(variant_id),
        }))
        .unwrap();
        assert_eq!(variant["scope"], "variant");
        assert_eq!(variant["product_variant_id"], variant_id.to_string());
        assert!(variant.get("option_id").is_none());
    }
}
