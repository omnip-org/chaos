use std::collections::HashSet;

use uuid::Uuid;

use crate::{DomainError, FieldViolation, catalog::CatalogMetadata, store::StoreId};

macro_rules! catalog_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

catalog_id!(ProductId);
catalog_id!(ProductOptionId);
catalog_id!(ProductOptionValueId);
catalog_id!(ProductVariantId);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProductHandle(String);

impl ProductHandle {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (2..=128).contains(&bytes.len())
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
        if !valid {
            return Err(validation(
                "handle",
                "must be 2-128 lowercase ASCII letters, digits, or hyphens",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Sku(String);

impl Sku {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty()
            || value.chars().count() > 64
            || value.chars().any(char::is_control)
        {
            return Err(validation(
                "sku",
                "must contain 1-64 non-control characters",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductContent {
    handle: ProductHandle,
    title: String,
    description: String,
    metadata: Option<CatalogMetadata>,
}

impl ProductContent {
    pub fn new(
        handle: ProductHandle,
        title: impl Into<String>,
        description: impl Into<String>,
        metadata: Option<CatalogMetadata>,
    ) -> Result<Self, DomainError> {
        let title = title.into();
        let description = description.into();
        validate_text("title", &title, 255)?;
        if description.chars().count() > 100_000 {
            return Err(validation(
                "description",
                "must contain at most 100000 characters",
            ));
        }
        Ok(Self {
            handle,
            title,
            description,
            metadata,
        })
    }

    pub const fn handle(&self) -> &ProductHandle {
        &self.handle
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn metadata(&self) -> Option<&CatalogMetadata> {
        self.metadata.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductStatus {
    Draft,
    Active,
    Archived,
}

impl ProductStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductLifecycle {
    status: ProductStatus,
    variant_count: u32,
}

impl ProductLifecycle {
    pub const fn from_snapshot(status: ProductStatus, variant_count: u32) -> Self {
        Self {
            status,
            variant_count,
        }
    }

    pub fn activate(&mut self) -> Result<(), DomainError> {
        if self.variant_count == 0 {
            return Err(validation(
                "variants",
                "must contain at least one variant before activation",
            ));
        }
        self.status = ProductStatus::Active;
        Ok(())
    }

    pub fn archive(&mut self) {
        self.status = ProductStatus::Archived;
    }

    pub fn require_publishable(self) -> Result<(), DomainError> {
        if self.status != ProductStatus::Active {
            return Err(validation(
                "product_status",
                "must be active before publication",
            ));
        }
        Ok(())
    }

    pub const fn status(self) -> ProductStatus {
        self.status
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VariantStatus {
    Active,
    Archived,
}

impl VariantStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductOptionValue {
    id: ProductOptionValueId,
    value: String,
    position: u16,
}

impl ProductOptionValue {
    pub const fn id(&self) -> ProductOptionValueId {
        self.id
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub const fn position(&self) -> u16 {
        self.position
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductOption {
    id: ProductOptionId,
    name: String,
    position: u16,
    values: Vec<ProductOptionValue>,
}

impl ProductOption {
    pub const fn id(&self) -> ProductOptionId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn position(&self) -> u16 {
        self.position
    }

    pub fn values(&self) -> &[ProductOptionValue] {
        &self.values
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SelectedOptionValue {
    option_id: ProductOptionId,
    option_value_id: ProductOptionValueId,
}

impl SelectedOptionValue {
    pub const fn option_id(self) -> ProductOptionId {
        self.option_id
    }

    pub const fn option_value_id(self) -> ProductOptionValueId {
        self.option_value_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductVariant {
    id: ProductVariantId,
    title: String,
    sku: Option<Sku>,
    status: VariantStatus,
    requires_shipping: bool,
    track_inventory: bool,
    selected_options: Vec<SelectedOptionValue>,
    metadata: Option<CatalogMetadata>,
}

impl ProductVariant {
    pub const fn id(&self) -> ProductVariantId {
        self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn sku(&self) -> Option<&Sku> {
        self.sku.as_ref()
    }

    pub const fn status(&self) -> VariantStatus {
        self.status
    }

    pub const fn requires_shipping(&self) -> bool {
        self.requires_shipping
    }

    pub const fn track_inventory(&self) -> bool {
        self.track_inventory
    }

    pub fn selected_options(&self) -> &[SelectedOptionValue] {
        &self.selected_options
    }

    pub fn metadata(&self) -> Option<&CatalogMetadata> {
        self.metadata.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Product {
    id: ProductId,
    store_id: StoreId,
    content: ProductContent,
    status: ProductStatus,
    options: Vec<ProductOption>,
    variants: Vec<ProductVariant>,
}

impl Product {
    pub fn create(
        store_id: StoreId,
        handle: ProductHandle,
        title: impl Into<String>,
        description: impl Into<String>,
        metadata: Option<CatalogMetadata>,
    ) -> Result<Self, DomainError> {
        let content = ProductContent::new(handle, title, description, metadata)?;
        Ok(Self {
            id: ProductId::new(),
            store_id,
            content,
            status: ProductStatus::Draft,
            options: Vec::new(),
            variants: Vec::new(),
        })
    }

    pub fn add_option(&mut self, name: impl Into<String>) -> Result<ProductOptionId, DomainError> {
        let name = name.into();
        validate_text("option_name", &name, 80)?;
        if !self.variants.is_empty() {
            return Err(validation(
                "options",
                "cannot add an option after variants exist",
            ));
        }
        if self.options.len() >= 10 {
            return Err(validation("options", "must contain at most 10 options"));
        }
        if self
            .options
            .iter()
            .any(|option| case_insensitive_eq(&option.name, &name))
        {
            return Err(validation(
                "option_name",
                "must be unique within the product",
            ));
        }
        let id = ProductOptionId::new();
        self.options.push(ProductOption {
            id,
            name,
            position: self.options.len() as u16,
            values: Vec::new(),
        });
        Ok(id)
    }

    pub fn add_option_value(
        &mut self,
        option_id: ProductOptionId,
        value: impl Into<String>,
    ) -> Result<ProductOptionValueId, DomainError> {
        let value = value.into();
        validate_text("option_value", &value, 120)?;
        let option = self
            .options
            .iter_mut()
            .find(|option| option.id == option_id)
            .ok_or_else(|| validation("option_id", "does not belong to the product"))?;
        if option.values.len() >= 100 {
            return Err(validation(
                "option_values",
                "must contain at most 100 values per option",
            ));
        }
        if option
            .values
            .iter()
            .any(|option_value| case_insensitive_eq(&option_value.value, &value))
        {
            return Err(validation(
                "option_value",
                "must be unique within the option",
            ));
        }
        let id = ProductOptionValueId::new();
        option.values.push(ProductOptionValue {
            id,
            value,
            position: option.values.len() as u16,
        });
        Ok(id)
    }

    pub fn add_variant(
        &mut self,
        title: impl Into<String>,
        sku: Option<Sku>,
        requires_shipping: bool,
        track_inventory: bool,
        selected_value_ids: Vec<ProductOptionValueId>,
        metadata: Option<CatalogMetadata>,
    ) -> Result<ProductVariantId, DomainError> {
        let title = title.into();
        validate_text("variant_title", &title, 255)?;
        if selected_value_ids.len() != self.options.len() {
            return Err(validation(
                "selected_options",
                "must select exactly one value for every product option",
            ));
        }
        let mut selected_options = Vec::with_capacity(self.options.len());
        let mut seen_value_ids = HashSet::new();
        for option in &self.options {
            let matches = option
                .values
                .iter()
                .filter(|value| selected_value_ids.contains(&value.id))
                .collect::<Vec<_>>();
            if matches.len() != 1 || !seen_value_ids.insert(matches[0].id) {
                return Err(validation(
                    "selected_options",
                    "must select one value belonging to each product option",
                ));
            }
            selected_options.push(SelectedOptionValue {
                option_id: option.id,
                option_value_id: matches[0].id,
            });
        }
        if self
            .variants
            .iter()
            .any(|variant| variant.selected_options == selected_options)
        {
            return Err(validation(
                "selected_options",
                "variant option combination must be unique",
            ));
        }
        if let Some(sku) = &sku
            && self.variants.iter().any(|variant| {
                variant
                    .sku
                    .as_ref()
                    .is_some_and(|existing| case_insensitive_eq(existing.as_str(), sku.as_str()))
            })
        {
            return Err(validation("sku", "must be unique within the product"));
        }

        let id = ProductVariantId::new();
        self.variants.push(ProductVariant {
            id,
            title,
            sku,
            status: VariantStatus::Active,
            requires_shipping,
            track_inventory,
            selected_options,
            metadata,
        });
        Ok(id)
    }

    pub fn activate(&mut self) -> Result<(), DomainError> {
        let mut lifecycle = ProductLifecycle::from_snapshot(
            self.status,
            u32::try_from(self.variants.len()).unwrap_or(u32::MAX),
        );
        lifecycle.activate()?;
        self.status = lifecycle.status();
        Ok(())
    }

    pub fn archive(&mut self) {
        self.status = ProductStatus::Archived;
    }

    pub fn update_content(&mut self, content: ProductContent) {
        self.content = content;
    }

    pub const fn id(&self) -> ProductId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub fn handle(&self) -> &ProductHandle {
        self.content.handle()
    }

    pub fn title(&self) -> &str {
        self.content.title()
    }

    pub fn description(&self) -> &str {
        self.content.description()
    }

    pub fn metadata(&self) -> Option<&CatalogMetadata> {
        self.content.metadata()
    }

    pub const fn status(&self) -> ProductStatus {
        self.status
    }

    pub fn options(&self) -> &[ProductOption] {
        &self.options
    }

    pub fn variants(&self) -> &[ProductVariant] {
        &self.variants
    }
}

fn validate_text(field: &'static str, value: &str, maximum: usize) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.chars().count() > maximum {
        return Err(validation(
            field,
            &format!("must contain 1-{maximum} characters"),
        ));
    }
    Ok(())
}

fn validation(field: &'static str, reason: &str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

fn case_insensitive_eq(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product() -> Product {
        Product::create(
            StoreId::new(),
            ProductHandle::parse("classic-shirt").unwrap(),
            "Classic Shirt",
            "A durable everyday shirt.",
            None,
        )
        .unwrap()
    }

    #[test]
    fn variant_selects_exactly_one_value_per_option() {
        let mut product = product();
        let color = product.add_option("Color").unwrap();
        let size = product.add_option("Size").unwrap();
        let blue = product.add_option_value(color, "Blue").unwrap();
        let medium = product.add_option_value(size, "M").unwrap();

        let variant = product
            .add_variant(
                "Blue / M",
                Some(Sku::parse("SHIRT-BLUE-M").unwrap()),
                true,
                true,
                vec![medium, blue],
                None,
            )
            .unwrap();

        assert_eq!(product.variants()[0].id(), variant);
        assert_eq!(product.variants()[0].selected_options().len(), 2);
    }

    #[test]
    fn duplicate_variant_combination_is_rejected_regardless_of_input_order() {
        let mut product = product();
        let color = product.add_option("Color").unwrap();
        let size = product.add_option("Size").unwrap();
        let blue = product.add_option_value(color, "Blue").unwrap();
        let medium = product.add_option_value(size, "M").unwrap();
        product
            .add_variant("Blue / M", None, true, true, vec![blue, medium], None)
            .unwrap();

        assert!(
            product
                .add_variant("Duplicate", None, true, true, vec![medium, blue], None)
                .is_err()
        );
    }

    #[test]
    fn product_requires_a_variant_before_activation() {
        let mut product = product();
        assert!(product.activate().is_err());

        product
            .add_variant("Default", None, true, true, vec![], None)
            .unwrap();
        product.activate().unwrap();
        assert_eq!(product.status(), ProductStatus::Active);
    }

    #[test]
    fn lifecycle_requires_active_state_for_publication() {
        let mut lifecycle = ProductLifecycle::from_snapshot(ProductStatus::Draft, 1);
        assert!(lifecycle.require_publishable().is_err());
        lifecycle.activate().unwrap();
        assert!(lifecycle.require_publishable().is_ok());
        lifecycle.archive();
        assert_eq!(lifecycle.status(), ProductStatus::Archived);
        assert!(lifecycle.require_publishable().is_err());
    }

    #[test]
    fn product_content_reuses_creation_validation_for_updates() {
        assert!(ProductContent::new(ProductHandle::parse("valid").unwrap(), "", "", None).is_err());
        assert!(
            ProductContent::new(
                ProductHandle::parse("valid").unwrap(),
                "Updated Product",
                "Updated description",
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn option_names_and_values_are_unique_case_insensitively() {
        let mut product = product();
        let color = product.add_option("Color").unwrap();
        assert!(product.add_option("color").is_err());
        product.add_option_value(color, "Blue").unwrap();
        assert!(product.add_option_value(color, "blue").is_err());
    }

    #[test]
    fn adding_an_option_after_variants_exist_is_rejected() {
        let mut product = product();
        product
            .add_variant("Default", None, true, true, vec![], None)
            .unwrap();

        assert!(product.add_option("Color").is_err());
    }
}
