//! The filesystem core shared by every mount adapter and every storage provider.
//!
//! Adapters ask for nodes and handles. Providers answer with their own item ids. The core keeps the node table, the name rules, the provider answers it reuses (listings, attributes, free space), the call limits, and the write spool.

mod error;
mod memory;
mod names;
mod provider;
mod spool;
mod vfs;

pub use error::VfsError;
pub use memory::MemoryProvider;
pub use names::{escape, finder_files, unescape, unique_names, NameRules, POSIX};
pub use provider::{Attributes, Item, ItemId, Kind, Provider, Space};
pub use spool::Spool;
pub use vfs::{Access, Entry, HandleId, IfExists, NodeId, Options, Vfs};
