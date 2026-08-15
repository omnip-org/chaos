use uuid::Uuid;

use crate::{DomainError, FieldViolation, identity::UserId};

use super::{CheckoutContact, PostalAddress};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CustomerId(Uuid);

impl CustomerId {
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

impl Default for CustomerId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Customer {
    id: CustomerId,
    user_id: UserId,
    contact: CheckoutContact,
}

impl Customer {
    pub fn create(user_id: UserId, email: impl Into<String>) -> Result<Self, DomainError> {
        Self::rehydrate(CustomerId::new(), user_id, email, None)
    }
    pub fn rehydrate(
        id: CustomerId,
        user_id: UserId,
        email: impl Into<String>,
        phone: Option<String>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id,
            user_id,
            contact: CheckoutContact::new(email, phone)?,
        })
    }
    pub const fn id(&self) -> CustomerId {
        self.id
    }
    pub const fn user_id(&self) -> UserId {
        self.user_id
    }
    pub const fn contact(&self) -> &CheckoutContact {
        &self.contact
    }
    pub fn update_phone(&mut self, phone: Option<String>) -> Result<(), DomainError> {
        self.contact = CheckoutContact::new(self.contact.email(), phone)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CustomerAddressId(Uuid);

impl CustomerAddressId {
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

impl Default for CustomerAddressId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomerAddress {
    id: CustomerAddressId,
    label: String,
    address: PostalAddress,
}

impl CustomerAddress {
    pub fn create(label: impl Into<String>, address: PostalAddress) -> Result<Self, DomainError> {
        Self::rehydrate(CustomerAddressId::new(), label, address)
    }
    pub fn rehydrate(
        id: CustomerAddressId,
        label: impl Into<String>,
        address: PostalAddress,
    ) -> Result<Self, DomainError> {
        let label = label.into();
        if label.trim().is_empty()
            || label.chars().count() > 64
            || label.chars().any(char::is_control)
        {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "label",
                reason: "must contain 1-64 printable characters".into(),
            }]));
        }
        Ok(Self { id, label, address })
    }
    pub const fn id(&self) -> CustomerAddressId {
        self.id
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub const fn address(&self) -> &PostalAddress {
        &self.address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn customer_canonicalizes_contact_and_validates_saved_address_labels() {
        let customer = Customer::create(UserId::new(), " Buyer@Example.COM ").unwrap();
        assert_eq!(customer.contact().email(), "buyer@example.com");
        let address =
            PostalAddress::new("Buyer", None, "1 Main", None, "Town", None, None, "US").unwrap();
        assert!(CustomerAddress::create("Home", address.clone()).is_ok());
        assert!(CustomerAddress::create("", address).is_err());
    }
}
