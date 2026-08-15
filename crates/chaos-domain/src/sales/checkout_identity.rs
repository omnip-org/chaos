use std::str::FromStr;

use email_address::EmailAddress;

use crate::{DomainError, FieldViolation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckoutContact {
    email: String,
    phone: Option<String>,
}

impl CheckoutContact {
    pub fn new(email: impl Into<String>, phone: Option<String>) -> Result<Self, DomainError> {
        let email = email.into().trim().to_lowercase();
        if email.len() > 320 || EmailAddress::from_str(&email).is_err() {
            return Err(validation(
                "contact.email",
                "must be a valid email address with at most 320 characters",
            ));
        }
        let phone = optional_text("contact.phone", phone, 32)?;
        if phone.as_ref().is_some_and(|value| {
            !value.starts_with('+')
                || !(9..=16).contains(&value.len())
                || value.as_bytes()[1] == b'0'
                || !value[1..]
                    .chars()
                    .all(|character| character.is_ascii_digit())
        }) {
            return Err(validation(
                "contact.phone",
                "must be an E.164 number such as +14155552671",
            ));
        }
        Ok(Self { email, phone })
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn phone(&self) -> Option<&str> {
        self.phone.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostalAddress {
    full_name: String,
    company: Option<String>,
    address_line1: String,
    address_line2: Option<String>,
    locality: String,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: String,
}

impl PostalAddress {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        full_name: impl Into<String>,
        company: Option<String>,
        address_line1: impl Into<String>,
        address_line2: Option<String>,
        locality: impl Into<String>,
        administrative_area: Option<String>,
        postal_code: Option<String>,
        country_code: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let full_name = required_text("address.full_name", full_name.into(), 200)?;
        let company = optional_text("address.company", company, 200)?;
        let address_line1 = required_text("address.address_line1", address_line1.into(), 255)?;
        let address_line2 = optional_text("address.address_line2", address_line2, 255)?;
        let locality = required_text("address.locality", locality.into(), 100)?;
        let administrative_area =
            optional_text("address.administrative_area", administrative_area, 100)?;
        let postal_code = optional_text("address.postal_code", postal_code, 32)?;
        let country_code = country_code.into().trim().to_uppercase();
        if country_code.len() != 2
            || !country_code
                .chars()
                .all(|character| character.is_ascii_uppercase())
        {
            return Err(validation(
                "address.country_code",
                "must be an ISO 3166-1 alpha-2 code",
            ));
        }
        Ok(Self {
            full_name,
            company,
            address_line1,
            address_line2,
            locality,
            administrative_area,
            postal_code,
            country_code,
        })
    }

    pub fn full_name(&self) -> &str {
        &self.full_name
    }

    pub fn company(&self) -> Option<&str> {
        self.company.as_deref()
    }

    pub fn address_line1(&self) -> &str {
        &self.address_line1
    }

    pub fn address_line2(&self) -> Option<&str> {
        self.address_line2.as_deref()
    }

    pub fn locality(&self) -> &str {
        &self.locality
    }

    pub fn administrative_area(&self) -> Option<&str> {
        self.administrative_area.as_deref()
    }

    pub fn postal_code(&self) -> Option<&str> {
        self.postal_code.as_deref()
    }

    pub fn country_code(&self) -> &str {
        &self.country_code
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckoutIdentity {
    contact: CheckoutContact,
    billing_address: PostalAddress,
    shipping_address: Option<PostalAddress>,
}

impl CheckoutIdentity {
    pub fn new(
        contact: CheckoutContact,
        billing_address: PostalAddress,
        shipping_address: Option<PostalAddress>,
    ) -> Self {
        Self {
            contact,
            billing_address,
            shipping_address,
        }
    }

    pub fn contact(&self) -> &CheckoutContact {
        &self.contact
    }

    pub fn billing_address(&self) -> &PostalAddress {
        &self.billing_address
    }

    pub fn shipping_address(&self) -> Option<&PostalAddress> {
        self.shipping_address.as_ref()
    }
}

fn required_text(field: &'static str, value: String, max: usize) -> Result<String, DomainError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        Err(validation(field, "must contain bounded printable text"))
    } else {
        Ok(value)
    }
}

fn optional_text(
    field: &'static str,
    value: Option<String>,
    max: usize,
) -> Result<Option<String>, DomainError> {
    value
        .map(|value| required_text(field, value, max))
        .transpose()
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_and_address_are_canonical_validated_snapshots() {
        let contact =
            CheckoutContact::new(" Guest@Example.COM ", Some("+14155552671".into())).unwrap();
        let address = PostalAddress::new(
            " Guest Buyer ",
            None,
            " 1 Market Street ",
            None,
            "San Francisco",
            Some("CA".into()),
            Some("94105".into()),
            "us",
        )
        .unwrap();

        assert_eq!(contact.email(), "guest@example.com");
        assert_eq!(address.full_name(), "Guest Buyer");
        assert_eq!(address.country_code(), "US");
    }

    #[test]
    fn contact_and_address_reject_invalid_customer_data() {
        assert!(CheckoutContact::new("invalid", None).is_err());
        assert!(CheckoutContact::new("guest@example.com", Some("4155552671".into())).is_err());
        assert!(PostalAddress::new("", None, "1 Main", None, "City", None, None, "USA").is_err());
    }
}
