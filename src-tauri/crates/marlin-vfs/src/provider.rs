use crate::VfsError;

/// The provider's own id for one item.
///
/// A provider should use an id that stays with the item when it is renamed or moved (Google Drive's file id does). The core then keeps the item's node and open handles across the rename. A provider that only has paths (SFTP) uses the path, and for it a rename that another client makes looks like one item removed and another added: the old node and any handle on it start failing with whatever the provider returns for the old path. For a rename this mount makes, see [`Provider::rename`].
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

    /// When the remote last changed the item. When a file's bytes change, this time or the size must change too, because an adapter may keep bytes it already read until one of the two changes.
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

/// A storage account the core can read. A provider that cannot change the remote leaves [`Provider::create_folder`], [`Provider::remove`], and [`Provider::rename`] out, and each then answers [`VfsError::ReadOnly`]. There is no call that writes a file's bytes yet.
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

    /// Makes an empty folder named `name` in `parent`, and returns it. The core has checked that its own listing of `parent` holds no such name. A remote that keeps one name per folder answers [`VfsError::Conflict`] when the name is taken.
    async fn create_folder(&self, parent: &ItemId, name: &str) -> Result<Item, VfsError> {
        let _ = (parent, name);
        Err(VfsError::ReadOnly)
    }

    /// Removes one item from the folder `parent`. An item that is no longer in `parent` is [`VfsError::NotFound`] when the provider can tell: the core's listing can be a few seconds old. A folder that holds anything is [`VfsError::NotEmpty`], and nothing in it is removed. A remote that has a trash moves the item there.
    async fn remove(&self, id: &ItemId, parent: &ItemId) -> Result<(), VfsError> {
        let _ = (id, parent);
        Err(VfsError::ReadOnly)
    }

    /// Moves `id` out of the folder `from` into the folder `to`, names it `name`, and returns the item as the remote now has it. For a plain rename `from` and `to` are the same folder. An item that is no longer in `from` is [`VfsError::NotFound`] when the provider can tell. A move of a folder into itself, or into a folder inside it, is [`VfsError::NotSupported`].
    ///
    /// The returned item keeps its id when the remote's ids allow it. When the id changed, the core moves the item's node and open handles to the new id.
    ///
    /// When `replace` names an item, it is the item the mount shows under the name, and it is gone when the call succeeds: a remote that has a trash moves it there. On a remote that allows two items with one name, its remote name can differ from `name`. The provider swaps the two in one step when the remote can. When it cannot, it moves `id` first and removes `replace` second, so a call that fails halfway leaves both items and never leaves the name with none. A `replace` item that is no longer in `to` is [`VfsError::NotFound`] when the provider can tell, and a folder to replace that holds anything is [`VfsError::NotEmpty`]: in both cases nothing changes. With no `replace`, the call never removes or overwrites another item: a remote that keeps one name per folder answers [`VfsError::Conflict`] when the name is taken.
    async fn rename(
        &self,
        id: &ItemId,
        from: &ItemId,
        to: &ItemId,
        name: &str,
        replace: Option<&ItemId>,
    ) -> Result<Item, VfsError> {
        let _ = (id, from, to, name, replace);
        Err(VfsError::ReadOnly)
    }
}
