use std::{collections::HashSet, sync::Arc};

use chaos_domain::{
    FieldViolation,
    catalog::{Product, ProductHandle, ProductId, ProductOptionValueId, Sku},
    store::StoreId,
};

use crate::{
    ApplicationError, adapters::postgres::PostgresCatalogProvisioningRepository,
    catalog::parse_metadata, contracts::AdminActor,
};

pub struct CreateProductOptionInput {
    pub name: String,
    pub values: Vec<String>,
}

pub struct CreateProductSelectedOptionInput {
    pub option: String,
    pub value: String,
}

pub struct CreateProductVariantInput {
    pub title: String,
    pub sku: Option<String>,
    pub requires_shipping: bool,
    pub track_inventory: bool,
    pub selected_options: Vec<CreateProductSelectedOptionInput>,
    pub metadata: Option<serde_json::Value>,
}

pub struct CreateProductInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub options: Vec<CreateProductOptionInput>,
    pub variants: Vec<CreateProductVariantInput>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct CreateProductOutput {
    pub product_id: ProductId,
}

pub struct CreateProduct {
    repository: Arc<PostgresCatalogProvisioningRepository>,
}

impl CreateProduct {
    pub fn new(repository: Arc<PostgresCatalogProvisioningRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        input: CreateProductInput,
    ) -> Result<CreateProductOutput, ApplicationError> {
        input.actor.require_human()?;

        let product = build_product(&input)?;
        let mut transaction = self.repository.begin(input.actor, input.store_id).await?;
        transaction.require_writable_store().await?;
        transaction.insert_product(&product).await?;
        transaction.commit().await?;
        Ok(CreateProductOutput {
            product_id: product.id(),
        })
    }
}

fn build_product(input: &CreateProductInput) -> Result<Product, ApplicationError> {
    let mut product = Product::create(
        input.store_id,
        ProductHandle::parse(input.handle.clone())?,
        input.title.clone(),
        input.description.clone(),
        parse_metadata(input.metadata.clone())?,
    )?;

    for option in &input.options {
        let option_id = product.add_option(option.name.clone())?;
        for value in &option.values {
            product.add_option_value(option_id, value.clone())?;
        }
    }

    for variant in &input.variants {
        let selected_value_ids = resolve_selected_options(&product, &variant.selected_options)?;
        let sku = variant.sku.clone().map(Sku::parse).transpose()?;
        product.add_variant(
            variant.title.clone(),
            sku,
            variant.requires_shipping,
            variant.track_inventory,
            selected_value_ids,
            parse_metadata(variant.metadata.clone())?,
        )?;
    }
    Ok(product)
}

fn resolve_selected_options(
    product: &Product,
    selections: &[CreateProductSelectedOptionInput],
) -> Result<Vec<ProductOptionValueId>, ApplicationError> {
    let mut option_names = HashSet::new();
    let mut value_ids = Vec::with_capacity(selections.len());
    for selection in selections {
        let normalized_option = selection.option.to_lowercase();
        if !option_names.insert(normalized_option.clone()) {
            return Err(selection_violation(
                "must not select the same option more than once",
            ));
        }
        let option = product
            .options()
            .iter()
            .find(|option| option.name().to_lowercase() == normalized_option)
            .ok_or_else(|| selection_violation("contains an unknown product option"))?;
        let normalized_value = selection.value.to_lowercase();
        let value = option
            .values()
            .iter()
            .find(|value| value.value().to_lowercase() == normalized_value)
            .ok_or_else(|| selection_violation("contains an unknown option value"))?;
        value_ids.push(value.id());
    }
    Ok(value_ids)
}

fn selection_violation(reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field: "selected_options",
            reason: reason.into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use chaos_domain::{identity::UserId, store::StoreRole};

    use super::*;

    fn input() -> CreateProductInput {
        CreateProductInput {
            actor: AdminActor::Store(crate::store::StoreActor::new(
                UserId::new(),
                StoreId::new(),
                StoreRole::Member,
            )),
            store_id: StoreId::new(),
            handle: "classic-shirt".into(),
            title: "Classic Shirt".into(),
            description: String::new(),
            options: vec![
                CreateProductOptionInput {
                    name: "Color".into(),
                    values: vec!["Blue".into(), "Black".into()],
                },
                CreateProductOptionInput {
                    name: "Size".into(),
                    values: vec!["S".into(), "M".into()],
                },
            ],
            variants: vec![CreateProductVariantInput {
                title: "Blue / M".into(),
                sku: Some("SHIRT-BLUE-M".into()),
                requires_shipping: true,
                track_inventory: true,
                selected_options: vec![
                    CreateProductSelectedOptionInput {
                        option: "color".into(),
                        value: "blue".into(),
                    },
                    CreateProductSelectedOptionInput {
                        option: "SIZE".into(),
                        value: "m".into(),
                    },
                ],
                metadata: None,
            }],
            metadata: None,
        }
    }

    #[test]
    fn builds_a_complete_product_from_case_insensitive_option_names() {
        let product = build_product(&input()).unwrap();

        assert_eq!(product.options().len(), 2);
        assert_eq!(product.variants().len(), 1);
        assert_eq!(product.variants()[0].selected_options().len(), 2);
    }

    #[test]
    fn rejects_unknown_selected_options_before_persistence() {
        let mut input = input();
        input.variants[0].selected_options[0].option = "Material".into();

        assert!(matches!(
            build_product(&input),
            Err(ApplicationError::Validation { .. })
        ));
    }
}
