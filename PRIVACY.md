# Marlin privacy policy

Last updated: November 13, 2026

Marlin is a file browser that runs on your computer. The Marlin maintainers run no server for it. **Marlin sends us nothing: no account, no analytics, no crash reports, and no file names or file contents.**

## What Marlin keeps on your computer

- Your settings and per-folder preferences.
- A cache of thumbnails for files you have looked at.
- The list of Google accounts you connect: the email address, the display name, and the address of the profile photo.
- Google sign-in tokens, in your operating system's keychain.
- The details you enter for a server you connect to, such as its address and your user name.

Removing Marlin's data folder and its keychain entries removes all of it.

## What Marlin sends over the network

Marlin talks only to the services you point it at, and to GitHub for updates.

| Service                       | When                               | What is sent                                                       |
| ----------------------------- | ---------------------------------- | ------------------------------------------------------------------ |
| Google                        | After you connect a Google account | Requests to sign in, and to list, read, or change your Drive files |
| An SMB or SFTP server you add | When you browse it                 | Your sign-in to that server, and requests for its files            |
| GitHub (`api.github.com`)     | When Marlin checks for an update   | A request for the latest release. It holds nothing about you       |

Each of these services sees your IP address, as any website does. Their own privacy policies cover what they do with it.

## Google user data

If you connect a Google account, Marlin asks Google for two permissions: to see your email address, and to see, edit, create, and delete your Google Drive files.

- **What Marlin reads:** your email address, display name, and profile photo, and the names, details, and contents of the Drive files and folders you open.
- **What Marlin does with it:** it shows you your files, and it makes the changes you ask for, such as a rename, a copy, an upload, or a delete. It does nothing else with it.
- **Where it goes:** it stays on your computer. Requests go straight from your computer to Google. Marlin does not send your Google data to the maintainers or to anyone else.
- **What Marlin never does:** it does not sell your Google data, use it for ads, or use it to train AI models. No person reads it.

Marlin's use of information received from Google APIs follows the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements.

### Removing Marlin's access

- In Marlin, remove the account from the sidebar. This deletes its sign-in token from your keychain.
- At Google, open [Third-party apps with account access](https://myaccount.google.com/permissions) and remove Marlin.

## Children

Marlin is not directed at children, and it collects no data from anyone.

## Changes

We change this page when Marlin changes what it stores or sends. The date at the top says when. The [history of this file](https://github.com/StirlingMarketingGroup/marlin/commits/master/PRIVACY.md) shows every change.

## Contact

Open an issue at <https://github.com/StirlingMarketingGroup/marlin/issues>.
