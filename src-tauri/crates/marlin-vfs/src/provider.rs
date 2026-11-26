use std::path::Path;
use std::time::SystemTime;

use crate::VfsError;

/// The provider's own id for one item.
///
/// A provider should use an id that stays with the item when it is renamed or moved (Google Drive's file id does), and the core then keeps the item's node and open handles across the rename. A provider that only has paths (SFTP, SMB) uses the path, and once it can write it answers `true` to [`Provider::ids_are_paths`]. For such a provider, a rename that another client makes looks like one item removed and another added.
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

/// A value the provider changes whenever the item's bytes change.
///
/// A provider that has the two byte calls gives every file one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Revision(pub String);

/// Kind, size, modified time, and revision for one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attributes {
    pub kind: Kind,

    /// Byte length. A folder's length is the provider's own choice.
    pub size: u64,

    /// When the remote last changed the item. When a file's bytes change, this time or the size must change too, because an adapter may keep bytes it already read until one of the two changes.
    pub modified: SystemTime,

    /// The revision of a file's bytes. `None` for a folder, and for a provider without the byte calls.
    pub revision: Option<Revision>,

    /// The provider knows the account may not give this file new bytes. `false` when the provider does not know, and always `false` for a folder. The core refuses what would put bytes in the spool for such a file. Name changes still go to the provider, whose answer decides.
    pub read_only: bool,
}

/// Hears how many bytes of an upload the provider has sent so far.
pub type Progress = dyn Fn(u64) + Send + Sync;

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

/// A picture of a file that its remote made, such as Google Drive's thumbnail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// The bytes of an image file, as the remote sends them. Google Drive sends PNG or JPEG.
    pub image: Vec<u8>,

    /// The width and height of the file itself in pixels, when the remote knows them, as Google Drive does for an image or a video. Never the picture's own size.
    pub dimensions: Option<(u32, u32)>,
}

/// A storage account the core can read. A provider that cannot change the remote leaves out the change calls and the two byte calls, leaves [`Self::can_write`] at its default, and each of those calls then answers [`VfsError::ReadOnly`].
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

    /// The items from the top of the mount down to the item that `reference` names, the top itself not included. `reference` is the remote's own public ID for an item, such as the file ID in a Google Drive link: the reverse of [`Self::reference`]. `None` when the remote has the item, but the mount shows it in no folder. A reference the remote does not know is [`VfsError::NotFound`], and one the account may not see is that or [`VfsError::AccessDenied`], as the remote answers. A provider that cannot find an item by a reference leaves this out, and it answers [`VfsError::NotSupported`].
    async fn locate(&self, reference: &str) -> Result<Option<Vec<ItemId>>, VfsError> {
        let _ = reference;
        Err(VfsError::NotSupported)
    }

    /// The remote's own public ID for the item `id`, which [`Self::locate`] takes back. `None` for an item that has none, such as a folder that only the mount shows. Makes no remote call. A provider without public IDs leaves this out, and no item has one.
    fn reference(&self, id: &ItemId) -> Option<String> {
        let _ = id;
        None
    }

    /// A picture of the file `id` that the remote made from the bytes it has, so a file browser can show the file without reading it. `size` is the longer side the caller asks for, in pixels; the remote may send another size. `None` when the remote has no picture of it. The core asks only about a file, never a folder. A provider whose remote makes no pictures leaves this out, and it answers `None`.
    async fn thumbnail(&self, id: &ItemId, size: u32) -> Result<Option<Thumbnail>, VfsError> {
        let _ = (id, size);
        Ok(None)
    }

    /// Whether this provider has the three change calls and the two byte calls. A mount of a provider that answers `false` is read-only (decision 0005, choice 13).
    fn can_write(&self) -> bool {
        false
    }

    /// Whether an item's id is its path: `/` for the folder at the top, and the folder's id joined to the item's name with `/` for an item in it (`/{name}` in the top folder). A rename then changes the id of the item and of everything inside it, and a path that this mount removed or replaced can name another item later. A provider whose ids stay with their items, as Google Drive's do, leaves this at `false`. Only this mount's own changes depend on it, so a provider that cannot write can leave it too.
    fn ids_are_paths(&self) -> bool {
        false
    }

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
    /// The returned item keeps its id when the remote's ids allow it. A provider whose ids are paths ([`Self::ids_are_paths`]) returns the new path, and the core moves every node at or under the old path to the new one. Any other provider that returns a new id, as Google Drive does for a file moved between My Drive and a shared drive, changes that one item's id, and the core moves its node.
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

    /// Makes a new file named `name` in `parent` with the bytes of the local file `from`, gives it the modified time `modified`, and returns it. `name` is a remote name. A remote that keeps one name per folder answers [`VfsError::Conflict`] when the name is taken; it never overwrites another item.
    ///
    /// Call `progress` with the bytes sent so far as they go out, at least every few seconds while bytes move, because the core stops an upload whose byte count has not grown for [`crate::Options::upload_stall`]. Each call reports its own bytes from zero. This call has no time limit of its own. The local file can change while the call reads it; the core then does not count the upload as sending the file's current bytes, and sends them again.
    async fn create_file(
        &self,
        parent: &ItemId,
        name: &str,
        from: &Path,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        let _ = (parent, name, from, modified, progress);
        Err(VfsError::ReadOnly)
    }

    /// Replaces the bytes of the file `id` with the bytes of the local file `from`, gives it the modified time `modified`, and returns the item as the remote now has it. The id stays the same. When the item's revision is not `base`, it answers [`VfsError::Conflict`] and changes nothing. A file that is gone is [`VfsError::NotFound`].
    ///
    /// Call `progress` with the bytes sent so far as they go out, at least every few seconds while bytes move, because the core stops an upload whose byte count has not grown for [`crate::Options::upload_stall`]. Each call reports its own bytes from zero. This call has no time limit of its own. The local file can change while the call reads it; the core then does not count the upload as sending the file's current bytes, and sends them again.
    async fn replace_bytes(
        &self,
        id: &ItemId,
        from: &Path,
        base: &Revision,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        let _ = (id, from, base, modified, progress);
        Err(VfsError::ReadOnly)
    }
}
