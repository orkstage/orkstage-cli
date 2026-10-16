# Code signing for releases

Release binaries of `ork` are signed so that Windows (SmartScreen, Smart App Control, WDAC
policies like the ones on many company laptops) and macOS (Gatekeeper) run them without warnings or
blocks. The `build` job in `.github/workflows/ci.yml` signs on version tags when the secrets below
exist; without them it publishes unsigned binaries.

The certificates prove your identity, so nobody can create them for you: Microsoft or a certificate
authority, and Apple, check who you are, and you pay them directly. This page lists every step and
the exact secret each value goes into. Prices and eligibility rules change; check the provider's
current terms before you pay.

Secrets go to the `orkstage-cli` repository: Settings > Secrets and variables > Actions > New
repository secret, or `gh secret set NAME -R orkstage/orkstage-cli` (it reads the value from
standard input, so it never lands in your shell history).

## Windows: Authenticode

Since 2023 the private key of a public code signing certificate must live in a hardware security
module (HSM). You cannot download a `.pfx` file any more: signing goes through the provider's cloud
service. Two ways work from GitHub Actions.

### Option A: Azure Trusted Signing (what the workflow uses today)

Microsoft's managed signing service (it may appear in the Azure portal under its newer name,
Artifact Signing). A monthly fee, no certificate to renew by hand.

1. **Check eligibility first.** Public Trust identity validation is open to organizations with a
   verifiable history and, for individual developers, only in some countries. If you sign as an
   individual outside them, use option B.
2. In the Azure portal, create a resource group and a **Trusted Signing account** in a region close
   to you. Note the account name and the region's endpoint, for example
   `https://eus.codesigning.azure.net/` for East US.
3. In the account, open **Identity validation** and request a **Public** validation. Microsoft asks
   for documents and can take several business days.
4. When it is approved, create a **Certificate profile** of type **Public Trust** linked to that
   validation. Note its name.
5. In Microsoft Entra ID, **App registrations > New registration** (for example
   `orkstage-cli-signing`). Note the **Directory (tenant) ID** and the **Application (client) ID**,
   then create a **client secret** under Certificates and secrets.
6. On the Trusted Signing account, **Access control (IAM) > Add role assignment**: role **Trusted
   Signing Certificate Profile Signer**, member the app registration.

| Secret | Value |
|---|---|
| `AZURE_TENANT_ID` | Directory (tenant) ID |
| `AZURE_CLIENT_ID` | Application (client) ID |
| `AZURE_CLIENT_SECRET` | The client secret's value |
| `AZURE_TRUSTED_SIGNING_ENDPOINT` | The region endpoint, for example `https://eus.codesigning.azure.net/` |
| `AZURE_TRUSTED_SIGNING_ACCOUNT` | The Trusted Signing account name |
| `AZURE_CERTIFICATE_PROFILE` | The certificate profile name |

The client secret expires (you choose when, at most two years): put a reminder in your calendar.

### Option B: a code signing certificate with cloud signing

A yearly OV (organization or individual validation) or EV certificate from a certificate authority
that offers signing through an API, for example SSL.com (eSigner), DigiCert (KeyLocker) or Certum
(SimplySign, with a lower priced certificate for open source developers). The authority validates
you, then you sign through its service. Choose one with a GitHub Actions integration that does not
need a phone confirmation for every signature, so releases can run unattended.

This needs a different signing step in `ci.yml` (the authority's action or command instead of
`azure/trusted-signing-action`) and its own secrets. Open an issue or ask in a session with the
provider you chose, and the workflow is adapted to it.

## macOS: Developer ID and notarization

1. **Join the Apple Developer Program** (yearly fee) at developer.apple.com/programs, as an
   individual (the certificate then shows your name) or as an organization (needs a D-U-N-S number).
   Your Apple ID needs two factor authentication.
2. **Create the certificate signing request.** On a Mac, Keychain Access > Certificate Assistant >
   Request a Certificate From a Certificate Authority, saved to disk. Without a Mac, with OpenSSL:

   ```bash
   openssl genrsa -out devid.key 2048
   openssl req -new -key devid.key -out devid.csr -subj "/emailAddress=you@example.com/CN=Your Name/C=BR"
   ```

   Keep `devid.key` private: it is the signing key.
3. In the developer portal, **Certificates, Identifiers & Profiles > Certificates > +**, choose
   **Developer ID Application**, upload the request and download the certificate
   (`developerID_application.cer`). Also download the **Developer ID - G2** intermediate certificate
   from Apple's certificate authority page.
4. **Make the `.p12` file** the workflow imports. On a Mac, import the `.cer` into Keychain Access
   and export the certificate with its key as `.p12`. With OpenSSL (the legacy options keep it
   readable by the macOS keychain):

   ```bash
   openssl x509 -inform DER -in developerID_application.cer -out devid.pem
   openssl x509 -inform DER -in DeveloperIDG2CA.cer -out g2.pem
   openssl pkcs12 -export -inkey devid.key -in devid.pem -certfile g2.pem -out devid.p12 \
     -name "Developer ID Application" -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1
   base64 -w0 devid.p12 > devid.p12.b64     # macOS: base64 -i devid.p12 -o devid.p12.b64
   ```

   OpenSSL asks for an export password: that is `MACOS_CERTIFICATE_PASSWORD`.
5. **App specific password for notarization:** appleid.apple.com > Sign-In and Security > App
   Specific Passwords > generate one named `orkstage-cli-notary`.
6. Find your **Team ID** under Membership details in the developer portal, and the identity name,
   which is `Developer ID Application: Your Name (TEAMID)`.

| Secret | Value |
|---|---|
| `MACOS_CERTIFICATE` | The contents of `devid.p12.b64` |
| `MACOS_CERTIFICATE_PASSWORD` | The `.p12` export password |
| `MACOS_SIGNING_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | The Apple ID email of the developer account |
| `APPLE_TEAM_ID` | The Team ID |
| `APPLE_APP_PASSWORD` | The app specific password |

Then delete `devid.key`, `devid.p12` and `devid.p12.b64` from your disk, after keeping a copy of the
key and the `.p12` in a password manager. The certificate is valid for five years.

## Check it before the first stable release

1. With the secrets set, push a prerelease tag, for example `v0.1.0-rc.6`. Prerelease tags build,
   sign and publish a GitHub prerelease, but never publish to crates.io (`publish.yml` only runs for
   `vX.Y.Z`).
2. In the run, the "Sign (Windows, Authenticode)" and "Sign and notarize (macOS)" steps must run
   (not skipped) and pass; notarization prints `status: Accepted`.
3. Check a downloaded binary. Windows: right click `ork.exe` > Properties > Digital Signatures, or
   `Get-AuthenticodeSignature .\ork.exe` (status `Valid`). macOS:
   `codesign --verify --strict --verbose=2 ./ork` and
   `codesign -dv ./ork 2>&1 | grep Authority` (shows your Developer ID).
4. Then tag `v0.1.0`: the release is signed and `publish.yml` publishes the crates.

Note: `install.sh` downloads with `curl`, which does not mark files as downloaded from the internet,
so macOS runs even an unsigned binary installed that way. Signing matters most for binaries
downloaded in a browser and on Windows machines with application control.
