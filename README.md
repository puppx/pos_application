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
  - **Defaults**: a main tax rate (Manage → Menu defaults) and per-category tax and "items here are starters". Saving the main rate can also fill in items still at 0% (a category set to 0% keeps its items tax-free)
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
- **Service charge**: Manage → Service charge sets a rate (e.g. 12.5%), its name on the bill and which order types get it (dine-in by default). It is a percentage of the food after discounts, not taxed, and shows on the ticket, bill, receipt, reports and end-of-day summary. Staff can change or remove it on any order by tapping it under the totals (logged).
- **Tax per order**: tap **Tax** under the totals to use each item's menu rate (the default), no tax, or one rate for every item on that order (e.g. a tax-free customer). Lines added later follow it; paid orders keep theirs (logged).
- **Quick notes**: Manage → Quick notes lists common requests (No onions, Extra spicy…); tap ✎ on an order line and tap a note to add it. They print on the kitchen ticket, in Thai too.
- **Kitchen tickets** print dishes, options and notes in large (double-size) text, with the table name at both the top and the bottom (Manage → Kitchen printer → Large text).
- **Look**: "paper and ink". Warm paper work area, near-black side rail and primary buttons, one cobalt accent from the logo, solid status colours (tables: sunflower = occupied, teal = mains waiting, red ring = not sent), flat 1px rules instead of shadows and small corners. The menu tiles carry their category colour as a solid edge, and text on a category colour is black or white, whichever reads better. The bundled Sarabun font (Latin and Thai, SIL Open Font License, `ui/fonts/`) is used everywhere, so Thai looks right on every device and offline.
- **Prices include tax** (Manage → Menu defaults, on by default): the price on the menu is what the customer pays, and the tax is worked out inside it. 10.00 at 20% is 8.33 of food and 1.67 of tax (10 × 20 ÷ 120), not 8.00 + 2.00, and the bill, receipt and reports show "Tax (included)". Each order remembers the way it was rung up, so switching the setting only affects new orders. Turn it off to add the tax on top of the price instead (10.00 + 2.00 = 12.00).
- **On-screen keyboard** for touch tills: English, Thai (Kedmanee) and a number pad for prices, quantities and PINs. It opens when you tap a field (not when the app focuses one itself), and the top-bar keyboard button turns it on or off for that device (on by default on the main till, off in browsers, which have their own).
- **Printouts** are bold throughout (Manage → printer → Bold text) and always print "#" as "#", even on printers set to the UK character set.
- **Finish & Pay**: one button opens the payment screen. Choose **Cash** or **Card** (or *Paid by Uber Eats* for platform orders). For cash, type or tap the amount received (with quick buttons for exact and common notes) and the change is shown as you type, then again in large figures after payment. Unsent items go to the kitchen automatically when paid.
- **End of day**: **Reports → Close day & print summary** prints the day's sales summary (orders, totals, tax, cash/card, order types, platforms, categories, voids) on the receipt printer and moves to the next business date. A day with no sales, voids or cancellations closes without printing, shown at the top of the screen. A day runs from one close to the next, so service after midnight still counts for the day it started. All orders must be paid or cancelled first. Previous days can be viewed and reprinted.
- **Use it on other devices (optional local server)**: turn on **Manage → Local server** and open the address it shows (e.g. `http://192.168.1.20:8080`) in a browser on any tablet, laptop or phone on the same Wi-Fi. Each device logs in with its own PIN and shares the same orders, tables, menu and printers. Tick *Start automatically when the POS opens* to have it always on. See [Other devices](#other-devices-local-server).
- **Full screen**: press **F11** or tap the full-screen button at the top right. Turn on **Manage → Display → Open in full screen when the POS starts** for a kiosk-style till.
- **PIN login** with **admin** and **staff** roles
- **Auto logout**: after 5 minutes without a touch or key press (Manage → Auto logout: never, 1–60 minutes), with a 30-second warning. Applies to the till and every device on the local server; open orders are kept
- **Delete mistakes (admin)**: History → an order → **Delete order** removes an order entered by mistake from history and reports for good (stock is put back); use Void for real refunds. In Menu, **Delete item** and **Delete category** remove them for good (items named on past orders are hidden everywhere, and those orders keep their own copy of the name and price)
- **History, voids/refunds and reports**: daily totals, tax, by order type, by payment method, by category, top items. History's **Today** is the open business day (from the last day close, as in the header), so orders after midnight before the day is closed are still under Today; **Yesterday** is the business day closed last and **Last 7 days** the business days dated in the last week. The **Day** picker shows calendar days (midnight to midnight). The line under the buttons says which times are shown
- **Backups**: automatic daily copy at startup (the last 14 kept, or as set in Backup & data), plus "Back up everything now". **Restore** any full backup from the list or from a file (*Restore everything from a file…*), e.g. to move to a new PC: see [Moving to a new PC](#moving-to-a-new-pc)
- **Menu backup**: Manage → Menu backup saves just the menu (categories, items, choices, options, prices) to restore later or load on another till (*Restore from a file…*)
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

**Starter or main course?** On a dine-in order, tapping a starter asks **Starter** or **Main course** (Manage → Orders → *Starter or main course* switches the question off). A dish not yet sent can be changed later: tap ✎ on it, then Starter or Main course.

**A dish not on the menu:** tap **+ Other dish** above the menu, type its name and price (and how many, starter or main, a note). It is taxed at the main tax rate and prints on the kitchen ticket like any dish.

**Copies of kitchen tickets (dine-in):** *Starters ticket copies* for starters sent first (or a table that orders only starters for now); *Mains ticket copies* for "Call mains" and for the first mains sent after the starters, also when the mains were ordered later; *Additional ticket copies* for more dishes after the mains (1 unless set).

**Wrong dish sent?** Tap ✎ on it: **−** or **Remove** prints a **CANCELLED** ticket for the kitchen, a new note prints a **CHANGED** ticket, and **Print again** prints just that dish again. Then add the right dish and send it as usual.
Staff can do this by default; Manage → Orders → **Staff can take off or reduce dishes already sent** keeps it to admins.

Prices are fixed when an item is added, so menu price changes never alter an open bill.
Each table can have only one open order at a time.

## Kitchen printer

**Manage → Kitchen printer** (admin):

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

Manage → **Kitchen teams** starts with Starter, Main, Curry and Rice; rename them, add your own, reorder or remove them.
Give each category a team (Menu → tap the category, "also move the items already here" updates existing items) or set a team on a single item.
When you send an order, every team gets its own ticket with its name as a big white-on-black header, listing only its dishes.
Items without a team print on a plain ticket last, and with no teams at all the kitchen gets one ticket as before.
The switch "Print a separate ticket for each team" in the same card turns the split off: one ticket for everything, still with each team's header over its dishes.
All the tickets go to the kitchen printer in one job; "Print sample tickets" in Settings tests the look.
A team is *where* a ticket prints; the Starter / Prep ticks on an item are still *when* it is sent.

**Call mains again** prints just the table in huge white-on-black letters and "MAIN COURSE" under it, without the dishes (the mains are already in the kitchen).
Manage → Kitchen printer → "Call mains again prints only the table" turns the dishes back on.

### Sub-categories

A category can sit under a main category (one level), e.g. **Food** › Burgers, Pizza, Sides. In Menu, tap a category and choose its **Main category** (or tap a main category, then **+ Sub-category here**). On the order screen the left column shows the main categories (with a ›); tapping one shows its sub-categories as buttons above the items, with **All …** for everything in it. Deleting a main category makes its sub-categories main categories again. Menu backups keep the sub-categories.
A sub-category takes its main category's defaults for new items (starter, prepared ahead, drink, dessert) unless told otherwise: in its editor each one is **Same as main** (default), **On** or **Off**, so e.g. *Drinks › Snacks* can be Off for "Drinks". Changing Drinks or Desserts there also moves the items already in the sub-category, as for a main category.

### Drinks

Tick **Drink** on an item (Menu list, item editor) or **Drinks (own ticket)** on a category (its items follow; new items in it start as drinks).
Drinks are never held for the mains: they go with the first send, like starters, and print first on their own **DRINKS** ticket.
Categories named "Drinks", "Beverages" or "เครื่องดื่ม" start as drinks when upgrading to 0.9.2.

### Split bill and tips

**Finish & Pay** is one screen. The usual case needs no choices: it opens on *Everything*, *No tip* and *Cash*, so tap **Take cash** (or a quick note like 20.00 to see the change), or tap **Card** and confirm. On the same screen:
- **Pay**: *Everything*, *Split evenly* (÷ 2 … ÷ 6, each person in turn), *Amount* (typed on the keypad) or *Dishes* (tap what this person had; the service charge is shared the same way). The order stays open, showing **Left to pay**, until the last part is paid; the receipt then lists every payment.
- **Tip**: 5%, 10%, 15% or another amount, on top of the bill. Tips are not sales and not taxed; Reports and the end-of-day summary show them on their own line.
- **Payment method**: Cash, Card, or the delivery platform for its orders. The keypad types into the highlighted box (cash received, amount or tip); tap a box to type there.
- A part payment taken by mistake: tap **undo** next to it. An order with part payments cannot be cancelled until they are taken back.

**Payment settings** (admins): Manage → Bills & receipts → **Payments**. Not set = the behaviour above.
- **Ask for tips** (on): off = no Tip row in Finish & Pay. **Tip buttons**: three percentages (5, 10, 15).
- **Cash quick buttons**: note values, comma separated (5, 10, 20, 50, 100). The total rounded up is offered too, and the four smallest amounts above the total show.
- **Split evenly: up to** N people, 2 to 12 (6).
- **Payment method when Finish & Pay opens**: Cash or Card (Cash).
- **Tapping "Exact" takes the cash payment at once** (off): the button then reads **Pay exact** and takes the payment with one tap.

### Shift & cash

Manage → **Shift & cash** (admins and managers): count the drawer and **Open shift** with that float. **Money in / Money out** (with a reason) during the shift. At the end type the cash counted: the POS shows what should be there (float + cash taken + cash tips − change + money in − money out − cash refunds for voids) and whether anything is short or over, and prints the shift report. Card payments and card tips are listed but never count towards the drawer.

### Sold out today

On the order screen tap **Sold out**, then tap the items that have run out (tap again to put one back), then **Done**. Sold-out items are greyed out, cannot be added (also inside set menus) and are back on sale after **Close day**.

### Move dishes and join tables

On a dine-in order tap **Move**, choose a table, then **Whole order** or **Some dishes** (tap the dishes and how many). A table that already has a bill gets the dishes added to it (one bill); the emptied order disappears without counting as cancelled. Dishes already sent stay sent and nothing is reprinted.

### Several kitchen printers

Manage → Kitchen printer → **Other kitchen printers**: add a printer (e.g. *Bar*), choose where it is and tick what it prints: Drinks, Desserts and/or kitchen teams. Those tickets print there; everything else on the main kitchen printer. With no other printers everything works as before. If one printer is offline the others still print and the error names it.

### Kitchen screen

Manage → **Kitchen screen** on a tablet in the kitchen (through the local server): one card per order with the dishes sent and not yet done, oldest first, turning orange after 10 minutes and red after 20. Tap a dish when it is ready, or **Done** for the whole card; **Undo last done** if tapped by mistake. Filter by team, food, drinks or desserts. It refreshes every few seconds and never logs out by itself.

Manage → Kitchen printer → **Kitchen screen** (admins and managers): **Orange after** (10) and **Red after** (20) minutes, **Refresh every** 2 to 30 seconds (4), and **Keep paid orders for** 0 to 24 hours (6): a paid order with dishes still to cook stays on the screen that long; 0 = it leaves at once. **Beep when a new ticket arrives** (off): the kitchen screen plays a short two-tone beep (made by the browser, no sound file) when dishes it has not shown before arrive; changing the filter does not beep. Browsers play sound only after a tap on the page: if the screen shows **Tap to turn on the sound**, tap it once (it beeps to confirm). Setting `kds_beep` (`1` / `0`). Managers see only this card in that group; the printers stay admin only.

### Desserts

Tick **Dessert** on an item or **Desserts (own ticket)** on a category. Desserts print last, on their own **DESSERTS** ticket.
Categories named "Desserts", "Sweets" or "ของหวาน" start as desserts when upgrading to 0.10.5.
Manage → Drinks & desserts switches the own DRINKS and DESSERTS tickets on or off; off = they print on the normal ticket with the rest.

### Manage (settings)

The side bar has only **Tables**, **Orders** and **Manage**. Manage opens on big tiles: *Daily work* (History, Customers, Reports, Shift & cash, Kitchen screen, Menu, Floor plan, Staff) and *Settings*, each showing only what the user may open:

- **Admins**: everything.
- **Managers**: History, Customers, Reports, Shift & cash, Kitchen screen, Menu, Floor plan; the settings Kitchen printer (kitchen screen card), Drinks & desserts, Orders, Kitchen teams; and About & updates (credits and version only).
- **Staff**: only **Kitchen screen** and **About & updates** (credits and version only). Selling (Tables, Orders, the order screen, kitchen, payment, bills, cancel with a PIN) is unchanged.

A page someone may not open (e.g. an old link) goes back to the Manage tiles. Every page opened from Manage has a **← Manage** button.

The settings tiles: Restaurant, Bills & receipts, Kitchen printer, Drinks & desserts, Orders, Kitchen teams, Login & staff, Delivery apps, Screen & devices, Backup & data, About & updates. Tap one to see only its settings; **← Manage** goes back.

Manage → Kitchen teams → **Use kitchen teams** switches teams off without deleting them (tickets print as before teams).

More settings (admins). Not set = the behaviour before they existed, except the discount buttons:
- **Start screen after login** (Login & staff): *Tables* (default), *Orders*, or *New takeaway*, which opens a new takeaway order straight after login (leaving it empty drops it, as with any empty order). Setting `start_screen` = `tables` / `orders` / `takeaway`.
- **Warn before** auto logout (Login & staff): *No warning*, 10, 20 or 30 seconds (default 30) or 1 minute. A big **Logging out in 10 s** covers the screen and counts down; any tap or key keeps you logged in, and the tap does not press anything under it. The warning never takes more than half the auto-logout time. Setting `auto_logout_warn_secs` (0 to 120, 0 = off).
- **Order types** (Orders): switch off **Takeaway**, **Collection** or **Delivery** (your own drivers) to hide their new-order buttons on the Orders screen and Customers page. Dine-in always shows, delivery-app buttons are set up under Delivery apps, and open orders of a hidden type still work as usual. Settings `show_takeaway`, `show_collection`, `show_delivery` (`1` / `0`, default `1`).
- **Discounts** (Orders): **Buttons**, up to 8 percentages (default 5, 10, 15, 20; empty = none), show when you tap ✎ on an order line, and one tap saves that much off the line. The discount box also takes a percentage such as `10%`. **Staff can give up to N %** without a manager PIN (empty = no limit, the default). Above it, the till asks for a manager's or admin's PIN to approve it (nobody is logged in or out, and the approval is in the audit log). The server checks the limit too. Managers and admins are never limited, and lowering or removing a discount never needs a PIN. Settings `discount_presets` and `discount_staff_max_pct`.
- **Customer receipts** (Bills & receipts → Receipt printer): **Show the VAT / tax breakdown** (off) adds the TAX / NET / VAT / GROSS table under the total of bills and receipts, with "Prices include tax" when it is inside the prices. **Show "Served by"** (on) can take the staff name off customer bills and receipts; the staff copy always has it. The name, address lines and bottom message were already editable there. With both left as they are, receipts print exactly as before.
- **Payment names** (Bills & receipts → Payments): **Call Cash** and **Call Card** rename how the two methods are shown on the till (Finish & Pay buttons, the payment button, part payments, the change screen), on receipts and the staff copy, in History, Reports, the shift screen and report, and the end-of-day report, e.g. `เงินสด` and `QR / Card`. Display only: payments are still stored and counted as cash and card. Up to 24 characters; empty = Cash / Card, and then receipts print exactly as before. Settings `pay_name_cash`, `pay_name_card`.
- **Keep the last N backups** (Backup & data): how many full backups stay in the backup folder, 3 to 90 (default 14, as before; empty = 14). The copies taken before an update, a reset or a restore, and files loaded with *Restore everything from a file…*, are kept apart (the newest 3 of each, and any from the last 7 days) and other files in the folder are never touched. Lowering it deletes the oldest daily backups at the next backup (startup or **Back up everything now**). Setting `backups_kept`.
- **Cancelling an order** (Orders; managers too): **Print a CANCELLED ticket for the kitchen when an order is cancelled** (on). Off: **Cancel** works exactly the same (same PIN rules, same audit log), but nothing is printed; the confirm box and the PIN box say "Nothing is printed for the kitchen." Taking one sent dish off (− or Remove) still prints its CANCELLED ticket. Setting `print_cancelled_order` (`1` / `0`, default `1`).
- **Menu button size** (Screen & devices): *Normal* or *Large*. Large makes the menu tiles, categories and the order screen's Send, Finish & Pay, tool and +/− buttons bigger on every device, with no sideways scrolling at 1024 × 600 or on a phone. Setting `button_size` = `normal` / `large`. This card also shows on phones and tablets; full screen and the local server stay on the main till.

### Set menus

Menu → **+ Set menu**: a name, price and tax, and sections (Starter, Main and Side to start with; rename, add, remove or reorder them).
Each section lists the dishes to choose from: tap **+ Add dishes** to search (English or Thai) or browse by category and tap dishes in or out. Any dish on the menu can go in any section. Add existing menu items, or make a new dish just for sets (marked "set only", so it has no tile of its own).
"Choose" says how many dishes the customer picks from the section (set it to the number of dishes to include them all), and "Starter course" sends that section with the starters.
Dishes with choices or options are fine too (see below).

A section can also say what its dishes **come with** (e.g. Main: with rice or noodles): add the choices under "Comes with" in the section, or leave it empty for sets that have none. When ordering, each chosen dish then needs one of them, and the kitchen ticket prints it under the dish (`+ with Jasmine rice`).

On the order screen the set's tile opens a picker; the order shows the set with its dishes under it.
The bill and receipt show only the set and its price. Kitchen tickets print the dishes under `SET: <name>` between `=` rules, each section under a `-` rule, still split by team, drinks and course like any other dish.

**Two or more sets sent with all their courses at once** (*All courses together*, a takeaway, a reprint, or cancelling them) print course by course instead of set by set: the sets' names on top (`SET: 2 x DINNER SET`, `SET: LUNCH SET`), then every starter under `STARTERS`, then every main and side under `MAINS` (drinks under `DRINKS` and desserts under `DESSERTS` when they print with the food). The same dish from different sets, with the same options and note, is one line (`2 x Soup`). Every team's, printer's, DRINKS and DESSERTS ticket of that send is laid out the same way. One set (even `2 x` one set), or courses sent one at a time (starters, then *Call mains*), print set by set as before.

```
========== SET: 2 x DINNER SET ===========
============= SET: LUNCH SET =============

---------------- STARTERS ----------------
2 x Soup
1 x Satay
----------------- MAINS ------------------
2 x Green curry
1 x Pad Thai
3 x Jasmine rice
```

Dishes with choices (e.g. chicken or beef) or options (how cooked, extras) can go in a set. When the set is added to an order, each such dish shows its choices right under it; required ones must be picked before **Add to order**. The choice keeps the set price; options with a price (e.g. + cheese) are added to the set's price, and the kitchen ticket shows the choice and options under the dish.

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

**Thai text doesn't print?** Some cheap printers (often unbranded "POS80" models) cannot
print the standard image. Tap **Test Thai printing** on that printer's settings (kitchen
printer, receipt printer or another kitchen printer). It prints one short page with the
same Thai sample in each **Image mode**: *1. Compatible (older printers)*, *2. Small
pieces*, *3. Standard*. Choose the mode whose Thai prints clearly under **Image mode (for
Thai text)**, then save. Each printer keeps its own mode. *Standard* is the default and
prints exactly as before. The other two modes also switch off the Chinese character mode
that many Chinese printers start in.

Printing happens after the order is saved and never blocks the till. A printer
that is off or unreachable gives up after 4 seconds and shows an error.

### Staff copy and the bill's name and address

Manage → Receipt printer lets you set the **name at the top of the bill**, the **address, phone and VAT number** under it, and the message at the bottom.
When an order is paid a second, **staff copy** prints after the customer's receipt: the items and the tax by class (rate, net, VAT, gross), marked "not for customer", to keep for the books.
Switch it off in the same card; "Print staff copy" on a paid order in History reprints it.

Bills and receipts list the dishes without category headings (still in menu order).
Manage → **Words on tickets and bills** changes what tickets call each kind of order (DINE-IN, TAKEAWAY, COLLECTION, DELIVERY, and TABLE on the "main course" call); leave dine-in empty to print just the table.

### Receipt printer and cash drawer

**Manage → Receipt printer & cash drawer** works the same way (it can be the same
printer as the kitchen one). Also set the number of copies, whether a receipt prints
automatically after payment, whether the drawer opens for cash, the currency symbol,
and the header (address, phone, VAT number) and footer lines. Use **Print test receipt**
and **Test cash drawer** before saving. The drawer must be cabled to the receipt
printer's drawer (RJ11/DK) port.

If **Bill** was tapped to give the customer their bill before paying, the receipt is not printed again at **Finish & Pay** (the drawer still opens for cash and the staff copy still prints). The customer's bill and receipt show no tax line when the tax is inside the prices; the staff copy shows the tax by class.

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
- To log in, tap your name, then type your PIN (4–8 digits). Two people may use the same PIN. On a keyboard a PIN can still be typed straight away when nobody else uses it. When a shared PIN approves something (e.g. cancelling an order), it counts as the person with the fewest rights.
- PINs are stored as Argon2id hashes with a per-install salt, never in plain text.
- After 5 wrong PINs in a row the terminal locks for 30 s. The lock doubles with each further failure, up to 15 min. Devices using the local server share one lock of their own, so wrong PINs typed on the network never lock out the main till.
- Permissions are checked in Rust on every command, and the user is re-checked against the database each time. A disabled or demoted user loses access immediately.
- There must always be at least one active admin.

| | Admin | Manager | Staff |
| --- | :---: | :---: | :---: |
| Open orders, add items, send to kitchen, reprint, take payment | ✔ | ✔ | ✔ |
| Move tables and dishes, edit customer details, discounts | ✔ | ✔ | ✔ (over the Settings limit with a manager's or admin's PIN) |
| Remove or reduce items already sent to the kitchen (unless Settings keep it to admins) | ✔ | ✔ | ✔ |
| Cancel an order the kitchen has not got yet | ✔ (no PIN) | ✔ (no PIN) | with their own PIN |
| Cancel an order that has items in the kitchen | ✔ (no PIN) | ✔ (no PIN) | with a manager's or admin's PIN |
| Void / refund paid orders | ✔ | ✔ | |
| Mark dishes sold out from the order screen | ✔ | ✔ | ✔ |
| Kitchen screen (tap dishes done) | ✔ | ✔ | ✔ |
| Manage → About & updates: credits and version | ✔ | ✔ | ✔ |
| Manage → History (paid orders, reprint) | all orders | all orders | |
| Manage → Customers | ✔ | ✔ | |
| Menu, options, stock, floor plan | ✔ | ✔ | |
| Kitchen settings (drinks / desserts tickets, kitchen teams, quick notes) | ✔ | ✔ | |
| Reports, End of day | ✔ | ✔ | |
| Shift & cash (open / close the shift, money in / out) | ✔ | ✔ | |
| Check for and install updates | ✔ | | |
| **Delete** anything (orders, menu items, categories, options, floors, customers, backups) | ✔ | | |
| Staff and PINs, printers, bills, delivery apps, other settings, backup, reset data | ✔ | | |

Staff see only **Kitchen screen** and **About & updates** in Manage; the server refuses them every other Manage command too (they can still find, add and edit a customer while taking an order).

**Cancel** opens a confirm box. Admins and managers just tap **Cancel order** (no PIN); the box says whether the kitchen has the order, and a **CANCELLED** ticket prints for the dishes still cooking (unless switched off in Manage → Orders → *Cancelling an order*). Staff type a PIN in the box: their own for an order the kitchen has not got yet, a manager's or admin's once it has. The PIN only approves the cancel; nobody is logged in or out. The server checks the same rules, and the audit log records who cancelled and whose PIN approved it.

**Delete several at once** (admins): History, Menu (items, categories, option groups), Customers, Floor plan (floors) and Manage → Backup & data (full backups and menu backups) each have a **Delete several…** button: tick what to delete (or **All**), tap Delete, then tap again to confirm. Single backups can also be deleted from their row.

## Other devices (local server)

The main till (the PC with the app installed) can share the POS with other
devices on the same network, with nothing to install on them.

1. On the main till, open **Manage → Local server**, choose a port (8080 unless something else uses it) and tap **Start server**. Tick **Start automatically when the POS opens** to keep it on.
2. The first time, Windows asks whether to allow *POS* on the network: choose **Allow** for private networks.
3. On the tablet, laptop or phone, open the address shown in the card (for example `http://192.168.1.20:8080`) in Chrome, Edge or Safari and log in with a PIN. Add it to the home screen for one-tap access.

How it works:
- Each device is its own terminal with its own login, so a waiter can use a phone while the till is in use. Logins on devices expire after 12 hours idle.
- On a phone the order screen shows the menu or the order. **View order** gives the dishes most of the screen: the bottom menu bar steps aside, the order's header is one line, Send and Finish & Pay sit side by side under small totals, and **+ Add** (top right) goes back to the menu. **←** leaves the order as before.
- Everything is stored and printed by the main till: kitchen tickets and receipts go to the printers set up there, and **Open drawer** opens the till's drawer.
- The main till must be on with the app open. Devices only work on the local network (no internet connection is used).
- Updates, backups and the server itself are managed on the main till.
- The connection is plain HTTP on your local network, as with most local POS and printer setups. Use it on your own secured Wi-Fi, not on public networks.

## Data location

`%APPDATA%\com.puppx.pos\`
- `pos.db`: the database
- `backups\pos-<timestamp>.db`: daily and manual backups
- `backups\pos-before-update-*.db`, `pos-before-reset-*.db`, `pos-before-restore-*.db`: safety copies taken before an update, a reset or a restore (the newest 3 of each are kept, and any from the last 7 days)
- `backups\pos-imported-*.db`: backups loaded with *Restore everything from a file…* (the newest 3 are kept, and any from the last 7 days)

To restore a backup, use **Manage → Backup & data → Restore** (below). By hand: close the app, delete `pos.db-wal` and `pos.db-shm` if they are there, and copy the backup over `pos.db`.

## Moving to a new PC

A full backup holds everything: orders and sales history, menu, customers, floor plan, staff and their PINs, and settings (printers, receipt text, delivery apps…).

On the **old PC**:
1. Log in as an admin, open **Manage → Backup & data** and tap **Back up everything now**.
2. Tap **Open backup folder**: Explorer opens the folder (its path is shown in the card). Copy the newest `pos-<number>.db` (the top one in the list) to a USB stick. In a browser on the local server, tap **Download** on the backup instead.

On the **new PC**:
1. Install the POS and open it. If it asks to set up an admin, create a temporary one (it will be replaced).
2. Open **Manage → Backup & data → Restore everything from a file…** (not the Menu backup card's *Restore from a file…*, which loads only a menu) and choose the file on the USB stick. It works in a browser on the local server too.
3. The till checks the file (a POS database, not damaged, not from a newer version of the app) and shows what is in it: restaurant name, orders, menu items, customers, staff. Type **RESTORE**, tap **Restore…** and enter your admin PIN (the one on this PC).
4. Everyone is logged out and every till, tablet and kitchen screen reloads. Log in with a PIN from the old PC.

Good to know:
- The current data is never lost: before restoring, it is saved as `pos-before-restore-<time>.db`, listed as *Before a restore*. Restore that one to go back.
- A file that is not a POS backup (including one with extra tables, views or triggers the app does not make), is damaged, or comes from a newer version of the app is refused and nothing changes. If restoring fails for any other reason (another program using the database, a full disk), nothing changes either and nobody is logged out. Update the new PC first if needed; backups from older versions are brought up to date when restored.
- Use a file made by the POS (a backup), not a copy of `pos.db` taken while the app was open: that can miss the newest changes.
- Printer names, the local server port and *Start full screen* come from the old PC: check **Kitchen printer** and **Receipt printer** on the new PC.
- Any full backup in the list can be restored the same way (**Restore** on its row). Admins only.

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
and settings are kept, as they live in `%APPDATA%\com.puppx.pos`. **Manage → About**
shows the version and, for admins, has **Check for updates**. Only admins check for,
see and install updates (the app checks while an admin is logged in); managers and
staff see the version and credits only, and the till refuses the update commands for them.

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
repository secret named `TAURI_SIGNING_PRIVATE_KEY` (Manage → Secrets and variables →
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

- Delivery status tracking (preparing, out for delivery, delivered) and driver assignment
- Code signing for the installer (avoids the Windows SmartScreen warning)
