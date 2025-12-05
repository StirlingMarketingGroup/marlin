# Apple Developer Code Signing Guide for Marlin

This guide walks you through setting up Apple code signing and notarization for Marlin releases.

## Why Sign Your App?

- **No "unidentified developer" warnings** - Users can open the app without going to Security preferences
- **Gatekeeper approval** - macOS trusts your app
- **Persistent permissions** - File access permissions won't reset between updates
- **Professional distribution** - Required for serious macOS apps

## Cost

Apple Developer Program membership costs **$99/year** (as of 2024).

---

## Step 1: Enroll in Apple Developer Program

1. Go to [developer.apple.com/programs/enroll](https://developer.apple.com/programs/enroll/)
2. Sign in with your Apple ID (or create one)
3. Choose enrollment type:
   - **Individual** - For personal projects (uses your real name)
   - **Organization** - For companies (requires D-U-N-S number)
4. Pay the $99 annual fee
5. Wait for approval (usually instant for individuals, 24-48 hours for organizations)

---

## Step 2: Create a Developer ID Certificate

Once enrolled, create a certificate for signing apps distributed outside the App Store:

### Option A: Using Xcode (Recommended)

1. Open **Xcode** → **Settings** (⌘,) → **Accounts**
2. Select your Apple ID → Click **Manage Certificates**
3. Click **+** → **Developer ID Application**
4. Xcode creates and installs the certificate automatically

### Option B: Using Apple Developer Portal

1. Go to [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates/list)
2. Click **+** to create a new certificate
3. Select **Developer ID Application**
4. Follow the CSR (Certificate Signing Request) instructions:
   - Open **Keychain Access** → **Certificate Assistant** → **Request a Certificate from a Certificate Authority**
   - Enter your email, leave CA Email blank, select "Saved to disk"
5. Upload the CSR and download the certificate
6. Double-click the `.cer` file to install it in Keychain

---

## Step 3: Export Certificate as .p12

For GitHub Actions, you need to export your certificate:

1. Open **Keychain Access**
2. Go to **My Certificates** (in the sidebar under Category)
3. Find **Developer ID Application: Your Name (TEAM_ID)**
4. Right-click → **Export**
5. Save as `.p12` format
6. Set a strong password (you'll need this later)

---

## Step 4: Create App-Specific Password

For notarization, Apple requires an app-specific password:

1. Go to [appleid.apple.com](https://appleid.apple.com/)
2. Sign in → **App-Specific Passwords**
3. Click **+** to generate a new password
4. Name it something like "Marlin Notarization"
5. Copy the generated password (format: `xxxx-xxxx-xxxx-xxxx`)

---

## Step 5: Find Your Team ID

1. Go to [developer.apple.com/account](https://developer.apple.com/account)
2. Scroll down to **Membership Details**
3. Copy your **Team ID** (10-character alphanumeric)

---

## Step 6: Configure GitHub Secrets

Go to your repository → **Settings** → **Secrets and variables** → **Actions** → **New repository secret**

Add these secrets:

| Secret Name                  | Value                                                                                      |
| ---------------------------- | ------------------------------------------------------------------------------------------ |
| `APPLE_CERTIFICATE`          | Base64-encoded .p12 file (see below)                                                       |
| `APPLE_CERTIFICATE_PASSWORD` | Password you set when exporting .p12                                                       |
| `APPLE_SIGNING_IDENTITY`     | Your certificate's Common Name, e.g. `Developer ID Application: Example Corp (A1B2C3D4E5)` |
| `APPLE_ID`                   | Your Apple ID email                                                                        |
| `APPLE_PASSWORD`             | App-specific password from Step 4                                                          |
| `APPLE_TEAM_ID`              | Your 10-character Team ID                                                                  |

### Encoding the Certificate

Run this command to base64-encode your .p12 file:

```bash
base64 -i path/to/certificate.p12 | pbcopy
```

This copies the encoded certificate to your clipboard. Paste it as the `APPLE_CERTIFICATE` secret.

---

## Step 7: Verify Your Setup

After adding all secrets, trigger a release build. Check the logs for:

- ✅ "Import Apple Certificate" step succeeds
- ✅ "codesign" commands execute without errors
- ✅ "notarytool" successfully submits for notarization
- ✅ Notarization completes with "Accepted"

---

## Troubleshooting

### "The specified item could not be found in the keychain"

- Ensure the certificate was imported correctly
- Check that `APPLE_CERTIFICATE_PASSWORD` matches exactly

### "Developer ID Application certificate not found"

- Verify `APPLE_SIGNING_IDENTITY` matches the certificate name exactly
- Format: `Developer ID Application: Your Name (TEAM_ID)`

### Notarization fails with "Invalid credentials"

- Regenerate the app-specific password
- Ensure `APPLE_ID` and `APPLE_PASSWORD` are correct
- Check that your Apple Developer membership is active

### "Code signature invalid"

- Make sure entitlements file exists at `src-tauri/Entitlements.plist`
- Verify the entitlements are valid for your app's capabilities

---

## Local Development Signing

For local builds, you can sign with your certificate:

```bash
# Build with signing (uses certificate from Keychain)
npm run tauri build

# The bundler will automatically use your Developer ID certificate
# if it's installed in your Keychain
```

---

## Summary Checklist

- [ ] Enrolled in Apple Developer Program ($99/year)
- [ ] Created Developer ID Application certificate
- [ ] Exported certificate as .p12
- [ ] Created app-specific password for notarization
- [ ] Found Team ID
- [ ] Added all 6 secrets to GitHub repository
- [ ] Triggered a test release to verify

Once configured, every release will be automatically signed and notarized!

---

## References

- [Apple Developer Program](https://developer.apple.com/programs/)
- [Tauri macOS Code Signing](https://v2.tauri.app/distribute/sign/macos/)
- [Apple Notarization Guide](https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution)
