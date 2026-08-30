use url::Url;
use uuid::Uuid;

use crate::{DomainError, FieldViolation};

use super::StoreId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SalesChannelId(Uuid);

impl SalesChannelId {
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

impl Default for SalesChannelId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StorefrontOrigin(String);

impl StorefrontOrigin {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let mut url = Url::parse(value.trim()).map_err(|_| origin_error())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || (!url.path().is_empty() && url.path() != "/")
        {
            return Err(origin_error());
        }
        url.set_path("/");
        Ok(Self(url.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn origin_error() -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field: "origin",
        reason: "must be an absolute HTTP(S) origin without credentials, path, query, or fragment"
            .into(),
    }])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SalesChannelStatus {
    Active,
    Archived,
}

impl SalesChannelStatus {
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
pub struct SalesChannel {
    id: SalesChannelId,
    store_id: StoreId,
    name: String,
    origin: StorefrontOrigin,
    status: SalesChannelStatus,
}

impl SalesChannel {
    pub fn create(
        store_id: StoreId,
        name: impl Into<String>,
        origin: StorefrontOrigin,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().count() > 120 {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "name",
                reason: "must contain 1-120 characters".into(),
            }]));
        }
        Ok(Self {
            id: SalesChannelId::new(),
            store_id,
            name,
            origin,
            status: SalesChannelStatus::Active,
        })
    }

    pub fn initial_web(store_id: StoreId, origin: StorefrontOrigin) -> Self {
        Self {
            id: SalesChannelId::new(),
            store_id,
            name: "Online Store".into(),
            origin,
            status: SalesChannelStatus::Active,
        }
    }

    pub const fn id(&self) -> SalesChannelId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn origin(&self) -> &StorefrontOrigin {
        &self.origin
    }

    pub const fn status(&self) -> SalesChannelStatus {
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_channel_is_an_active_web_surface() {
        let channel = SalesChannel::initial_web(
            StoreId::new(),
            StorefrontOrigin::parse("https://shop.example.test").unwrap(),
        );

        assert_eq!(channel.origin().as_str(), "https://shop.example.test/");
        assert_eq!(channel.status(), SalesChannelStatus::Active);
    }

    #[test]
    fn custom_channel_validates_content() {
        let channel = SalesChannel::create(
            StoreId::new(),
            "Mobile App",
            StorefrontOrigin::parse("https://mobile.example.test/").unwrap(),
        )
        .unwrap();
        assert_eq!(channel.status(), SalesChannelStatus::Active);
    }

    #[test]
    fn origin_normalizes_and_rejects_non_origins() {
        let origin = StorefrontOrigin::parse("https://SHOP.example.test").unwrap();
        assert_eq!(origin.as_str(), "https://shop.example.test/");

        for value in [
            "shop.example.test",
            "https://shop.example.test/orders",
            "https://user:password@shop.example.test",
            "https://shop.example.test?store=one",
            "https://shop.example.test/#token=secret",
        ] {
            assert!(StorefrontOrigin::parse(value).is_err(), "accepted {value}");
        }
    }
}
