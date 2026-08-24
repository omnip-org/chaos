/// Supported external payment providers. Adding a provider requires an
/// explicit adapter and a matching database enum value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PaymentProvider {
    Stripe,
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
    use super::{PaymentProvider, ShippingProvider};

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
    }
}
