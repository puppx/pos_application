# Restaurant POS

An offline restaurant point-of-sale app for Windows. It installs like a normal
Windows program, keeps all data in a local SQLite database, and is built to
run well on low-end desktops and laptops (4 GB RAM).

- **Order types**: dine-in (on a table), takeaway, collection and delivery
- **Floor plan editor** with **multiple floors**: drag tables into place, square or round, resize, rename. A table is just a name, with no seat numbers or covers to enter.
- **Live table view**: free and occupied tables, running bill, time seated, and a red ring when items haven't been sent to the kitchen
- **Orders stay open** while you add items over several rounds. *Send to kitchen* marks what has gone; *Move table* moves an order to any free table on any floor.
- **Menu** with coloured categories, a tap-to-add item grid, notes per item ("no onions"), line discounts and optional stock counting (for example bottled drinks)
- **Item options with prices**: sizes, extras and choices such as "Burger size" (pick exactly one), "Extras" (pick up to 5) or "Cooking". Option groups are reusable across items, and each option's price (+ or −) is added to the item.
- **Kitchen ticket printing** on *Send to kitchen*, to any ESC/POS thermal printer (USB through Windows, network/IP, or a serial port). Tickets show only the new round, marked *NEW ORDER* or *ADDITIONAL*, with options and notes. A **Reprint** button covers paper jams.
- **Payment**: cash (with change) or card. Unsent items go to the kitchen automatically when paid.
- **PIN login** with **admin** and **staff** roles
- **History, voids/refunds and reports**: daily totals, tax, by order type, by payment method, top items
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
| Delivery | **Orders → + Delivery** | name, phone and address required |

1. Tap items to add them. Items with options open a picker first; required choices (such as size) must be made. Tapping the same item with the same choices again increases the quantity.
2. **Send to kitchen** sends only the new items and prints their ticket. You can do this any number of times per order. If the printer is offline, the items still count as sent and the app tells you to use **Reprint kitchen**.
3. Take payment by cash or card. A table becomes free as soon as its order is paid.

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
  src/orders.rs      Order life cycle: open, items, kitchen, pay, cancel, void
  src/floors.rs      Floors, tables, layout editing, live floor status
  src/menu.rs        Categories and the menu grid
  src/options.rs     Option groups (sizes, extras) and validation of choices
  src/printing.rs    Kitchen tickets: layout, ESC/POS, network/Windows/serial printing
  src/schema.sql     Database schema (migrations via PRAGMA user_version)
  tests/             Integration tests and the performance benchmark
  examples/          demo_data: a sample restaurant for trying the app
src-tauri/           Desktop shell: window, IPC commands, backups, installer config
ui/                  Placeholder UI (index.html, app.js, style.css)
```

## Next steps (not built yet)

- Final UI design
- Customer receipts / bills on the same printers, plus cash-drawer kick
- Several kitchen printers with routing by category (e.g. drinks to the bar printer)
- Courses and "hold/fire"
- Split bill and partial payments, service charge and tips
- Delivery status tracking (preparing, out for delivery, delivered) and driver assignment
- Shifts / cash-up (opening float, end-of-day report per shift)
- Code signing for the installer (avoids the Windows SmartScreen warning)
