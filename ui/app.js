'use strict';
// Minimal placeholder UI. Every rule (prices, tax, permissions, stock, table
// occupancy) is enforced in Rust; this file only calls commands and draws.

const invoke = window.__TAURI__.core.invoke;
const app = document.getElementById('app');
const state = {
  session: null, storeName: null, plan: { w: 1000, h: 700 },
  view: 'floor', floorId: null, orderId: null, returnTo: 'floor',
  menu: null, cats: null, catId: null, planDirty: false,
};
const TYPE = { dine_in: 'Dine-in', takeaway: 'Takeaway', collection: 'Collection', delivery: 'Delivery' };

// ---------- helpers ----------

function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'value') el.value = v;
    else if (k === 'checked') el.checked = !!v;
    else if (v === true) el.setAttribute(k, '');
    else if (v !== false && v != null) el.setAttribute(k, v);
  }
  for (const c of kids.flat(Infinity)) if (c != null && c !== false) el.append(c instanceof Node ? c : String(c));
  return el;
}
const money = (c) => (c / 100).toFixed(2);
function parseMoney(s) {
  s = String(s).trim();
  if (!/^\d+(\.\d{1,2})?$/.test(s)) throw new Error('enter an amount like 12.50');
  const [w, f = ''] = s.split('.');
  return Number(w) * 100 + Number(f.padEnd(2, '0'));
}
function parseSignedMoney(s) {
  s = String(s).trim();
  const neg = s.startsWith('-');
  return (neg ? -1 : 1) * parseMoney(s.replace(/^[+-]/, '') || '0');
}
const signed = (c) => (c > 0 ? '+' : c < 0 ? '-' : '') + money(Math.abs(c));
const pct = (bp) => (bp / 100).toString();
const fmtTime = (t) => new Date(t * 1000).toLocaleString();
const ago = (t) => `${Math.max(0, Math.floor((Date.now() / 1000 - t) / 60))} min`;
const today = () => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};
function dayRange(iso) {
  const [y, m, d] = iso.split('-').map(Number);
  return [new Date(y, m - 1, d).getTime() / 1000, new Date(y, m - 1, d + 1).getTime() / 1000];
}
const isAdmin = () => state.session?.role === 'admin';
const where = (o) => o.order_type === 'dine_in'
  ? `${o.table_name}${o.floor_name ? ' · ' + o.floor_name : ''}`
  : (o.customer_name || '—');

let toastTimer;
function toast(msg, err = false) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.className = err ? 'err' : '';
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), err ? 4000 : 2500);
}

async function call(cmd, args = {}) {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    const msg = String(e?.message ?? e);
    if (msg === 'please log in') start();
    else toast(msg, true);
    const err = new Error(msg);
    err.handled = true;
    throw err;
  }
}
// Wrap an event handler: shows any error not already shown by call().
const act = (fn) => async (ev) => {
  try { await fn(ev); } catch (e) { if (!e?.handled) toast(String(e?.message ?? e), true); }
};

let refreshTimer;
function screen(...kids) {
  clearInterval(refreshTimer);
  app.replaceChildren(...kids);
}
// Poll a live view (floor, open orders). Each refresh is well under 1 ms.
function live(fn, ms = 10000) {
  refreshTimer = setInterval(() => fn().catch(() => {}), ms);
}

// ---------- start / setup / login ----------

async function start() {
  const st = await invoke('app_status');
  state.session = st.session;
  state.storeName = st.store_name;
  state.plan = { w: st.plan_width, h: st.plan_height };
  document.getElementById('store').textContent = st.store_name || 'POS';
  if (st.needs_setup) return renderSetup();
  if (!st.session) return renderLogin();
  renderShell();
}

function renderSetup() {
  document.getElementById('bar').hidden = true;
  const name = h('input', { placeholder: 'Your name' });
  const pin = h('input', { type: 'password', inputmode: 'numeric', placeholder: 'PIN (4-8 digits)' });
  const pin2 = h('input', { type: 'password', inputmode: 'numeric', placeholder: 'Repeat PIN' });
  const go = act(async () => {
    if (pin.value !== pin2.value) return toast('PINs do not match', true);
    state.session = await call('setup_admin', { name: name.value, pin: pin.value });
    state.view = 'plan';
    renderShell();
    toast('Welcome! Start by drawing your floor plan, then add your menu.');
  });
  screen(h('div', { class: 'center' },
    h('h2', {}, 'Welcome. Create the admin account'),
    name, pin, pin2, h('button', { class: 'primary', onclick: go }, 'Create admin')));
  name.focus();
}

function renderLogin() {
  document.getElementById('bar').hidden = true;
  let pin = '';
  const dots = h('div', { class: 'pindots' });
  const show = () => (dots.textContent = '•'.repeat(pin.length));
  const submit = act(async () => {
    const p = pin;
    pin = ''; show();
    state.session = await call('login', { pin: p });
    document.removeEventListener('keydown', onKey);
    renderShell();
  });
  const press = (k) => {
    if (k === 'C') pin = '';
    else if (k === 'OK') return submit();
    else if (pin.length < 8) pin += k;
    show();
  };
  const onKey = (e) => {
    if (/^\d$/.test(e.key)) press(e.key);
    else if (e.key === 'Backspace') { pin = pin.slice(0, -1); show(); }
    else if (e.key === 'Enter') press('OK');
    else if (e.key === 'Escape') press('C');
  };
  document.removeEventListener('keydown', state.pinKeys);
  state.pinKeys = onKey;
  document.addEventListener('keydown', onKey);
  const keys = ['1', '2', '3', '4', '5', '6', '7', '8', '9', 'C', '0', 'OK'];
  screen(h('div', { class: 'center' },
    h('h2', {}, state.storeName || 'POS'), h('div', {}, 'Enter your PIN'), dots,
    h('div', { class: 'pinpad' }, keys.map((k) =>
      h('button', { class: k === 'OK' ? 'primary' : '', onclick: () => press(k) }, k)))));
}

// ---------- shell ----------

const VIEWS = [
  ['floor', 'Tables', false],
  ['orders', 'Orders', false],
  ['history', 'History', false],
  ['menuAdmin', 'Menu', true],
  ['plan', 'Floor plan', true],
  ['users', 'Users', true],
  ['reports', 'Reports', true],
  ['settings', 'Settings', true],
];

function go(view) {
  if (state.planDirty) return toast('Save or discard your floor plan changes first', true);
  state.view = view;
  state.orderId = null;
  renderShell();
}

function renderShell() {
  if (!VIEWS.some(([id, , adm]) => id === state.view && (!adm || isAdmin()))) state.view = 'floor';
  document.getElementById('bar').hidden = false;
  document.getElementById('who').textContent = `${state.session.name} (${state.session.role})`;
  document.getElementById('nav').replaceChildren(...VIEWS.filter(([, , adm]) => !adm || isAdmin()).map(([id, label]) =>
    h('button', { class: !state.orderId && id === state.view ? 'on' : '', onclick: () => go(id) }, label)));
  const view = state.orderId ? renderOrder : {
    floor: renderFloor, orders: renderOrders, history: renderHistory, menuAdmin: renderMenuAdmin,
    plan: renderPlan, users: renderUsers, reports: renderReports, settings: renderSettings,
  }[state.view];
  view().catch((e) => { if (!e?.handled) toast(String(e?.message ?? e), true); });
}

function openOrder(id) {
  state.returnTo = state.view;
  state.orderId = id;
  renderShell();
}

function closeOrder() {
  state.orderId = null;
  state.view = state.returnTo;
  renderShell();
}

document.getElementById('logout').onclick = act(async () => {
  if (state.planDirty) return toast('Save or discard your floor plan changes first', true);
  await call('logout');
  Object.assign(state, { session: null, orderId: null, view: 'floor', menu: null, cats: null });
  renderLogin();
});

// ---------- floor plan canvas (shared by live view and editor) ----------

function planBox() {
  const { w, h: ph } = state.plan;
  return h('div', { class: 'plan', style: `aspect-ratio:${w}/${ph}; width:min(100%, calc((100vh - 150px) * ${w / ph}))` });
}

function placeTable(el, t) {
  const { w, h: ph } = state.plan;
  el.style.left = `${(t.x / w) * 100}%`;
  el.style.top = `${(t.y / ph) * 100}%`;
  el.style.width = `${(t.w / w) * 100}%`;
  el.style.height = `${(t.h / ph) * 100}%`;
}

function tableEl(t, extra = '') {
  const el = h('div', { class: `tbl ${t.shape === 'round' ? 'round' : ''} ${extra}` });
  placeTable(el, t);
  return el;
}

async function floorTabs(onPick, extra = []) {
  const floors = await call('list_floors');
  if (!floors.some((f) => f.id === state.floorId)) state.floorId = floors[0]?.id ?? null;
  const tabs = h('div', { class: 'row' },
    floors.map((f) => h('button', { class: f.id === state.floorId ? 'on' : '', onclick: () => onPick(f.id) }, f.name)),
    extra);
  return { floors, tabs };
}

// ---------- live floor view ----------

async function renderFloor() {
  const { floors, tabs } = await floorTabs((id) => { state.floorId = id; renderFloor(); });
  if (!floors.length) {
    return screen(h('div', { class: 'card' }, 'No floor plan yet. ',
      isAdmin() ? h('button', { onclick: () => go('plan') }, 'Draw the floor plan') : 'Ask an admin to set it up.'));
  }
  const plan = planBox();
  const draw = async () => {
    const tables = await call('floor_status', { floorId: state.floorId });
    plan.replaceChildren(...tables.map((t) => {
      const el = tableEl(t, `${t.order_id ? 'busy' : ''} ${t.unsent_count ? 'unsent' : ''}`);
      el.append(h('b', {}, t.name));
      if (t.order_id) el.append(h('span', {}, money(t.total_cents)), h('span', { class: 'muted' }, ago(t.opened_at)));
      el.onclick = act(async () => {
        if (t.order_id) return openOrder(t.order_id);
        try {
          const o = await call('open_order', { order: { order_type: 'dine_in', table_id: t.id } });
          openOrder(o.id);
        } catch {
          const o = await invoke('table_order', { tableId: t.id }); // someone else just opened it
          if (o) openOrder(o.id);
        }
      });
      return el;
    }));
  };
  screen(h('div', { class: 'stack' },
    h('div', { class: 'row' }, tabs, h('span', { style: 'flex:1' }),
      h('span', { class: 'muted' }, 'Tap a table to open or continue its order. Red ring = items not sent to kitchen.')),
    plan));
  await draw();
  live(draw);
}

// ---------- order screen ----------

async function loadMenu() {
  if (!state.menu) {
    const [menu, cats, opts] = await Promise.all([
      call('menu'), call('list_categories', { includeInactive: false }), call('menu_options'),
    ]);
    const byId = Object.fromEntries(opts.groups.map((g) => [g.id, g]));
    state.itemGroups = {};
    for (const l of opts.links) (state.itemGroups[l.product_id] ??= []).push(byId[l.group_id]);
    [state.menu, state.cats] = [menu, cats];
  }
}

/// Short rule text for an option group, e.g. "choose 1" or "optional, up to 3".
function groupRule(g) {
  if (g.min_select === g.max_select) return `choose ${g.min_select}`;
  if (g.min_select) return `choose ${g.min_select} to ${g.max_select}`;
  return g.max_select === 1 ? 'optional' : `optional, up to ${g.max_select}`;
}

async function renderOrder() {
  await loadMenu();
  let order = await call('get_order', { id: state.orderId });
  if (order.status !== 'open') {
    return screen(h('div', { class: 'stack', style: 'max-width:640px' },
      h('div', {}, h('button', { onclick: closeOrder }, '← Back')), orderDetail(order, () => renderShell())));
  }
  const orderId = order.id;

  // --- menu side ---
  const search = h('input', { placeholder: 'Search menu or scan barcode…', style: 'flex:1' });
  const cats = h('div', { class: 'cats' });
  const grid = h('div', { class: 'items' });
  const picker = h('div', { class: 'stack picker', hidden: true });
  const catColor = Object.fromEntries(state.cats.map((c) => [c.id, c.color]));
  const drawMenu = () => {
    cats.replaceChildren(
      h('button', { class: state.catId == null ? 'on' : '', onclick: () => { state.catId = null; drawMenu(); } }, 'All'),
      ...state.cats.map((c) => h('button', {
        class: state.catId === c.id ? 'on' : '', style: c.color ? `border-left:6px solid ${c.color}` : '',
        onclick: () => { state.catId = c.id; search.value = ''; drawMenu(); },
      }, c.name)));
    const q = search.value.trim().toLowerCase();
    const list = state.menu.filter((p) => q ? p.name.toLowerCase().includes(q) : state.catId == null || p.category_id === state.catId);
    grid.replaceChildren(...list.slice(0, 300).map((p) => h('button', {
      style: `border-left-color:${catColor[p.category_id] || '#d9dce1'}`, onclick: choose(p),
    }, h('span', {}, p.name), h('small', {}, money(p.price_cents), state.itemGroups[p.id] ? ' …' : ''))));
    return list;
  };
  search.addEventListener('input', drawMenu);
  search.addEventListener('keydown', act(async (e) => {
    if (e.key !== 'Enter' || !search.value.trim()) return;
    const list = drawMenu();
    const p = list.length === 1 ? list[0] : await call('find_by_code', { code: search.value });
    if (!p) return toast('No item found', true);
    await choose(p)();
    search.value = '';
    drawMenu();
  }));

  // --- ticket side ---
  const lines = h('div', { class: 'lines' });
  const totals = h('table', { class: 'totals' });
  const sendBtn = h('button', { onclick: act(send) });
  const tendered = h('input', { placeholder: 'Cash tendered', inputmode: 'decimal', style: 'width:130px' });
  const extra = h('div');
  let editing = null;

  async function addItem(productId, optionIds = []) {
    order = await call('add_item', { orderId, productId, qty: 1, note: null, optionIds });
    draw();
  }
  // Tap on a menu item: add it straight away, or ask for its options first.
  function choose(p) {
    return act(async () => {
      const groups = (state.itemGroups[p.id] || []).filter((g) => g.options.length);
      if (!groups.length) return addItem(p.id);
      openPicker(p, groups);
    });
  }
  function openPicker(p, groups) {
    const chosen = new Map(groups.map((g) => [g.id, new Set()]));
    const body = h('div', { class: 'stack' });
    const price = h('strong');
    const redraw = () => {
      body.replaceChildren(...groups.map((g) => h('div', { class: 'stack' },
        h('div', {}, h('strong', {}, g.name), ' ', h('span', { class: 'muted' }, groupRule(g))),
        h('div', { class: 'opts' }, g.options.map((o) => {
          const set = chosen.get(g.id);
          const on = set.has(o.id);
          return h('button', { class: on ? 'on' : '', onclick: () => {
            if (on) set.delete(o.id);
            else if (g.max_select === 1) { set.clear(); set.add(o.id); }
            else if (set.size < g.max_select) set.add(o.id);
            else return toast(`Up to ${g.max_select} for ${g.name}`, true);
            redraw();
          } }, o.name, o.price_cents ? h('small', {}, signed(o.price_cents)) : '');
        })))));
      const extra = groups.flatMap((g) => g.options.filter((o) => chosen.get(g.id).has(o.id)))
        .reduce((n, o) => n + o.price_cents, 0);
      price.textContent = money(p.price_cents + extra);
    };
    const add = act(async () => {
      const missing = groups.find((g) => chosen.get(g.id).size < g.min_select);
      if (missing) return toast(`${missing.name}: ${groupRule(missing)}`, true);
      await addItem(p.id, groups.flatMap((g) => [...chosen.get(g.id)]));
      closePicker();
    });
    picker.replaceChildren(h('h3', {}, p.name), body,
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: add }, 'Add to order'),
        h('button', { onclick: closePicker }, 'Cancel'), h('span', { style: 'flex:1' }), price));
    redraw();
    [picker.hidden, grid.hidden, cats.hidden] = [false, true, true];
  }
  function closePicker() {
    [picker.hidden, grid.hidden, cats.hidden] = [true, false, false];
  }
  // Report a kitchen print attempt alongside `msg`.
  function reportPrint(p, msg, needPrinter = false) {
    if (p.error) return toast(`${msg}, but the kitchen ticket did not print (${p.error}). Use Reprint.`, true);
    if (!p.attempted && needPrinter) return toast('No kitchen printer is set up (Settings → Kitchen printer)', true);
    toast(p.attempted ? `${msg} · ticket printed` : msg);
  }
  async function setLine(it, changes) {
    order = await call('update_item', {
      itemId: it.id,
      update: { qty: it.qty, discount_cents: it.discount_cents, note: it.note, ...changes },
    });
    draw();
  }
  async function send() {
    const r = await call('send_to_kitchen', { orderId });
    order = await call('get_order', { id: orderId });
    draw();
    const n = r.items.reduce((n, i) => n + i.qty, 0);
    if (!n) return toast('Nothing new to send');
    reportPrint(r.print, `Sent ${n} item(s) to the kitchen`);
  }
  const pay = (method) => act(async () => {
    if (!order.items.length) return toast('The order is empty', true);
    const tenderedCents = method === 'cash' ? (tendered.value ? parseMoney(tendered.value) : order.total_cents) : 0;
    if (method === 'cash' && tenderedCents < order.total_cents) return toast('Cash tendered is less than the total', true);
    // Anything not yet sent goes to the kitchen with the payment.
    let printError = null;
    if (order.items.some((i) => !i.sent_at)) printError = (await call('send_to_kitchen', { orderId })).print.error;
    const paid = await call('pay_order', { orderId, method, tenderedCents });
    const msg = `Order #${paid.id} paid: ${money(paid.total_cents)}` + (paid.change_cents ? ` · change ${money(paid.change_cents)}` : '');
    closeOrder();
    if (printError) toast(`${msg}. Kitchen ticket did not print (${printError}); reprint it from History.`, true);
    else toast(msg);
  });

  function draw() {
    lines.replaceChildren(h('table', {}, order.items.map((it) => {
      const sent = !!it.sent_at;
      const row = h('tr', {},
        h('td', {}, h('div', {}, it.name, sent ? h('span', { class: 'sent' }, ' ✓') : ''),
          it.options.length > 0 && h('div', { class: 'muted' }, it.options.map((o) => o.name).join(', ')),
          it.note && h('div', { class: 'muted' }, '“', it.note, '”'),
          it.discount_cents ? h('div', { class: 'muted' }, `discount -${money(it.discount_cents)}`) : ''),
        h('td', { style: 'white-space:nowrap' },
          h('button', { onclick: act(() => setLine(it, { qty: it.qty - 1 })) }, '−'),
          ` ${it.qty} `,
          h('button', { onclick: act(() => (sent
            ? addItem(it.product_id, it.options.map((o) => o.option_id))
            : setLine(it, { qty: it.qty + 1 }))) }, '+')),
        h('td', { class: 'num' }, money(it.line_total_cents)),
        h('td', {}, h('button', { onclick: () => { editing = editing === it.id ? null : it.id; draw(); } }, '✎')));
      if (editing !== it.id) return row;
      const note = h('input', { value: it.note ?? '', placeholder: 'Note (no onions…)', disabled: sent });
      const disc = h('input', { value: it.discount_cents ? money(it.discount_cents) : '', placeholder: 'Discount', style: 'width:90px' });
      return [row, h('tr', {}, h('td', { colspan: 4 }, h('div', { class: 'row' }, note, disc,
        h('button', { class: 'primary', onclick: act(async () => {
          await setLine(it, { note: note.value || null, discount_cents: disc.value ? parseMoney(disc.value) : 0 });
          editing = null; draw();
        }) }, 'Save'),
        h('button', { class: 'danger', onclick: act(async () => { editing = null; await setLine(it, { qty: 0 }); }) }, 'Remove'))))];
    })));
    if (!order.items.length) lines.append(h('p', { class: 'muted' }, 'Tap menu items to add them.'));
    totals.replaceChildren(
      h('tr', {}, h('td', {}, 'Subtotal'), h('td', { class: 'num' }, money(order.subtotal_cents))),
      order.discount_cents ? h('tr', {}, h('td', {}, 'Discount'), h('td', { class: 'num' }, '-' + money(order.discount_cents))) : '',
      h('tr', {}, h('td', {}, 'Tax'), h('td', { class: 'num' }, money(order.tax_cents))),
      h('tr', { class: 'grand' }, h('td', {}, 'Total'), h('td', { class: 'num' }, money(order.total_cents))));
    const unsent = order.items.filter((i) => !i.sent_at).reduce((n, i) => n + i.qty, 0);
    sendBtn.textContent = unsent ? `Send to kitchen (${unsent})` : 'Send to kitchen';
    sendBtn.disabled = !unsent;
    sendBtn.className = unsent ? 'primary' : '';
  }

  // --- secondary actions: move table, customer details, cancel ---
  const showMove = act(async () => {
    const floors = await call('list_floors');
    const statuses = await Promise.all(floors.map((f) => call('floor_status', { floorId: f.id })));
    const sel = h('select', {}, floors.map((f, i) => h('optgroup', { label: f.name },
      statuses[i].filter((t) => !t.order_id).map((t) => h('option', { value: t.id }, t.name)))));
    extra.replaceChildren(h('div', { class: 'row card' }, 'Move to', sel,
      h('button', { class: 'primary', onclick: act(async () => {
        order = await call('move_order', { id: orderId, tableId: Number(sel.value) });
        extra.replaceChildren(); header(); toast(`Moved to ${order.table_name}`);
      }) }, 'Move'),
      h('button', { onclick: () => extra.replaceChildren() }, 'Close')));
  });
  const showDetails = () => {
    const f = customerFields(order.order_type, order);
    extra.replaceChildren(h('div', { class: 'stack card' }, f.el, h('div', { class: 'row' },
      h('button', { class: 'primary', onclick: act(async () => {
        order = await call('update_order_details', { id: orderId, details: f.value() });
        extra.replaceChildren(); header(); toast('Saved');
      }) }, 'Save'),
      h('button', { onclick: () => extra.replaceChildren() }, 'Close'))));
  };
  const showCancel = () => {
    const reason = h('input', { placeholder: 'Reason', style: 'flex:1' });
    extra.replaceChildren(h('div', { class: 'row card' }, reason,
      h('button', { class: 'danger', onclick: act(async () => {
        await call('cancel_order', { orderId, reason: reason.value });
        toast('Order cancelled'); closeOrder();
      }) }, 'Cancel order'),
      h('button', { onclick: () => extra.replaceChildren() }, 'Keep order')));
  };

  const head = h('div');
  function header() {
    head.replaceChildren(h('div', { class: 'row' },
      h('button', { onclick: closeOrder }, '← Back'),
      h('strong', {}, `#${order.id} ${TYPE[order.order_type]} · ${where(order)}`)),
    h('div', { class: 'muted' }, [
      order.customer_phone, order.delivery_address, order.notes,
      `opened ${ago(order.opened_at)} ago by ${order.opened_by_name}`,
    ].filter(Boolean).join(' · ')));
  }
  header();
  draw();
  drawMenu();
  screen(h('div', { class: 'order' },
    h('div', { class: 'card menu' }, h('div', { class: 'row' }, search), cats, grid, picker),
    h('div', { class: 'card ticket' },
      head, lines, totals,
      h('div', { class: 'row' }, sendBtn),
      h('div', { class: 'row' }, tendered,
        h('button', { class: 'primary', onclick: pay('cash') }, 'Cash'),
        h('button', { class: 'primary', onclick: pay('card') }, 'Card')),
      h('div', { class: 'row' },
        order.order_type === 'dine_in'
          ? h('button', { onclick: showMove }, 'Move table')
          : h('button', { onclick: showDetails }, 'Customer'),
        h('button', { onclick: act(async () => reportPrint(await call('reprint_kitchen', { orderId }), 'Kitchen ticket reprinted', true)) }, 'Reprint kitchen'),
        h('button', { class: 'danger', onclick: showCancel }, 'Cancel order')),
      extra)));
  search.focus();
}

/// Inputs for customer details, shown according to order type.
function customerFields(type, o = {}) {
  const name = h('input', { value: o.customer_name ?? '', placeholder: 'Customer name' + (type === 'takeaway' ? ' (optional)' : '') });
  const phone = h('input', { value: o.customer_phone ?? '', placeholder: 'Phone' + (type === 'delivery' ? '' : ' (optional)'), inputmode: 'tel' });
  const addr = h('textarea', { placeholder: 'Delivery address', rows: 2, style: 'width:100%' });
  addr.value = o.delivery_address ?? '';
  const notes = h('input', { value: o.notes ?? '', placeholder: 'Notes (optional)', style: 'flex:1' });
  return {
    el: h('div', { class: 'stack' }, h('div', { class: 'row' }, name, phone), type === 'delivery' && addr, h('div', { class: 'row' }, notes)),
    value: () => ({
      customer_name: name.value || null, customer_phone: phone.value || null,
      delivery_address: type === 'delivery' ? addr.value || null : null, notes: notes.value || null,
    }),
  };
}

/// Read-only view of a closed order, with void for admins.
function orderDetail(o, onChange) {
  const reason = h('input', { placeholder: 'Void reason' });
  return h('div', { class: 'card stack' },
    h('h3', {}, `#${o.id} ${TYPE[o.order_type]} · ${where(o)} — ${o.status}`),
    h('div', { class: 'muted' }, `Opened ${fmtTime(o.opened_at)} by ${o.opened_by_name}` +
      (o.paid_at ? ` · paid ${fmtTime(o.paid_at)} (${o.payment_method})` : '')),
    (o.customer_phone || o.delivery_address) && h('div', { class: 'muted' }, [o.customer_phone, o.delivery_address].filter(Boolean).join(' · ')),
    h('table', {}, o.items.map((i) => h('tr', {},
      h('td', {}, i.name, i.options.length > 0 && h('div', { class: 'muted' }, i.options.map((x) => x.name).join(', ')),
        i.note && h('div', { class: 'muted' }, i.note)),
      h('td', { class: 'num' }, `${i.qty} × ${money(i.unit_price_cents)}`),
      h('td', { class: 'num' }, i.discount_cents ? '-' + money(i.discount_cents) : ''),
      h('td', { class: 'num' }, money(i.line_total_cents))))),
    h('div', {}, `Tax ${money(o.tax_cents)} · `, h('strong', {}, `Total ${money(o.total_cents)}`),
      o.tendered_cents != null ? ` · Tendered ${money(o.tendered_cents)} · Change ${money(o.change_cents)}` : ''),
    o.close_reason && h('div', {}, `Reason: ${o.close_reason}`),
    o.status === 'paid' && h('div', {}, h('button', { onclick: act(async () => {
      const p = await call('reprint_kitchen', { orderId: o.id });
      if (p.error) toast(`Did not print: ${p.error}`, true);
      else toast(p.attempted ? 'Kitchen ticket reprinted' : 'No kitchen printer is set up', !p.attempted);
    }) }, 'Reprint kitchen ticket')),
    isAdmin() && o.status === 'paid' && h('div', { class: 'row' }, reason,
      h('button', { class: 'danger', onclick: act(async () => {
        await call('void_order', { orderId: o.id, reason: reason.value });
        toast('Order voided');
        onChange();
      }) }, 'Void / refund')));
}

// ---------- open orders board + new takeaway / collection / delivery ----------

async function renderOrders() {
  const form = h('div');
  const board = h('div');
  const newOrder = (type) => act(async () => {
    if (type === 'takeaway') {
      const o = await call('open_order', { order: { order_type: 'takeaway' } });
      return openOrder(o.id);
    }
    const f = customerFields(type);
    form.replaceChildren(h('div', { class: 'card stack' }, h('h3', {}, `New ${TYPE[type].toLowerCase()} order`), f.el,
      h('div', { class: 'row' },
        h('button', { class: 'primary', onclick: act(async () => {
          const o = await call('open_order', { order: { order_type: type, ...f.value() } });
          openOrder(o.id);
        }) }, 'Start order'),
        h('button', { onclick: () => form.replaceChildren() }, 'Cancel'))));
    form.querySelector('input').focus();
  });
  const draw = async () => {
    const orders = await call('list_open_orders');
    board.replaceChildren(orders.length ? h('table', {},
      h('tr', {}, h('th', {}, '#'), h('th', {}, 'Type'), h('th', {}, 'Table / customer'), h('th', {}, 'Waiting'),
        h('th', {}, 'Staff'), h('th', { class: 'num' }, 'Items'), h('th', { class: 'num' }, 'Total')),
      orders.map((o) => h('tr', { class: 'click', onclick: () => openOrder(o.id) },
        h('td', {}, o.id), h('td', {}, h('span', { class: 'badge' }, TYPE[o.order_type])),
        h('td', {}, where(o), o.customer_phone ? h('div', { class: 'muted' }, o.customer_phone) : ''),
        h('td', {}, ago(o.opened_at)), h('td', {}, o.opened_by_name),
        h('td', { class: 'num' }, o.item_count, o.unsent_count ? h('div', {}, h('span', { class: 'badge warn' }, `${o.unsent_count} not sent`)) : ''),
        h('td', { class: 'num' }, money(o.total_cents)))))
      : h('p', { class: 'muted' }, 'No open orders.'));
  };
  screen(h('div', { class: 'stack' },
    h('div', { class: 'row' },
      h('button', { class: 'primary', onclick: newOrder('takeaway') }, '+ Takeaway'),
      h('button', { class: 'primary', onclick: newOrder('collection') }, '+ Collection'),
      h('button', { class: 'primary', onclick: newOrder('delivery') }, '+ Delivery'),
      h('span', { class: 'muted' }, 'Dine-in orders start from the Tables screen.')),
    form, h('h3', {}, 'Open orders'), board));
  await draw();
  live(draw);
}

// ---------- history ----------

async function renderHistory() {
  const date = h('input', { type: 'date', value: today() });
  const list = h('div');
  const detail = h('div');
  const load = act(async () => {
    const [from, to] = dayRange(date.value);
    const orders = await call('list_orders', { from, to, limit: 500, offset: 0 });
    list.replaceChildren(h('table', {},
      h('tr', {}, h('th', {}, '#'), h('th', {}, 'Opened'), h('th', {}, 'Type'), h('th', {}, 'Table / customer'), h('th', {}, 'Status'), h('th', { class: 'num' }, 'Total')),
      orders.map((o) => h('tr', { class: 'click', onclick: act(() => show(o.id)) },
        h('td', {}, o.id), h('td', {}, new Date(o.opened_at * 1000).toLocaleTimeString()), h('td', {}, TYPE[o.order_type]),
        h('td', {}, where(o)), h('td', {}, o.status), h('td', { class: 'num' }, money(o.total_cents))))));
  });
  async function show(id) {
    detail.replaceChildren(orderDetail(await call('get_order', { id }), async () => { await show(id); await load(); }));
  }
  date.onchange = load;
  screen(h('div', { class: 'grid2' }, h('div', { class: 'stack' }, h('div', { class: 'row' }, 'Day', date), list), detail));
  await load();
}

// ---------- menu (admin) ----------

async function renderMenuAdmin() {
  const [cats, groups, feed] = await Promise.all([
    call('list_categories', { includeInactive: true }),
    call('list_option_groups', { includeInactive: true }),
    call('menu_options'),
  ]);
  const activeGroups = groups.filter((g) => g.active);
  const catName = Object.fromEntries(cats.map((c) => [c.id, c.name]));
  const form = h('div');
  const list = h('div');
  const filter = h('select', {}, h('option', { value: '' }, 'All categories'), cats.filter((c) => c.active).map((c) => h('option', { value: c.id }, c.name)));
  const showInactive = h('input', { type: 'checkbox' });
  const changed = async () => { state.menu = null; form.replaceChildren(); await renderMenuAdmin(); };

  const loadProducts = act(async () => {
    let products = await call('list_products', { includeInactive: showInactive.checked, limit: 500, offset: 0 });
    if (filter.value) products = products.filter((p) => p.category_id === Number(filter.value));
    list.replaceChildren(h('table', {},
      h('tr', {}, h('th', {}, 'Item'), h('th', {}, 'Category'), h('th', { class: 'num' }, 'Price'), h('th', { class: 'num' }, 'Tax %'), h('th', { class: 'num' }, 'Stock')),
      products.map((p) => h('tr', { class: 'click', style: p.active ? '' : 'opacity:.5', onclick: () => editProduct(p) },
        h('td', {}, p.name), h('td', {}, catName[p.category_id] ?? '—'), h('td', { class: 'num' }, money(p.price_cents)),
        h('td', { class: 'num' }, pct(p.tax_rate_bp)), h('td', { class: 'num' }, p.track_stock ? p.stock_qty : '—')))));
  });

  function editCategory(c) {
    const name = h('input', { value: c?.name ?? '', placeholder: 'Category name' });
    const color = h('input', { type: 'color', value: c?.color ?? '#9ca3af' });
    const order = h('input', { value: c?.sort_order ?? cats.length + 1, inputmode: 'numeric', style: 'width:70px', title: 'Position' });
    const active = h('input', { type: 'checkbox', checked: c ? c.active : true });
    form.replaceChildren(h('div', { class: 'card stack' }, h('h3', {}, c ? `Edit ${c.name}` : 'New category'),
      h('div', { class: 'row' }, name, 'Colour', color, 'Position', order, c && h('label', {}, active, ' Active')),
      h('div', { class: 'row' },
        h('button', { class: 'primary', onclick: act(async () => {
          const args = { name: name.value, color: color.value, sortOrder: parseInt(order.value, 10) || 0 };
          if (c) await call('update_category', { id: c.id, ...args, active: active.checked });
          else await call('create_category', args);
          toast('Saved'); await changed();
        }) }, 'Save'),
        h('button', { onclick: () => form.replaceChildren() }, 'Cancel'))));
    name.focus();
  }

  function editGroup(g) {
    const name = h('input', { value: g?.name ?? '', placeholder: 'Group name, e.g. Burger size' });
    const min = h('input', { type: 'number', value: g?.min_select ?? 0, min: 0, max: 20, style: 'width:70px' });
    const max = h('input', { type: 'number', value: g?.max_select ?? 1, min: 1, max: 20, style: 'width:70px' });
    const active = h('input', { type: 'checkbox', checked: g ? g.active : true });
    const rows = (g?.options ?? []).filter((o) => o.active).map((o) => ({ id: o.id, name: o.name, price: o.price_cents ? signed(o.price_cents) : '' }));
    const list = h('div', { class: 'stack' });
    const drawRows = () => list.replaceChildren(...rows.map((r, i) => h('div', { class: 'row' },
      h('input', { value: r.name, placeholder: 'Option, e.g. Large', oninput: (e) => (r.name = e.target.value) }),
      h('input', { value: r.price, placeholder: '+0.00', style: 'width:90px', inputmode: 'decimal', oninput: (e) => (r.price = e.target.value) }),
      h('button', { class: 'danger', onclick: () => { rows.splice(i, 1); drawRows(); } }, '×'))));
    const addRow = () => { rows.push({ id: null, name: '', price: '' }); drawRows(); list.lastChild.querySelector('input').focus(); };
    const save = act(async () => {
      const args = { name: name.value, minSelect: parseInt(min.value, 10) || 0, maxSelect: parseInt(max.value, 10) || 1 };
      const options = rows.filter((r) => r.name.trim()).map((r) => ({ id: r.id, name: r.name, price_cents: parseSignedMoney(r.price) }));
      const saved = g
        ? await call('update_option_group', { id: g.id, ...args, sortOrder: g.sort_order, active: active.checked })
        : await call('create_option_group', args);
      await call('save_group_options', { groupId: saved.id, options });
      toast('Saved'); await changed();
    });
    form.replaceChildren(h('div', { class: 'card stack' },
      h('h3', {}, g ? `Edit ${g.name}` : 'New option group'),
      h('div', { class: 'row' }, name, g && h('label', {}, active, ' In use')),
      h('div', { class: 'row' }, 'Customer must choose at least', min, 'and at most', max),
      h('div', { class: 'muted' }, 'Sizes: at least 1, at most 1. Extras: at least 0, at most e.g. 5. Prices are added to the item (use -1.00 for a cheaper option).'),
      list,
      h('div', {}, h('button', { onclick: addRow }, '+ Option')),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Save'), h('button', { onclick: () => form.replaceChildren() }, 'Cancel'))));
    drawRows();
    if (!rows.length) addRow();
    name.focus();
  }

  function editProduct(p) {
    const f = {
      name: h('input', { value: p?.name ?? '', placeholder: 'Name' }),
      cat: h('select', {}, h('option', { value: '' }, 'No category'), cats.filter((c) => c.active || c.id === p?.category_id).map((c) => h('option', { value: c.id }, c.name))),
      price: h('input', { value: p ? money(p.price_cents) : '', placeholder: 'Price', inputmode: 'decimal', style: 'width:100px' }),
      tax: h('input', { value: p ? pct(p.tax_rate_bp) : '0', inputmode: 'decimal', style: 'width:70px' }),
      cost: h('input', { value: p ? money(p.cost_cents) : '0', inputmode: 'decimal', style: 'width:100px' }),
      barcode: h('input', { value: p?.barcode ?? '', placeholder: 'Barcode (optional)' }),
      sku: h('input', { value: p?.sku ?? '', placeholder: 'Code / PLU (optional)' }),
      track: h('input', { type: 'checkbox', checked: p ? p.track_stock : false }),
      stock: h('input', { value: '0', inputmode: 'numeric', style: 'width:80px' }),
      active: h('input', { type: 'checkbox', checked: p ? p.active : true }),
    };
    const linked = new Set(feed.links.filter((l) => l.product_id === p?.id).map((l) => l.group_id));
    const groupBoxes = activeGroups.map((g) => ({ g, box: h('input', { type: 'checkbox', checked: linked.has(g.id) }) }));
    f.cat.value = p ? (p.category_id ?? '') : filter.value;
    const data = () => ({
      name: f.name.value, category_id: f.cat.value ? Number(f.cat.value) : null,
      barcode: f.barcode.value || null, sku: f.sku.value || null,
      price_cents: parseMoney(f.price.value), cost_cents: parseMoney(f.cost.value || '0'),
      tax_rate_bp: Math.round(Number(f.tax.value || 0) * 100), track_stock: f.track.checked,
    });
    const save = act(async () => {
      const saved = p
        ? await call('update_product', { id: p.id, product: { ...data(), active: f.active.checked } })
        : await call('create_product', { product: { ...data(), stock_qty: parseInt(f.stock.value || '0', 10) } });
      const groupIds = groupBoxes.filter((x) => x.box.checked).map((x) => x.g.id);
      if (p || groupIds.length) await call('set_product_option_groups', { productId: saved.id, groupIds });
      toast('Saved'); await changed();
    });
    const delta = h('input', { placeholder: '+/- qty', style: 'width:90px', inputmode: 'numeric' });
    const reason = h('input', { placeholder: 'Reason (delivery, count…)' });
    form.replaceChildren(h('div', { class: 'card stack' },
      h('h3', {}, p ? `Edit ${p.name}` : 'New menu item'),
      h('div', { class: 'row' }, f.name, f.cat),
      h('div', { class: 'row' }, 'Price', f.price, 'Tax %', f.tax, 'Cost', f.cost),
      h('div', { class: 'row' }, f.barcode, f.sku),
      groupBoxes.length > 0 && h('div', { class: 'stack' }, h('strong', {}, 'Options'),
        h('div', { class: 'row' }, groupBoxes.map(({ g, box }) => h('label', {}, box, ` ${g.name} `, h('span', { class: 'muted' }, `(${groupRule(g)})`))))),
      h('div', { class: 'row' }, h('label', {}, f.track, ' Count stock (e.g. bottled drinks)'),
        !p && ['Opening stock', f.stock], p && h('label', {}, f.active, ' On the menu')),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Save'), h('button', { onclick: () => form.replaceChildren() }, 'Cancel')),
      p && p.track_stock && h('div', { class: 'row' }, `Stock: ${p.stock_qty}`, delta, reason,
        h('button', { onclick: act(async () => {
          await call('adjust_stock', { id: p.id, delta: parseInt(delta.value, 10) || 0, reason: reason.value });
          toast('Stock updated'); await changed();
        }) }, 'Adjust'))));
    f.name.focus();
  }

  filter.onchange = loadProducts;
  showInactive.onchange = loadProducts;
  screen(h('div', { class: 'grid2' },
    h('div', { class: 'stack' },
      h('div', { class: 'card stack' },
        h('div', { class: 'row' }, h('h3', { style: 'flex:1' }, 'Categories'), h('button', { onclick: () => editCategory(null) }, '+ Category')),
        h('div', { class: 'row' }, cats.map((c) => h('button', {
          style: `border-left:6px solid ${c.color || '#d9dce1'}${c.active ? '' : ';opacity:.5'}`, onclick: () => editCategory(c),
        }, c.name)))),
      h('div', { class: 'card stack' },
        h('div', { class: 'row' }, h('h3', { style: 'flex:1' }, 'Options (sizes, extras…)'), h('button', { onclick: () => editGroup(null) }, '+ Option group')),
        h('div', { class: 'row' }, groups.length ? groups.map((g) => h('button', {
          style: g.active ? '' : 'opacity:.5', onclick: () => editGroup(g), title: g.options.filter((o) => o.active).map((o) => o.name).join(', '),
        }, g.name, h('small', { class: 'muted' }, ` ${groupRule(g)}`)))
          : h('span', { class: 'muted' }, 'Create a group like "Size" or "Extras", then tick it on the menu items that use it.'))),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: () => editProduct(null) }, '+ Menu item'), filter,
        h('label', {}, showInactive, ' Show removed')),
      list),
    form));
  await loadProducts();
}

// ---------- floor plan editor (admin) ----------

async function renderPlan() {
  const newFloor = h('input', { placeholder: 'New floor name', style: 'width:150px' });
  const addFloor = act(async () => {
    if (state.planDirty) return toast('Save or discard changes on this floor first', true);
    const f = await call('create_floor', { name: newFloor.value });
    state.floorId = f.id;
    renderPlan();
  });
  const { floors, tabs } = await floorTabs((id) => {
    if (state.planDirty) return toast('Save or discard changes on this floor first', true);
    state.floorId = id;
    renderPlan();
  }, [newFloor, h('button', { onclick: addFloor }, '+ Floor')]);
  if (!floors.length) {
    return screen(h('div', { class: 'card stack', style: 'max-width:520px' },
      h('h3', {}, 'Floor plan'), 'Create your first floor (for example "Ground floor"), then add tables to it.', tabs));
  }
  const floor = floors.find((f) => f.id === state.floorId);
  const { w: PW, h: PH } = state.plan;
  const SNAP = 10;
  let tables = await call('list_tables', { floorId: floor.id });
  let sel = null;
  const plan = planBox();
  plan.classList.add('edit');
  const panel = h('div', { class: 'stack' });
  const saveBtn = h('button', { class: 'primary', onclick: act(save) });
  const setDirty = (d) => {
    state.planDirty = d;
    saveBtn.textContent = d ? 'Save layout •' : 'Saved';
    saveBtn.disabled = !d;
  };
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
  const snap = (v) => Math.round(v / SNAP) * SNAP;

  function draw() {
    plan.replaceChildren(...tables.map((t) => {
      const el = tableEl(t, t === sel ? 'sel' : '');
      el.append(h('b', {}, t.name));
      el.addEventListener('pointerdown', (e) => {
        e.preventDefault();
        sel = t;
        const rect = plan.getBoundingClientRect();
        const scale = PW / rect.width;
        const sx = e.clientX, sy = e.clientY, x0 = t.x, y0 = t.y;
        let moved = false;
        el.setPointerCapture(e.pointerId);
        el.onpointermove = (m) => {
          const nx = clamp(snap(x0 + (m.clientX - sx) * scale), 0, PW - t.w);
          const ny = clamp(snap(y0 + (m.clientY - sy) * scale), 0, PH - t.h);
          if (nx !== t.x || ny !== t.y) { t.x = nx; t.y = ny; moved = true; placeTable(el, t); }
        };
        el.onpointerup = () => {
          el.onpointermove = el.onpointerup = null;
          if (moved) setDirty(true);
          draw();
        };
        plan.querySelectorAll('.tbl').forEach((x) => x.classList.remove('sel'));
        el.classList.add('sel');
        drawPanel();
      });
      return el;
    }));
    drawPanel();
  }

  function nextName() {
    const used = new Set(tables.map((t) => t.name.toLowerCase()));
    for (let n = 1; ; n++) if (!used.has(`t${n}`)) return `T${n}`;
  }
  function freeSpot(w, hh) {
    const hit = (x, y) => tables.some((t) => x < t.x + t.w + SNAP && t.x < x + w + SNAP && y < t.y + t.h + SNAP && t.y < y + hh + SNAP);
    for (let y = 20; y + hh <= PH; y += 20) for (let x = 20; x + w <= PW; x += 20) if (!hit(x, y)) return { x, y };
    return { x: 0, y: 0 };
  }
  const addTable = (shape) => () => {
    const size = shape === 'round' ? 90 : 100;
    const t = { id: null, name: nextName(), shape, w: size, h: size, ...freeSpot(size, size) };
    tables.push(t);
    sel = t;
    setDirty(true);
    draw();
  };

  function drawPanel() {
    const kids = [
      h('div', { class: 'row' }, h('button', { onclick: addTable('rect') }, '+ Square table'), h('button', { onclick: addTable('round') }, '+ Round table')),
      h('div', { class: 'muted' }, 'Drag tables to arrange them. No seat numbers needed: a table is just a name.'),
    ];
    if (sel) {
      const t = sel;
      const name = h('input', { value: t.name, style: 'width:110px' });
      const shape = h('select', {}, h('option', { value: 'rect' }, 'Square / rectangle'), h('option', { value: 'round' }, 'Round'));
      shape.value = t.shape;
      const w = h('input', { type: 'number', value: t.w, step: SNAP, min: 30, max: 400, style: 'width:80px' });
      const hh = h('input', { type: 'number', value: t.h, step: SNAP, min: 30, max: 400, style: 'width:80px' });
      const apply = () => {
        t.name = name.value.trim() || t.name;
        t.shape = shape.value;
        t.w = clamp(snap(Number(w.value) || t.w), 30, 400);
        t.h = clamp(snap(Number(hh.value) || t.h), 30, 400);
        t.x = clamp(t.x, 0, PW - t.w);
        t.y = clamp(t.y, 0, PH - t.h);
        setDirty(true);
        draw();
      };
      [name, shape, w, hh].forEach((el) => el.addEventListener('change', apply));
      kids.push(h('div', { class: 'card stack' },
        h('h3', {}, `Table ${t.name}`),
        h('div', { class: 'row' }, 'Name', name),
        h('div', { class: 'row' }, 'Shape', shape),
        h('div', { class: 'row' }, 'Width', w, 'Depth', hh),
        h('div', { class: 'row' },
          h('button', { onclick: () => { const c = { ...t, id: null, name: nextName(), ...freeSpot(t.w, t.h) }; tables.push(c); sel = c; setDirty(true); draw(); } }, 'Duplicate'),
          h('button', { class: 'danger', onclick: () => { tables = tables.filter((x) => x !== t); sel = null; setDirty(true); draw(); } }, 'Remove table'))));
    }
    const rename = h('input', { value: floor.name, style: 'width:150px' });
    kids.push(
      h('div', { class: 'row' }, saveBtn,
        h('button', { onclick: () => { setDirty(false); renderPlan(); } }, 'Discard changes')),
      h('div', { class: 'card stack' }, h('h3', {}, 'This floor'),
        h('div', { class: 'row' }, rename, h('button', { onclick: act(async () => {
          await call('update_floor', { id: floor.id, name: rename.value, sortOrder: floor.sort_order });
          toast('Renamed'); renderPlan();
        }) }, 'Rename')),
        h('button', { class: 'danger', onclick: act(async () => {
          await call('delete_floor', { id: floor.id });
          setDirty(false); state.floorId = null; toast('Floor removed'); renderPlan();
        }) }, 'Delete floor')));
    panel.replaceChildren(...kids);
  }

  async function save() {
    const saved = await call('save_floor_layout', {
      floorId: floor.id,
      tables: tables.map(({ id, name, shape, x, y, w, h: th }) => ({ id: id ?? null, name, shape, x, y, w, h: th })),
    });
    const selName = sel?.name;
    tables = saved;
    sel = tables.find((t) => t.name === selName) ?? null;
    setDirty(false);
    draw();
    toast('Floor plan saved');
  }

  setDirty(false);
  screen(h('div', { class: 'stack' }, tabs,
    h('div', { style: 'display:grid; grid-template-columns:1fr 320px; gap:12px; align-items:start' }, plan, panel)));
  draw();
}

// ---------- users (admin) ----------

async function renderUsers() {
  const list = h('div');
  const form = h('div');
  const load = async () => {
    const users = await call('list_users');
    list.replaceChildren(h('table', {},
      h('tr', {}, h('th', {}, 'Name'), h('th', {}, 'Role'), h('th', {}, 'Status'), h('th', {}, 'Last login')),
      users.map((u) => h('tr', { class: 'click', onclick: () => edit(u) },
        h('td', {}, u.name), h('td', {}, u.role), h('td', {}, u.active ? 'active' : 'disabled'),
        h('td', {}, u.last_login_at ? fmtTime(u.last_login_at) : '—')))));
  };
  function edit(u) {
    const name = h('input', { value: u?.name ?? '', placeholder: 'Name' });
    const role = h('select', {}, h('option', { value: 'staff' }, 'Staff'), h('option', { value: 'admin' }, 'Admin'));
    role.value = u?.role ?? 'staff';
    const active = h('input', { type: 'checkbox', checked: u ? u.active : true });
    const pin = h('input', { type: 'password', inputmode: 'numeric', placeholder: u ? 'New PIN (optional)' : 'PIN (4-8 digits)' });
    const save = act(async () => {
      if (u) {
        await call('update_user', { id: u.id, name: name.value, role: role.value, active: active.checked });
        if (pin.value) await call('reset_pin', { id: u.id, pin: pin.value });
      } else {
        await call('create_user', { name: name.value, role: role.value, pin: pin.value });
      }
      toast('Saved');
      form.replaceChildren();
      await start(); // own role/name may have changed
    });
    form.replaceChildren(h('div', { class: 'card stack' },
      h('h3', {}, u ? `Edit ${u.name}` : 'New user'),
      h('div', { class: 'row' }, name, role, u && h('label', {}, active, ' Active')),
      h('div', { class: 'row' }, pin),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Save'), h('button', { onclick: () => form.replaceChildren() }, 'Cancel'))));
    name.focus();
  }
  screen(h('div', { class: 'grid2' },
    h('div', { class: 'stack' }, h('div', {}, h('button', { class: 'primary', onclick: () => edit(null) }, '+ User')), list), form));
  await load();
}

// ---------- reports (admin) ----------

async function renderReports() {
  const date = h('input', { type: 'date', value: today() });
  const out = h('div');
  const rows = (pairs) => h('table', {}, pairs.map(([k, v]) => h('tr', {}, h('td', {}, k), h('td', { class: 'num' }, v))));
  const load = act(async () => {
    const [from, to] = dayRange(date.value);
    const [s, top] = await Promise.all([call('sales_summary', { from, to }), call('top_products', { from, to, limit: 20 })]);
    out.replaceChildren(h('div', { class: 'grid2' },
      h('div', { class: 'stack' },
        h('div', { class: 'card' }, h('h3', {}, 'Summary'), rows([
          ['Paid orders', s.order_count], ['Gross', money(s.subtotal_cents)], ['Discounts', money(s.discount_cents)],
          ['Tax', money(s.tax_cents)], ['Total', money(s.total_cents)],
          ['Voided / refunded', `${s.voided_count} (${money(s.voided_total_cents)})`], ['Cancelled', s.cancelled_count]])),
        h('div', { class: 'card' }, h('h3', {}, 'By order type'),
          rows(s.by_type.map((g) => [TYPE[g.key] ?? g.key, `${g.count} · ${money(g.total_cents)}`]))),
        h('div', { class: 'card' }, h('h3', {}, 'By payment'),
          rows(s.by_payment.map((g) => [g.key, `${g.count} · ${money(g.total_cents)}`])))),
      h('div', { class: 'card' }, h('h3', {}, 'Top items'), h('table', {},
        top.map((t) => h('tr', {}, h('td', {}, t.name), h('td', { class: 'num' }, t.qty), h('td', { class: 'num' }, money(t.revenue_cents))))))));
  });
  date.onchange = load;
  screen(h('div', { class: 'stack' }, h('div', { class: 'row' }, 'Day', date), out));
  await load();
}

// ---------- settings (admin) ----------

async function renderSettings() {
  const store = h('input', { value: state.storeName ?? '', placeholder: 'Restaurant name' });
  const [printer, systemPrinters] = await Promise.all([call('get_kitchen_printer'), call('list_system_printers')]);

  // --- kitchen printer ---
  const kind = h('select', {},
    h('option', { value: 'none' }, 'No kitchen printer'),
    h('option', { value: 'network' }, 'Network printer (IP address)'),
    h('option', { value: 'windows' }, 'Printer installed in Windows (USB)'),
    h('option', { value: 'file' }, 'Serial port or shared printer path'));
  kind.value = printer.kind;
  const host = h('input', { value: printer.host ?? '', placeholder: '192.168.1.50' });
  const port = h('input', { value: printer.port ?? 9100, inputmode: 'numeric', style: 'width:90px' });
  const winName = systemPrinters.length
    ? h('select', {}, systemPrinters.map((n) => h('option', { value: n }, n)))
    : h('input', { placeholder: 'Printer name as shown in Windows' });
  if (printer.name) {
    if (systemPrinters.length && !systemPrinters.includes(printer.name)) winName.append(h('option', { value: printer.name }, printer.name));
    winName.value = printer.name;
  }
  const path = h('input', { value: printer.path ?? '', placeholder: 'COM3 or \\\\PC-NAME\\Kitchen', style: 'width:260px' });
  const width = h('select', {}, h('option', { value: 42 }, '80 mm paper'), h('option', { value: 32 }, '58 mm paper'), h('option', { value: 48 }, '80 mm, small font (48)'));
  width.value = String(printer.width ?? 42);
  const fields = h('div', { class: 'row' });
  const drawFields = () => fields.replaceChildren(...{
    none: [],
    network: ['IP address', host, 'Port', port],
    windows: ['Printer', winName],
    file: ['Port or path', path],
  }[kind.value]);
  kind.onchange = drawFields;
  drawFields();
  const config = () => ({
    kind: kind.value, width: Number(width.value),
    ...(kind.value === 'network' && { host: host.value.trim(), port: Number(port.value) || 9100 }),
    ...(kind.value === 'windows' && { name: winName.value }),
    ...(kind.value === 'file' && { path: path.value.trim() }),
  });

  screen(h('div', { class: 'stack', style: 'max-width:760px' },
    h('div', { class: 'card stack' },
      h('h3', {}, 'Restaurant'),
      h('div', { class: 'row' }, store, h('button', { class: 'primary', onclick: act(async () => {
        await call('set_setting', { key: 'store_name', value: store.value });
        toast('Saved');
        await start();
      }) }, 'Save'))),
    h('div', { class: 'card stack' },
      h('h3', {}, 'Kitchen printer'),
      h('div', { class: 'muted' }, 'Tickets print on "Send to kitchen" on any ESC/POS thermal printer (Epson, Star, Xprinter…). ' +
        'For USB printers choose "installed in Windows"; for network printers use the IP address from its self-test page.'),
      h('div', { class: 'row' }, kind, width), fields,
      h('div', { class: 'row' },
        h('button', { onclick: act(async () => {
          if (kind.value === 'none') return toast('Choose a printer first', true);
          const r = await call('test_kitchen_printer', { config: config() });
          toast(r.error ? `Test failed: ${r.error}` : 'Test ticket sent', !!r.error);
        }) }, 'Print test ticket'),
        h('button', { class: 'primary', onclick: act(async () => {
          await call('set_kitchen_printer', { config: config() });
          toast('Kitchen printer saved');
        }) }, 'Save printer'))),
    h('div', { class: 'card stack' },
      h('h3', {}, 'Backup'),
      h('p', {}, 'A backup is made automatically once a day at startup (last 14 kept).'),
      h('div', {}, h('button', { onclick: act(async () => toast('Backup saved: ' + (await call('backup_now')))) }, 'Back up now')))));
}

start().catch((e) => toast(String(e), true));
