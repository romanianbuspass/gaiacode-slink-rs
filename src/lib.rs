//! gaiacode-slink-rs — asynchronous SeedLink client and miniSEED parsing.
//!
//! Architecture:
//! - [`protocol`]: SeedLink wire constants, commands, and frame layout.
//! - [`parser`]:   miniSEED fixed-header (FSDH) record parsing.
//! - [`client`]:   Tokio-based [`SeedLinkClient`] state machine; packets are
//!   delivered over an `mpsc` channel so consuming UI threads never block.

pub mod client;
pub mod parser;
pub mod protocol;

pub use client::{SeedLinkClient, SeedLinkConfig, SeedLinkPacket};
pub use parser::MiniSeedHeader;
pub use protocol::SeedLinkError;
