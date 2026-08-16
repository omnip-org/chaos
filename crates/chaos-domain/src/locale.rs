use language_tags::LanguageTag;

use crate::{DomainError, FieldViolation};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Locale(String);

impl Locale {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let value = value.as_ref();
        let parsed = LanguageTag::parse(value).map_err(|_| invalid_locale())?;
        parsed.validate().map_err(|_| invalid_locale())?;
        if !parsed.is_language_range() || parsed.primary_language() == "und" {
            return Err(invalid_locale());
        }
        let canonical = parsed.canonicalize().map_err(|_| invalid_locale())?;
        if canonical.as_str().len() > 63 {
            return Err(invalid_locale());
        }
        Ok(Self(canonical.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn language(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }
}

fn invalid_locale() -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field: "locale",
        reason: "must be a canonicalizable BCP 47 language tag without extensions or private use"
            .into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::Locale;

    #[test]
    fn canonicalizes_valid_bcp47_language_tags() {
        assert_eq!(Locale::parse("EN-us").unwrap().as_str(), "en-US");
        assert_eq!(Locale::parse("zh-hans-sg").unwrap().as_str(), "zh-Hans-SG");
        assert_eq!(Locale::parse("iw-IL").unwrap().as_str(), "he-IL");
    }

    #[test]
    fn rejects_undefined_extensions_and_private_use() {
        for value in ["und", "en-US-u-ca-gregory", "x-shop", "not_a_locale"] {
            assert!(Locale::parse(value).is_err());
        }
    }
}
