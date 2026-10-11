'use strict';
// Minimal placeholder UI. Every rule (prices, tax, permissions, stock) is
// enforced in Rust; this file only calls commands and draws results.

const invoke = window.__TAURI__.core.invoke;
const app = document.getElementById('app');
const state = { session: null, storeName: null, view: 'sell', cart: [], quote: null };

function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'value') el.value = v;
    else if (v === true) el.setAttribute(k, '');
    else if (v !== false && v != null) el.setAttribute(k, v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) el.append(c instanceof Node ? c : String(c));
  return el;
}
const money = (c) => (c / 100).toFixed(2);
function parseMoney(s) {
  s = String(s).trim();
  if (!/^\d+(\.\d{1,2})?$/.test(s)) throw new Error('enter an amount like 12.50');
  const [w, f = ''] = s.split('.');
  return Number(w) * 100 + Number(f.padEnd(2, '0'));
}
const pct = (bp) => (bp / 100).toString();
const fmtTime = (t) => new Date(t * 1000).toLocaleString();
const today = () => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};
function dayRange(iso) {
  const [y, m, d] = iso.split('-').map(Number);
  const from = new Date(y, m - 1, d).getTime() / 1000;
  return [from, new Date(y, m - 1, d + 1).getTime() / 1000];
}

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

function screen(...kids) {
  app.replaceChildren(...kids);
}

// ---------- start / setup / login ----------

async function start() {
  const st = await invoke('app_status');
  state.session = st.session;
  state.storeName = st.store_name;
  document.getElementById('store').textContent = st.store_name || 'POS';
  if (st.needs_setup) return renderSetup();
  if (!st.session) return renderLogin();
  renderShell();
}

function renderSetup() {
  document.getElementById('bar').hidden = true;
  const name = h('input', { placeholder: 'Your name', autofocus: true });
  const pin = h('input', { type: 'password', inputmode: 'numeric', placeholder: 'PIN (4-8 digits)' });
  const pin2 = h('input', { type: 'password', inputmode: 'numeric', placeholder: 'Repeat PIN' });
  const go = act(async () => {
    if (pin.value !== pin2.value) return toast('PINs do not match', true);
    state.session = await call('setup_admin', { name: name.value, pin: pin.value });
    renderShell();
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
  ['sell', 'Sell', false],
  ['sales', 'Sales', false],
  ['products', 'Products', true],
  ['users', 'Users', true],
  ['reports', 'Reports', true],
  ['settings', 'Settings', true],
];

function renderShell() {
  const isAdmin = state.session.role === 'admin';
  if (!VIEWS.some(([id, , adm]) => id === state.view && (!adm || isAdmin))) state.view = 'sell';
  document.getElementById('bar').hidden = false;
  document.getElementById('who').textContent = `${state.session.name} (${state.session.role})`;
  document.getElementById('nav').replaceChildren(...VIEWS.filter(([, , adm]) => !adm || isAdmin).map(([id, label]) =>
    h('button', { class: id === state.view ? 'on' : '', onclick: () => { state.view = id; renderShell(); } }, label)));
  ({ sell: renderSell, sales: renderSales, products: renderProducts, users: renderUsers,
     reports: renderReports, settings: renderSettings })[state.view]();
}

document.getElementById('logout').onclick = act(async () => {
  await call('logout');
  state.session = null;
  state.cart = [];
  state.quote = null;
  state.view = 'sell';
  renderLogin();
});

// ---------- sell ----------

function renderSell() {
  const scan = h('input', { placeholder: 'Scan barcode or search…', style: 'width:100%', autofocus: true });
  const results = h('div', { class: 'results' });
  const lines = h('div', { class: 'lines' });
  const totals = h('table', { class: 'totals' });
  const tendered = h('input', { placeholder: 'Cash tendered', inputmode: 'decimal', style: 'width:140px' });
  let found = [];
  let timer;

  const addProduct = (p) => {
    const line = state.cart.find((l) => l.product.id === p.id);
    if (line) line.qty += 1;
    else state.cart.push({ product: p, qty: 1, discount_cents: 0 });
    scan.value = '';
    results.replaceChildren();
    refreshCart();
    scan.focus();
  };

  const showResults = (list) => {
    found = list;
    results.replaceChildren(h('table', {}, list.map((p) =>
      h('tr', { style: 'cursor:pointer', onclick: () => addProduct(p) },
        h('td', {}, p.name), h('td', {}, p.barcode || p.sku || ''),
        h('td', { class: 'num' }, p.track_stock ? `stock ${p.stock_qty}` : ''),
        h('td', { class: 'num' }, money(p.price_cents))))));
  };

  scan.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(async () => showResults(await call('search_products', { query: scan.value, limit: 40 })), 120);
  });
  scan.addEventListener('keydown', act(async (e) => {
    if (e.key !== 'Enter' || !scan.value.trim()) return;
    clearTimeout(timer);
    const p = await call('find_by_code', { code: scan.value });
    if (p) return addProduct(p);
    const list = await call('search_products', { query: scan.value, limit: 40 });
    if (list.length === 1) return addProduct(list[0]);
    showResults(list);
    if (!list.length) toast('No product found', true);
  }));

  async function refreshCart() {
    lines.replaceChildren(h('table', {}, state.cart.map((l, i) => h('tr', {},
      h('td', {}, l.product.name),
      h('td', {},
        h('button', { onclick: () => { if (l.qty > 1) l.qty--; refreshCart(); } }, '−'),
        ` ${l.qty} `,
        h('button', { onclick: () => { l.qty++; refreshCart(); } }, '+')),
      h('td', {}, h('input', {
        placeholder: 'disc', style: 'width:64px', value: l.discount_cents ? money(l.discount_cents) : '',
        onchange: act(async (e) => { l.discount_cents = e.target.value ? parseMoney(e.target.value) : 0; await refreshCart(); }),
      })),
      h('td', { class: 'num' }, money(l.product.price_cents * l.qty)),
      h('td', {}, h('button', { class: 'danger', onclick: () => { state.cart.splice(i, 1); refreshCart(); } }, '×'))))));
    state.quote = null;
    if (state.cart.length) {
      state.quote = await call('quote_sale', { lines: cartLines() });
    }
    const q = state.quote || { subtotal_cents: 0, discount_cents: 0, tax_cents: 0, total_cents: 0 };
    totals.replaceChildren(
      h('tr', {}, h('td', {}, 'Subtotal'), h('td', { class: 'num' }, money(q.subtotal_cents))),
      h('tr', {}, h('td', {}, 'Discount'), h('td', { class: 'num' }, '-' + money(q.discount_cents))),
      h('tr', {}, h('td', {}, 'Tax'), h('td', { class: 'num' }, money(q.tax_cents))),
      h('tr', { class: 'grand' }, h('td', {}, 'Total'), h('td', { class: 'num' }, money(q.total_cents))));
  }

  const cartLines = () => state.cart.map((l) => ({ product_id: l.product.id, qty: l.qty, discount_cents: l.discount_cents }));

  const pay = (method) => act(async () => {
    if (!state.cart.length) return toast('Cart is empty', true);
    const tenderedCents = method === 'cash' ? (tendered.value ? parseMoney(tendered.value) : state.quote.total_cents) : 0;
    const sale = await call('complete_sale', { lines: cartLines(), method, tenderedCents });
    state.cart = [];
    tendered.value = '';
    await refreshCart();
    toast(`Sale #${sale.id} done. Total ${money(sale.total_cents)}` + (sale.change_cents ? `, change ${money(sale.change_cents)}` : ''));
    scan.focus();
  });

  screen(h('div', { class: 'sell' },
    h('div', { class: 'card' }, scan, results),
    h('div', { class: 'card cart' },
      lines, totals,
      h('div', { class: 'row' }, tendered,
        h('button', { class: 'primary', onclick: pay('cash') }, 'Cash'),
        h('button', { class: 'primary', onclick: pay('card') }, 'Card'),
        h('button', { onclick: () => { state.cart = []; refreshCart(); } }, 'Clear')))));
  refreshCart();
  scan.focus();
}

// ---------- sales ----------

function renderSales() {
  const date = h('input', { type: 'date', value: today() });
  const list = h('div');
  const detail = h('div');
  const load = act(async () => {
    const [from, to] = dayRange(date.value);
    const sales = await call('list_sales', { from, to, limit: 500, offset: 0 });
    list.replaceChildren(h('table', {},
      h('tr', {}, h('th', {}, '#'), h('th', {}, 'Time'), h('th', {}, 'Cashier'), h('th', {}, 'Paid'), h('th', {}, 'Status'), h('th', { class: 'num' }, 'Total')),
      sales.map((s) => h('tr', { style: 'cursor:pointer', onclick: act(() => showSale(s.id)) },
        h('td', {}, s.id), h('td', {}, fmtTime(s.created_at)), h('td', {}, s.user_name),
        h('td', {}, s.payment_method), h('td', {}, s.status), h('td', { class: 'num' }, money(s.total_cents))))));
  });
  async function showSale(id) {
    const s = await call('get_sale', { id });
    const reason = h('input', { placeholder: 'Void reason' });
    detail.replaceChildren(h('div', { class: 'card' },
      h('h3', {}, `Sale #${s.id} — ${s.status}`),
      h('div', {}, `${fmtTime(s.created_at)} · ${s.user_name} · ${s.payment_method}`),
      h('table', {}, s.items.map((i) => h('tr', {},
        h('td', {}, i.name), h('td', { class: 'num' }, `${i.qty} × ${money(i.unit_price_cents)}`),
        h('td', { class: 'num' }, i.discount_cents ? '-' + money(i.discount_cents) : ''),
        h('td', { class: 'num' }, money(i.line_total_cents))))),
      h('p', {}, `Tax ${money(s.tax_cents)} · Total ${money(s.total_cents)} · Tendered ${money(s.tendered_cents)} · Change ${money(s.change_cents)}`),
      s.void_reason && h('p', {}, `Voided: ${s.void_reason}`),
      state.session.role === 'admin' && s.status === 'completed' && h('div', { class: 'row' }, reason,
        h('button', { class: 'danger', onclick: act(async () => { await call('void_sale', { id, reason: reason.value }); toast('Sale voided'); await showSale(id); await load(); }) }, 'Void sale'))));
  }
  date.onchange = load;
  screen(h('div', { class: 'grid2' }, h('div', {}, h('div', { class: 'row' }, 'Day', date), list), detail));
  load();
}

// ---------- products (admin) ----------

function renderProducts() {
  const list = h('div');
  const form = h('div');
  const showInactive = h('input', { type: 'checkbox' });
  const load = act(async () => {
    const products = await call('list_products', { includeInactive: showInactive.checked, limit: 500, offset: 0 });
    list.replaceChildren(h('table', {},
      h('tr', {}, h('th', {}, 'Name'), h('th', {}, 'Barcode'), h('th', { class: 'num' }, 'Price'), h('th', { class: 'num' }, 'Tax %'), h('th', { class: 'num' }, 'Stock')),
      products.map((p) => h('tr', { style: 'cursor:pointer' + (p.active ? '' : ';opacity:.5'), onclick: () => edit(p) },
        h('td', {}, p.name), h('td', {}, p.barcode || p.sku || ''), h('td', { class: 'num' }, money(p.price_cents)),
        h('td', { class: 'num' }, pct(p.tax_rate_bp)), h('td', { class: 'num' }, p.track_stock ? p.stock_qty : '—')))));
  });
  function edit(p) {
    const f = {
      name: h('input', { value: p?.name ?? '', placeholder: 'Name' }),
      barcode: h('input', { value: p?.barcode ?? '', placeholder: 'Barcode' }),
      sku: h('input', { value: p?.sku ?? '', placeholder: 'SKU' }),
      price: h('input', { value: p ? money(p.price_cents) : '', placeholder: 'Price', inputmode: 'decimal' }),
      cost: h('input', { value: p ? money(p.cost_cents) : '0', placeholder: 'Cost', inputmode: 'decimal' }),
      tax: h('input', { value: p ? pct(p.tax_rate_bp) : '0', placeholder: 'Tax %', inputmode: 'decimal' }),
      stock: h('input', { value: '0', placeholder: 'Opening stock', inputmode: 'numeric' }),
      track: h('input', { type: 'checkbox', checked: p ? p.track_stock : true }),
      active: h('input', { type: 'checkbox', checked: p ? p.active : true }),
    };
    const data = () => ({
      name: f.name.value, barcode: f.barcode.value || null, sku: f.sku.value || null,
      price_cents: parseMoney(f.price.value), cost_cents: parseMoney(f.cost.value || '0'),
      tax_rate_bp: Math.round(Number(f.tax.value || 0) * 100), track_stock: f.track.checked,
    });
    const save = act(async () => {
      if (p) await call('update_product', { id: p.id, product: { ...data(), active: f.active.checked } });
      else await call('create_product', { product: { ...data(), stock_qty: parseInt(f.stock.value || '0', 10) } });
      toast('Saved');
      form.replaceChildren();
      await load();
    });
    const delta = h('input', { placeholder: '+/- qty', style: 'width:90px', inputmode: 'numeric' });
    const reason = h('input', { placeholder: 'Reason (delivery, count…)' });
    const adjust = act(async () => {
      await call('adjust_stock', { id: p.id, delta: parseInt(delta.value, 10) || 0, reason: reason.value });
      toast('Stock updated');
      form.replaceChildren();
      await load();
    });
    form.replaceChildren(h('div', { class: 'card' },
      h('h3', {}, p ? `Edit ${p.name}` : 'New product'),
      h('div', { class: 'row' }, f.name, f.barcode, f.sku),
      h('div', { class: 'row' }, 'Price', f.price, 'Cost', f.cost, 'Tax %', f.tax),
      h('div', { class: 'row' }, h('label', {}, f.track, ' Track stock'), p && h('label', {}, f.active, ' Active'), !p && ['Opening stock', f.stock]),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Save'), h('button', { onclick: () => form.replaceChildren() }, 'Cancel')),
      p && p.track_stock && h('div', {}, h('h4', {}, `Stock: ${p.stock_qty}`), h('div', { class: 'row' }, delta, reason, h('button', { onclick: adjust }, 'Adjust')))));
    f.name.focus();
  }
  showInactive.onchange = load;
  screen(h('div', { class: 'grid2' },
    h('div', {}, h('div', { class: 'row' }, h('button', { class: 'primary', onclick: () => edit(null) }, 'New product'), h('label', {}, showInactive, ' Show inactive')), list),
    form));
  load();
}

// ---------- users (admin) ----------

function renderUsers() {
  const list = h('div');
  const form = h('div');
  const roleSelect = () => h('select', {}, h('option', { value: 'staff' }, 'Staff'), h('option', { value: 'admin' }, 'Admin'));
  const load = act(async () => {
    const users = await call('list_users');
    list.replaceChildren(h('table', {},
      h('tr', {}, h('th', {}, 'Name'), h('th', {}, 'Role'), h('th', {}, 'Status'), h('th', {}, 'Last login')),
      users.map((u) => h('tr', { style: 'cursor:pointer', onclick: () => edit(u) },
        h('td', {}, u.name), h('td', {}, u.role), h('td', {}, u.active ? 'active' : 'disabled'),
        h('td', {}, u.last_login_at ? fmtTime(u.last_login_at) : '—')))));
  });
  function edit(u) {
    const name = h('input', { value: u?.name ?? '', placeholder: 'Name' });
    const role = roleSelect();
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
      await load();
      await start(); // own role/name may have changed
    });
    form.replaceChildren(h('div', { class: 'card' },
      h('h3', {}, u ? `Edit ${u.name}` : 'New user'),
      h('div', { class: 'row' }, name, role, u && h('label', {}, active, ' Active')),
      h('div', { class: 'row' }, pin),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Save'), h('button', { onclick: () => form.replaceChildren() }, 'Cancel'))));
    name.focus();
  }
  screen(h('div', { class: 'grid2' },
    h('div', {}, h('button', { class: 'primary', onclick: () => edit(null) }, 'New user'), list), form));
  load();
}

// ---------- reports (admin) ----------

function renderReports() {
  const date = h('input', { type: 'date', value: today() });
  const out = h('div');
  const load = act(async () => {
    const [from, to] = dayRange(date.value);
    const [s, top] = await Promise.all([
      call('sales_summary', { from, to }),
      call('top_products', { from, to, limit: 20 }),
    ]);
    out.replaceChildren(h('div', { class: 'grid2' },
      h('div', { class: 'card' }, h('h3', {}, 'Summary'), h('table', {},
        [['Sales', s.sale_count], ['Gross', money(s.subtotal_cents)], ['Discounts', money(s.discount_cents)],
         ['Tax', money(s.tax_cents)], ['Total', money(s.total_cents)],
         ['Voided', `${s.voided_count} (${money(s.voided_total_cents)})`],
         ...s.by_payment.map((p) => [`  ${p.method}`, `${p.count} · ${money(p.total_cents)}`])]
          .map(([k, v]) => h('tr', {}, h('td', {}, k), h('td', { class: 'num' }, v))))),
      h('div', { class: 'card' }, h('h3', {}, 'Top products'), h('table', {},
        top.map((t) => h('tr', {}, h('td', {}, t.name), h('td', { class: 'num' }, t.qty), h('td', { class: 'num' }, money(t.revenue_cents))))))));
  });
  date.onchange = load;
  screen(h('div', {}, h('div', { class: 'row' }, 'Day', date), out));
  load();
}

// ---------- settings (admin) ----------

function renderSettings() {
  const store = h('input', { value: state.storeName ?? '', placeholder: 'Store name' });
  screen(h('div', { class: 'card' },
    h('h3', {}, 'Store'),
    h('div', { class: 'row' }, store, h('button', { class: 'primary', onclick: act(async () => {
      await call('set_setting', { key: 'store_name', value: store.value });
      toast('Saved');
      await start();
    }) }, 'Save')),
    h('h3', {}, 'Backup'),
    h('p', {}, 'A backup is made automatically once a day at startup (last 14 kept).'),
    h('button', { onclick: act(async () => toast('Backup saved: ' + (await call('backup_now')))) }, 'Back up now')));
}

start().catch((e) => toast(String(e), true));
