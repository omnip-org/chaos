use crate::{DomainError, FieldViolation};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CurrencyCode([u8; 3]);

impl CurrencyCode {
    pub const USD: Self = Self(*b"USD");

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let bytes = value.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(u8::is_ascii_uppercase) {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "currency",
                reason: "must be a three-letter uppercase ISO 4217 code".into(),
            }]));
        }

        Ok(Self([bytes[0], bytes[1], bytes[2]]))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("currency code is always ASCII")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_uppercase_three_letter_code() {
        assert_eq!(CurrencyCode::parse("USD").unwrap().as_str(), "USD");
    }

    #[test]
    fn rejects_non_canonical_code() {
        assert!(CurrencyCode::parse("usd").is_err());
        assert!(CurrencyCode::parse("USDT").is_err());
    }
}
