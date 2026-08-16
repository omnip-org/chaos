use crate::{DomainError, FieldViolation, Locale};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalizedContent {
    locale: Locale,
    title: String,
    description: String,
}

impl LocalizedContent {
    pub fn new(
        locale: Locale,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let title = title.into();
        let description = description.into();
        validate_title(&title)?;
        if description.chars().count() > 100_000 {
            return Err(validation(
                "description",
                "must contain at most 100000 characters",
            ));
        }
        Ok(Self {
            locale,
            title,
            description,
        })
    }

    pub fn locale(&self) -> &Locale {
        &self.locale
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalizedTitle(String);

impl LocalizedTitle {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_title(&value)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalizedAltText(String);

impl LocalizedAltText {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.chars().count() > 500 || value.chars().any(char::is_control) {
            return Err(validation(
                "alt_text",
                "must contain at most 500 non-control characters",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_title(value: &str) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.chars().count() > 255 {
        Err(validation("title", "must contain 1-255 characters"))
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

#[cfg(test)]
mod tests {
    use crate::Locale;

    use super::{LocalizedAltText, LocalizedContent, LocalizedTitle};

    #[test]
    fn localized_catalog_content_reuses_canonical_bounds() {
        let content = LocalizedContent::new(
            Locale::parse("zh-SG").unwrap(),
            "Localized Product",
            "Localized description",
        )
        .unwrap();
        assert_eq!(content.locale().as_str(), "zh-SG");
        assert!(LocalizedTitle::new(" ").is_err());
        assert!(LocalizedAltText::new("bad\ntext").is_err());
    }
}
