use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    CurrencyCode, DomainError, FieldViolation,
    catalog::{ProductId, ProductVariantId},
    fulfillment::ShippingSelection,
    inventory::InventoryReservationId,
    merchant::{SalesChannelId, StoreId},
    pricing::{Money, PriceListId, PromotionSnapshot, TaxRuleSnapshot},
};

use super::CheckoutIdentity;

macro_rules! sales_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

sales_id!(CartId);
sales_id!(CheckoutId);
sales_id!(ShopperId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CartStatus {
    Active,
    Completed,
    Abandoned,
}

impl CartStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "completed" => Some(Self::Completed),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartLine {
    product_id: ProductId,
    product_variant_id: ProductVariantId,
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price: Money,
    tax_inclusive: bool,
}

impl CartLine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        product_id: ProductId,
        product_variant_id: ProductVariantId,
        product_title: impl Into<String>,
        variant_title: impl Into<String>,
        sku: Option<String>,
        requires_shipping: bool,
        track_inventory: bool,
        quantity: u32,
        unit_price: Money,
        tax_inclusive: bool,
    ) -> Result<Self, DomainError> {
        require_quantity(quantity)?;
        if unit_price.amount_minor() < 0 {
            return Err(validation("unit_price", "must be zero or greater"));
        }
        let product_title = product_title.into();
        let variant_title = variant_title.into();
        validate_snapshot_text("product_title", &product_title, 255)?;
        validate_snapshot_text("variant_title", &variant_title, 255)?;
        if sku.as_ref().is_some_and(|value| {
            value.trim().is_empty()
                || value.chars().count() > 64
                || value.chars().any(char::is_control)
        }) {
            return Err(validation("sku", "must contain 1-64 printable characters"));
        }
        Ok(Self {
            product_id,
            product_variant_id,
            product_title,
            variant_title,
            sku,
            requires_shipping,
            track_inventory,
            quantity,
            unit_price,
            tax_inclusive,
        })
    }

    pub const fn product_id(&self) -> ProductId {
        self.product_id
    }

    pub const fn product_variant_id(&self) -> ProductVariantId {
        self.product_variant_id
    }

    pub fn product_title(&self) -> &str {
        &self.product_title
    }

    pub fn variant_title(&self) -> &str {
        &self.variant_title
    }

    pub fn sku(&self) -> Option<&str> {
        self.sku.as_deref()
    }

    pub const fn requires_shipping(&self) -> bool {
        self.requires_shipping
    }

    pub const fn track_inventory(&self) -> bool {
        self.track_inventory
    }

    pub const fn quantity(&self) -> u32 {
        self.quantity
    }

    pub const fn unit_price(&self) -> &Money {
        &self.unit_price
    }

    pub const fn tax_inclusive(&self) -> bool {
        self.tax_inclusive
    }

    pub fn subtotal(&self) -> Result<Money, DomainError> {
        self.unit_price.checked_mul(u64::from(self.quantity))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cart {
    id: CartId,
    store_id: StoreId,
    sales_channel_id: SalesChannelId,
    price_list_id: PriceListId,
    currency: CurrencyCode,
    status: CartStatus,
    lines: Vec<CartLine>,
}

impl Cart {
    pub fn create(
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        price_list_id: PriceListId,
        currency: CurrencyCode,
    ) -> Self {
        Self {
            id: CartId::new(),
            store_id,
            sales_channel_id,
            price_list_id,
            currency,
            status: CartStatus::Active,
            lines: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rehydrate(
        id: CartId,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        price_list_id: PriceListId,
        currency: CurrencyCode,
        status: CartStatus,
        lines: Vec<CartLine>,
    ) -> Result<Self, DomainError> {
        let mut cart = Self {
            id,
            store_id,
            sales_channel_id,
            price_list_id,
            currency,
            status,
            lines: Vec::new(),
        };
        for line in lines {
            if line.unit_price.currency() != currency {
                return Err(validation(
                    "currency",
                    "Cart lines must use the Cart currency",
                ));
            }
            if cart
                .lines
                .iter()
                .any(|existing| existing.product_variant_id == line.product_variant_id)
            {
                return Err(validation(
                    "lines",
                    "must contain each Variant at most once",
                ));
            }
            if cart
                .lines
                .first()
                .is_some_and(|existing| existing.tax_inclusive != line.tax_inclusive)
            {
                return Err(validation(
                    "tax_inclusive",
                    "Cart lines must use one Price List tax semantic",
                ));
            }
            cart.lines.push(line);
        }
        cart.lines
            .sort_by_key(|line| line.product_variant_id().as_uuid());
        Ok(cart)
    }

    pub const fn id(&self) -> CartId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub const fn sales_channel_id(&self) -> SalesChannelId {
        self.sales_channel_id
    }

    pub const fn price_list_id(&self) -> PriceListId {
        self.price_list_id
    }

    pub const fn currency(&self) -> CurrencyCode {
        self.currency
    }

    pub const fn status(&self) -> CartStatus {
        self.status
    }

    pub fn lines(&self) -> &[CartLine] {
        &self.lines
    }

    pub fn upsert_line(&mut self, line: CartLine) -> Result<(), DomainError> {
        self.require_active()?;
        if line.unit_price.currency() != self.currency {
            return Err(validation(
                "currency",
                "Cart lines must use the Cart currency",
            ));
        }
        if self.lines.iter().any(|existing| {
            existing.product_variant_id != line.product_variant_id
                && existing.tax_inclusive != line.tax_inclusive
        }) {
            return Err(validation(
                "tax_inclusive",
                "Cart lines must use one Price List tax semantic",
            ));
        }
        if let Some(existing) = self
            .lines
            .iter_mut()
            .find(|existing| existing.product_variant_id == line.product_variant_id)
        {
            *existing = line;
        } else {
            self.lines.push(line);
            self.lines
                .sort_by_key(|line| line.product_variant_id().as_uuid());
        }
        Ok(())
    }

    pub fn remove_line(&mut self, product_variant_id: ProductVariantId) -> Result<(), DomainError> {
        self.require_active()?;
        self.lines
            .retain(|line| line.product_variant_id != product_variant_id);
        Ok(())
    }

    pub fn total(&self) -> Result<Money, DomainError> {
        self.lines
            .iter()
            .try_fold(Money::zero(self.currency), |total, line| {
                total.checked_add(&line.subtotal()?)
            })
    }

    pub fn complete(&mut self) -> Result<(), DomainError> {
        self.require_active()?;
        if self.lines.is_empty() {
            return Err(validation("lines", "Cart must contain at least one line"));
        }
        self.status = CartStatus::Completed;
        Ok(())
    }

    fn require_active(&self) -> Result<(), DomainError> {
        if self.status == CartStatus::Active {
            Ok(())
        } else {
            Err(validation("cart", "is no longer active"))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommercialAdjustments {
    discount: Money,
    tax: Money,
}

impl CommercialAdjustments {
    pub fn new(discount: Money, tax: Money) -> Result<Self, DomainError> {
        if discount.amount_minor() < 0 {
            return Err(validation("discount", "must be zero or greater"));
        }
        if tax.amount_minor() < 0 {
            return Err(validation("tax", "must be zero or greater"));
        }
        if discount.currency() != tax.currency() {
            return Err(validation(
                "currency",
                "discount and tax currencies must match",
            ));
        }
        Ok(Self { discount, tax })
    }

    pub fn zero(currency: CurrencyCode) -> Self {
        Self {
            discount: Money::zero(currency),
            tax: Money::zero(currency),
        }
    }

    pub const fn discount(&self) -> &Money {
        &self.discount
    }
    pub const fn tax(&self) -> &Money {
        &self.tax
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckoutLine {
    cart_line: CartLine,
    subtotal: Money,
    discount: Money,
    tax: Money,
    total: Money,
}

impl CheckoutLine {
    fn freeze(
        cart_line: &CartLine,
        adjustments: CommercialAdjustments,
    ) -> Result<Self, DomainError> {
        let subtotal = cart_line.subtotal()?;
        if adjustments.discount.currency() != subtotal.currency()
            || adjustments.tax.currency() != subtotal.currency()
        {
            return Err(validation(
                "currency",
                "Checkout adjustments must use the Cart currency",
            ));
        }
        if adjustments.discount.amount_minor() > subtotal.amount_minor() {
            return Err(validation("discount", "must not exceed line subtotal"));
        }
        let discounted = subtotal.checked_sub(&adjustments.discount)?;
        let total = if cart_line.tax_inclusive() {
            if adjustments.tax.amount_minor() > discounted.amount_minor() {
                return Err(validation(
                    "tax",
                    "included tax must not exceed the discounted subtotal",
                ));
            }
            discounted
        } else {
            discounted.checked_add(&adjustments.tax)?
        };
        Ok(Self {
            cart_line: cart_line.clone(),
            subtotal,
            discount: adjustments.discount,
            tax: adjustments.tax,
            total,
        })
    }

    pub const fn cart_line(&self) -> &CartLine {
        &self.cart_line
    }

    pub const fn subtotal(&self) -> &Money {
        &self.subtotal
    }

    pub const fn discount(&self) -> &Money {
        &self.discount
    }

    pub const fn tax(&self) -> &Money {
        &self.tax
    }

    pub const fn total(&self) -> &Money {
        &self.total
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checkout {
    id: CheckoutId,
    cart_id: CartId,
    reservation_id: Option<InventoryReservationId>,
    price_list_id: PriceListId,
    currency: CurrencyCode,
    expires_at: OffsetDateTime,
    identity: CheckoutIdentity,
    shipping: Option<ShippingSelection>,
    tax_rule: TaxRuleSnapshot,
    promotion: Option<PromotionSnapshot>,
    tax_inclusive: bool,
    lines: Vec<CheckoutLine>,
    subtotal: Money,
    discount: Money,
    tax: Money,
    total: Money,
}

impl Checkout {
    #[allow(clippy::too_many_arguments)]
    pub fn freeze(
        cart: &Cart,
        reservation_id: Option<InventoryReservationId>,
        expires_at: OffsetDateTime,
        identity: CheckoutIdentity,
        shipping: Option<ShippingSelection>,
        tax_rule: TaxRuleSnapshot,
        promotion: Option<PromotionSnapshot>,
        adjustments: Vec<CommercialAdjustments>,
    ) -> Result<Self, DomainError> {
        if cart.status != CartStatus::Active {
            return Err(validation("cart", "must be active"));
        }
        if cart.lines.is_empty() {
            return Err(validation("lines", "Cart must contain at least one line"));
        }
        if adjustments.len() != cart.lines.len() {
            return Err(validation(
                "adjustments",
                "must contain one entry for every Cart line",
            ));
        }
        if cart.lines.iter().any(CartLine::requires_shipping)
            && identity.shipping_address().is_none()
        {
            return Err(validation(
                "shipping_address",
                "is required when the Cart contains shippable lines",
            ));
        }
        let requires_shipping = cart.lines.iter().any(CartLine::requires_shipping);
        if requires_shipping != shipping.is_some() {
            return Err(validation(
                "shipping_service_id",
                "is required exactly when the Cart contains shippable lines",
            ));
        }
        if shipping
            .as_ref()
            .is_some_and(|selection| selection.amount().currency() != cart.currency)
        {
            return Err(validation(
                "currency",
                "Shipping selection must use the Cart currency",
            ));
        }
        let lines = cart
            .lines
            .iter()
            .zip(adjustments)
            .map(|(line, adjustments)| CheckoutLine::freeze(line, adjustments))
            .collect::<Result<Vec<_>, _>>()?;
        let tax_inclusive = cart.lines[0].tax_inclusive();
        let subtotal = sum(lines.iter().map(CheckoutLine::subtotal), cart.currency)?;
        let discount = sum(lines.iter().map(CheckoutLine::discount), cart.currency)?;
        let tax = sum(lines.iter().map(CheckoutLine::tax), cart.currency)?;
        let line_total = sum(lines.iter().map(CheckoutLine::total), cart.currency)?;
        let total = if let Some(selection) = &shipping {
            line_total.checked_add(selection.amount())?
        } else {
            line_total
        };
        Ok(Self {
            id: CheckoutId::new(),
            cart_id: cart.id,
            reservation_id,
            price_list_id: cart.price_list_id,
            currency: cart.currency,
            expires_at,
            identity,
            shipping,
            tax_rule,
            promotion,
            tax_inclusive,
            lines,
            subtotal,
            discount,
            tax,
            total,
        })
    }

    pub const fn id(&self) -> CheckoutId {
        self.id
    }

    pub const fn cart_id(&self) -> CartId {
        self.cart_id
    }

    pub const fn reservation_id(&self) -> Option<InventoryReservationId> {
        self.reservation_id
    }

    pub const fn price_list_id(&self) -> PriceListId {
        self.price_list_id
    }

    pub const fn currency(&self) -> CurrencyCode {
        self.currency
    }

    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    pub const fn identity(&self) -> &CheckoutIdentity {
        &self.identity
    }

    pub const fn shipping(&self) -> Option<&ShippingSelection> {
        self.shipping.as_ref()
    }

    pub const fn tax_rule(&self) -> &TaxRuleSnapshot {
        &self.tax_rule
    }
    pub const fn promotion(&self) -> Option<&PromotionSnapshot> {
        self.promotion.as_ref()
    }
    pub const fn tax_inclusive(&self) -> bool {
        self.tax_inclusive
    }

    pub fn lines(&self) -> &[CheckoutLine] {
        &self.lines
    }

    pub const fn subtotal(&self) -> &Money {
        &self.subtotal
    }

    pub const fn discount(&self) -> &Money {
        &self.discount
    }

    pub const fn tax(&self) -> &Money {
        &self.tax
    }

    pub const fn total(&self) -> &Money {
        &self.total
    }
}

fn sum<'a>(
    mut values: impl Iterator<Item = &'a Money>,
    currency: CurrencyCode,
) -> Result<Money, DomainError> {
    values.try_fold(Money::zero(currency), |total, value| {
        total.checked_add(value)
    })
}

fn require_quantity(quantity: u32) -> Result<(), DomainError> {
    if (1..=999).contains(&quantity) {
        Ok(())
    } else {
        Err(validation("quantity", "must be between 1 and 999"))
    }
}

fn validate_snapshot_text(field: &'static str, value: &str, max: usize) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.chars().count() > max {
        Err(validation(field, "must contain valid snapshot text"))
    } else {
        Ok(())
    }
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(variant_id: ProductVariantId, quantity: u32, amount: i64) -> CartLine {
        CartLine::new(
            ProductId::new(),
            variant_id,
            "Product",
            "Variant",
            Some("SKU".into()),
            true,
            true,
            quantity,
            Money::new(amount, CurrencyCode::USD),
            false,
        )
        .unwrap()
    }

    fn cart() -> Cart {
        Cart::create(
            StoreId::new(),
            SalesChannelId::new(),
            PriceListId::new(),
            CurrencyCode::USD,
        )
    }

    fn checkout_identity() -> CheckoutIdentity {
        let address = super::super::PostalAddress::new(
            "Guest Buyer",
            None,
            "1 Market Street",
            None,
            "San Francisco",
            Some("CA".into()),
            Some("94105".into()),
            "US",
        )
        .unwrap();
        CheckoutIdentity::new(
            super::super::CheckoutContact::new("guest@example.com", None).unwrap(),
            address.clone(),
            Some(address),
        )
    }

    fn shipping_selection() -> ShippingSelection {
        let service = crate::fulfillment::ShippingService::create(
            "standard",
            "Standard shipping",
            Money::new(250, CurrencyCode::USD),
            2,
            5,
            vec!["US".into()],
        )
        .unwrap();
        ShippingSelection::from_service(&service).unwrap()
    }

    fn tax_rule() -> TaxRuleSnapshot {
        let rule = crate::pricing::TaxRule::create("sales-tax", "Sales tax", "US", 900).unwrap();
        TaxRuleSnapshot::from_rule(&rule).unwrap()
    }

    #[test]
    fn cart_upserts_variant_lines_and_totals_checked_money() {
        let variant_id = ProductVariantId::new();
        let mut cart = cart();
        cart.upsert_line(line(variant_id, 2, 125)).unwrap();
        cart.upsert_line(line(variant_id, 3, 125)).unwrap();
        assert_eq!(cart.lines().len(), 1);
        assert_eq!(cart.lines()[0].quantity(), 3);
        assert_eq!(cart.total().unwrap().amount_minor(), 375);
    }

    #[test]
    fn completed_cart_rejects_later_mutations() {
        let variant_id = ProductVariantId::new();
        let mut cart = cart();
        cart.upsert_line(line(variant_id, 1, 100)).unwrap();
        cart.complete().unwrap();
        assert!(cart.upsert_line(line(variant_id, 2, 100)).is_err());
        assert!(cart.remove_line(variant_id).is_err());
    }

    #[test]
    fn checkout_freezes_line_and_aggregate_calculations() {
        let mut cart = cart();
        cart.upsert_line(line(ProductVariantId::new(), 2, 500))
            .unwrap();
        let adjustments = CommercialAdjustments::new(
            Money::new(100, CurrencyCode::USD),
            Money::new(90, CurrencyCode::USD),
        )
        .unwrap();
        let checkout = Checkout::freeze(
            &cart,
            Some(InventoryReservationId::new()),
            OffsetDateTime::UNIX_EPOCH,
            checkout_identity(),
            Some(shipping_selection()),
            tax_rule(),
            None,
            vec![adjustments],
        )
        .unwrap();
        assert_eq!(checkout.subtotal().amount_minor(), 1_000);
        assert_eq!(checkout.discount().amount_minor(), 100);
        assert_eq!(checkout.tax().amount_minor(), 90);
        assert_eq!(checkout.total().amount_minor(), 1_240);
        assert_eq!(checkout.lines()[0].cart_line().product_title(), "Product");
    }

    #[test]
    fn checkout_rejects_discount_above_subtotal() {
        let mut cart = cart();
        cart.upsert_line(line(ProductVariantId::new(), 1, 100))
            .unwrap();
        let adjustment = CommercialAdjustments::new(
            Money::new(101, CurrencyCode::USD),
            Money::zero(CurrencyCode::USD),
        )
        .unwrap();
        assert!(
            Checkout::freeze(
                &cart,
                Some(InventoryReservationId::new()),
                OffsetDateTime::UNIX_EPOCH,
                checkout_identity(),
                Some(shipping_selection()),
                tax_rule(),
                None,
                vec![adjustment],
            )
            .is_err()
        );
    }

    #[test]
    fn checkout_requires_shipping_address_for_shippable_lines() {
        let mut cart = cart();
        cart.upsert_line(line(ProductVariantId::new(), 1, 100))
            .unwrap();
        let address = super::super::PostalAddress::new(
            "Guest Buyer",
            None,
            "1 Market Street",
            None,
            "San Francisco",
            Some("CA".into()),
            Some("94105".into()),
            "US",
        )
        .unwrap();
        let identity = CheckoutIdentity::new(
            super::super::CheckoutContact::new("guest@example.com", None).unwrap(),
            address,
            None,
        );

        assert!(
            Checkout::freeze(
                &cart,
                Some(InventoryReservationId::new()),
                OffsetDateTime::UNIX_EPOCH,
                identity,
                None,
                tax_rule(),
                None,
                vec![CommercialAdjustments::zero(CurrencyCode::USD)],
            )
            .is_err()
        );
    }
}
