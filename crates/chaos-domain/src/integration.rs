/// A capability is the contract an external provider fulfils. Provider
/// accounts are stored once in `integration.provider_accounts`; the
/// capability keeps payment, shipping, and email semantics separate in the
/// application layer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IntegrationCapability {
    Email,
    Payment,
    Shipping,
}

impl IntegrationCapability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Payment => "payment",
            Self::Shipping => "shipping",
        }
    }
}

/// Supported external payment providers. Adding a provider requires an
/// explicit payment adapter; the database stores the provider name as text so
/// adding an adapter does not require changing a global enum.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PaymentProvider {
    Stripe,
}

/// Supported external email providers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EmailProvider {
    Resend,
}

impl EmailProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resend => "resend",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "resend" => Some(Self::Resend),
            _ => None,
        }
    }
}

impl PaymentProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stripe" => Some(Self::Stripe),
            _ => None,
        }
    }
}

/// Supported external shipping providers. Manual fulfillment is deliberately
/// modeled as a provider so the shipping flow is selected explicitly rather
/// than inferred from a nullable account or free-form text.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ShippingProvider {
    Manual,
}

impl ShippingProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EmailProvider, IntegrationCapability, PaymentProvider, ShippingProvider};

    #[test]
    fn providers_round_trip_their_database_values() {
        assert_eq!(
            PaymentProvider::parse("stripe"),
            Some(PaymentProvider::Stripe)
        );
        assert_eq!(
            ShippingProvider::parse("manual"),
            Some(ShippingProvider::Manual)
        );
        assert_eq!(PaymentProvider::Stripe.as_str(), "stripe");
        assert_eq!(ShippingProvider::Manual.as_str(), "manual");
        assert_eq!(EmailProvider::parse("resend"), Some(EmailProvider::Resend));
        assert_eq!(IntegrationCapability::Payment.as_str(), "payment");
    }
}
