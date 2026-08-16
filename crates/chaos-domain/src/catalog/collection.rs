use uuid::Uuid;

use crate::{DomainError, FieldViolation};

macro_rules! collection_id {
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

collection_id!(CollectionId);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CollectionHandle(String);

impl CollectionHandle {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionContent {
    handle: CollectionHandle,
    title: String,
    description: String,
}

impl CollectionContent {
    pub fn new(
        handle: CollectionHandle,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let title = title.into();
        let description = description.into();
        if title.trim().is_empty() || title.chars().count() > 255 {
            return Err(validation("title", "must contain 1-255 characters"));
        }
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
        })
    }

    pub const fn handle(&self) -> &CollectionHandle {
        &self.handle
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionStatus {
    Draft,
    Active,
    Archived,
}

impl CollectionStatus {
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

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::{CollectionContent, CollectionHandle};

    #[test]
    fn collection_content_has_canonical_bounded_fields() {
        assert!(CollectionHandle::parse("summer-sale").is_ok());
        assert!(CollectionHandle::parse("Summer Sale").is_err());
        assert!(CollectionHandle::parse("a").is_err());
        assert!(
            CollectionContent::new(
                CollectionHandle::parse("summer-sale").unwrap(),
                "Summer Sale",
                "Seasonal products",
            )
            .is_ok()
        );
        assert!(
            CollectionContent::new(CollectionHandle::parse("summer-sale").unwrap(), " ", "",)
                .is_err()
        );
    }
}
