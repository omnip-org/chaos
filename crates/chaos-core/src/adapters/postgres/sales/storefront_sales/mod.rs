//! PostgreSQL persistence for the Storefront shopper, cart, and checkout flow.

mod cart;
mod operations;
mod repository;

pub use repository::PostgresStorefrontSalesRepository;
