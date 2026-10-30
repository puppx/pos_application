# Restaurant POS

An offline restaurant point-of-sale app for Windows. It installs like a normal
Windows program, keeps all data in a local SQLite database, and is built to
run well on low-end desktops and laptops (4 GB RAM).

- **Order types**: dine-in (on a table), takeaway, collection and delivery
- **Floor plan editor** with **multiple floors**: drag tables into place, square or round, resize, rename. A table is just a name, with no seat numbers or covers to enter.
- **Live table view**: free and occupied tables, running bill, time seated, and a red ring when items haven't been sent to the kitchen
- **Orders stay open** while you add items over several rounds. *Send to kitchen* marks what has gone; *Move table* moves an order to any free table on any floor.
- **Fast menu building** (Menu screen):
  - Items, categories and option groups are edited in a **pop-up** over the full-width list (Esc or × closes it)
  - **Save & add another** keeps the category; Enter saves
  - The item editor shows only name, category and price; **More options** holds tax, starter, choices, extras, stock and codes
  - **Defaults**: a main tax rate (Settings → Menu defaults) and per-category tax and "items here are starters". Saving the main rate can also fill in items still at 0% (a category set to 0% keeps its items tax-free)
  - Prices can be typed as 12, 12.5, 12,50 or £12.50
  - **Paste a menu** from Excel or Google Sheets (name, price, category), with a preview; missing categories are created
  - **Duplicate** an item with its choices and options
  - **Category tabs**, and drag ⠿ to reorder categories and items (the order screen follows)
  - Create an option group from inside the item editor, see a live preview of the item's tile, and errors shown next to the field
- **Menu** with coloured categories, a tap-to-add item grid, notes per item ("no onions"), line discounts and optional stock counting (for example bottled drinks)
- **Item choices with their own prices**: one menu item sold in versions, e.g. *Tom Yum Soup*: Chicken 6.50 / Beef 7.50 / Prawn 8.50. Set them up in the item's own editor (no option groups needed); the menu shows "from 6.50", staff tap the soup then the meat, and the bill, kitchen ticket and reports show *Tom Yum Soup (Beef)*.
- **Item options with prices**: sizes, extras and choices such as "Burger size" (pick exactly one), "Extras" (pick up to 5) or "Cooking". Option groups are reusable across items, and each option's price (+ or −) is added to the item. A group can be paused ("In use") or deleted; deleting removes it from every item, and past orders keep what was chosen.
- **Starters first, then "Call mains"**: tick **Starter** on menu items (in the menu list or the item editor). On a dine-in order, *Send starters* prints only the starters, with *MAINS TO FOLLOW* at the bottom; everything else waits until you tap **Call mains**, which prints a *MAINS AWAY* ticket. Tick **All courses together** to send everything at once. Takeaway, collection and delivery orders always go in one ticket. Once the mains are in the kitchen (called, or sent together with the starters), **Call mains again** prints a short ticket with just the table, *MAIN COURSE* and the main dishes, so the kitchen fires them now. The kitchen printer can print a different number of copies for starters tickets and for mains tickets.
- **Prepare ahead**: tick **Prep** on dishes that take long or must be got ready early (in the menu list, the item editor, or as a category default). When a dine-in order sends its starters and the mains wait, these dishes print at once on a **PREPARE NOW** ticket (with *SERVE WITH MAINS* at the bottom) so the kitchen can start them; they still go out on the *MAINS AWAY* ticket when you call mains. The order shows them as *preparing*. The kitchen printer has its own copies setting for these tickets.
- **Delivery platforms**: one-tap buttons for Uber Eats, Just Eat and Deliveroo (add, rename, recolour or remove them in Settings). They record the platform's order number, have a *Paid by Uber Eats* button, a ticket heading such as `UBER EATS #7F3K2`, and their own line in reports.
- **Saved customers** for delivery and collection: find them by any part of their phone number, name or postcode and fill the order in one tap. Edit them on the **Customers** screen, which also shows each customer's recent orders.
- **Postcode suggestions, free and offline**: typing a postcode lists the addresses already saved there (door number and street), so streets you deliver to often come up instantly. No paid address service or internet connection is used.
- **Customer receipts and cash drawer**: a receipt prints automatically after payment (your name, address/VAT header and footer), **Print bill** prints the bill before paying, and receipts can be reprinted from History. The cash drawer (plugged into the receipt printer) opens on cash payments and with **Open drawer**, which is logged.
- **Copies, delay and category grouping**: Settings sets how many copies the kitchen and receipt printers make and how many seconds to wait between copies (default 2, so each can be torn off); the first copy prints straight away. Every ticket and receipt lists items under their menu category (STARTERS, MAINS, DRINKS…) in menu order.
- **Kitchen ticket printing** on *Send to kitchen*, to any ESC/POS thermal printer (USB through Windows, network/IP, or a serial port). Tickets show only the new round, marked *NEW ORDER* or *ADDITIONAL*, with options and notes. A **Reprint** button covers paper jams.
- **Service charge**: Settings → Service charge sets a rate (e.g. 12.5%), its name on the bill and which order types get it (dine-in by default). It is a percentage of the food after discounts, not taxed, and shows on the ticket, bill, receipt, reports and end-of-day summary. Staff can change or remove it on any order by tapping it under the totals (logged).
- **Tax per order**: tap **Tax** under the totals to use each item's menu rate (the default), no tax, or one rate for every item on that order (e.g. a tax-free customer). Lines added later follow it; paid orders keep theirs (logged).
- **Quick notes**: Settings → Quick notes lists common requests (No onions, Extra spicy…); tap ✎ on an order line and tap a note to add it. They print on the kitchen ticket, in Thai too.
- **Kitchen tickets** print dishes, options and notes in large (double-size) text, with the table name at both the top and the bottom (Settings → Kitchen printer → Large text).
- **Look**: "paper and ink". Warm paper work area, near-black side rail and primary buttons, one cobalt accent from the logo, solid status colours (tables: sunflower = occupied, teal = mains waiting, red ring = not sent), flat 1px rules instead of shadows and small corners. The menu tiles carry their category colour as a solid edge, and text on a category colour is black or white, whichever reads better. The bundled Sarabun font (Latin and Thai, SIL Open Font License, `ui/fonts/`) is used everywhere, so Thai looks right on every device and offline.
- **Prices include tax** (Settings → Menu defaults, on by default): the price on the menu is what the customer pays, and the tax is worked out inside it. 10.00 at 20% is 8.33 of food and 1.67 of tax (10 × 20 ÷ 120), not 8.00 + 2.00, and the bill, receipt and reports show "Tax (included)". Each order remembers the way it was rung up, so switching the setting only affects new orders. Turn it off to add the tax on top of the price instead (10.00 + 2.00 = 12.00).
- **On-screen keyboard** for touch tills: English, Thai (Kedmanee) and a number pad for prices, quantities and PINs. It opens when you tap a field (not when the app focuses one itself), and the top-bar keyboard button turns it on or off for that device (on by default on the main till, off in browsers, which have their own).
- **Printouts** are bold throughout (Settings → printer → Bold text) and always print "#" as "#", even on printers set to the UK character set.
- **Finish & Pay**: one button opens the payment screen. Choose **Cash** or **Card** (or *Paid by Uber Eats* for platform orders). For cash, type or tap the amount received (with quick buttons for exact and common notes) and the change is shown as you type, then again in large figures after payment. Unsent items go to the kitchen automatically when paid.
- **End of day**: **Reports → Close day & print summary** prints the day's sales summary (orders, totals, tax, cash/card, order types, platforms, categories, voids) on the receipt printer and moves to the next business date. A day with no sales, voids or cancellations closes without printing, shown at the top of the screen. A day runs from one close to the next, so service after midnight still counts for the day it started. All orders must be paid or cancelled first. Previous days can be viewed and reprinted.
- **Use it on other devices (optional local server)**: turn on **Settings → Local server** and open the address it shows (e.g. `http://192.168.1.20:8080`) in a browser on any tablet, laptop or phone on the same Wi-Fi. Each device logs in with its own PIN and shares the same orders, tables, menu and printers. Tick *Start automatically when the POS opens* to have it always on. See [Other devices](#other-devices-local-server).
- **Full screen**: press **F11** or tap the full-screen button at the top right. Turn on **Settings → Display → Open in full screen when the POS starts** for a kiosk-style till.
- **PIN login** with **admin** and **staff** roles
- **Auto logout**: after 5 minutes without a touch or key press (Settings → Auto logout: never, 1–60 minutes), with a 30-second warning. Applies to the till and every device on the local server; open orders are kept
- **Delete mistakes (admin)**: History → an order → **Delete order** removes an order entered by mistake from history and reports for good (stock is put back); use Void for real refunds. In Menu, **Delete item** and **Delete category** remove them for good (items named on past orders are hidden everywhere, and those orders keep their own copy of the name and price)
- **History, voids/refunds and reports**: daily totals, tax, by order type, by payment method, by category, top items
- **Backups**: automatic daily copy at startup (last 14 kept), plus "Back up everything now"
- **Menu backup**: Settings → Menu backup saves just the menu (categories, items, choices, options, prices) to restore later or load on another till (*Restore from a file…*)
- **Reset data (admin, danger zone)**: tick what to delete (sales history, all or before a date; customers; menu; floor plan; staff; settings) and keep the rest. Needs your PIN and typing RESET, and a full backup is saved automatically first
- **Audit log** of logins, voids, discounts, removed items, menu, floor and user changes

The interface is built for touch terminals: a dark side rail, large tiles, a
category column, a running ticket with *Finish & Pay*, and a PIN pad lock
screen. It also adapts to tablets and phones. All business rules live in
Rust, so the UI can change without touching them.

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

### Teams (a ticket for each part of the kitchen)

Settings → **Kitchen teams** starts with Starter, Main, Curry and Rice; rename them, add your own, reorder or remove them.
Give each category a team (Menu → tap the category, "also move the items already here" updates existing items) or set a team on a single item.
When you send an order, every team gets its own ticket with its name as a big white-on-black header, listing only its dishes.
Items without a team print on a plain ticket last, and with no teams at all the kitchen gets one ticket as before.
The switch "Print a separate ticket for each team" in the same card turns the split off (one ticket for everything).
All the tickets go to the kitchen printer in one job; "Print sample tickets" in Settings tests the look.
A team is *where* a ticket prints; the Starter / Prep ticks on an item are still *when* it is sent.

**Call mains again** prints just the table in huge white-on-black letters and "MAIN COURSE" under it, without the dishes (the mains are already in the kitchen).
Settings → Kitchen printer → "Call mains again prints only the table" turns the dishes back on.

### Thai and English names

Give a menu item a **Thai name** (in the item editor or the 4th
column of *Paste a menu*), and its kitchen ticket shows the English name with the Thai
name under it, so every cook can read it. Item choices, options and categories can
have Thai names too. Under **Item names** each printer prints *English with Thai under
it* (kitchen default), *English only* or *Thai only*; receipts default to English only.
Items without a Thai name always print in English.

```
-- MAINS / อาหารจานหลัก --
1 x Pad Thai (Chicken)
    ผัดไทย (ไก่)
   + Fried egg / ไข่ดาว
```

Thermal printers have no Thai characters built in, so lines with Thai are drawn with
the bundled Sarabun font (SIL Open Font License, `crates/pos-core/fonts/`) and sent as
a raster image, which every ESC/POS printer can print. The screens stay in English;
the Thai name is shown small under the item on the order screen.

Printing happens after the order is saved and never blocks the till. A printer
that is off or unreachable gives up after 4 seconds and shows an error.

### Staff copy and the bill's name and address

Settings → Receipt printer lets you set the **name at the top of the bill**, the **address, phone and VAT number** under it, and the message at the bottom.
When an order is paid a second, **staff copy** prints after the customer's receipt: the items and the tax by class (rate, net, VAT, gross), marked "not for customer", to keep for the books.
Switch it off in the same card; "Print staff copy" on a paid order in History reprints it.

Bills and receipts list the dishes without category headings (still in menu order).
Settings → **Words on tickets and bills** changes what tickets call each kind of order (DINE-IN, TAKEAWAY, COLLECTION, DELIVERY, and TABLE on the "main course" call); leave dine-in empty to print just the table.

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
- After 5 wrong PINs in a row the terminal locks for 30 s. The lock doubles with each further failure, up to 15 min. Devices using the local server share one lock of their own, so wrong PINs typed on the network never lock out the main till.
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
| Reports and End of day (close day, print summary) | ✔ | ✔ |
| Settings, backup, reset data | ✔ | |

## Other devices (local server)

The main till (the PC with the app installed) can share the POS with other
devices on the same network, with nothing to install on them.

1. On the main till, open **Settings → Local server**, choose a port (8080 unless something else uses it) and tap **Start server**. Tick **Start automatically when the POS opens** to keep it on.
2. The first time, Windows asks whether to allow *POS* on the network: choose **Allow** for private networks.
3. On the tablet, laptop or phone, open the address shown in the card (for example `http://192.168.1.20:8080`) in Chrome, Edge or Safari and log in with a PIN. Add it to the home screen for one-tap access.

How it works:
- Each device is its own terminal with its own login, so a waiter can use a phone while the till is in use. Logins on devices expire after 12 hours idle.
- Everything is stored and printed by the main till: kitchen tickets and receipts go to the printers set up there, and **Open drawer** opens the till's drawer.
- The main till must be on with the app open. Devices only work on the local network (no internet connection is used).
- Updates, backups and the server itself are managed on the main till.
- The connection is plain HTTP on your local network, as with most local POS and printer setups. Use it on your own secured Wi-Fi, not on public networks.

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

**PC without internet?** Use `POS_x.y.z_x64-offline-setup.exe` instead. The app
draws its screens with Microsoft WebView2, which Windows 10/11 usually already
have; when it is missing, the normal installer downloads it during setup, so it
needs internet. The offline installer includes WebView2 (it is bigger, about
150-200 MB): download it on any computer, copy it to the till with a USB stick and
run it. Once installed, the POS itself never needs internet (only automatic
updates do).

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
  src/raster.rs      Thai text on thermal printers (shaped, drawn as an image)
  src/dayclose.rs    End of day: business date, close, sales summary printout
  src/lan.rs         Local server settings (port, start automatically)
  src/schema.sql     Database schema (migrations via PRAGMA user_version)
  tests/             Integration tests and the performance benchmark
  examples/          demo_data: a sample restaurant for trying the app
src-tauri/           Desktop shell: window, backups, updates, installer config
  src/commands.rs    Every command, shared by the window and the local server
  src/server.rs      Optional local network server (browsers on other devices)
ui/                  Touch UI (index.html, app.js, keyboard.js, style.css, fonts/), no build step
```

## Next steps (not built yet)

- Several kitchen printers with routing by category (e.g. drinks to the bar printer)
- Split bill and partial payments, service charge and tips
- Delivery status tracking (preparing, out for delivery, delivered) and driver assignment
- Shifts / cash-up (opening float, counted cash vs expected, report per shift)
- Code signing for the installer (avoids the Windows SmartScreen warning)
