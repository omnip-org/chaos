use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    DomainError, FieldViolation,
    catalog::{ProductId, ProductVariantId},
    sales::{CartId, CheckoutId},
};

pub const BROWSER_EVENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_ENGAGEMENT_INTERVAL_MILLISECONDS: u32 = 60_000;
pub const DEFAULT_RAW_EVENT_RETENTION_DAYS: u16 = 30;
pub const MAX_RAW_EVENT_RETENTION_DAYS: u16 = 400;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserCollectionMode {
    OptIn,
    OptOut,
}

impl BrowserCollectionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OptIn => "opt_in",
            Self::OptOut => "opt_out",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserCollectionBasis {
    Consent,
    StorePolicy,
}

impl BrowserCollectionBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Consent => "consent",
            Self::StorePolicy => "store_policy",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrafficTouchpoint {
    source: Option<String>,
    medium: Option<String>,
    campaign: Option<String>,
    campaign_id: Option<String>,
    term: Option<String>,
    content: Option<String>,
    referrer_domain: Option<String>,
    fbclid: Option<String>,
    gclid: Option<String>,
}

impl TrafficTouchpoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: Option<String>,
        medium: Option<String>,
        campaign: Option<String>,
        campaign_id: Option<String>,
        term: Option<String>,
        content: Option<String>,
        referrer_domain: Option<String>,
        fbclid: Option<String>,
        gclid: Option<String>,
    ) -> Result<Self, DomainError> {
        for (field, value, maximum) in [
            ("traffic.source", source.as_deref(), 100),
            ("traffic.medium", medium.as_deref(), 100),
            ("traffic.campaign", campaign.as_deref(), 200),
            ("traffic.campaign_id", campaign_id.as_deref(), 200),
            ("traffic.term", term.as_deref(), 200),
            ("traffic.content", content.as_deref(), 200),
            ("traffic.fbclid", fbclid.as_deref(), 512),
            ("traffic.gclid", gclid.as_deref(), 512),
        ] {
            validate_optional_text(field, value, maximum)?;
        }
        if let Some(domain) = referrer_domain.as_deref()
            && !valid_referrer_domain(domain)
        {
            return Err(validation(
                "traffic.referrer_domain",
                "must be a bounded ASCII host name",
            ));
        }
        Ok(Self {
            source,
            medium,
            campaign,
            campaign_id,
            term,
            content,
            referrer_domain,
            fbclid,
            gclid,
        })
    }

    pub fn fields(&self) -> [Option<&str>; 9] {
        [
            self.source.as_deref(),
            self.medium.as_deref(),
            self.campaign.as_deref(),
            self.campaign_id.as_deref(),
            self.term.as_deref(),
            self.content.as_deref(),
            self.referrer_domain.as_deref(),
            self.fbclid.as_deref(),
            self.gclid.as_deref(),
        ]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrafficAttribution {
    first: TrafficTouchpoint,
    session: TrafficTouchpoint,
    last_non_direct: Option<TrafficTouchpoint>,
}

impl TrafficAttribution {
    pub const fn new(
        first: TrafficTouchpoint,
        session: TrafficTouchpoint,
        last_non_direct: Option<TrafficTouchpoint>,
    ) -> Self {
        Self {
            first,
            session,
            last_non_direct,
        }
    }

    pub const fn first(&self) -> &TrafficTouchpoint {
        &self.first
    }
    pub const fn session(&self) -> &TrafficTouchpoint {
        &self.session
    }
    pub const fn last_non_direct(&self) -> Option<&TrafficTouchpoint> {
        self.last_non_direct.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticsSettings {
    collection_enabled: bool,
    browser_collection_mode: BrowserCollectionMode,
    provider_reporting_enabled: bool,
    identity_linking_enabled: bool,
    raw_event_retention_days: u16,
}

impl AnalyticsSettings {
    pub fn new(
        collection_enabled: bool,
        browser_collection_mode: BrowserCollectionMode,
        provider_reporting_enabled: bool,
        identity_linking_enabled: bool,
        raw_event_retention_days: u16,
    ) -> Result<Self, DomainError> {
        if !(1..=MAX_RAW_EVENT_RETENTION_DAYS).contains(&raw_event_retention_days) {
            return Err(validation(
                "raw_event_retention_days",
                "must be between 1 and 400",
            ));
        }
        Ok(Self {
            collection_enabled,
            browser_collection_mode,
            provider_reporting_enabled,
            identity_linking_enabled,
            raw_event_retention_days,
        })
    }

    pub fn builtin() -> Self {
        Self {
            collection_enabled: true,
            browser_collection_mode: BrowserCollectionMode::OptOut,
            provider_reporting_enabled: false,
            identity_linking_enabled: false,
            raw_event_retention_days: DEFAULT_RAW_EVENT_RETENTION_DAYS,
        }
    }

    pub const fn collection_enabled(self) -> bool {
        self.collection_enabled
    }

    pub const fn browser_collection_mode(self) -> BrowserCollectionMode {
        self.browser_collection_mode
    }

    pub const fn provider_reporting_enabled(self) -> bool {
        self.provider_reporting_enabled
    }

    pub const fn identity_linking_enabled(self) -> bool {
        self.identity_linking_enabled
    }

    pub const fn raw_event_retention_days(self) -> u16 {
        self.raw_event_retention_days
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsentSnapshot {
    analytics_storage: bool,
    advertising_storage: bool,
    policy_version: String,
}

impl ConsentSnapshot {
    pub fn new(
        analytics_storage: bool,
        advertising_storage: bool,
        policy_version: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let policy_version = policy_version.into();
        if policy_version.is_empty()
            || policy_version.len() > 64
            || !policy_version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(validation(
                "consent.policy_version",
                "must contain 1-64 ASCII letters, digits, dots, colons, hyphens, or underscores",
            ));
        }
        Ok(Self {
            analytics_storage,
            advertising_storage,
            policy_version,
        })
    }

    pub const fn analytics_storage(&self) -> bool {
        self.analytics_storage
    }

    pub const fn advertising_storage(&self) -> bool {
        self.advertising_storage
    }

    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserEventName {
    PageView,
    ViewContent,
    Search,
    AddToCart,
    InitiateCheckout,
    ViewDuration,
}

impl BrowserEventName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PageView => "page_view",
            Self::ViewContent => "view_content",
            Self::Search => "search",
            Self::AddToCart => "add_to_cart",
            Self::InitiateCheckout => "initiate_checkout",
            Self::ViewDuration => "view_duration",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "page_view" => Some(Self::PageView),
            "view_content" => Some(Self::ViewContent),
            "search" => Some(Self::Search),
            "add_to_cart" => Some(Self::AddToCart),
            "initiate_checkout" => Some(Self::InitiateCheckout),
            "view_duration" => Some(Self::ViewDuration),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrowserEventProperties {
    PageView {
        path: String,
        title: Option<String>,
        referrer_domain: Option<String>,
    },
    ViewContent {
        product_id: ProductId,
        product_variant_id: Option<ProductVariantId>,
    },
    Search {
        query: String,
        result_count: Option<u32>,
    },
    AddToCart {
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
    },
    InitiateCheckout {
        cart_id: CartId,
        checkout_id: Option<CheckoutId>,
    },
    ViewDuration {
        page_view_event_id: Uuid,
        active_milliseconds: u32,
    },
}

impl BrowserEventProperties {
    pub fn page_view(
        path: impl Into<String>,
        title: Option<String>,
        referrer_domain: Option<String>,
    ) -> Result<Self, DomainError> {
        let path = path.into();
        if !path.starts_with('/')
            || path.len() > 1024
            || path.contains(['?', '#'])
            || path.chars().any(char::is_control)
        {
            return Err(validation(
                "properties.path",
                "must be a control-free path without a query or fragment and contain at most 1024 bytes",
            ));
        }
        validate_optional_text("properties.title", title.as_deref(), 200)?;
        if let Some(domain) = referrer_domain.as_deref()
            && (domain.is_empty()
                || domain.len() > 253
                || !domain
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':')))
        {
            return Err(validation(
                "properties.referrer_domain",
                "must be a bounded ASCII host name",
            ));
        }
        Ok(Self::PageView {
            path,
            title,
            referrer_domain,
        })
    }

    pub fn view_content(
        product_id: ProductId,
        product_variant_id: Option<ProductVariantId>,
    ) -> Self {
        Self::ViewContent {
            product_id,
            product_variant_id,
        }
    }

    pub fn search(
        query: impl Into<String>,
        result_count: Option<u32>,
    ) -> Result<Self, DomainError> {
        let query = query.into();
        if query.trim().is_empty() || query.len() > 200 || query.chars().any(char::is_control) {
            return Err(validation(
                "properties.query",
                "must contain 1-200 control-free bytes",
            ));
        }
        if result_count.is_some_and(|count| count > 1_000_000) {
            return Err(validation(
                "properties.result_count",
                "must not exceed 1000000",
            ));
        }
        Ok(Self::Search {
            query,
            result_count,
        })
    }

    pub fn add_to_cart(
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
    ) -> Result<Self, DomainError> {
        if !(1..=10_000).contains(&quantity) {
            return Err(validation(
                "properties.quantity",
                "must be between 1 and 10000",
            ));
        }
        Ok(Self::AddToCart {
            cart_id,
            product_variant_id,
            quantity,
        })
    }

    pub const fn initiate_checkout(cart_id: CartId, checkout_id: Option<CheckoutId>) -> Self {
        Self::InitiateCheckout {
            cart_id,
            checkout_id,
        }
    }

    pub fn view_duration(
        page_view_event_id: Uuid,
        active_milliseconds: u32,
    ) -> Result<Self, DomainError> {
        if page_view_event_id.is_nil() {
            return Err(validation(
                "properties.page_view_event_id",
                "must be a non-nil UUID",
            ));
        }
        if !(1..=MAX_ENGAGEMENT_INTERVAL_MILLISECONDS).contains(&active_milliseconds) {
            return Err(validation(
                "properties.active_milliseconds",
                "must be between 1 and 60000",
            ));
        }
        Ok(Self::ViewDuration {
            page_view_event_id,
            active_milliseconds,
        })
    }

    pub const fn name(&self) -> BrowserEventName {
        match self {
            Self::PageView { .. } => BrowserEventName::PageView,
            Self::ViewContent { .. } => BrowserEventName::ViewContent,
            Self::Search { .. } => BrowserEventName::Search,
            Self::AddToCart { .. } => BrowserEventName::AddToCart,
            Self::InitiateCheckout { .. } => BrowserEventName::InitiateCheckout,
            Self::ViewDuration { .. } => BrowserEventName::ViewDuration,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserEvent {
    event_id: Uuid,
    schema_version: u16,
    occurred_at: OffsetDateTime,
    visitor_id: Uuid,
    session_id: Uuid,
    consent: ConsentSnapshot,
    collection_basis: BrowserCollectionBasis,
    traffic: Option<TrafficAttribution>,
    properties: BrowserEventProperties,
}

impl BrowserEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_id: Uuid,
        schema_version: u16,
        occurred_at: OffsetDateTime,
        visitor_id: Uuid,
        session_id: Uuid,
        consent: ConsentSnapshot,
        collection_basis: BrowserCollectionBasis,
        traffic: Option<TrafficAttribution>,
        properties: BrowserEventProperties,
    ) -> Result<Self, DomainError> {
        if event_id.is_nil() {
            return Err(validation("event_id", "must be a non-nil UUID"));
        }
        if schema_version != BROWSER_EVENT_SCHEMA_VERSION {
            return Err(validation("schema_version", "must equal 1"));
        }
        if visitor_id.is_nil() {
            return Err(validation("visitor_id", "must be a non-nil UUID"));
        }
        if session_id.is_nil() {
            return Err(validation("session_id", "must be a non-nil UUID"));
        }
        Ok(Self {
            event_id,
            schema_version,
            occurred_at,
            visitor_id,
            session_id,
            consent,
            collection_basis,
            traffic,
            properties,
        })
    }

    pub const fn event_id(&self) -> Uuid {
        self.event_id
    }

    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub const fn occurred_at(&self) -> OffsetDateTime {
        self.occurred_at
    }

    pub const fn visitor_id(&self) -> Uuid {
        self.visitor_id
    }

    pub const fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub const fn consent(&self) -> &ConsentSnapshot {
        &self.consent
    }

    pub const fn collection_basis(&self) -> BrowserCollectionBasis {
        self.collection_basis
    }

    pub const fn traffic(&self) -> Option<&TrafficAttribution> {
        self.traffic.as_ref()
    }

    pub const fn properties(&self) -> &BrowserEventProperties {
        &self.properties
    }

    pub const fn name(&self) -> BrowserEventName {
        self.properties.name()
    }
}

fn validate_optional_text(
    field: &'static str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<(), DomainError> {
    if value.is_some_and(|value| {
        value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control)
    }) {
        return Err(validation(
            field,
            format!("must contain 1-{max_bytes} control-free bytes when present"),
        ));
    }
    Ok(())
}

fn valid_referrer_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
}

fn validation(field: &'static str, reason: impl Into<String>) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_events_reject_unbounded_engagement_and_unsafe_paths() {
        let page_view_event_id = Uuid::now_v7();
        assert_eq!(
            BrowserEventProperties::view_duration(page_view_event_id, 60_000).unwrap(),
            BrowserEventProperties::ViewDuration {
                page_view_event_id,
                active_milliseconds: 60_000,
            }
        );
        assert!(BrowserEventProperties::view_duration(Uuid::now_v7(), 60_001).is_err());
        assert!(BrowserEventProperties::page_view("/products?total=100", None, None).is_err());
        assert!(
            TrafficTouchpoint::new(
                Some("x".repeat(101)),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            TrafficTouchpoint::new(
                Some("newsletter".into()),
                Some("email".into()),
                Some("launch".into()),
                None,
                None,
                None,
                Some("search.example".into()),
                None,
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn consent_requires_an_explicit_bounded_policy_version() {
        assert!(ConsentSnapshot::new(true, false, "cmp-2026-08").is_ok());
        assert!(ConsentSnapshot::new(true, false, "contains spaces").is_err());
    }

    #[test]
    fn analytics_settings_have_bounded_retention_and_conservative_defaults() {
        let default = AnalyticsSettings::builtin();
        assert!(default.collection_enabled());
        assert_eq!(
            default.browser_collection_mode(),
            BrowserCollectionMode::OptOut
        );
        assert!(!default.provider_reporting_enabled());
        assert!(!default.identity_linking_enabled());
        assert_eq!(default.raw_event_retention_days(), 30);
        assert!(
            AnalyticsSettings::new(true, BrowserCollectionMode::OptIn, false, false, 0).is_err()
        );
        assert!(
            AnalyticsSettings::new(true, BrowserCollectionMode::OptIn, false, false, 401,).is_err()
        );
    }
}
