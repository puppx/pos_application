# Restaurant POS

An offline restaurant point-of-sale app for Windows. It installs like a normal
Windows program, keeps all data in a local SQLite database, and is built to
run well on low-end desktops and laptops (4 GB RAM).

- **Order types**: dine-in (on a table), takeaway, collection and delivery
- **Floor plan editor** with **multiple floors**: drag tables into place, square or round, resize, rename. A table is just a name, with no seat numbers or covers to enter.
- **Live table view**: free and occupied tables, running bill, time seated, and a red ring when items haven't been sent to the kitchen
- **Orders stay open** while you add items over several rounds. *Send to kitchen* marks what has gone; *Move table* moves an order to any free table on any floor.
- **Menu** with coloured categories, a tap-to-add item grid, notes per item ("no onions"), line discounts and optional stock counting (for example bottled drinks)
- **Item choices with their own prices**: one menu item sold in versions, e.g. *Tom Yum Soup*: Chicken 6.50 / Beef 7.50 / Prawn 8.50. Set them up in the item's own editor (no option groups needed); the menu shows "from 6.50", staff tap the soup then the meat, and the bill, kitchen ticket and reports show *Tom Yum Soup (Beef)*.
- **Item options with prices**: sizes, extras and choices such as "Burger size" (pick exactly one), "Extras" (pick up to 5) or "Cooking". Option groups are reusable across items, and each option's price (+ or −) is added to the item. A group can be paused ("In use") or deleted; deleting removes it from every item, and past orders keep what was chosen.
- **Starters first, then "Call mains"**: tick **Starter** on menu items (in the menu list or the item editor). On a dine-in order, *Send starters* prints only the starters, with *MAINS TO FOLLOW* at the bottom; everything else waits until you tap **Call mains**, which prints a *MAINS AWAY* ticket. Tick **All courses together** to send everything at once. Takeaway, collection and delivery orders always go in one ticket.
- **Delivery platforms**: one-tap buttons for Uber Eats, Just Eat and Deliveroo (add, rename, recolour or remove them in Settings). They record the platform's order number, have a *Paid by Uber Eats* button, a ticket heading such as `UBER EATS #7F3K2`, and their own line in reports.
- **Saved customers** for delivery and collection: find them by any part of their phone number, name or postcode and fill the order in one tap. Edit them on the **Customers** screen, which also shows each customer's recent orders.
- **Postcode suggestions, free and offline**: typing a postcode lists the addresses already saved there (door number and street), so streets you deliver to often come up instantly. No paid address service or internet connection is used.
- **Customer receipts and cash drawer**: a receipt prints automatically after payment (your name, address/VAT header and footer), **Print bill** prints the bill before paying, and receipts can be reprinted from History. The cash drawer (plugged into the receipt printer) opens on cash payments and with **Open drawer**, which is logged.
- **Copies, delay and category grouping**: Settings sets how many copies the kitchen and receipt printers make and how many seconds to wait between copies (default 2, so each can be torn off); the first copy prints straight away. Every ticket and receipt lists items under their menu category (STARTERS, MAINS, DRINKS…) in menu order.
- **Kitchen ticket printing** on *Send to kitchen*, to any ESC/POS thermal printer (USB through Windows, network/IP, or a serial port). Tickets show only the new round, marked *NEW ORDER* or *ADDITIONAL*, with options and notes. A **Reprint** button covers paper jams.
- **Finish & Pay**: one button opens the payment screen. Choose **Cash** or **Card** (or *Paid by Uber Eats* for platform orders). For cash, type or tap the amount received (with quick buttons for exact and common notes) and the change is shown as you type, then again in large figures after payment. Unsent items go to the kitchen automatically when paid.
- **End of day**: **Reports → Close day & print summary** prints the day's sales summary (orders, totals, tax, cash/card, order types, platforms, categories, voids) on the receipt printer and moves to the next business date, shown at the top of the screen. A day runs from one close to the next, so service after midnight still counts for the day it started. All orders must be paid or cancelled first. Previous days can be viewed and reprinted.
- **PIN login** with **admin** and **staff** roles
- **History, voids/refunds and reports**: daily totals, tax, by order type, by payment method, by category, top items
- **Backups**: automatic daily copy at startup (last 14 kept), plus "Back up now"
- **Audit log** of logins, voids, discounts, removed items, menu, floor and user changes

The current UI is a functional placeholder. All business rules live in Rust,
so the UI can be redesigned without touching them.

## How service works

| Order type | Starts from | Customer details |
| --- | --- | --- |
| Dine-in | Tap a table on the **Tables** screen | none |
| Takeaway | **Orders → + Takeaway** | optional |
| Collection | **Orders → + Collection** | name required, phone optional |
| Delivery | **Orders → + Delivery** | name, phone and address required (or pick a saved customer) |
| Uber Eats / Just Eat / Deliveroo… | **Orders → + Uber Eats** etc. | none (the platform has them); platform order number optional |

1. Tap items to add them. Items with options open a picker first; required choices (such as size) must be made. Tapping the same item with the same choices again increases the quantity.
2. **Send to kitchen** sends only the new items and prints their ticket. You can do this any number of times per order. If the printer is offline, the items still count as sent and the app tells you to use **Reprint kitchen**.
   On dine-in orders with starters, the button reads **Send starters**: the rest waits (shown as *waits for mains* on the order and *mains waiting* on the table) until you tap **Call mains**.
3. **Finish & Pay**, then Cash or Card. A table becomes free as soon as its order is paid.
4. At closing time an admin uses **Reports → Close day & print summary**.

Prices are fixed when an item is added, so menu price changes never alter an open bill.
Each table can have only one open order at a time.

## Kitchen printer

**Settings → Kitchen printer** (admin):

| Printer connection | Choose | Enter |
| --- | --- | --- |
| USB (with the maker's Windows driver installed) | *Printer installed in Windows* | pick it from the list |
| Network / Ethernet / Wi-Fi | *Network printer* | its IP address (printed on its self-test page), port 9100 |
| Serial or shared printer | *Serial port or shared printer path* | e.g. `COM3` or `\\PC-NAME\Kitchen` |

Pick 80 mm or 58 mm paper, press **Print test ticket**, then **Save printer**.
Tickets are sent as raw ESC/POS (supported by Epson TM, Star, Bixolon, Xprinter and most
other receipt printers). They use large text, list each item's options and highlight notes:

```
   NEW ORDER
DINE-IN 4
Ground
#27  19:42  Sam
------------------------------------------
2 x Classic burger
   + Double
   + Cheese
   ** no onions
1 x Cola
   + Large
------------------------------------------
```

Printing happens after the order is saved and never blocks the till. A printer
that is off or unreachable gives up after 4 seconds and shows an error.

### Receipt printer and cash drawer

**Settings → Receipt printer & cash drawer** works the same way (it can be the same
printer as the kitchen one). Also set the number of copies, whether a receipt prints
automatically after payment, whether the drawer opens for cash, the currency symbol,
and the header (address, phone, VAT number) and footer lines. Use **Print test receipt**
and **Test cash drawer** before saving. The drawer must be cabled to the receipt
printer's drawer (RJ11/DK) port.

## Try it with demo data

To get a restaurant with floors, tables, a menu and options already set up
(admin PIN 1234, staff PIN 2222), run this before the first start of the app:

```
cargo run -p pos-core --example demo_data -- "%APPDATA%\com.puppx.pos\pos.db"
```

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
- The menu is loaded once per order screen and filtered in the UI, so tapping and searching items needs no database round trip.
- The live floor view is a single query per refresh (about 0.25 ms with 30 busy tables).
- Money is stored as integer cents and tax rates as basis points, so there is no floating-point drift.
- The release build uses LTO, one codegen unit and stripped symbols.

To benchmark on your own machine (2,000-item menu, 60 tables on two floors,
20,000 past orders):

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
| Open orders, add items, send to kitchen, reprint, take payment | ✔ | ✔ |
| Move tables, edit customer details, discounts | ✔ | ✔ |
| Remove or reduce items already sent to the kitchen | ✔ | |
| Cancel an order that has items in the kitchen | ✔ | |
| Void / refund paid orders | ✔ | |
| Order history | all | own |
| Menu, options, stock, floor plan, users | ✔ | |
| Kitchen printer setup | ✔ | |
| Reports, settings, backup | ✔ | |

## Data location

`%APPDATA%\com.puppx.pos\`
- `pos.db`: the database
- `backups\pos-<timestamp>.db`: daily and manual backups

To restore a backup, close the app and copy the backup over `pos.db`.

## Install and update

1. Open **Releases** on this repository's GitHub page.
2. Download `POS_x.y.z_x64-setup.exe` from the newest release and run it.
   Because the installer is not code-signed yet, Windows may say "Windows protected
   your PC": click **More info → Run anyway**.

**Automatic updates (v0.5.0 and later):** the app checks GitHub Releases when it starts
and every 6 hours. When a new version is out, a bar appears at the top:
*Update x.y.z available*. An admin taps **Install update**: the database is backed up
(`backups\pos-before-update-*.db`), the app closes, updates and reopens. Orders, menu
and settings are kept, as they live in `%APPDATA%\com.puppx.pos`. **Settings → About**
shows the version and has **Check for updates**.

Updates are signed: the app only installs files signed with the project's private key,
so a tampered download is refused. Updates need the repository to be **public**
(the app downloads releases without logging in to GitHub).

Versions before 0.5.0 cannot update themselves: install 0.5.0 once by hand.

## Building

### Publishing a new version (maintainers)

1. Bump the version in **both** `Cargo.toml` (`[workspace.package]`) and `src-tauri/tauri.conf.json`.
2. If the database changes, **append** a migration (never edit `schema.sql`, it has shipped). See `MIGRATIONS` in `crates/pos-core/src/lib.rs`.
3. Commit and push to `main`.
4. On GitHub open **Actions → Windows build → Run workflow**, tick **Publish a release** and run it
   (or push a tag such as `v0.2.0`).

The **Windows build** workflow tests, builds the installers and publishes the
release, tagged `v` + the app version.

**One-time setup for automatic updates:** add the update-signing private key as a
repository secret named `TAURI_SIGNING_PRIVATE_KEY` (Settings → Secrets and variables →
Actions → New repository secret). The matching public key is in
`src-tauri/tauri.conf.json`. Releases built without the secret still have installers,
but no `latest.json`, so installed apps won't see them as updates. **Keep a backup of the
private key**: if it is lost, installed apps cannot accept new updates until they are
reinstalled with a new key. Every push to `main` also builds installers you can download from the
run's *Artifacts* section without making a release.

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
  src/orders.rs      Order life cycle: open, items, kitchen, pay, cancel, void
  src/floors.rs      Floors, tables, layout editing, live floor status
  src/menu.rs        Categories and the menu grid
  src/options.rs     Option groups (sizes, extras) and validation of choices
  src/channels.rs    Delivery platforms (Uber Eats, Just Eat, Deliveroo…)
  src/customers.rs   Saved customers, phone/postcode search
  src/variants.rs    Item choices with their own prices (soup: chicken / beef)
  src/migrations/    Database upgrades applied on start (never edit a shipped one)
  src/printing.rs    Kitchen tickets: layout, ESC/POS, network/Windows/serial printing
  src/receipt.rs     Customer receipts/bills and the cash drawer pulse
  src/dayclose.rs    End of day: business date, close, sales summary printout
  src/schema.sql     Database schema (migrations via PRAGMA user_version)
  tests/             Integration tests and the performance benchmark
  examples/          demo_data: a sample restaurant for trying the app
src-tauri/           Desktop shell: window, IPC commands, backups, installer config
ui/                  Placeholder UI (index.html, app.js, style.css)
```

## Next steps (not built yet)

- Final UI design
- Several kitchen printers with routing by category (e.g. drinks to the bar printer)
- Split bill and partial payments, service charge and tips
- Delivery status tracking (preparing, out for delivery, delivered) and driver assignment
- Shifts / cash-up (opening float, counted cash vs expected, report per shift)
- Code signing for the installer (avoids the Windows SmartScreen warning)
