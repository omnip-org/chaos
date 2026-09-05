use crate::{
    ApplicationError,
    contracts::{
        EmailAccountConfiguration, EmailBrandConfiguration, EmailBrandDetail, EmailMessage,
        EmailOrderLineItem, EmailProviderAccountDetail, EmailProviderAccountPage,
    },
    email_templates::{
        OrderConfirmationTemplateData, default_order_confirmation_template,
        render_order_confirmation,
    },
    error::database_error,
    store::StoreActor,
};
use chaos_domain::{
    identity::Email,
    sales::PostalAddress,
    store::{StoreId, StorefrontOrigin},
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresEmailRepository {
    pool: PgPool,
}

pub(crate) struct EmailProviderAccountWrite<'a> {
    pub display_name: &'a str,
    pub credential_secret_reference: &'a str,
    pub webhook_secret_reference: Option<&'a str>,
    pub configuration: &'a EmailAccountConfiguration,
    pub enabled: bool,
}

pub(crate) struct EmailBrandWrite {
    pub brand_name: Option<String>,
    pub logo_url: Option<String>,
    pub primary_color: String,
    pub accent_color: String,
    pub background_color: String,
    pub surface_color: String,
    pub text_color: String,
    pub muted_text_color: String,
    pub support_email: Option<String>,
    pub support_url: Option<String>,
}

impl PostgresEmailRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_human(
        &self,
        actor: StoreActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_admin_context(
            &mut transaction,
            Some(actor.user_id()),
            actor.store_id(),
        )
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }

    pub(crate) async fn list_provider_accounts(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<EmailProviderAccountPage, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let rows = sqlx::query_as::<_, EmailProviderAccountRow>(
            "SELECT id, provider, display_name, enabled, \
                    credential_secret_reference IS NOT NULL, \
                    webhook_secret_reference IS NOT NULL, configuration, \
                    created_at, updated_at \
             FROM integration.provider_accounts \
             WHERE store_id = $1 AND capability = 'email' \
               AND ($2::uuid IS NULL OR id < $2) \
             ORDER BY id DESC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let has_more = rows.len() > usize::from(limit);
        let items = rows
            .into_iter()
            .take(usize::from(limit))
            .map(email_provider_account_detail)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(database_error)?;
        Ok(EmailProviderAccountPage { items, has_more })
    }

    pub(crate) async fn get_provider_account(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        id: Uuid,
    ) -> Result<Option<EmailProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let value = load_email_provider_account(&mut transaction, store_id, id).await?;
        transaction.commit().await.map_err(database_error)?;
        value.map(email_provider_account_detail).transpose()
    }

    pub(crate) async fn create_provider_account(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        input: EmailProviderAccountWrite<'_>,
    ) -> Result<EmailProviderAccountDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO integration.provider_accounts \
             (id, store_id, capability, provider, display_name, \
              credential_secret_reference, webhook_secret_reference, configuration, enabled) \
             VALUES ($1, $2, 'email', 'resend', $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(store_id.as_uuid())
        .bind(input.display_name)
        .bind(input.credential_secret_reference)
        .bind(input.webhook_secret_reference)
        .bind(email_configuration_json(input.configuration))
        .bind(input.enabled)
        .execute(&mut *transaction)
        .await
        .map_err(map_email_provider_account_write_error)?;
        let value = load_email_provider_account(&mut transaction, store_id, id)
            .await?
            .ok_or_else(email_provider_account_corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        email_provider_account_detail(value)
    }

    pub(crate) async fn update_provider_account(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        id: Uuid,
        input: EmailProviderAccountWrite<'_>,
    ) -> Result<EmailProviderAccountDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let result = sqlx::query(
            "UPDATE integration.provider_accounts SET display_name = $3, \
                    credential_secret_reference = $4, \
                    webhook_secret_reference = $5, configuration = configuration || $6, \
                    enabled = $7, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2 AND capability = 'email'",
        )
        .bind(store_id.as_uuid())
        .bind(id)
        .bind(input.display_name)
        .bind(input.credential_secret_reference)
        .bind(input.webhook_secret_reference)
        .bind(email_configuration_json(input.configuration))
        .bind(input.enabled)
        .execute(&mut *transaction)
        .await
        .map_err(map_email_provider_account_write_error)?;
        if result.rows_affected() != 1 {
            return Err(email_provider_account_not_found(id));
        }
        let value = load_email_provider_account(&mut transaction, store_id, id)
            .await?
            .ok_or_else(email_provider_account_corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        email_provider_account_detail(value)
    }

    pub(crate) async fn get_email_brand(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<EmailBrandDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let row = load_email_brand(&mut transaction, store_id)
            .await?
            .ok_or_else(email_provider_account_not_found_for_brand)?;
        let detail = email_brand_detail(row)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn upsert_email_brand(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        input: &EmailBrandWrite,
    ) -> Result<EmailBrandDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let result = sqlx::query(
            "UPDATE integration.provider_accounts \
             SET configuration = configuration || jsonb_build_object('brand', $2::jsonb), \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND capability = 'email' AND provider = 'resend'",
        )
        .bind(store_id.as_uuid())
        .bind(email_brand_configuration_json(input))
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(email_provider_account_not_found_for_brand());
        }
        let row = load_email_brand(&mut transaction, store_id)
            .await?
            .ok_or_else(email_provider_account_not_found_for_brand)?;
        let detail = email_brand_detail(row)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn reset_email_brand(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<EmailBrandDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let result = sqlx::query(
            "UPDATE integration.provider_accounts \
             SET configuration = configuration - 'brand', updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND capability = 'email' AND provider = 'resend'",
        )
        .bind(store_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(email_provider_account_not_found_for_brand());
        }
        let row = load_email_brand(&mut transaction, store_id)
            .await?
            .ok_or_else(email_provider_account_not_found_for_brand)?;
        let detail = email_brand_detail(row)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    /// Returns `None` when the Order has no contact email yet (the shopper's
    /// payment webhook has not backfilled one). That is a terminal outcome,
    /// not a transient failure: there is nobody to send the confirmation to,
    /// and retrying will not change that once the checkout session itself
    /// has settled without an email.
    pub async fn prepare_order_confirmation(
        &self,
        store_id: Uuid,
        order_id: Uuid,
    ) -> Result<Option<(String, String, EmailMessage)>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let row = sqlx::query_as::<_, EmailOrderConfirmationRow>(
            "SELECT order_row.contact_email::text AS contact_email, \
                    order_row.order_number AS order_number, \
                    order_row.subtotal_amount_minor AS subtotal_amount_minor, \
                    order_row.discount_amount_minor AS discount_amount_minor, \
                    order_row.tax_amount_minor AS tax_amount_minor, \
                    order_row.shipping_amount_minor AS shipping_amount_minor, \
                    order_row.total_amount_minor AS total_amount_minor, \
                    order_row.currency::text AS currency, \
                    order_row.shipping_full_name AS shipping_full_name, \
                    order_row.shipping_address_line1 AS shipping_address_line1, \
                    order_row.shipping_address_line2 AS shipping_address_line2, \
                    order_row.shipping_locality AS shipping_locality, \
                    order_row.shipping_administrative_area AS shipping_administrative_area, \
                    order_row.shipping_postal_code AS shipping_postal_code, \
                    order_row.shipping_country_code::text AS shipping_country_code, \
                    channel.origin AS origin, \
                    account.provider AS provider, \
                    account.credential_secret_reference AS credential_secret_reference, \
                    account.configuration AS account_configuration \
             FROM commerce.orders AS order_row \
             INNER JOIN commerce.channels AS channel \
               ON channel.store_id = order_row.store_id \
              AND channel.id = order_row.channel_id \
             INNER JOIN integration.provider_accounts AS account \
               ON account.store_id = order_row.store_id \
              AND account.capability = 'email' \
              AND account.provider = 'resend' \
              AND account.enabled \
              AND account.credential_secret_reference IS NOT NULL \
             WHERE order_row.store_id = $1 AND order_row.id = $2 \
             ORDER BY account.id LIMIT 1",
        )
        .bind(store_id)
        .bind(order_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(email_provider_unavailable)?;
        let EmailOrderConfirmationRow {
            contact_email,
            order_number,
            subtotal_amount_minor,
            discount_amount_minor,
            tax_amount_minor,
            shipping_amount_minor,
            total_amount_minor,
            currency,
            shipping_full_name,
            shipping_address_line1,
            shipping_address_line2,
            shipping_locality,
            shipping_administrative_area,
            shipping_postal_code,
            shipping_country_code,
            origin,
            provider,
            credential_secret_reference,
            account_configuration,
        } = row;
        let Some(contact_email) = contact_email else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let shipping_address = optional_shipping_address(
            shipping_full_name,
            shipping_address_line1,
            shipping_address_line2,
            shipping_locality,
            shipping_administrative_area,
            shipping_postal_code,
            shipping_country_code,
        )?;
        let sender = parse_email_account_configuration(account_configuration)?.sender();
        let lookup_url = order_lookup_url(&origin, &order_number, &contact_email)?;
        let brand = load_email_brand(&mut transaction, StoreId::from_uuid(store_id))
            .await?
            .ok_or_else(email_provider_account_not_found_for_brand)
            .and_then(email_brand_detail)?
            .configuration;
        let line_items = sqlx::query_as::<_, EmailOrderLineRow>(
            "SELECT product_title, variant_title, sku, quantity, \
                    unit_price_amount_minor, subtotal_amount_minor \
             FROM commerce.order_lines \
             WHERE store_id = $1 AND order_id = $2 \
             ORDER BY position",
        )
        .bind(store_id)
        .bind(order_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(email_order_line_item)
        .collect::<Vec<_>>();
        transaction.commit().await.map_err(database_error)?;
        let template = render_order_confirmation(
            &default_order_confirmation_template(),
            &OrderConfirmationTemplateData {
                order_number: &order_number,
                subtotal_amount_minor,
                discount_amount_minor,
                tax_amount_minor,
                shipping_amount_minor,
                total_amount_minor,
                currency: &currency,
                lookup_url: lookup_url.as_str(),
                brand: &brand,
                line_items: &line_items,
                shipping_address: shipping_address.as_ref(),
            },
        );
        Ok(Some((
            provider,
            credential_secret_reference,
            EmailMessage {
                from: sender.to_owned(),
                to: contact_email,
                subject: template.subject,
                text: template.text,
                html: Some(template.html),
                idempotency_key: format!("order-confirmed-{}", order_id.simple()),
            },
        )))
    }
}

type EmailProviderAccountRow = (
    Uuid,
    String,
    String,
    bool,
    bool,
    bool,
    Value,
    time::OffsetDateTime,
    time::OffsetDateTime,
);

#[derive(sqlx::FromRow)]
struct EmailOrderConfirmationRow {
    contact_email: Option<String>,
    order_number: String,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    currency: String,
    shipping_full_name: Option<String>,
    shipping_address_line1: Option<String>,
    shipping_address_line2: Option<String>,
    shipping_locality: Option<String>,
    shipping_administrative_area: Option<String>,
    shipping_postal_code: Option<String>,
    shipping_country_code: Option<String>,
    origin: String,
    provider: String,
    credential_secret_reference: String,
    account_configuration: Value,
}

type EmailOrderLineRow = (String, String, Option<String>, i32, i64, i64);

type EmailBrandRow = (String, Value);

async fn load_email_brand(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
) -> Result<Option<EmailBrandRow>, ApplicationError> {
    sqlx::query_as::<_, EmailBrandRow>(
        "SELECT store.name, account.configuration \
         FROM commerce.stores AS store \
         INNER JOIN integration.provider_accounts AS account \
           ON account.store_id = store.id \
          AND account.capability = 'email' \
          AND account.provider = 'resend' \
         WHERE store.id = $1 \
         ORDER BY account.id LIMIT 1",
    )
    .bind(store_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

fn email_brand_detail(row: EmailBrandRow) -> Result<EmailBrandDetail, ApplicationError> {
    let (store_name, value) = row;
    let store_name = valid_brand_name(store_name)?;
    let Some(brand) = value.get("brand") else {
        return Ok(EmailBrandDetail {
            configuration: EmailBrandConfiguration::defaults(store_name),
            customized: false,
        });
    };
    let brand = brand.as_object().ok_or_else(email_brand_corrupt_state)?;
    let configuration = EmailBrandConfiguration {
        brand_name: brand_name(brand, &store_name)?,
        logo_url: optional_brand_url(brand, "logo_url")?,
        primary_color: required_brand_color(brand, "primary_color")?,
        accent_color: required_brand_color(brand, "accent_color")?,
        background_color: required_brand_color(brand, "background_color")?,
        surface_color: required_brand_color(brand, "surface_color")?,
        text_color: required_brand_color(brand, "text_color")?,
        muted_text_color: required_brand_color(brand, "muted_text_color")?,
        support_email: optional_brand_email(brand, "support_email")?,
        support_url: optional_brand_url(brand, "support_url")?,
    };
    Ok(EmailBrandDetail {
        configuration,
        customized: true,
    })
}

fn required_brand_string(
    brand: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, ApplicationError> {
    brand
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(email_brand_corrupt_state)
}

fn brand_name(
    brand: &serde_json::Map<String, Value>,
    fallback: &str,
) -> Result<String, ApplicationError> {
    match brand.get("brand_name") {
        None | Some(Value::Null) => Ok(fallback.to_owned()),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Ok(fallback.to_owned())
            } else if value.chars().count() > 120 || value.chars().any(char::is_control) {
                Err(email_brand_corrupt_state())
            } else {
                Ok(value.to_owned())
            }
        }
        _ => Err(email_brand_corrupt_state()),
    }
}

fn valid_brand_name(value: String) -> Result<String, ApplicationError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 120 || value.chars().any(char::is_control) {
        return Err(email_brand_corrupt_state());
    }
    Ok(value.to_owned())
}

fn required_brand_color(
    brand: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, ApplicationError> {
    let value = required_brand_string(brand, key)?;
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(email_brand_corrupt_state());
    }
    Ok(value.to_ascii_uppercase())
}

fn optional_brand_string(
    brand: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ApplicationError> {
    match brand.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(email_brand_corrupt_state()),
    }
}

fn optional_brand_url(
    brand: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ApplicationError> {
    let Some(value) = optional_brand_string(brand, key)? else {
        return Ok(None);
    };
    let value = value.trim();
    let parsed = Url::parse(value).map_err(|_| email_brand_corrupt_state())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || value.len() > 2048
        || value.chars().any(char::is_control)
    {
        return Err(email_brand_corrupt_state());
    }
    Ok(Some(value.to_owned()))
}

fn optional_brand_email(
    brand: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ApplicationError> {
    optional_brand_string(brand, key)?
        .map(|value| {
            Email::parse(value)
                .map(|email| email.as_str().to_owned())
                .map_err(|_| email_brand_corrupt_state())
        })
        .transpose()
}

fn email_order_line_item(row: EmailOrderLineRow) -> EmailOrderLineItem {
    EmailOrderLineItem {
        product_title: row.0,
        variant_title: row.1,
        sku: row.2,
        quantity: row.3,
        unit_price_amount_minor: row.4,
        subtotal_amount_minor: row.5,
    }
}

async fn load_email_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: Uuid,
) -> Result<Option<EmailProviderAccountRow>, ApplicationError> {
    sqlx::query_as::<_, EmailProviderAccountRow>(
        "SELECT id, provider, display_name, enabled, \
                credential_secret_reference IS NOT NULL, \
                webhook_secret_reference IS NOT NULL, configuration, \
                created_at, updated_at \
         FROM integration.provider_accounts \
         WHERE store_id = $1 AND id = $2 AND capability = 'email'",
    )
    .bind(store_id.as_uuid())
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

fn email_provider_account_detail(
    row: EmailProviderAccountRow,
) -> Result<EmailProviderAccountDetail, ApplicationError> {
    Ok(EmailProviderAccountDetail {
        id: row.0,
        provider: row.1,
        display_name: row.2,
        enabled: row.3,
        credentials_configured: row.4,
        webhook_configured: row.5,
        configuration: parse_email_account_configuration(row.6)?,
        created_at: row.7,
        updated_at: row.8,
    })
}

fn email_configuration_json(configuration: &EmailAccountConfiguration) -> Value {
    json!({
        "from_email": configuration.from_email,
        "from_name": configuration.from_name,
    })
}

fn email_brand_configuration_json(configuration: &EmailBrandWrite) -> Value {
    json!({
        "brand_name": configuration.brand_name,
        "logo_url": configuration.logo_url,
        "primary_color": configuration.primary_color,
        "accent_color": configuration.accent_color,
        "background_color": configuration.background_color,
        "surface_color": configuration.surface_color,
        "text_color": configuration.text_color,
        "muted_text_color": configuration.muted_text_color,
        "support_email": configuration.support_email,
        "support_url": configuration.support_url,
    })
}

fn parse_email_account_configuration(
    value: Value,
) -> Result<EmailAccountConfiguration, ApplicationError> {
    let from_email = value
        .get("from_email")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(email_provider_account_corrupt_state)?;
    let from_name = match value.get("from_name") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        _ => return Err(email_provider_account_corrupt_state()),
    };
    Ok(EmailAccountConfiguration {
        from_email: from_email.to_owned(),
        from_name,
    })
}

fn map_email_provider_account_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error
        && database.constraint() == Some("provider_accounts_store_capability_provider_key")
    {
        return ApplicationError::Conflict {
            code: "email_provider_already_configured",
            message: "the Resend Email Provider is already configured for this Store",
        };
    }
    database_error(error)
}

fn email_provider_account_not_found(id: Uuid) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "email_provider_account",
        id: id.to_string(),
    }
}

fn email_provider_account_corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Email Provider account state"
    ))
}

fn email_provider_account_not_found_for_brand() -> ApplicationError {
    ApplicationError::NotFound {
        resource: "email_provider_account",
        id: "email_brand".into(),
    }
}

fn email_brand_corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Email brand state"
    ))
}

#[allow(clippy::too_many_arguments)]
fn optional_shipping_address(
    full_name: Option<String>,
    address_line1: Option<String>,
    address_line2: Option<String>,
    locality: Option<String>,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: Option<String>,
) -> Result<Option<PostalAddress>, ApplicationError> {
    let full_name = normalize_optional_text(full_name);
    let address_line1 = normalize_optional_text(address_line1);
    let address_line2 = normalize_optional_text(address_line2);
    let locality = normalize_optional_text(locality);
    let administrative_area = normalize_optional_text(administrative_area);
    let postal_code = normalize_optional_text(postal_code);
    let country_code = normalize_optional_text(country_code);
    let any = full_name.is_some()
        || address_line1.is_some()
        || address_line2.is_some()
        || locality.is_some()
        || administrative_area.is_some()
        || postal_code.is_some()
        || country_code.is_some();
    match (full_name, address_line1, locality, country_code) {
        (None, None, None, None) if !any => Ok(None),
        (Some(full_name), Some(address_line1), Some(locality), Some(country_code)) => {
            Ok(Some(PostalAddress::new(
                full_name,
                address_line1,
                address_line2,
                locality,
                administrative_area,
                postal_code,
                country_code,
            )?))
        }
        _ => Err(email_order_corrupt_state()),
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn email_order_corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Order shipping address"
    ))
}

fn order_lookup_url(
    origin: &str,
    order_number: &str,
    email: &str,
) -> Result<Url, ApplicationError> {
    let origin = StorefrontOrigin::parse(origin.to_owned())
        .map_err(|error| invalid_email_url(error.to_string()))?;
    let mut lookup_url = Url::parse(origin.as_str())
        .map_err(|error| invalid_email_url(error.to_string()))?
        .join("orders/lookup")
        .map_err(|error| invalid_email_url(error.to_string()))?;
    lookup_url
        .query_pairs_mut()
        .append_pair("order_number", order_number)
        .append_pair("email", email);
    Ok(lookup_url)
}

fn email_provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "email_provider_unavailable",
        message: "no configured Email provider account is available",
    }
}

fn invalid_email_url(error: String) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("failed to build order lookup URL: {error}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{email_brand_detail, order_lookup_url};

    #[test]
    fn lookup_url_uses_the_sales_channel_origin_and_encodes_the_pair() {
        let first = order_lookup_url(
            "https://first.example.test",
            "W-20260820-7K4M9Q2D",
            "buyer@example.test",
        )
        .unwrap();
        let second = order_lookup_url(
            "https://second.example.test/",
            "W-20260820-7K4M9Q2D",
            "a+b@example.test",
        )
        .unwrap();

        assert_eq!(
            first.as_str(),
            "https://first.example.test/orders/lookup?order_number=W-20260820-7K4M9Q2D&email=buyer%40example.test"
        );
        assert_eq!(
            second.as_str(),
            "https://second.example.test/orders/lookup?order_number=W-20260820-7K4M9Q2D&email=a%2Bb%40example.test"
        );
    }

    #[test]
    fn reads_the_embedded_brand_configuration() {
        let detail = email_brand_detail((
            "Store fallback".into(),
            json!({
                "brand": {
                    "brand_name": "  Example Brand  ",
                    "logo_url": "https://cdn.example/logo.png",
                    "primary_color": "#175cd3",
                    "accent_color": "#0e7490",
                    "background_color": "#f4f6f8",
                    "surface_color": "#ffffff",
                    "text_color": "#17202a",
                    "muted_text_color": "#667085",
                    "support_email": "SUPPORT@example.com",
                    "support_url": "https://example.com/help"
                }
            }),
        ))
        .unwrap();

        assert!(detail.customized);
        assert_eq!(detail.configuration.brand_name, "Example Brand");
        assert_eq!(detail.configuration.primary_color, "#175CD3");
        assert_eq!(
            detail.configuration.support_email.as_deref(),
            Some("support@example.com")
        );
    }

    #[test]
    fn rejects_unsafe_embedded_brand_values() {
        let result = email_brand_detail((
            "Store fallback".into(),
            json!({
                "brand": {
                    "brand_name": "Example Brand",
                    "logo_url": "javascript:alert(1)",
                    "primary_color": "#175CD3",
                    "accent_color": "#0E7490",
                    "background_color": "#F4F6F8",
                    "surface_color": "#FFFFFF",
                    "text_color": "#17202A",
                    "muted_text_color": "not-a-color"
                }
            }),
        ));

        assert!(result.is_err());
    }
}
