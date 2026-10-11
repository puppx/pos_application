# POS Application

An offline point-of-sale app for Windows. It installs like a normal Windows
program, keeps all data in a local SQLite database, and is built to run well
on low-end desktops and laptops (4 GB RAM).

- **PIN login** with **admin** and **staff** roles
- **Products** with barcode/SKU, price, cost, tax rate and stock tracking
- **Selling** by barcode scan or search, line discounts, cash (with change) and card payments
- **Sales history**, voids (admin) that put stock back, and an inventory ledger
- **Reports**: daily totals, tax, payment-method breakdown, top products
- **Backups**: automatic daily copy at startup (last 14 kept), plus "Back up now"
- **Audit log** of logins, failed logins, user/product changes, voids and backups

The current UI is a functional placeholder. All business rules live in Rust,
so the UI can be redesigned without touching them.

## Why this stack

| Choice | Reason |
| --- | --- |
| **Rust** core | No garbage collector or runtime, so memory use is low and predictable. |
| **Tauri 2** shell | Uses the Windows WebView2 runtime that ships with Windows 10/11 instead of bundling Chromium. That means roughly a 10 MB installer and a fraction of Electron's RAM. |
| **SQLite** (compiled in) | Zero setup, a single file, and very fast for one terminal. |
| Plain HTML/JS UI | No framework or build step, so the UI is cheap to load and easy to replace. |

### Performance measures in the code

- WAL mode, so reports and searches never block a checkout.
- `synchronous=FULL`, so a completed sale survives a power cut. This costs one fsync per sale.
- A 16 MB page cache plus memory-mapped I/O. Every hot query uses an index, and prepared statements are cached.
- Barcode scans are a single index lookup. Search uses the name index for 1–2 characters and an FTS5 trigram index for 3 or more, so it stays fast with 100k+ products.
- Money is stored as integer cents and tax rates as basis points, so there is no floating-point drift.
- The release build uses LTO, one codegen unit and stripped symbols.

To benchmark on your own machine (50k products, 2k sales):

```
cargo test --release -p pos-core --test perf -- --ignored --nocapture
```

## Users and security

- On first launch the app asks you to create the first **admin** (name and PIN).
- PINs are 4–8 digits and identify the user, so each PIN must be unique.
- PINs are stored as Argon2id hashes with a per-install salt, never in plain text.
- After 5 wrong PINs in a row the terminal locks for 30 s. The lock doubles with each further failure, up to 15 min.
- Permissions are checked in Rust on every command, and the user is re-checked against the database each time. A disabled or demoted user loses access immediately.
- There must always be at least one active admin.

| | Admin | Staff |
| --- | :---: | :---: |
| Sell | ✔ | ✔ |
| See own sales | ✔ | ✔ |
| See all sales, void sales | ✔ | |
| Products and stock | ✔ | |
| Users and PINs | ✔ | |
| Reports, settings, backup | ✔ | |

## Data location

`%APPDATA%\com.puppx.pos\`
- `pos.db`: the database
- `backups\pos-<timestamp>.db`: daily and manual backups

To restore a backup, close the app and copy the backup over `pos.db`.

## Building

### Windows installer (recommended: GitHub Actions)

Each push to `main`, each `v*` tag and each manual run of the **Windows build**
workflow builds both installers. Download them from the run's
`pos-windows-installers` artifact:

- `POS_0.1.0_x64-setup.exe` (NSIS)
- `POS_0.1.0_x64_en-US.msi`

The installer embeds the WebView2 bootstrapper, so it also works on machines
that don't have WebView2 yet.

### On a Windows PC

Requirements: [Rust](https://rustup.rs), [Node.js](https://nodejs.org) (only
for the Tauri CLI), and the "Desktop development with C++" workload of Visual
Studio Build Tools.

```
cargo test -p pos-core                 # business-logic tests
npx @tauri-apps/cli@2 dev              # run the app in development
npx @tauri-apps/cli@2 build            # build installers into target\release\bundle
```

## Project layout

```
crates/pos-core/     Business logic + SQLite (all rules, permissions, tests)
  src/schema.sql     Database schema (migrations via PRAGMA user_version)
  tests/             Integration tests and the performance benchmark
src-tauri/           Desktop shell: window, IPC commands, backups, installer config
ui/                  Placeholder UI (index.html, app.js, style.css)
```

## Next steps (not built yet)

- Final UI design
- Receipt printing (ESC/POS thermal printers) and cash-drawer kick
- Shifts / cash-up (opening float, Z-report per shift)
- Returns/refunds of individual items (whole-sale void exists)
- Product categories, weighed items, customer accounts
- Code signing for the installer (avoids the Windows SmartScreen warning)
