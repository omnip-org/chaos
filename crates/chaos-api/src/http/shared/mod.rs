//! Shared HTTP extractors, errors, pagination, responses, and contract support.

pub(crate) mod error;
pub(crate) mod extract;
pub(crate) mod openapi;
pub(crate) mod pagination;
pub(crate) mod response;

#[cfg(test)]
pub(crate) mod test_support;
