//! Use cases and ports. Infrastructure adapters implement ports defined here.

pub mod catalog;
mod error;
pub mod merchant;
pub mod ports;

pub use error::ApplicationError;
