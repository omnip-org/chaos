//! Use cases and ports. Infrastructure adapters implement ports defined here.

mod error;
pub mod merchant;
pub mod ports;

pub use error::ApplicationError;
