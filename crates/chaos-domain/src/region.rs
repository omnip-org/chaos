use crate::{DomainError, FieldViolation};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegionCode([u8; 2]);

impl RegionCode {
    pub const US: Self = Self(*b"US");

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let bytes = value.as_bytes();
        if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_uppercase) {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "region",
                reason: "must be a two-letter uppercase ISO 3166-1 alpha-2 code".into(),
            }]));
        }
        Ok(Self([bytes[0], bytes[1]]))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("region code is always ASCII")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_uppercase_two_letter_code() {
        assert_eq!(RegionCode::parse("US").unwrap(), RegionCode::US);
    }

    #[test]
    fn rejects_a_non_canonical_code() {
        assert!(RegionCode::parse("us").is_err());
        assert!(RegionCode::parse("USA").is_err());
    }
}
