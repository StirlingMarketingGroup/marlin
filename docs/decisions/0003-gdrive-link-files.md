# 0003: A Google Drive mount shows Google editor files and shortcuts as link files

**Status:** accepted
**Date:** 2026-11-16
**Issue:** #527 (epic #368)

## Decision

A Google Drive mount shows every Drive item that has no bytes of its own as a **link file**: a small web page that sends the browser to the item.

- This covers Google Docs, Sheets, Slides, Forms, and every other Google editor type, and it covers shortcuts to files and shortcuts to folders.
- The name is the item's name, then the type, then `.html`: `Notes.document.html`, `Budget.spreadsheet.html`, `Old report.shortcut.html`. The type is the word after `application/vnd.google-apps.` in the item's MIME type.
- The page holds one address, `https://drive.google.com/open?id={id}`. An editor file opens itself. A shortcut opens its target.
- The bytes of the page depend on that one ID and on nothing else.
- The provider never asks Google about a shortcut's target.

[Google Drive mount provider](../mount-gdrive.md) has the exact page, the item IDs, and the tests.

## Why

**These items have no bytes.** Google refuses a plain download of an editor file. The size it reports for one is not the length of anything a mount can read. A shortcut is a pointer to another item, which can be anywhere in the account or outside it. Until this record, the provider left all of them out, so a folder of Google Docs looked empty.

The mount has five rules that a presentation must keep:

| Rule                                                  | How a link file keeps it                                                                                    |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| A file's size is known when its folder is listed.     | The page is built from the ID, and the listing has the ID. The size is the length of the page.              |
| No folder is reachable through two parents.           | A shortcut to a folder is a file. It lists nothing.                                                         |
| The mount is read-only.                               | A read of a link file sends Google no request at all.                                                       |
| An entry is the same on every listing.                | The name comes from the item's name and type. The size comes from the ID. Neither depends on time or order. |
| A broken shortcut does not fail its folder's listing. | The provider never asks Google about a shortcut's target.                                                   |

**A broken shortcut costs nothing.** The listing gives each shortcut's target ID. The provider does not check that the target still exists or that the account may read it. So a broken shortcut lists like any other, and its page leads to Google's own error page.

**A web page opens on every OS.** Windows, macOS, and Linux all open an `.html` file in the browser with no setup. The page also holds a plain link, for a browser that does not follow the redirect.

**What was checked.** One request to `https://drive.google.com/open?id=` with the ID of a public Google Sheet, sent with no sign-in on 2026-11-16, was redirected to that sheet's editor address.

**Untested:**

- Opening a link file from a real mount in a real browser, on any OS.
- That the address opens a Doc, a Slides file, a Form, a folder, and a file with bytes. Only a Sheet was tried.
- A shortcut from a real account. The tests use a fake Drive server.
- A browser signed in to more than one Google account. The page opens in the browser's default account, which may not be the mount's account (#546).

## Rejected

- **Export each editor file** (a Doc as `.docx`, a Sheet as `.xlsx`). Google makes an export when asked and does not report the export's size before that. Listing a folder would need one export per file to learn the sizes, and a wrong size breaks `cp` and most viewers. An export of the same file can also differ in bytes from one request to the next. A user would edit the export and expect the Doc to change.
- **Pointer files as Google Drive for desktop makes them** (`.gdoc`, `.gsheet`, `.gslides`). They hold JSON that only Google's own app can open. Without that app, a double click does nothing.
- **The link format of each OS** (`.webloc`, `.url`, `.desktop`). That is three formats and one mount that looks different on each OS. Linux file managers refuse to open a `.desktop` link until the user marks it as trusted.
- **Show a shortcut to a file as the target file.** The listing does not give the target's size, so each shortcut would cost one more request per listing. A target that cannot be read would need a second presentation, and then the entry would change with the state of the target.
- **Show a shortcut to a folder as that folder.** The folder would be reachable through two parents. The core ties a node to one item ID, and FUSE does not allow a folder in two places.
- **A symbolic link for a shortcut.** The core has files and folders only. A link needs the target's path in the mount, and the provider would have to walk the target's parents to build it. A target in "Shared with me" has no path in the mount at all.
- **Leave them out, or show entries that fail to open.** A folder of Google Docs looks empty, or looks broken.
- **Take the address from Google's `webViewLink` field.** Google can change that text, and then the size of the entry would change between listings. An address built from the ID also lets a read answer with no request.

## What follows

- **A copy out of a mount copies the link, not the document.** That is what Google Drive for desktop does too. A real export, such as "Download as PDF", is an action for Marlin's own window (#377), not for the mount.
- **Writes (#376) must decide what a link file allows.** Deleting one would put the Google file in the trash. Renaming one would rename the Google file, and the new name must end with the same type and `.html`. Writing bytes to one has no meaning.
- **Marlin's window (#377) can tell a link file by its name** and give it the icon of its type.
- **A new Google editor type needs no code.** Its type word comes from its MIME type.
