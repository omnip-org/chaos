use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    DomainError, FieldViolation,
    catalog::{ProductId, ProductVariantId},
    sales::{CartId, CheckoutId},
};

pub const BROWSER_EVENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_ENGAGEMENT_INTERVAL_MILLISECONDS: u32 = 60_000;
pub const MAX_SESSION_ENGAGEMENT_MILLISECONDS: u64 = 14_400_000;
pub const SESSION_INACTIVITY_MINUTES: i64 = 30;
pub const DEFAULT_RAW_EVENT_RETENTION_DAYS: u16 = 30;
pub const MAX_RAW_EVENT_RETENTION_DAYS: u16 = 400;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticsPolicy {
    behavior_collection_enabled: bool,
    advertising_exports_enabled: bool,
    identity_linking_enabled: bool,
    raw_event_retention_days: u16,
}

impl AnalyticsPolicy {
    pub fn new(
        behavior_collection_enabled: bool,
        advertising_exports_enabled: bool,
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
            behavior_collection_enabled,
            advertising_exports_enabled,
            identity_linking_enabled,
            raw_event_retention_days,
        })
    }

    pub fn builtin() -> Self {
        Self {
            behavior_collection_enabled: true,
            advertising_exports_enabled: false,
            identity_linking_enabled: false,
            raw_event_retention_days: DEFAULT_RAW_EVENT_RETENTION_DAYS,
        }
    }

    pub const fn behavior_collection_enabled(self) -> bool {
        self.behavior_collection_enabled
    }

    pub const fn advertising_exports_enabled(self) -> bool {
        self.advertising_exports_enabled
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
    PageViewed,
    ProductViewed,
    SearchPerformed,
    CartLineAdded,
    CheckoutStarted,
    EngagementHeartbeat,
}

impl BrowserEventName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PageViewed => "page_viewed",
            Self::ProductViewed => "product_viewed",
            Self::SearchPerformed => "search_performed",
            Self::CartLineAdded => "cart_line_added",
            Self::CheckoutStarted => "checkout_started",
            Self::EngagementHeartbeat => "engagement_heartbeat",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "page_viewed" => Some(Self::PageViewed),
            "product_viewed" => Some(Self::ProductViewed),
            "search_performed" => Some(Self::SearchPerformed),
            "cart_line_added" => Some(Self::CartLineAdded),
            "checkout_started" => Some(Self::CheckoutStarted),
            "engagement_heartbeat" => Some(Self::EngagementHeartbeat),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionEventContribution {
    pub event_count: u64,
    pub page_view_count: u64,
    pub product_view_count: u64,
    pub search_count: u64,
    pub cart_line_added_count: u64,
    pub checkout_started_count: u64,
    pub active_engagement_milliseconds: u64,
}

impl SessionEventContribution {
    pub fn from_event(
        event_name: BrowserEventName,
        active_engagement_milliseconds: Option<u32>,
    ) -> Result<Self, DomainError> {
        let mut value = Self {
            event_count: 1,
            ..Self::default()
        };
        match event_name {
            BrowserEventName::PageViewed => value.page_view_count = 1,
            BrowserEventName::ProductViewed => value.product_view_count = 1,
            BrowserEventName::SearchPerformed => value.search_count = 1,
            BrowserEventName::CartLineAdded => value.cart_line_added_count = 1,
            BrowserEventName::CheckoutStarted => value.checkout_started_count = 1,
            BrowserEventName::EngagementHeartbeat => {
                let active = active_engagement_milliseconds.ok_or_else(|| {
                    validation(
                        "active_engagement_milliseconds",
                        "is required for an engagement heartbeat",
                    )
                })?;
                if !(1..=MAX_ENGAGEMENT_INTERVAL_MILLISECONDS).contains(&active) {
                    return Err(validation(
                        "active_engagement_milliseconds",
                        "must be between 1 and 60000",
                    ));
                }
                value.active_engagement_milliseconds = u64::from(active);
            }
        }
        if event_name != BrowserEventName::EngagementHeartbeat
            && active_engagement_milliseconds.is_some()
        {
            return Err(validation(
                "active_engagement_milliseconds",
                "is allowed only for an engagement heartbeat",
            ));
        }
        Ok(value)
    }
}

pub fn capped_session_engagement(current: u64, addition: u64) -> Result<u64, DomainError> {
    if current > MAX_SESSION_ENGAGEMENT_MILLISECONDS {
        return Err(validation(
            "active_engagement_milliseconds",
            "stored session engagement exceeds the supported cap",
        ));
    }
    Ok(current
        .saturating_add(addition)
        .min(MAX_SESSION_ENGAGEMENT_MILLISECONDS))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrowserEventProperties {
    PageViewed {
        path: String,
        title: Option<String>,
        referrer_domain: Option<String>,
        campaign_source: Option<String>,
        campaign_medium: Option<String>,
        campaign_name: Option<String>,
    },
    ProductViewed {
        product_id: ProductId,
        product_variant_id: Option<ProductVariantId>,
    },
    SearchPerformed {
        query: String,
        result_count: Option<u32>,
    },
    CartLineAdded {
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
    },
    CheckoutStarted {
        cart_id: CartId,
        checkout_id: Option<CheckoutId>,
    },
    EngagementHeartbeat {
        page_view_event_id: Uuid,
        active_milliseconds: u32,
    },
}

impl BrowserEventProperties {
    pub fn page_viewed(
        path: impl Into<String>,
        title: Option<String>,
        referrer_domain: Option<String>,
        campaign_source: Option<String>,
        campaign_medium: Option<String>,
        campaign_name: Option<String>,
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
        validate_optional_text(
            "properties.campaign_source",
            campaign_source.as_deref(),
            100,
        )?;
        validate_optional_text(
            "properties.campaign_medium",
            campaign_medium.as_deref(),
            100,
        )?;
        validate_optional_text("properties.campaign_name", campaign_name.as_deref(), 200)?;
        if campaign_source.is_none() && (campaign_medium.is_some() || campaign_name.is_some()) {
            return Err(validation(
                "properties.campaign_source",
                "is required when campaign medium or name is present",
            ));
        }
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
        Ok(Self::PageViewed {
            path,
            title,
            referrer_domain,
            campaign_source,
            campaign_medium,
            campaign_name,
        })
    }

    pub fn product_viewed(
        product_id: ProductId,
        product_variant_id: Option<ProductVariantId>,
    ) -> Self {
        Self::ProductViewed {
            product_id,
            product_variant_id,
        }
    }

    pub fn search_performed(
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
        Ok(Self::SearchPerformed {
            query,
            result_count,
        })
    }

    pub fn cart_line_added(
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
        Ok(Self::CartLineAdded {
            cart_id,
            product_variant_id,
            quantity,
        })
    }

    pub const fn checkout_started(cart_id: CartId, checkout_id: Option<CheckoutId>) -> Self {
        Self::CheckoutStarted {
            cart_id,
            checkout_id,
        }
    }

    pub fn engagement_heartbeat(
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
        Ok(Self::EngagementHeartbeat {
            page_view_event_id,
            active_milliseconds,
        })
    }

    pub const fn name(&self) -> BrowserEventName {
        match self {
            Self::PageViewed { .. } => BrowserEventName::PageViewed,
            Self::ProductViewed { .. } => BrowserEventName::ProductViewed,
            Self::SearchPerformed { .. } => BrowserEventName::SearchPerformed,
            Self::CartLineAdded { .. } => BrowserEventName::CartLineAdded,
            Self::CheckoutStarted { .. } => BrowserEventName::CheckoutStarted,
            Self::EngagementHeartbeat { .. } => BrowserEventName::EngagementHeartbeat,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserEvent {
    event_id: Uuid,
    schema_version: u16,
    occurred_at: OffsetDateTime,
    anonymous_id: Uuid,
    session_id: Uuid,
    consent: ConsentSnapshot,
    properties: BrowserEventProperties,
}

impl BrowserEvent {
    pub fn new(
        event_id: Uuid,
        schema_version: u16,
        occurred_at: OffsetDateTime,
        anonymous_id: Uuid,
        session_id: Uuid,
        consent: ConsentSnapshot,
        properties: BrowserEventProperties,
    ) -> Result<Self, DomainError> {
        if event_id.is_nil() {
            return Err(validation("event_id", "must be a non-nil UUID"));
        }
        if schema_version != BROWSER_EVENT_SCHEMA_VERSION {
            return Err(validation("schema_version", "must equal 1"));
        }
        if anonymous_id.is_nil() {
            return Err(validation("anonymous_id", "must be a non-nil UUID"));
        }
        if session_id.is_nil() {
            return Err(validation("session_id", "must be a non-nil UUID"));
        }
        Ok(Self {
            event_id,
            schema_version,
            occurred_at,
            anonymous_id,
            session_id,
            consent,
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

    pub const fn anonymous_id(&self) -> Uuid {
        self.anonymous_id
    }

    pub const fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub const fn consent(&self) -> &ConsentSnapshot {
        &self.consent
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
            BrowserEventProperties::engagement_heartbeat(page_view_event_id, 60_000).unwrap(),
            BrowserEventProperties::EngagementHeartbeat {
                page_view_event_id,
                active_milliseconds: 60_000,
            }
        );
        assert!(BrowserEventProperties::engagement_heartbeat(Uuid::now_v7(), 60_001).is_err());
        assert!(
            BrowserEventProperties::page_viewed(
                "/products?total=100",
                None,
                None,
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            BrowserEventProperties::page_viewed(
                "/products",
                None,
                None,
                None,
                Some("email".into()),
                Some("launch".into()),
            )
            .is_err()
        );
        assert!(
            BrowserEventProperties::page_viewed(
                "/products",
                None,
                None,
                Some("newsletter".into()),
                Some("email".into()),
                Some("launch".into()),
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
    fn session_contributions_cap_estimated_engagement() {
        let contribution = SessionEventContribution::from_event(
            BrowserEventName::EngagementHeartbeat,
            Some(60_000),
        )
        .unwrap();
        assert_eq!(contribution.active_engagement_milliseconds, 60_000);
        assert_eq!(
            capped_session_engagement(14_390_000, contribution.active_engagement_milliseconds)
                .unwrap(),
            MAX_SESSION_ENGAGEMENT_MILLISECONDS
        );
        assert!(
            SessionEventContribution::from_event(BrowserEventName::PageViewed, Some(1)).is_err()
        );
    }

    #[test]
    fn analytics_policy_has_bounded_retention_and_conservative_defaults() {
        let default = AnalyticsPolicy::builtin();
        assert!(default.behavior_collection_enabled());
        assert!(!default.advertising_exports_enabled());
        assert!(!default.identity_linking_enabled());
        assert_eq!(default.raw_event_retention_days(), 30);
        assert!(AnalyticsPolicy::new(true, false, false, 0).is_err());
        assert!(AnalyticsPolicy::new(true, false, false, 401).is_err());
    }
}
