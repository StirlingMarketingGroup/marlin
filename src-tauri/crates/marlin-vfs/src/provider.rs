use crate::VfsError;

/// The provider's own id for one item.
///
/// A provider should use an id that stays with the item when it is renamed or moved (Google Drive's file id does). The core then keeps the item's node and open handles across the rename. A provider that only has paths (SFTP) uses the path, and for it a rename looks like one item removed and another added: the old node and any handle on it start failing with whatever the provider returns for the old path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub String);

/// Whether an item is a file or a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A file, with a byte length.
    File,

    /// A folder, which can be listed.
    Folder,
}

/// Kind, size, and modified time for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributes {
    pub kind: Kind,

    /// Byte length. A folder's length is the provider's own choice.
    pub size: u64,

    /// When the remote last changed the item.
    pub modified: std::time::SystemTime,
}

/// One item as the provider lists it, before the core maps the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,

    /// The name the remote uses. It may collide or hold characters the OS rejects.
    pub name: String,

    pub attributes: Attributes,
}

/// How much room the remote reports, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub total: u64,

    pub free: u64,
}

/// A storage account the core can read. There are no write methods yet.
#[async_trait::async_trait]
pub trait Provider: Send + Sync + 'static {
    /// The folder a mount shows at its top.
    fn root(&self) -> ItemId;

    /// The item for this id.
    async fn item(&self, id: &ItemId) -> Result<Item, VfsError>;

    /// Every item in a folder, with the remote names. A file returns [`VfsError::NotAFolder`].
    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError>;

    /// Up to `length` bytes from `offset`. Shorter only at the end of the file, and empty at or past the end.
    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError>;

    /// The account's total and free space, in bytes.
    async fn space(&self) -> Result<Space, VfsError>;
}
