//! Data API feature
//!
//! Plugin/UI facing API that bridges data requests to resolvers.

mod service;
mod types;

pub use service::DataService;
pub use types::{DataRequest, DataResult, DataSource, DataStatus, SourceChain};
