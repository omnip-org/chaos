//! Shared HTTP extractors, errors, pagination, responses, and test support.

pub(crate) mod analytics;
pub(crate) mod error;
pub(crate) mod extract;
pub(crate) mod pagination;
pub(crate) mod response;

#[cfg(test)]
pub(crate) mod test_support;
