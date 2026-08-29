use std::{collections::HashSet, sync::Arc};

use chaos_domain::{
    FieldViolation,
    catalog::{
        ProductId, ProductOptionId, ProductOptionValueId, ProductStatus, ProductVariantId, Sku,
    },
    store::StoreId,
};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresCatalogConfigurationRepository,
    catalog::parse_metadata,
    contracts::{AdminActor, CatalogProductDetail},
};

pub const MAX_PRODUCT_OPTIONS: usize = 10;
pub const MAX_OPTION_VALUES: usize = 100;
pub const MAX_PRODUCT_VARIANTS: usize = 1_000;

#[derive(Clone)]
pub struct ProductConfigurationOptionValueInput {
    pub id: ProductOptionValueId,
    pub value: String,
    pub position: u16,
}

#[derive(Clone)]
pub struct ProductConfigurationOptionInput {
    pub id: ProductOptionId,
    pub name: String,
    pub position: u16,
    pub values: Vec<ProductConfigurationOptionValueInput>,
}

#[derive(Clone)]
pub struct ProductConfigurationVariantInput {
    pub id: ProductVariantId,
    pub title: String,
    pub sku: Option<String>,
    pub track_inventory: bool,
    pub selected_option_value_ids: Vec<ProductOptionValueId>,
    pub metadata: Option<Value>,
}

#[derive(Clone)]
pub struct ProductConfigurationDraft {
    pub options: Vec<ProductConfigurationOptionInput>,
    pub variants: Vec<ProductConfigurationVariantInput>,
}

pub struct SyncProductConfigurationInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub draft: ProductConfigurationDraft,
    pub expected_revision: Option<i64>,
    pub now: OffsetDateTime,
}

pub struct SyncProductConfigurationOutput {
    pub product_id: ProductId,
    pub revision: i64,
    pub draft: ProductConfigurationDraft,
}

#[derive(Clone)]
pub struct ConfigurationViolation {
    pub field: String,
    pub reason: String,
}

pub struct ProductConfigurationValidation {
    pub errors: Vec<ConfigurationViolation>,
    pub warnings: Vec<ConfigurationViolation>,
    pub options_added: Vec<ProductOptionId>,
    pub options_archived: Vec<ProductOptionId>,
    pub values_added: Vec<ProductOptionValueId>,
    pub values_archived: Vec<ProductOptionValueId>,
    pub variants_added: Vec<ProductVariantId>,
    pub variants_archived: Vec<ProductVariantId>,
}

impl ProductConfigurationValidation {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

pub struct ProductConfigurationManagement {
    repository: Arc<PostgresCatalogConfigurationRepository>,
}

impl ProductConfigurationManagement {
    pub fn new(repository: Arc<PostgresCatalogConfigurationRepository>) -> Self {
        Self { repository }
    }

    pub async fn sync(
        &self,
        input: SyncProductConfigurationInput,
    ) -> Result<SyncProductConfigurationOutput, ApplicationError> {
        input.actor.require_human()?;
        validate_configuration(&input.draft)?;
        let revision = self
            .repository
            .sync(
                input.actor,
                input.store_id,
                input.product_id,
                &input.draft,
                input.expected_revision,
                input.now,
            )
            .await?;
        Ok(SyncProductConfigurationOutput {
            product_id: input.product_id,
            revision,
            draft: input.draft,
        })
    }
}

pub fn validate_product_configuration(
    current: &CatalogProductDetail,
    draft: &ProductConfigurationDraft,
) -> ProductConfigurationValidation {
    let mut result = ProductConfigurationValidation {
        errors: Vec::new(),
        warnings: Vec::new(),
        options_added: Vec::new(),
        options_archived: Vec::new(),
        values_added: Vec::new(),
        values_archived: Vec::new(),
        variants_added: Vec::new(),
        variants_archived: Vec::new(),
    };
    if let Err(ApplicationError::Validation { violations }) = validate_configuration(draft) {
        result.errors.extend(
            violations
                .into_iter()
                .map(|violation| ConfigurationViolation {
                    field: violation.field.to_owned(),
                    reason: violation.reason,
                }),
        );
        return result;
    }
    if current.status == ProductStatus::Active && draft.variants.is_empty() {
        result.errors.push(ConfigurationViolation {
            field: "variants".into(),
            reason: "an active Product must retain at least one active Variant".into(),
        });
        return result;
    }

    let desired_options = draft
        .options
        .iter()
        .map(|option| option.id)
        .collect::<HashSet<_>>();
    for option in &current.options {
        if option.archived_at.is_none() && !desired_options.contains(&option.id) {
            result.options_archived.push(option.id);
        }
        for value in &option.values {
            if value.archived_at.is_none()
                && !draft
                    .options
                    .iter()
                    .find(|desired| desired.id == option.id)
                    .is_some_and(|desired| desired.values.iter().any(|item| item.id == value.id))
            {
                result.values_archived.push(value.id);
            }
        }
    }
    for option in &draft.options {
        if !current
            .options
            .iter()
            .any(|current| current.id == option.id)
        {
            result.options_added.push(option.id);
        }
        for value in &option.values {
            if !current
                .options
                .iter()
                .flat_map(|current| current.values.iter())
                .any(|current| current.id == value.id)
            {
                result.values_added.push(value.id);
            }
        }
    }
    let desired_variants = draft
        .variants
        .iter()
        .map(|variant| variant.id)
        .collect::<HashSet<_>>();
    for variant in &current.variants {
        if variant.status == chaos_domain::catalog::VariantStatus::Active
            && !desired_variants.contains(&variant.id)
        {
            result.variants_archived.push(variant.id);
        }
    }
    for variant in &draft.variants {
        if !current
            .variants
            .iter()
            .any(|current| current.id == variant.id)
        {
            result.variants_added.push(variant.id);
        }
    }
    if draft.options.len()
        != current
            .options
            .iter()
            .filter(|option| option.archived_at.is_none())
            .count()
        || draft.variants.len()
            != current
                .variants
                .iter()
                .filter(|variant| variant.status == chaos_domain::catalog::VariantStatus::Active)
                .count()
    {
        result.warnings.push(ConfigurationViolation {
            field: "configuration".into(),
            reason: "the synchronized configuration changes the active option or variant set"
                .into(),
        });
    }
    result
}

pub fn validate_configuration(draft: &ProductConfigurationDraft) -> Result<(), ApplicationError> {
    let mut violations = Vec::new();
    if draft.options.len() > MAX_PRODUCT_OPTIONS {
        violations.push(FieldViolation {
            field: "options",
            reason: format!("must contain at most {MAX_PRODUCT_OPTIONS} options"),
        });
    }
    let mut option_ids = HashSet::new();
    let mut option_names = HashSet::new();
    let mut value_ids = HashSet::new();
    for option in &draft.options {
        if !option_ids.insert(option.id) {
            violations.push(FieldViolation {
                field: "options",
                reason: "option IDs must be unique".into(),
            });
        }
        if option.position >= MAX_PRODUCT_OPTIONS as u16 {
            violations.push(FieldViolation {
                field: "option_position",
                reason: format!("must be between 0 and {}", MAX_PRODUCT_OPTIONS - 1),
            });
        }
        validate_text("option_name", &option.name, 80, &mut violations);
        if !option_names.insert(option.name.to_lowercase()) {
            violations.push(FieldViolation {
                field: "option_name",
                reason: "must be unique within the product".into(),
            });
        }
        if option.values.is_empty() {
            violations.push(FieldViolation {
                field: "option_values",
                reason: "every active option must contain at least one value".into(),
            });
        }
        if option.values.len() > MAX_OPTION_VALUES {
            violations.push(FieldViolation {
                field: "option_values",
                reason: format!("must contain at most {MAX_OPTION_VALUES} values per option"),
            });
        }
        let mut value_names = HashSet::new();
        let mut positions = HashSet::new();
        for value in &option.values {
            if !value_ids.insert(value.id) {
                violations.push(FieldViolation {
                    field: "option_value_id",
                    reason: "option value IDs must be unique".into(),
                });
            }
            if !positions.insert(value.position) {
                violations.push(FieldViolation {
                    field: "option_value_position",
                    reason: "must be unique within the option".into(),
                });
            }
            if value.position > 999 {
                violations.push(FieldViolation {
                    field: "option_value_position",
                    reason: "must be between 0 and 999".into(),
                });
            }
            validate_text("option_value", &value.value, 120, &mut violations);
            if !value_names.insert(value.value.to_lowercase()) {
                violations.push(FieldViolation {
                    field: "option_value",
                    reason: "must be unique within the option".into(),
                });
            }
        }
    }
    let mut option_positions = HashSet::new();
    for option in &draft.options {
        if !option_positions.insert(option.position) {
            violations.push(FieldViolation {
                field: "option_position",
                reason: "must be unique within the product".into(),
            });
        }
    }

    if draft.variants.len() > MAX_PRODUCT_VARIANTS {
        violations.push(FieldViolation {
            field: "variants",
            reason: format!("must contain at most {MAX_PRODUCT_VARIANTS} variants"),
        });
    }
    let mut variant_ids = HashSet::new();
    let mut skus = HashSet::new();
    let mut combinations = HashSet::new();
    for variant in &draft.variants {
        if !variant_ids.insert(variant.id) {
            violations.push(FieldViolation {
                field: "variants",
                reason: "variant IDs must be unique".into(),
            });
        }
        validate_text("variant_title", &variant.title, 255, &mut violations);
        if let Some(sku) = &variant.sku {
            if let Err(error) = Sku::parse(sku.clone()) {
                extend_domain_violations(error, &mut violations);
            }
            if !skus.insert(sku.to_lowercase()) {
                violations.push(FieldViolation {
                    field: "sku",
                    reason: "must be unique within the Store".into(),
                });
            }
        }
        if variant.selected_option_value_ids.len() != draft.options.len() {
            violations.push(FieldViolation {
                field: "selected_option_value_ids",
                reason: "must select exactly one value for every active product option".into(),
            });
        }
        let selected = variant
            .selected_option_value_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if selected.len() != variant.selected_option_value_ids.len() {
            violations.push(FieldViolation {
                field: "selected_option_value_ids",
                reason: "must not select the same option value more than once".into(),
            });
        }
        let mut canonical_selection = Vec::with_capacity(draft.options.len());
        let mut has_invalid_selection = false;
        for option in &draft.options {
            let matching = option
                .values
                .iter()
                .filter(|value| selected.contains(&value.id))
                .map(|value| value.id)
                .collect::<Vec<_>>();
            if matching.len() != 1 {
                has_invalid_selection = true;
                violations.push(FieldViolation {
                    field: "selected_option_value_ids",
                    reason: "must select one value belonging to each active product option".into(),
                });
            } else {
                canonical_selection.push(matching[0]);
            }
        }
        if !has_invalid_selection && !combinations.insert(canonical_selection) {
            violations.push(FieldViolation {
                field: "selected_option_value_ids",
                reason: "variant option combinations must be unique".into(),
            });
        }
        if let Err(ApplicationError::Validation { violations: nested }) =
            parse_metadata(variant.metadata.clone())
        {
            violations.extend(nested);
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ApplicationError::Validation { violations })
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    maximum: usize,
    violations: &mut Vec<FieldViolation>,
) {
    if value.trim().is_empty()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        violations.push(FieldViolation {
            field,
            reason: format!("must contain 1-{maximum} non-control characters"),
        });
    }
}

fn extend_domain_violations(
    error: chaos_domain::DomainError,
    violations: &mut Vec<FieldViolation>,
) {
    let chaos_domain::DomainError::Validation(nested) = error;
    violations.extend(nested);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(
        id: ProductOptionId,
        name: &str,
        values: &[(ProductOptionValueId, &str)],
        position: u16,
    ) -> ProductConfigurationOptionInput {
        ProductConfigurationOptionInput {
            id,
            name: name.into(),
            position,
            values: values
                .iter()
                .enumerate()
                .map(
                    |(index, (id, value))| ProductConfigurationOptionValueInput {
                        id: *id,
                        value: (*value).into(),
                        position: index as u16,
                    },
                )
                .collect(),
        }
    }

    fn variant(
        id: ProductVariantId,
        title: &str,
        selected_option_value_ids: Vec<ProductOptionValueId>,
    ) -> ProductConfigurationVariantInput {
        ProductConfigurationVariantInput {
            id,
            title: title.into(),
            sku: None,
            track_inventory: true,
            selected_option_value_ids,
            metadata: None,
        }
    }

    #[test]
    fn accepts_reusable_option_values_across_variants() {
        let color = ProductOptionId::new();
        let length = ProductOptionId::new();
        let red = ProductOptionValueId::new();
        let blue = ProductOptionValueId::new();
        let short = ProductOptionValueId::new();
        let long = ProductOptionValueId::new();
        let draft = ProductConfigurationDraft {
            options: vec![
                option(color, "Color", &[(red, "Red"), (blue, "Blue")], 0),
                option(length, "Length", &[(short, "100cm"), (long, "160cm")], 1),
            ],
            variants: vec![
                variant(ProductVariantId::new(), "Red / 100cm", vec![short, red]),
                variant(ProductVariantId::new(), "Blue / 100cm", vec![short, blue]),
                variant(ProductVariantId::new(), "Red / 160cm", vec![long, red]),
            ],
        };

        assert!(validate_configuration(&draft).is_ok());
    }

    #[test]
    fn rejects_duplicate_variant_combinations_even_when_selection_order_differs() {
        let color = ProductOptionId::new();
        let length = ProductOptionId::new();
        let red = ProductOptionValueId::new();
        let short = ProductOptionValueId::new();
        let draft = ProductConfigurationDraft {
            options: vec![
                option(color, "Color", &[(red, "Red")], 0),
                option(length, "Length", &[(short, "100cm")], 1),
            ],
            variants: vec![
                variant(ProductVariantId::new(), "Red / 100cm", vec![red, short]),
                variant(ProductVariantId::new(), "100cm / Red", vec![short, red]),
            ],
        };

        assert!(matches!(
            validate_configuration(&draft),
            Err(ApplicationError::Validation { violations })
                if violations.iter().any(|violation| violation.reason.contains("combinations"))
        ));
    }
}
