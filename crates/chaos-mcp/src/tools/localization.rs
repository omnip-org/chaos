use chaos_application::{
    catalog::{
        MediaTranslationActionInput, ProductVariantTranslationInput, StoreLocaleInput,
        TranslationActionInput, UpsertCollectionTranslationInput, UpsertMediaTranslationInput,
        UpsertProductTranslationInput,
    },
    ports::{AdminActor, CollectionTranslation, MediaTranslation, ProductTranslation},
};
use chaos_domain::{
    catalog::{CollectionId, MediaAssetId, ProductId, ProductVariantId},
    merchant::ApiKeyScope,
};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use super::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeLocaleParams {
    /// BCP 47 locale tag (e.g. "de", "fr-CA").
    pub locale: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetProductTranslationParams {
    /// The product's UUID.
    pub product_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductVariantTranslationParams {
    /// The product variant's UUID.
    pub product_variant_id: String,
    pub title: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpsertProductTranslationParams {
    /// The product's UUID.
    pub product_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub variants: Vec<ProductVariantTranslationParams>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RemoveProductTranslationParams {
    /// The product's UUID.
    pub product_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetCollectionTranslationParams {
    /// The collection's UUID.
    pub collection_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpsertCollectionTranslationParams {
    /// The collection's UUID.
    pub collection_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
    pub title: String,
    pub description: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RemoveCollectionTranslationParams {
    /// The collection's UUID.
    pub collection_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetMediaTranslationParams {
    /// The product's UUID.
    pub product_id: String,
    /// The media asset's UUID.
    pub media_asset_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpsertMediaTranslationParams {
    /// The product's UUID.
    pub product_id: String,
    /// The media asset's UUID.
    pub media_asset_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
    pub alt_text: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RemoveMediaTranslationParams {
    /// The product's UUID.
    pub product_id: String,
    /// The media asset's UUID.
    pub media_asset_id: String,
    /// BCP 47 locale tag.
    pub locale: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = localization_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(
        description = "List the enabled locales and default locale for the Store bound to \
                        this API key."
    )]
    async fn list_store_locales(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminRead,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;

        match self
            .state
            .catalog_localization
            .store_locales(actor, store_id)
            .await
        {
            Ok(configuration) => Ok(text_result(json!({
                "default_locale": configuration.default_locale.as_str(),
                "enabled_locales": configuration.enabled_locales.iter().map(|locale| locale.as_str()).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Enable a locale for the Store bound to this API key. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn enable_locale(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeLocaleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_locale(parts, params, LocaleAction::Enable)
            .await
    }

    #[tool(
        description = "Set the default locale for the Store bound to this API key. The locale \
                        must already be enabled. Requires confirm: true and an idempotency_key."
    )]
    async fn set_default_locale(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeLocaleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_locale(parts, params, LocaleAction::SetDefault)
            .await
    }

    #[tool(
        description = "Disable a locale for the Store bound to this API key. The default \
                        locale cannot be disabled. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn disable_locale(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeLocaleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_locale(parts, params, LocaleAction::Disable)
            .await
    }

    #[tool(
        description = "Get a product's translated title, description, and variant titles for \
                        a locale, in the Store bound to this API key."
    )]
    async fn get_product_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetProductTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::ProductsRead,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .catalog_localization
            .product_translation(actor, store_id, product_id, &params.locale)
            .await
        {
            Ok(translation) => Ok(text_result(product_translation_json(translation))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create or replace a product's translated title, description, and \
                        variant titles for a locale, in the Store bound to this API key. \
                        Requires confirm: true and an idempotency_key."
    )]
    async fn upsert_product_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpsertProductTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::ProductsWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let variants = match parse_variant_translations(&params.variants) {
            Ok(variants) => variants,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .catalog_localization
            .upsert_product_translation(UpsertProductTranslationInput {
                actor,
                store_id,
                product_id,
                locale: params.locale,
                title: params.title,
                description: params.description,
                variants,
                idempotency,
                now,
            })
            .await
        {
            Ok(translation) => Ok(text_result(product_translation_json(translation))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Remove a product's translation for a locale, in the Store bound to \
                        this API key. Requires confirm: true and an idempotency_key."
    )]
    async fn remove_product_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RemoveProductTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::ProductsWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .catalog_localization
            .remove_product_translation(TranslationActionInput {
                actor,
                store_id,
                resource_id: product_id,
                locale: params.locale,
                idempotency,
                now,
            })
            .await
        {
            Ok(()) => Ok(text_result(json!({ "removed": true }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get a collection's translated title and description for a locale, in \
                        the Store bound to this API key."
    )]
    async fn get_collection_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetCollectionTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::CollectionsRead,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .catalog_localization
            .collection_translation(actor, store_id, collection_id, &params.locale)
            .await
        {
            Ok(translation) => Ok(text_result(collection_translation_json(translation))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create or replace a collection's translated title and description for \
                        a locale, in the Store bound to this API key. Requires confirm: true \
                        and an idempotency_key."
    )]
    async fn upsert_collection_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpsertCollectionTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::CollectionsWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .catalog_localization
            .upsert_collection_translation(UpsertCollectionTranslationInput {
                actor,
                store_id,
                collection_id,
                locale: params.locale,
                title: params.title,
                description: params.description,
                idempotency,
                now,
            })
            .await
        {
            Ok(translation) => Ok(text_result(collection_translation_json(translation))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Remove a collection's translation for a locale, in the Store bound to \
                        this API key. Requires confirm: true and an idempotency_key."
    )]
    async fn remove_collection_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RemoveCollectionTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::CollectionsWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .catalog_localization
            .remove_collection_translation(TranslationActionInput {
                actor,
                store_id,
                resource_id: collection_id,
                locale: params.locale,
                idempotency,
                now,
            })
            .await
        {
            Ok(()) => Ok(text_result(json!({ "removed": true }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get a media asset's translated alt text for a locale, in the Store \
                        bound to this API key."
    )]
    async fn get_media_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetMediaTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::MediaRead,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_uuid_field(&params.media_asset_id, "media_asset_id") {
            Ok(id) => MediaAssetId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .catalog_localization
            .media_translation(actor, store_id, product_id, media_asset_id, &params.locale)
            .await
        {
            Ok(translation) => Ok(text_result(media_translation_json(translation))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create or replace a media asset's translated alt text for a locale, in \
                        the Store bound to this API key. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn upsert_media_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpsertMediaTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::MediaWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_uuid_field(&params.media_asset_id, "media_asset_id") {
            Ok(id) => MediaAssetId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .catalog_localization
            .upsert_media_translation(UpsertMediaTranslationInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                locale: params.locale,
                alt_text: params.alt_text,
                idempotency,
                now,
            })
            .await
        {
            Ok(translation) => Ok(text_result(media_translation_json(translation))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Remove a media asset's translation for a locale, in the Store bound to \
                        this API key. Requires confirm: true and an idempotency_key."
    )]
    async fn remove_media_translation(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RemoveMediaTranslationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::MediaWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_uuid_field(&params.media_asset_id, "media_asset_id") {
            Ok(id) => MediaAssetId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .catalog_localization
            .remove_media_translation(MediaTranslationActionInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                locale: params.locale,
                idempotency,
                now,
            })
            .await
        {
            Ok(()) => Ok(text_result(json!({ "removed": true }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

enum LocaleAction {
    Enable,
    SetDefault,
    Disable,
}

impl ChaosMcp {
    async fn change_locale(
        &self,
        parts: http::request::Parts,
        params: ChangeLocaleParams,
        action: LocaleAction,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        let input = StoreLocaleInput {
            actor,
            store_id,
            locale: params.locale,
            idempotency,
            now,
        };
        let result = match action {
            LocaleAction::Enable => self.state.catalog_localization.enable_locale(input).await,
            LocaleAction::SetDefault => {
                self.state
                    .catalog_localization
                    .set_default_locale(input)
                    .await
            }
            LocaleAction::Disable => self.state.catalog_localization.disable_locale(input).await,
        };
        match result {
            Ok(configuration) => Ok(text_result(json!({
                "default_locale": configuration.default_locale.as_str(),
                "enabled_locales": configuration.enabled_locales.iter().map(|locale| locale.as_str()).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_variant_translations(
    params: &[ProductVariantTranslationParams],
) -> Result<Vec<ProductVariantTranslationInput>, CallToolResult> {
    params
        .iter()
        .map(|variant| {
            parse_uuid_field(&variant.product_variant_id, "product_variant_id").map(|id| {
                ProductVariantTranslationInput {
                    product_variant_id: ProductVariantId::from_uuid(id),
                    title: variant.title.clone(),
                }
            })
        })
        .collect()
}

fn product_translation_json(translation: ProductTranslation) -> serde_json::Value {
    json!({
        "locale": translation.content.locale().as_str(),
        "title": translation.content.title(),
        "description": translation.content.description(),
        "variants": translation.variants.into_iter().map(|variant| json!({
            "product_variant_id": variant.product_variant_id.as_uuid(),
            "title": variant.title.as_str(),
        })).collect::<Vec<_>>(),
        "created_at": format_time(translation.created_at),
        "updated_at": format_time(translation.updated_at),
    })
}

fn collection_translation_json(translation: CollectionTranslation) -> serde_json::Value {
    json!({
        "locale": translation.content.locale().as_str(),
        "title": translation.content.title(),
        "description": translation.content.description(),
        "created_at": format_time(translation.created_at),
        "updated_at": format_time(translation.updated_at),
    })
}

fn media_translation_json(translation: MediaTranslation) -> serde_json::Value {
    json!({
        "locale": translation.locale.as_str(),
        "alt_text": translation.alt_text.as_str(),
        "created_at": format_time(translation.created_at),
        "updated_at": format_time(translation.updated_at),
    })
}

fn parse_uuid_field(value: &str, field: &'static str) -> Result<uuid::Uuid, CallToolResult> {
    uuid::Uuid::parse_str(value).map_err(|_| {
        CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": format!("{field} must be a valid UUID"),
        }))
    })
}

fn format_time(value: time::OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}
