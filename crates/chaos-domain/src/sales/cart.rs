use uuid::Uuid;

use crate::{
    CurrencyCode, DomainError, FieldViolation,
    catalog::{ProductId, ProductVariantId},
    pricing::{Money, PriceListId},
    store::{SalesChannelId, StoreId},
};

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

sales_id!(CartId);
sales_id!(ShopperId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CartStatus {
    Active,
    Locked,
    Completed,
    Abandoned,
}

impl CartStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Locked => "locked",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "locked" => Some(Self::Locked),
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
    track_inventory: bool,
    quantity: u32,
    unit_price: Money,
}

impl CartLine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        product_id: ProductId,
        product_variant_id: ProductVariantId,
        product_title: impl Into<String>,
        variant_title: impl Into<String>,
        sku: Option<String>,
        track_inventory: bool,
        quantity: u32,
        unit_price: Money,
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
            track_inventory,
            quantity,
            unit_price,
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

    pub const fn track_inventory(&self) -> bool {
        self.track_inventory
    }

    pub const fn quantity(&self) -> u32 {
        self.quantity
    }

    pub const fn unit_price(&self) -> &Money {
        &self.unit_price
    }

    pub fn subtotal(&self) -> Result<Money, DomainError> {
        self.unit_price.checked_mul(u64::from(self.quantity))
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cart {
    id: CartId,
    store_id: StoreId,
    channel_id: SalesChannelId,
    price_list_id: PriceListId,
    currency: CurrencyCode,
    status: CartStatus,
    lines: Vec<CartLine>,
}

impl Cart {
    pub fn create(
        store_id: StoreId,
        channel_id: SalesChannelId,
        price_list_id: PriceListId,
        currency: CurrencyCode,
    ) -> Self {
        Self {
            id: CartId::new(),
            store_id,
            channel_id,
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
        channel_id: SalesChannelId,
        price_list_id: PriceListId,
        currency: CurrencyCode,
        status: CartStatus,
        lines: Vec<CartLine>,
    ) -> Result<Self, DomainError> {
        let mut cart = Self {
            id,
            store_id,
            channel_id,
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

    pub const fn channel_id(&self) -> SalesChannelId {
        self.channel_id
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

    pub fn begin_checkout(&mut self) -> Result<(), DomainError> {
        self.require_active()?;
        if self.lines.is_empty() {
            return Err(validation("lines", "Cart must contain at least one line"));
        }
        self.status = CartStatus::Locked;
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
            quantity,
            Money::new(amount, CurrencyCode::USD),
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
    fn locked_cart_rejects_later_mutations() {
        let variant_id = ProductVariantId::new();
        let mut cart = cart();
        cart.upsert_line(line(variant_id, 1, 100)).unwrap();
        cart.begin_checkout().unwrap();
        assert!(cart.upsert_line(line(variant_id, 2, 100)).is_err());
        assert!(cart.remove_line(variant_id).is_err());
    }

    #[test]
    fn checkout_locks_cart_before_payment() {
        let variant_id = ProductVariantId::new();
        let mut cart = cart();
        cart.upsert_line(line(variant_id, 1, 100)).unwrap();
        cart.begin_checkout().unwrap();
        assert_eq!(cart.status(), CartStatus::Locked);
        assert!(cart.upsert_line(line(variant_id, 2, 100)).is_err());
        assert!(cart.remove_line(variant_id).is_err());
    }
}
