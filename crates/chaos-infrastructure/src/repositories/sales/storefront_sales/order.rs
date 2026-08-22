// Order line materialization and order detail reconstruction.

fn order_line_item(row: OrderLineRow) -> Result<OrderLineItem, ApplicationError> {
    Ok(OrderLineItem {
        product_id: ProductId::from_uuid(row.0),
        product_variant_id: ProductVariantId::from_uuid(row.1),
        product_title: row.2,
        variant_title: row.3,
        sku: row.4,
        requires_shipping: row.5,
        track_inventory: row.6,
        quantity: u32::try_from(row.7).map_err(unexpected_conversion)?,
        unit_price_amount_minor: row.8,
        subtotal_amount_minor: row.9,
        discount_amount_minor: row.10,
        tax_amount_minor: row.11,
        total_amount_minor: row.12,
        tax_inclusive: row.13,
    })
}

#[derive(Serialize, Deserialize)]
struct CartSnapshot {
    id: Uuid,
    shopper_id: Uuid,
    price_list_id: Uuid,
    currency: String,
    #[serde(default = "default_locale_snapshot")]
    locale: String,
    status: String,
    version: u64,
    lines: Vec<CartLineSnapshot>,
    subtotal_amount_minor: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct CartLineSnapshot {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    tax_inclusive: bool,
    #[serde(default)]
    media: Vec<CartLineMediaSnapshot>,
}

#[derive(Serialize, Deserialize)]
struct CartLineMediaSnapshot {
    id: Uuid,
    product_variant_id: Option<Uuid>,
    media_type: String,
    kind: String,
    alt_text: String,
    position: u16,
    url: String,
}

#[derive(Serialize, Deserialize)]
struct CheckoutSnapshot {
    id: Uuid,
    shopper_id: Uuid,
    cart_id: Uuid,
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    #[serde(default = "default_locale_snapshot")]
    locale: String,
    status: String,
    identity: CheckoutIdentitySnapshot,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    tax_rule: TaxRuleSnapshotData,
    promotion: Option<PromotionSnapshotData>,
    tax_inclusive: bool,
    shipping: Option<ShippingSelectionSnapshot>,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    expires_at: String,
    lines: Vec<CheckoutLineSnapshot>,
    created_at: String,
}

#[derive(Serialize, Deserialize)]
struct CheckoutLineSnapshot {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    requires_shipping: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize, Deserialize)]
struct OrderSnapshot {
    id: Uuid,
    order_number: String,
    shopper_id: Uuid,
    checkout_id: Uuid,
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    #[serde(default = "default_locale_snapshot")]
    locale: String,
    status: String,
    fulfillment_status: String,
    delivery_status: String,
    identity: CheckoutIdentitySnapshot,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    tax_rule: TaxRuleSnapshotData,
    promotion: Option<PromotionSnapshotData>,
    tax_inclusive: bool,
    shipping: Option<ShippingSelectionSnapshot>,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    lines: Vec<OrderLineSnapshot>,
    transitions: Vec<OrderTransitionSnapshot>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct OrderLineSnapshot {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize, Deserialize)]
struct OrderTransitionSnapshot {
    id: Uuid,
    from_status: Option<String>,
    to_status: String,
    kind: String,
    actor_user_id: Option<Uuid>,
    occurred_at: String,
}

#[derive(Serialize, Deserialize)]
struct ShippingSelectionSnapshot {
    service_id: Uuid,
    code: String,
    name: String,
    amount_minor: i64,
    currency: String,
    estimated_min_days: u16,
    estimated_max_days: u16,
}

#[derive(Serialize, Deserialize)]
struct TaxRuleSnapshotData {
    rule_id: Uuid,
    code: String,
    name: String,
    country_code: String,
    rate_basis_points: u32,
}

#[derive(Serialize, Deserialize)]
struct PromotionSnapshotData {
    promotion_id: Uuid,
    handle: String,
    name: String,
    trigger: String,
    redemption_code: Option<String>,
    value_kind: String,
    rate_basis_points: Option<u32>,
    amount_minor: Option<i64>,
    maximum_amount_minor: Option<i64>,
    currency: String,
    minimum_subtotal_amount_minor: i64,
    priority: u16,
    starts_at: Option<String>,
    ends_at: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CheckoutIdentitySnapshot {
    contact: CheckoutContactSnapshot,
    billing_address: PostalAddressSnapshot,
    shipping_address: Option<PostalAddressSnapshot>,
}

#[derive(Serialize, Deserialize)]
struct CheckoutContactSnapshot {
    email: String,
    phone: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct PostalAddressSnapshot {
    full_name: String,
    company: Option<String>,
    address_line1: String,
    address_line2: Option<String>,
    locality: String,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: String,
}
