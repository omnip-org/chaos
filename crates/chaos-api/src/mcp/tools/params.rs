use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct StoreIdParams {
    /// The Store UUID to inspect.
    pub store_id: String,
}
