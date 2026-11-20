'use strict';
// POS terminal UI. Every rule (prices, tax, permissions, stock, table
// occupancy) is enforced in Rust; this file only calls commands and draws.
// It runs in the desktop window and, through the optional local server, in
// any browser on the network.

const TAURI = window.__TAURI__?.core;
/// Commands only the desktop window has (they act on this PC itself).
const DESKTOP_ONLY = new Set(['check_update', 'install_update', 'server_status', 'set_server', 'toggle_fullscreen', 'open_backup_folder']);
/// A browser's own random id: its login belongs to this id only.
const terminalId = (() => {
  const make = () => [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, '0')).join('');
  try {
    let id = localStorage.getItem('pos-terminal');
    if (!id) localStorage.setItem('pos-terminal', (id = make()));
    return id;
  } catch { return make(); }
})();
/// Anything sent for the empty order on screen (a dish, details…) means it
/// is no longer to be dropped, even before the server has answered.
const ORDER_READS = new Set(['get_order', 'cancel_empty_order']);
async function invoke(cmd, args = {}) {
  if (args.orderId != null && args.orderId === state.orderEmpty && !ORDER_READS.has(cmd)) state.orderEmpty = null;
  if (TAURI) return DESKTOP_ONLY.has(cmd) ? TAURI.invoke(cmd, args) : TAURI.invoke('call', { cmd, args });
  let r;
  try {
    r = await fetch(`/api/${cmd}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Terminal': terminalId }, body: JSON.stringify(args),
    });
  } catch {
    throw 'Cannot reach the POS. Check the main till is on and its server is running.';
  }
  const body = await r.json().catch(() => null);
  if (!r.ok) throw body || `server error ${r.status}`;
  return body;
}
const app = document.getElementById('app');
const state = {
  session: null, storeName: null, plan: { w: 1000, h: 700 },
  view: 'floor', floorId: null, orderId: null, returnTo: 'floor',
  menu: null, cats: null, catId: null, planDirty: false,
};
const TYPE = { dine_in: 'Dine-in', takeaway: 'Takeaway', collection: 'Collection', delivery: 'Delivery' };
const typeChip = (t) => h('span', { class: `chip type-${t}` }, TYPE[t]);

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
/// Replace an element's children, skipping null/false like h() does.
function setKids(el, ...kids) {
  el.replaceChildren(...kids.flat(Infinity).filter((c) => c != null && c !== false).map((c) => (c instanceof Node ? c : String(c))));
}
/// Line icons (24px grid, stroke = currentColor).
const ICONS = {
  keyboard: '<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/>',
  floor: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  orders: '<path d="M5 3h14v18l-3-2-2 2-2-2-2 2-2-2-3 2z"/><path d="M9 8h6M9 12h6M9 16h3"/>',
  history: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  customers: '<circle cx="9" cy="8" r="4"/><path d="M1 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1"/><path d="M16 3.5a4 4 0 0 1 0 8M19 14a6 6 0 0 1 4 6v1"/>',
  menuAdmin: '<path d="M2 4h6a4 4 0 0 1 4 4v13a3 3 0 0 0-3-3H2z"/><path d="M22 4h-6a4 4 0 0 0-4 4v13a3 3 0 0 1 3-3h7z"/>',
  plan: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>',
  users: '<circle cx="12" cy="8" r="4"/><path d="M4 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1"/>',
  reports: '<path d="M18 20V10M12 20V4M6 20v-6"/>',
  settings: '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>',
  rail: '<path d="M4 6h16M4 12h16M4 18h16"/>',
  logout: '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><path d="M16 17l5-5-5-5M21 12H9"/>',
  back: '<path d="M15 18l-6-6 6-6"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/>',
  printer: '<path d="M6 9V2h12v7"/><rect x="2" y="9" width="20" height="9" rx="2"/><path d="M6 14h12v8H6z"/>',
  bag: '<path d="M6 7h12l1 14H5z"/><path d="M9 7a3 3 0 0 1 6 0"/>',
  bike: '<circle cx="5.5" cy="17.5" r="3.5"/><circle cx="18.5" cy="17.5" r="3.5"/><path d="M15 6h3l3 11.5M5.5 17.5L9 9h6l-3 8.5h3.5"/>',
  walk: '<circle cx="13" cy="4" r="2"/><path d="M10 22l2-7 3 3v4M7 12l3-4 4 1 3 3"/>',
  drawer: '<rect x="2" y="13" width="20" height="8" rx="2"/><path d="M5 13l2-9h10l2 9M10 17h4"/>',
  wifi: '<path d="M2 8.8a15 15 0 0 1 20 0M5 12.6a10 10 0 0 1 14 0M8.5 16.4a5 5 0 0 1 7 0"/><circle cx="12" cy="20" r="1"/>',
  check: '<path d="M20 6L9 17l-5-5"/>',
  expand: '<path d="M8 3H3v5M16 3h5v5M8 21H3v-5M16 21h5v-5"/>',
  store: '<path d="M3 9l2-5h14l2 5"/><path d="M3 9h18v2a3 3 0 0 1-6 0 3 3 0 0 1-6 0 3 3 0 0 1-6 0z"/><path d="M5 13v8h14v-8M10 21v-5h4v5"/>',
  receipt: '<path d="M6 2h12v20l-3-2-3 2-3-2-3 2z"/><path d="M9 7h6M9 11h6M9 15h4"/>',
  chef: '<path d="M6 14a4 4 0 0 1-1-7.9A5 5 0 0 1 12 3a5 5 0 0 1 7 3.1A4 4 0 0 1 18 14v6H6z"/><path d="M6 17h12"/>',
  cup: '<path d="M5 8h12v6a6 6 0 0 1-12 0z"/><path d="M17 10h2a2 2 0 0 1 0 4h-2M8 2v3M12 2v3"/>',
  lock: '<rect x="4" y="10" width="16" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>',
  monitor: '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>',
  database: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7h.01"/>',
  tag: '<path d="M3 3h8l10 10-8 8L3 11z"/><circle cx="7.5" cy="7.5" r="1.5"/>',
  shift: '<rect x="2" y="6" width="20" height="12" rx="2"/><circle cx="12" cy="12" r="3"/><path d="M6 12h.01M18 12h.01"/>',
  kitchen: '<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4M7 9h4M7 12h7"/>',
  move: '<path d="M5 9l-3 3 3 3M9 5l3-3 3 3M15 19l-3 3-3-3M19 9l3 3-3 3M2 12h20M12 2v20"/>',
  x: '<path d="M18 6L6 18M6 6l12 12"/>',
  card: '<rect x="2" y="5" width="20" height="14" rx="2"/><path d="M2 10h20M6 15h4"/>',
  cash: '<rect x="2" y="6" width="20" height="12" rx="2"/><circle cx="12" cy="12" r="2.5"/><path d="M6 12h.01M18 12h.01"/>',
  user: '<circle cx="12" cy="8" r="4"/><path d="M4 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
  moon: '<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/>',
  help: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9a2.5 2.5 0 0 1 4.9.8c0 1.7-2.4 2.2-2.4 3.7M12 17h.01"/>',
};

// ---------- light / dark theme (per device) ----------

/// "light", "dark" or "auto" (follow the device), kept in this browser only:
/// a till by a bright window and a tablet in a dark bar can differ.
const THEME_KEY = 'pos.theme';
function themePref() {
  try { return ['light', 'dark'].includes(localStorage.getItem(THEME_KEY)) ? localStorage.getItem(THEME_KEY) : 'auto'; } catch { return 'auto'; }
}
const darkQuery = matchMedia('(prefers-color-scheme: dark)');
function applyTheme() {
  const p = themePref();
  const dark = p === 'dark' || (p === 'auto' && darkQuery.matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  document.querySelector('meta[name=theme-color]')?.setAttribute('content', dark ? '#10151f' : '#ffffff');
  for (const b of document.querySelectorAll('.theme-btn')) {
    b.replaceChildren(icon(dark ? 'sun' : 'moon'));
    b.title = dark ? 'Switch to light mode' : 'Switch to dark mode';
    b.setAttribute('aria-label', b.title);
  }
}
function setTheme(p) {
  try { if (p === 'auto') localStorage.removeItem(THEME_KEY); else localStorage.setItem(THEME_KEY, p); } catch {}
  applyTheme();
}
/// The moon / sun button: one tap swaps light and dark.
const toggleTheme = () => setTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark');
darkQuery.addEventListener?.('change', applyTheme);
function icon(name, size = 22) {
  const el = h('span', { class: 'icon' });
  el.innerHTML = `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] ?? ''}</svg>`;
  return el;
}
/// Text colour that stays readable on a user-chosen category colour.
const onColor = (hex) => {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex || '');
  if (!m) return '#fff';
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16) / 255)
    .map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.2 ? '#101828' : '#fff';
};
/// Inline style giving an element its category colour (--c) and the text colour to use on it (--on).
const catStyle = (color) => `--c:${color || '#98a2b3'};--on:${onColor(color || '#98a2b3')}`;
const logoImg = () => h('img', { src: 'icon.png', alt: '' });
const initials = (name) => name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0].toUpperCase()).join('');
const money = (c) => (c / 100).toFixed(2);
/// Money as people type it: "12", "12.5", "12,50", "£12.50", " 12.50 ".
function parseMoney(s) {
  s = String(s).replace(/[£$€\s]/g, '');
  if (s.includes(',') && !s.includes('.')) s = s.replace(',', '.');
  else s = s.replace(/,/g, '');
  if (!/^\d+(\.\d{1,2})?$/.test(s) && !/^\.\d{1,2}$/.test(s)) throw new Error('enter a price like 12.50');
  const [w, f = ''] = s.split('.');
  return Number(w || 0) * 100 + Number(f.padEnd(2, '0'));
}

/// Drag to reorder, with mouse or finger: press on `handle` inside an
/// `item` and move. `onDrop(ids)` gets the new order of `data-id`s.
function sortable(container, { item, handle, axis = 'y', onDrop }) {
  container.addEventListener('pointerdown', (e) => {
    const grip = e.target.closest(handle);
    if (!grip || !container.contains(grip)) return;
    const el = grip.closest(item);
    if (!el) return;
    e.preventDefault();
    const order = () => [...el.parentNode.children].filter((x) => x.matches(item)).map((x) => Number(x.dataset.id));
    const before = order().join();
    el.classList.add('dragging');
    // Listen on the whole window: moving the row in the page drops pointer
    // capture, so listeners on the handle would miss the rest of the drag.
    const move = (m) => {
      if (m.pointerId !== e.pointerId) return;
      const over = document.elementFromPoint(m.clientX, m.clientY)?.closest(item);
      if (!over || over === el || over.parentNode !== el.parentNode) return;
      const r = over.getBoundingClientRect();
      const after = axis === 'y' ? m.clientY > r.top + r.height / 2 : m.clientX > r.left + r.width / 2;
      over.parentNode.insertBefore(el, after ? over.nextSibling : over);
    };
    const up = (u) => {
      if (u.pointerId !== e.pointerId) return;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
      el.classList.remove('dragging');
      el.dataset.dragged = '1';
      setTimeout(() => delete el.dataset.dragged, 0);
      const now = order();
      if (now.join() !== before) onDrop(now);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
  });
}
/// A discount as typed: an amount ("2.50") or a percentage of `base` cents ("10%").
function parseDiscount(s, base) {
  const m = /^\s*(\d+(?:[.,]\d+)?)\s*%\s*$/.exec(String(s));
  if (!m) return parseMoney(s);
  const p = Number(m[1].replace(',', '.'));
  if (p > 100) throw new Error('at most 100%');
  return Math.round(base * p / 100);
}
function parseSignedMoney(s) {
  s = String(s).trim();
  const neg = s.startsWith('-');
  return (neg ? -1 : 1) * parseMoney(s.replace(/^[+-]/, '') || '0');
}
const signed = (c) => (c > 0 ? '+' : c < 0 ? '-' : '') + money(Math.abs(c));
const pct = (bp) => (bp / 100).toString();
const fmtTime = (t) => new Date(t * 1000).toLocaleString();
/// Minutes as a short age: "12 min", "2 h 10 m", "1 d 19 h".
function ageText(mins) {
  if (mins < 60) return `${mins} min`;
  if (mins < 1440) return `${Math.floor(mins / 60)} h ${mins % 60} m`;
  return `${Math.floor(mins / 1440)} d ${Math.floor((mins % 1440) / 60)} h`;
}
const ago = (t) => ageText(Math.max(0, Math.floor((Date.now() / 1000 - t) / 60)));
/// A time for headers: "today 09:00", "yesterday 23:10" or "18 Nov 09:00" (in the device's own style).
function friendlyTime(t) {
  const d = new Date(t * 1000);
  const hm = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const day = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(new Date()) - day(d)) / 86400000);
  if (diff === 0) return `today ${hm}`;
  if (diff === 1) return `yesterday ${hm}`;
  return `${d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })} ${hm}`;
}
const isoDate = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
const today = () => isoDate(new Date());
function dayRange(iso) {
  const [y, m, d] = iso.split('-').map(Number);
  return [new Date(y, m - 1, d).getTime() / 1000, new Date(y, m - 1, d + 1).getTime() / 1000];
}
const isAdmin = () => state.session?.role === 'admin';
/// Admins and managers (menu, floor plan, kitchen settings).
const isManager = () => ['admin', 'manager'].includes(state.session?.role);
const ROLE_LABEL = { admin: 'Admin', manager: 'Manager', staff: 'Staff' };
/// May this user open a page that needs `need` (false, 'manager' or 'admin')?
const canSee = (need) => !need || (need === 'manager' ? isManager() : isAdmin());
/// "2026-10-12" -> "Mon 12 Oct"
function dayLabel(iso) {
  const [y, m, d] = iso.split('-').map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
}
/// Show the open business day in the header.
async function drawBusinessDay() {
  try {
    state.day = await invoke('business_day');
    document.getElementById('bizday').textContent = dayLabel(state.day.date);
  } catch { /* logged out */ }
}
/// An order's name as staff say it: "Table 4" (a plain number gets the word),
/// "Window" for a named table, else the platform or customer, else the order type.
const orderTitle = (o) => (o.order_type === 'dine_in'
  ? (/^\d+[a-z]?$/i.test(o.table_name ?? '') ? `Table ${o.table_name}` : o.table_name)
  : where(o) || TYPE[o.order_type]);
const where = (o) => o.order_type === 'dine_in'
  ? `${o.table_name}${o.floor_name ? ' · ' + o.floor_name : ''}`
  : [o.channel_name && `${o.channel_name}${o.external_ref ? ' ' + o.external_ref : ''}`, o.customer_name]
    .filter(Boolean).join(' · ');
const addressText = (c) => [c.address_line1, c.address_line2, c.town, c.postcode].filter(Boolean).join(', ');

let toastTimer;
/// `ms`: how long it stays (longer messages need longer).
function toast(msg, err = false, ms = err ? 4000 : 2500) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.className = err ? 'err' : '';
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), ms);
}

/// Saving a setting can change the order screen ("Starter or Main?", tax,
/// service charge…): the next order screen loads the menu and settings again.
const SETTINGS_WRITES = new Set(['set_setting', 'set_service_settings', 'set_receipt_settings', 'save_teams']);
/// The server's answer for anything done to an order that is no longer open (orders.rs `require_open`).
const ORDER_CLOSED = 'this order is already closed';
/// True when `msg` (from a call made on the order screen) means the order on
/// screen is gone: paid, cancelled, or merged into another bill on another till.
function isDeadOrder(msg, args = {}, cmd = null) {
  if (!state.orderOpen || state.orderId == null) return false;
  if (msg === ORDER_CLOSED) return true;
  // Joined onto another bill or emptied by a move: the order no longer exists.
  return msg === 'not found' && (args.orderId === state.orderId || (cmd === 'get_order' && args.id === state.orderId));
}
/// "No permission" for the order on screen: a closed order is hidden from
/// staff who did not open or pay it, while an open one never is. Ask again
/// to tell a dead order from a real refusal.
async function deniedForDeadOrder(msg, args, cmd) {
  if (!state.orderOpen || state.orderId == null || !/permission/i.test(msg)) return false;
  if (args.orderId !== state.orderId && !(cmd === 'get_order' && args.id === state.orderId)) return false;
  try { return (await invoke('get_order', { id: state.orderId })).status !== 'open'; } catch { return true; }
}
/// Leave a dead order instead of failing on every tap.
function leaveDeadOrder() {
  toast('This order was closed on another till', true);
  state.onModalClose = null;
  closeModal();
  state.orderEmpty = null;
  closeOrder();
}
async function call(cmd, args = {}) {
  try {
    const r = await invoke(cmd, args);
    if (SETTINGS_WRITES.has(cmd)) { state.menu = null; loadUiSettings(); }
    return r;
  } catch (e) {
    const msg = String(e?.message ?? e);
    // Logged out on the server (idle, or a backup was restored on any till):
    // forget everything from the old session before showing the login.
    if (msg === 'please log in') { forgetSession(); start(); }
    else if (isDeadOrder(msg, args, cmd) || await deniedForDeadOrder(msg, args, cmd)) leaveDeadOrder();
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
  // Pages opened from Manage get a way back to it.
  const back = state.session && !state.orderId && inManage(state.view) && h('div', { class: 'manage-back' },
    h('button', { class: 'back-btn', onclick: () => go('settings') }, icon('back', 22), 'Manage'),
    MANAGE_SUB[state.view] && h('span', { class: 'page-sub' }, MANAGE_SUB[state.view]));
  setKids(app, back, ...kids);
}
// Poll a live view (floor, open orders). Each refresh is well under 1 ms.
function live(fn, ms = 10000) {
  refreshTimer = setInterval(() => fn().catch(() => {}), ms);
}

// ---------- start / setup / login ----------

/// Drop everything remembered from the session and its data (as logging
/// out does): the order on screen, the menu and what comes with it, the
/// floor, the open settings page. The next screens load them again.
function forgetSession() {
  closeModal();
  Object.assign(state, {
    session: null, orderId: null, orderOpen: false, orderEmpty: null, view: 'floor', returnTo: 'floor', floorId: null,
    menu: null, cats: null, catId: null, subId: null, itemGroups: null, itemVariants: null, service: null, defaultTax: null,
    quickNotes: null, starterChoice: null, menuTab: null, menuShown: null, newSubOf: null, openEditor: null, settingsTab: null,
    planDirty: false, day: null,
  });
}

/// The data on the main till was replaced (a backup was restored): reload
/// the page so nothing from the old data survives anywhere. True if so.
function dataChanged(st) {
  if (state.dbId && st?.db_id && st.db_id !== state.dbId) {
    location.reload();
    return true;
  }
  if (st?.db_id) state.dbId = st.db_id;
  return false;
}
/// "Restored backup…": shown again after the reload that follows a restore.
const FLASH_KEY = 'pos.flash';

async function start() {
  const st = await invoke('app_status');
  if (dataChanged(st)) return;
  state.session = st.session;
  state.storeName = st.store_name;
  state.plan = { w: st.plan_width, h: st.plan_height };
  state.version = st.version;
  state.remote = st.remote;
  document.title = st.store_name || 'POS';
  if (st.needs_setup) return renderSetup();
  if (!st.session) return renderLogin();
  await loadUiSettings();
  renderShell();
}

/// Full-screen layout for setup and login (no side rail).
function lockScreen(...kids) {
  document.body.classList.add('locked');
  clearInterval(state.clockTimer);
  const clock = h('div', { class: 'lock-clock' });
  const date = h('div', { class: 'lock-date' });
  const tick = () => {
    const d = new Date();
    clock.textContent = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    date.textContent = d.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
  };
  tick();
  state.clockTimer = setInterval(tick, 10000);
  screen(h('div', { class: 'lock' },
    h('div', { class: 'lock-side' },
      h('div', { class: 'brand-mark' }, logoImg()),
      h('div', { class: 'lock-store' }, state.storeName || 'POS'),
      clock, date,
      h('div', { class: 'lock-foot' }, h('span', { class: 'grow' }, state.remote ? 'Connected to the main till' : `Version ${state.version ?? ''}`),
        h('button', { class: 'ghost icon-btn theme-btn', onclick: toggleTheme }))),
    h('div', { class: 'lock-main' }, ...kids)));
  applyTheme();
}

function renderSetup() {
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
  lockScreen(h('div', { class: 'lock-card stack' },
    h('h2', {}, 'Welcome'), h('p', { class: 'muted' }, 'Create the admin account to get started.'),
    name, pin, pin2, h('button', { class: 'primary lg', onclick: go }, 'Create admin')));
  name.focus();
}

/// Log in: tap your name, then type your PIN (two people may share a PIN).
/// Typing a PIN straight away on a keyboard still works when it is unique.
function renderLogin() {
  let pin = '';
  let user = null;
  let users = [];
  let autoOn = true;
  const dots = h('div', { class: 'pindots' });
  const main = h('div', { class: 'lock-stack' });
  const show = (err = false) => {
    setKids(dots, ...Array.from({ length: Math.max(user?.pin_len ?? 4, pin.length) }, (_, i) => h('span', { class: i < pin.length ? 'on' : '' })));
    dots.classList.toggle('shake', err);
  };
  const submit = act(async () => {
    const p = pin;
    pin = '';
    show();
    try {
      state.session = await call('login', { pin: p, userId: user?.id ?? null });
    } catch (e) {
      show(true);
      throw e;
    }
    try { localStorage.setItem('pos.lastUser', String(state.session.user_id)); } catch {}
    document.removeEventListener('keydown', onKey);
    await openStartScreen();
  });
  const press = (k) => {
    if (k === 'C') pin = '';
    else if (k === '⌫') pin = pin.slice(0, -1);
    else if (k === 'OK') return submit();
    else if (pin.length < 8) pin += k;
    show();
    // Log in as soon as the picked person's PIN is complete, when Settings allow it.
    if (/^\d$/.test(k) && autoOn && user?.pin_len && pin.length === user.pin_len) submit();
  };
  const onKey = (e) => {
    if (/^\d$/.test(e.key)) { if (!user && users.length) pinStep(null); press(e.key); }
    else if (e.key === 'Backspace') press('⌫');
    else if (e.key === 'Enter') press('OK');
    else if (e.key === 'Escape') { if (pin) press('C'); else if (users.length) pickStep(); }
  };
  document.removeEventListener('keydown', state.pinKeys);
  state.pinKeys = onKey;
  document.addEventListener('keydown', onKey);
  const closeBtn = () => TAURI && !state.remote && confirmBtn('Close POS', 'Tap again to close', async () => { await TAURI.invoke('quit_app'); }, 'quit');

  function pickStep() {
    user = null; pin = '';
    let last = null;
    try { last = Number(localStorage.getItem('pos.lastUser')); } catch {}
    setKids(main,
      h('div', { class: 'lock-card who-card' },
        h('h2', {}, 'Who are you?'),
        h('div', { class: 'who-grid' }, users.map((u) => h('button', { class: `who-tile ${u.id === last ? 'last' : ''}`, onclick: () => pinStep(u) },
          h('span', { class: 'avatar' }, initials(u.name)), h('strong', {}, u.name),
          h('small', {}, ROLE_LABEL[u.role] ?? ''),
          u.id === last && h('span', { class: 'last-tag' }, 'Last here'))))),
      closeBtn());
  }
  function pinStep(u) {
    user = u; pin = '';
    const keys = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '⌫', '0', 'OK'];
    show();
    // ⌫ deletes one digit; holding it clears the whole PIN.
    const backKey = () => {
      let timer, long = false;
      const stop = () => clearTimeout(timer);
      return h('button', { class: 'ghost', title: 'Delete (hold to clear)', 'aria-label': 'Delete',
        onpointerdown: () => { long = false; stop(); timer = setTimeout(() => { long = true; press('C'); }, 600); },
        onpointerup: stop, onpointerleave: stop, onpointercancel: stop,
        onclick: () => { if (long) long = false; else press('⌫'); } }, '⌫');
    };
    setKids(main,
      h('div', { class: 'lock-card' },
        u ? h('div', { class: 'who-picked' },
          h('span', { class: 'avatar' }, initials(u.name)), h('h2', {}, u.name)) : h('h2', {}, 'Enter your PIN'),
        dots,
        h('div', { class: 'pinpad' }, keys.map((k) => k === '⌫' ? backKey()
          : h('button', { class: k === 'OK' ? 'go' : '', onclick: () => press(k) }, k))),
        users.length > 0 && h('button', { class: 'ghost', onclick: pickStep }, icon('back', 18), 'Not you? Pick another name')),
      closeBtn());
  }
  lockScreen(main);
  invoke('app_status').then((st) => { if (!dataChanged(st)) autoOn = st.auto_pin_on !== false; }).catch(() => {});
  invoke('login_users').then((list) => {
    users = list;
    if (users.length) pickStep(); else pinStep(null);
  }).catch(() => pinStep(null));
  pinStep(null);
}

// ---------- settings the screens read (start screen, order types, sizes…) ----------

/// Settings every screen may need, read once per screen change in one call.
/// Not set = the defaults in uiSettings(), which are how the app always worked.
const UI_KEYS = ['auto_logout_minutes', 'auto_logout_warn_secs', 'start_screen', 'show_takeaway', 'show_collection', 'show_delivery',
  'button_size', 'discount_presets', 'discount_staff_max_pct', 'pay_name_cash', 'pay_name_card'];
const DISCOUNT_PRESETS_DEFAULT = [5, 10, 15, 20];
const START_SCREENS = [['tables', 'Tables'], ['orders', 'Orders'], ['takeaway', 'New takeaway']];
function uiSettings(raw = {}) {
  const whole = (v, lo, hi, def) => { const n = Number(String(v ?? '').trim()); return v != null && String(v).trim() !== '' && Number.isInteger(n) && n >= lo && n <= hi ? n : def; };
  const presets = raw.discount_presets == null ? DISCOUNT_PRESETS_DEFAULT
    : String(raw.discount_presets).split(/[,;\s]+/).map((x) => Number(x.replace('%', ''))).filter((n) => Number.isInteger(n) && n >= 1 && n <= 100).slice(0, 8);
  return {
    autoLogout: parseAutoLogout(raw.auto_logout_minutes),
    warnSecs: whole(raw.auto_logout_warn_secs, 0, 120, AUTO_LOGOUT_WARN_DEFAULT),
    startScreen: START_SCREENS.some(([v]) => v === raw.start_screen) ? raw.start_screen : 'tables',
    types: { takeaway: raw.show_takeaway !== '0', collection: raw.show_collection !== '0', delivery: raw.show_delivery !== '0' },
    bigButtons: raw.button_size === 'large',
    discountPresets: presets,
    /// Staff need a manager's PIN for more than this percentage off a line; null = no limit.
    discountLimit: whole(raw.discount_staff_max_pct, 0, 100, null),
    /// What Cash and Card are called on screen (Settings → Payments); stored as "cash" / "card".
    payNames: { cash: String(raw.pay_name_cash ?? '').trim() || 'Cash', card: String(raw.pay_name_card ?? '').trim() || 'Card' },
  };
}
/// A payment method as shown: "cash" / "card" by their names in Settings, "other" as the platform.
function payName(method, platform = null) {
  const names = (state.ui ?? uiSettings()).payNames;
  return method === 'cash' || method === 'card' ? names[method] : method === 'other' ? (platform || 'Other') : method;
}
async function loadUiSettings() {
  try {
    state.ui = uiSettings(await invoke('get_settings', { keys: UI_KEYS }));
    state.autoLogout = state.ui.autoLogout;
  } catch { state.ui ??= uiSettings(); }
  document.body.classList.toggle('big-buttons', state.ui.bigButtons);
}
/// New orders of `type` can be started (Settings → Orders → Order types). Dine-in always.
const typeShown = (type) => type === 'dine_in' || (state.ui ?? uiSettings()).types[type] !== false;

/// Right after login: Tables, Orders or a new takeaway order (Settings → Login & staff).
async function openStartScreen() {
  await loadUiSettings();
  const pick = state.ui.startScreen;
  state.view = pick === 'tables' ? 'floor' : 'orders';
  if (pick === 'takeaway' && typeShown('takeaway')) {
    try {
      const o = await invoke('open_order', { order: { order_type: 'takeaway' } });
      return openOrder(o.id);
    } catch (e) { toast(String(e?.message ?? e), true); }
  }
  renderShell();
}

// ---------- shell ----------

/// [id, label, who may open it (false = everyone, 'manager' or 'admin'), on the side bar].
/// The rest open from Manage. Staff only get the Kitchen screen there (and About).
const VIEWS = [
  ['floor', 'Tables', false, true],
  ['orders', 'Orders', false, true],
  ['history', 'History', 'manager'],
  ['customers', 'Customers', 'manager'],
  ['reports', 'Reports', 'manager'],
  ['settings', 'Manage', false, true],
  ['shift', 'Shift & cash', 'manager'],
  ['kitchen', 'Kitchen screen', false],
  ['menuAdmin', 'Menu', 'manager'],
  ['plan', 'Floor plan', 'manager'],
  ['users', 'Staff', 'admin'],
];
/// What each Manage tile under "Daily work" says.
const MANAGE_SUB = {
  history: 'Paid orders, reprint, void', shift: 'Float, money in/out, count the drawer', kitchen: 'Dishes to cook on a tablet, tap when done', customers: 'Saved customers and addresses', reports: 'Sales, close the day',
  menuAdmin: 'Items, categories, sets, prices', plan: 'Floors and tables', users: 'Staff, PINs, roles',
};
/// Settings areas on the Manage page (admin only); the cards are in renderSettings.
const SETTINGS_GROUPS = [
  ['shop', 'store', 'Restaurant', 'Name, service charge, tax'],
  ['bills', 'receipt', 'Bills & receipts', 'Receipt printer, cash drawer, words on bills, tips'],
  ['kitchen', 'printer', 'Kitchen printer', 'Printer, copies, font, kitchen screen'],
  ['tickets', 'cup', 'Drinks & desserts', 'Own tickets on or off'],
  ['orders', 'orders', 'Orders', 'Order types, discounts, cancelling, quick notes'],
  ['teams', 'chef', 'Kitchen teams', 'Starter, Main, Curry, Rice…'],
  ['login', 'lock', 'Login & staff', 'Start screen, auto logout, PIN'],
  ['delivery', 'bike', 'Delivery apps', 'Uber Eats, Just Eat, Deliveroo…'],
  ['screen', 'monitor', 'Screen & devices', 'Button size, full screen, phones and tablets'],
  ['data', 'database', 'Backup & data', 'Backups, menu copy, reset'],
  ['about', 'info', 'About & updates', ''],
];
const inManage = (view) => !VIEWS.find(([id]) => id === view)?.[3];

function go(view) {
  if (state.planDirty) return toast('Save or discard your floor plan changes first', true);
  if (view === 'settings') state.settingsTab = null; // the settings menu first
  state.view = view;
  const left = state.orderId;
  state.orderId = null;
  dropEmptyOrder(left).finally(renderShell);
}

// ---------- automatic updates ----------

let updateTimer;
/// Check GitHub Releases for a newer signed version; show it in the header.
/// Admins only (the server refuses everyone else). Silent when offline.
/// Returns the check result (or null on failure).
async function checkForUpdate(quiet = true) {
  if (!isAdmin()) return null;
  let info;
  try {
    info = await invoke('check_update');
  } catch (e) {
    if (!quiet) toast(String(e?.message ?? e), true);
    return null;
  }
  state.update = info;
  drawUpdateBar();
  return info;
}

/// Header bar for an available update, with its install button: admins
/// only. Managers and staff see nothing about updates.
function drawUpdateBar() {
  const bar = document.getElementById('update');
  const v = state.update?.available;
  bar.hidden = !v || !isAdmin();
  if (v && isAdmin()) setKids(bar, `Update ${v} available `, h('button', { class: 'primary', onclick: installUpdate }, 'Install update'));
}

const installUpdate = act(async (ev) => {
  const btn = ev?.target;
  if (btn) { btn.disabled = true; btn.textContent = 'Installing…'; }
  toast('Downloading the update. The app will close and reopen by itself in a moment.');
  try {
    await call('install_update');
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = 'Install update'; }
  }
});

const UPDATE_EVERY = 6 * 60 * 60 * 1000;
let lastUpdateCheck = 0, updateChecking = false;
/// Check now if an admin is logged in and the last check is over 6 hours
/// old; then look again every hour (nothing happens while others are logged in).
function startUpdateChecks() {
  if (!updateTimer) updateTimer = setInterval(startUpdateChecks, 60 * 60 * 1000);
  if (!isAdmin() || updateChecking || Date.now() - lastUpdateCheck < UPDATE_EVERY) return;
  // Only a check that got an answer counts: offline, it tries again next hour.
  updateChecking = true;
  checkForUpdate().then((info) => { if (info) lastUpdateCheck = Date.now(); }).finally(() => { updateChecking = false; });
}

function renderShell() {
  document.body.classList.remove('locked');
  lastActivity = Date.now();
  loadUiSettings();
  clearInterval(state.clockTimer);
  if (!state.remote) startUpdateChecks();
  // A page this user may not open goes back to Manage (or Tables if it is not a page at all).
  if (!VIEWS.some(([id, , need]) => id === state.view && canSee(need))) {
    state.view = VIEWS.some(([id]) => id === state.view) ? 'settings' : 'floor';
    state.settingsTab = null;
  }
  document.getElementById('bar').hidden = false;
  setKids(document.getElementById('brand'), logoImg());
  document.getElementById('brand').title = state.storeName || 'POS';
  const navOn = inManage(state.view) ? 'settings' : state.view;
  document.getElementById('nav').replaceChildren(...VIEWS.filter(([, , need, rail]) => rail && canSee(need)).map(([id, label]) =>
    h('button', { class: !state.orderId && id === navOn ? 'on' : '', onclick: () => go(id), title: label }, icon(id), h('span', {}, label))));
  const name = VIEWS.find(([id]) => id === state.view)?.[1];
  drawTitle(state.orderId ? 'Order' : name, !state.orderId && inManage(state.view) ? 'Manage' : null);
  document.getElementById('who').replaceChildren(h('span', { class: 'avatar' }, initials(state.session.name)),
    h('span', { class: 'who-text' }, h('strong', {}, state.session.name), h('small', {}, ROLE_LABEL[state.session.role] ?? 'Staff')));
  document.getElementById('mode').hidden = !state.remote;
  drawUpdateBar();
  drawBusinessDay();
  drawClock();
  app.classList.toggle('fill', !!state.orderId);
  const view = state.orderId ? renderOrder : {
    floor: renderFloor, orders: renderOrders, history: renderHistory, menuAdmin: renderMenuAdmin,
    plan: renderPlan, users: renderUsers, reports: renderReports, settings: renderSettings,
    customers: renderCustomers, shift: renderShift, kitchen: renderKitchen,
  }[state.view];
  view().catch((e) => { if (!e?.handled) toast(String(e?.message ?? e), true); });
}

// ---------- help: "How does this screen work?" ----------

/// Short steps for each screen, opened by the ? button in the top bar.
const HELP = {
  floor: ['Tables', [
    'Tap a free (white) table to start its order.',
    'Tap a coloured table to open its bill. Amber: people are eating. Teal: mains are waiting to be called. Red ring: dishes not sent to the kitchen yet.',
    'The strip at the top counts each kind and shows the money on open bills.',
    'Takeaway, collection and delivery orders start from Orders.']],
  orders: ['Orders', [
    'Tap Takeaway, Collection, Delivery or a delivery app to start a new order.',
    'Every open order is listed below; tap one to carry on with it.',
    'A red edge means something has not gone to the kitchen yet.']],
  order: ['Taking an order', [
    'Tap a dish to add it. Use the search box to find one fast.',
    'On the right is the bill. Use − and + to change how many. Tap a dish name for a note ("no onions"), a discount or to remove it.',
    'Send to kitchen prints the ticket. On a table, starters go first; tap Call mains when it is time for the mains.',
    'Finish & Pay takes the money: cash, card or split.',
    'Move, Reprint, Bill, Drawer and Cancel are under the order name.']],
  history: ['History', [
    'Pick a day, or Today / Yesterday / Last 7 days.',
    'Tap an order to see it, reprint its receipt or void it (a refund).']],
  reports: ['Reports', [
    "See the day's sales, tax, payment methods and best sellers.",
    'At closing time tap Close day & print summary. All orders must be paid or cancelled first.']],
  settings: ['Manage', [
    'Type what you are looking for in the search box, like "printer", "tax" or "PIN".',
    'Daily work has the screens you use often; Settings sets up how the POS works.']],
  menuAdmin: ['Menu', [
    'Tap + Menu item to add a dish: a name, a category and a price is enough.',
    'Tap any item to change it. More options holds tax, choices, extras and stock.',
    'Paste a menu copies a whole list from Excel or Google Sheets.']],
  plan: ['Floor plan', ['Pick a floor at the top, or add one with + Floor. This floor renames or deletes it.', 'Add a square or round table, or an Area for things that are not tables (the bar, a window).', 'Drag shapes to move them. Tap one to change its name, shape, size or kind in the card on the right.', 'Tap Save layout when you are done.']],
  users: ['Staff', ['Add each person with a name, a role and a PIN they log in with.', 'Staff sell; managers also see reports and history; admins change settings.']],
  customers: ['Customers', ['Find a saved customer by phone, name or postcode.', 'Tap one to change their details or see their recent orders.']],
  shift: ['Shift & cash', ['Start the shift with the float in the drawer.', 'Record money put in or taken out, and count the drawer at the end.']],
  kitchen: ['Kitchen screen', ['Each card is one order, oldest first.', 'Tap a dish when it is ready; tap Done when the whole order is out.']],
};
function showHelp() {
  const key = state.orderId ? 'order' : state.view;
  const [title, steps] = HELP[key] ?? HELP.settings;
  showModal(h('div', { class: 'row nowrap' }, h('h3', { class: 'grow' }, title), h('button', { class: 'icon-btn ghost', 'aria-label': 'Close', onclick: closeModal }, icon('x'))),
    h('ol', { class: 'help-steps' }, steps.map((s) => h('li', {}, s))),
    h('div', { class: 'muted small' }, 'Tap ? at the top whenever you need this again.'),
    h('button', { class: 'primary', onclick: closeModal }, 'Got it'));
  document.querySelector('#modal .dialog')?.classList.add('help-dialog');
}

/// Top bar title: "Restaurant / Manage › Reports". `crumb` ("Manage") is left out on phones.
function drawTitle(title, crumb = null) {
  setKids(document.getElementById('title'), h('span', {}, state.storeName || 'POS'), h('span', { class: 'sep' }, '/'),
    crumb && h('span', { class: 'crumb' }, `${crumb} ›`), h('span', { class: 'page-name' }, title));
}

function drawClock() {
  const el = document.getElementById('clock');
  const tick = () => (el.textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }));
  tick();
  clearInterval(state.topClock);
  state.topClock = setInterval(tick, 15000);
}

function openOrder(id) {
  state.returnTo = state.view;
  state.orderId = id;
  renderShell();
}

function closeOrder() {
  const left = state.orderId;
  state.orderId = null;
  state.orderOpen = false;
  state.view = state.returnTo;
  dropEmptyOrder(left).finally(renderShell);
}

/// Leaving an order with nothing on it (a free table tapped, then Back)
/// cancels it, so the table is free again and the day can be closed.
/// `state.orderEmpty` is the id of the order on screen while it is empty.
async function dropEmptyOrder(id) {
  if (id == null || state.orderEmpty !== id) return;
  state.orderEmpty = null;
  try {
    // The server cancels it only if it is still empty (another till may have added something).
    await invoke('cancel_empty_order', { orderId: id });
  } catch { /* it stays open, as before */ }
}

async function logOut(message = null) {
  closeModal();
  hideIdleWarn();
  await dropEmptyOrder(state.orderId);
  try { await invoke('logout'); } catch { /* already logged out on the server */ }
  Object.assign(state, { session: null, orderId: null, view: 'floor', menu: null, cats: null });
  document.getElementById('bar').hidden = true;
  renderLogin();
  if (message) toast(message);
}

document.getElementById('logout').onclick = act(async () => {
  if (state.planDirty) return toast('Save or discard your floor plan changes first', true);
  await logOut();
});

// ---------- auto logout ----------

/// Minutes without a touch or key press before logging out (Settings >
/// Auto logout); 0 = never. Shared by the till and every browser device.
const AUTO_LOGOUT_KEY = 'auto_logout_minutes';
const AUTO_LOGOUT_DEFAULT = 5;
/// Seconds of on-screen warning before it logs out (auto_logout_warn_secs); 0 = no warning.
const AUTO_LOGOUT_WARN_DEFAULT = 30;
let lastActivity = Date.now();
let idleWarned = false;
const parseAutoLogout = (v) => {
  const n = parseInt(v ?? '', 10);
  return Number.isFinite(n) && n >= 0 ? n : AUTO_LOGOUT_DEFAULT;
};
/// "Logging out in 10 s — tap to stay": covers the screen, so the tap that
/// keeps you logged in does not also press a button under it.
function showIdleWarn(secs) {
  let el = document.getElementById('idle-warn');
  if (!el) {
    // preventDefault on the press: the field being typed in keeps its focus.
    el = h('div', { id: 'idle-warn', role: 'alertdialog', 'aria-live': 'assertive', onclick: hideIdleWarn, onpointerdown: (e) => e.preventDefault(), onmousedown: (e) => e.preventDefault() },
      h('div', { class: 'idle-card' }, icon('lock', 36), h('strong', {}), h('span', {}, 'Tap anywhere to stay logged in')));
    document.body.append(el);
  }
  el.querySelector('strong').textContent = `Logging out in ${secs} s`;
  el.hidden = false;
}
function hideIdleWarn() {
  const el = document.getElementById('idle-warn');
  if (el) el.hidden = true;
}
for (const ev of ['pointerdown', 'keydown', 'wheel', 'touchstart']) {
  document.addEventListener(ev, (e) => {
    lastActivity = Date.now();
    if (!idleWarned) return;
    idleWarned = false;
    // A tap on the warning closes it on its click; anything else closes it now.
    if (e.type === 'keydown' || !document.getElementById('idle-warn')?.contains(e.target)) hideIdleWarn();
  }, { capture: true, passive: true });
}
setInterval(() => {
  const mins = state.autoLogout ?? AUTO_LOGOUT_DEFAULT;
  if (!state.session || !mins) { idleWarned = false; return hideIdleWarn(); }
  // Never throw away an unsaved floor plan: wait until it is saved or discarded.
  // The floor plan editor with unsaved changes, and the kitchen screen for staff, stay logged in.
  if (state.planDirty || (state.view === 'kitchen' && !state.orderId && !isAdmin())) { lastActivity = Date.now(); return; }
  const idle = Date.now() - lastActivity;
  const limit = mins * 60_000;
  // The warning never takes more than half the time.
  const warn = Math.min((state.ui?.warnSecs ?? AUTO_LOGOUT_WARN_DEFAULT) * 1000, limit / 2);
  if (idle >= limit) {
    idleWarned = false;
    hideIdleWarn();
    logOut(`Logged out after ${mins} minute${mins === 1 ? '' : 's'} without activity`);
  } else if (warn && idle >= limit - warn) {
    idleWarned = true;
    showIdleWarn(Math.ceil((limit - idle) / 1000));
  } else if (!idleWarned && idle > 1500) hideIdleWarn();
}, 1000);

// ---------- floor plan canvas (shared by live view and editor) ----------

function planBox(reserve = 170) {
  const { w, h: ph } = state.plan;
  return h('div', { class: 'plan', style: `aspect-ratio:${w}/${ph}; width:min(100%, calc((100vh - ${reserve}px) * ${w / ph}))` });
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

// ---------- live floor view ----------

/// Segmented control: `items` are [value, label]; returns the element.
function segmented(items, current, onPick) {
  return h('div', { class: 'seg' }, items.map(([v, label]) =>
    h('button', { class: v === current ? 'on' : '', onclick: () => onPick(v) }, label)));
}

async function renderFloor() {
  const floors = await call('list_floors');
  if (!floors.some((f) => f.id === state.floorId)) state.floorId = floors[0]?.id ?? null;
  if (!floors.length) {
    return screen(h('div', { class: 'empty' }, icon('plan', 48), h('h2', {}, 'No floor plan yet'),
      isManager() ? h('button', { class: 'primary lg', onclick: () => go('plan') }, 'Draw the floor plan') : h('p', {}, 'Ask an admin to set it up.')));
  }
  const plan = planBox(200);
  // One strip: each colour the tables use with how many show it now (it is
  // also the legend), then the money on open bills.
  const stats = h('div', { class: 'floor-stats' });
  const draw = async () => {
    const all = await call('floor_status', { floorId: state.floorId });
    const tables = all.filter((t) => t.is_table !== false);
    const busy = tables.filter((t) => t.order_id);
    const unsent = busy.filter((t) => t.unsent_count).length;
    setKids(stats,
      [['free', 'Free', tables.length - busy.length], ['busy', 'Occupied', busy.length],
        ['unsent', 'Not sent', unsent], ['held', 'Mains waiting', busy.filter((t) => t.held_count).length]]
        .map(([cls, label, n]) => h('span', { class: `fs ${cls === 'unsent' && n ? 'bad' : ''}` }, h('i', { class: cls }), label, h('b', {}, n))),
      h('span', { class: 'fs bills' }, 'Open bills', h('b', {}, money(busy.reduce((n, t) => n + t.total_cents, 0)))));
    setKids(plan, ...all.map((t) => {
      if (t.is_table === false) {
        const el = tableEl(t, 'deco');
        el.append(h('b', {}, t.name));
        return el;
      }
      const el = tableEl(t, `${t.order_id ? 'busy' : 'free'} ${t.unsent_count ? 'unsent' : ''} ${t.held_count ? 'held' : ''}`);
      el.append(h('b', {}, t.name));
      // Small tables show less, so the text always fits.
      const roomy = Math.min(t.w, t.h) * (plan.clientWidth / state.plan.w || 1) >= 96;
      if (t.order_id) el.append(h('span', { class: 'amt' }, money(t.total_cents)), roomy ? h('span', { class: 'time' }, ago(t.opened_at)) : '');
      if (t.held_count) el.append(h('span', { class: 'flag' }, roomy ? 'mains waiting' : 'mains'));
      else if (t.unsent_count && roomy) el.append(h('span', { class: 'flag bad' }, 'not sent'));
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
  screen(h('div', { class: 'page floor-page' },
    h('div', { class: 'toolbar floor-bar' },
      floors.length > 1 && segmented(floors.map((f) => [f.id, f.name]), state.floorId, (id) => { state.floorId = id; renderFloor(); }),
      stats),
    plan));
  await draw();
  live(draw);
}

// ---------- order screen ----------

/// Settings > Quick notes: one per line; tapped onto an order line's note.
/// A sub-category's "off" bits (categories.flags_off): main-category defaults it does not take.
const CAT_FLAG_OFF = { starter: 1, prep: 2, drink: 4, dessert: 8 };
const QUICK_NOTES_KEY = 'quick_notes';
const DEFAULT_QUICK_NOTES = ['No onions', 'No chilli', 'Extra spicy', 'Mild', 'No ice', 'Allergy: ask staff'];
function quickNotes(raw) {
  return raw == null ? DEFAULT_QUICK_NOTES : raw.split('\n').map((n) => n.trim()).filter(Boolean);
}

async function loadMenu() {
  if (!state.menu) {
    const [menu, cats, opts, variants, service, notes, tax] = await Promise.all([
      call('menu'), call('list_categories', { includeInactive: false }), call('menu_options'),
      call('list_variants', { productId: null }), call('get_service_settings'),
      call('get_setting', { key: QUICK_NOTES_KEY }), call('get_setting', { key: 'default_tax_bp' }),
    ]);
    state.starterChoice = (await call('get_setting', { key: 'starter_choice' }).catch(() => null)) !== '0';
    // "Which team makes this?" (Settings → Kitchen teams): the teams to choose from; empty = off.
    const [choice, teams] = await Promise.all([call('team_choice').catch(() => []), call('list_teams').catch(() => [])]);
    state.teams = teams;
    state.teamChoice = choice.map((id) => teams.find((t) => t.id === id)).filter(Boolean);
    state.service = service;
    state.quickNotes = quickNotes(notes);
    state.defaultTax = Math.max(0, parseInt(tax ?? '0', 10) || 0);
    state.itemVariants = {};
    for (const v of variants) (state.itemVariants[v.product_id] ??= []).push(v);
    const byId = Object.fromEntries(opts.groups.map((g) => [g.id, g]));
    state.itemGroups = {};
    for (const l of opts.links) (state.itemGroups[l.product_id] ??= []).push(byId[l.group_id]);
    [state.menu, state.cats] = [menu, cats];
  }
}

/// Price label for a menu item: "6.50", or "from 6.50" when it has choices.
function itemPrice(p, variants) {
  if (!variants?.length) return money(p.price_cents);
  const min = Math.min(...variants.map((v) => v.price_cents));
  return variants.every((v) => v.price_cents === min) ? money(min) : `from ${money(min)}`;
}

/// Short rule text for an option group, e.g. "choose 1" or "optional, up to 3".
function groupRule(g) {
  if (g.min_select === g.max_select) return `choose ${g.min_select}`;
  if (g.min_select) return `choose ${g.min_select} to ${g.max_select}`;
  return g.max_select === 1 ? 'optional' : `optional, up to ${g.max_select}`;
}
/// A required single choice ("choose 1", e.g. a size) starts on its first option,
/// so a plain order is one tap on Add. Other groups start empty.
/// A required single choice starts on its no-extra-cost option (else the first),
/// so one tap adds the usual dish without undercharging for a cheaper size.
const preselect = (g) => (g.min_select === 1 && g.max_select === 1 && g.options.length
  ? [(g.options.find((o) => o.price_cents === 0) ?? g.options[0]).id] : []);

/// The teams a menu item's dish can be moved to ("Which team makes this?"):
/// the chosen teams when the item's own team is one of them, else none
/// (the dish stays with its team and nobody is asked).
function teamsFor(productId) {
  const p = state.menu?.find((x) => x.id === productId);
  return p && p.team_id != null && state.teamChoice?.some((t) => t.id === p.team_id) ? state.teamChoice : [];
}
/// The team that makes an order line: the one it was moved to, else its menu item's.
const lineTeam = (it) => it.team_id ?? state.menu?.find((x) => x.id === it.product_id)?.team_id ?? null;
const teamName = (id) => state.teams?.find((t) => t.id === id)?.name ?? '';

async function renderOrder() {
  await loadMenu();
  state.orderOpen = false;
  let order = await call('get_order', { id: state.orderId });
  state.orderOpen = order.status === 'open';
  if (order.status !== 'open') {
    app.classList.remove('fill');
    return screen(h('div', { class: 'page narrow' },
      h('div', { class: 'toolbar' }, h('button', { class: 'ghost', onclick: closeOrder }, icon('back'), 'Back')),
      orderDetail(order, (what) => (what === 'deleted' ? closeOrder() : renderShell()))));
  }
  const orderId = order.id;

  // --- menu side ---
  const search = h('input', { type: 'search', placeholder: 'Search or scan', title: 'Search the menu or scan a barcode' });
  // "Sold out" mode: tapping an item marks it sold out for today (or back on sale).
  let soldMode = false;
  const soldBtn = h('button', { class: 'sold-btn', title: 'Mark items sold out for today', onclick: () => { soldMode = !soldMode; drawMenu(); if (soldMode) toast('Tap the items that are sold out today (tap again to put back). Tap Done when finished.'); } });
  const toggleSold = (p) => act(async () => {
    await call('set_sold_out', { id: p.id, soldOut: !p.sold_out });
    p.sold_out = !p.sold_out;
    drawMenu();
    toast(p.sold_out ? `${p.name}: sold out today` : `${p.name}: back on sale`);
  });
  const cats = h('div', { class: 'cats' });
  // A dish that is not on the menu: type its name and price.
  const otherBtn = h('button', { class: 'sold-btn', title: 'Add a dish that is not on the menu', onclick: () => {
    const name = h('input', { placeholder: 'Dish name, e.g. Extra rice', style: 'flex:1' });
    const price = h('input', { inputmode: 'decimal', placeholder: '0.00', style: 'width:130px' });
    const note = h('input', { placeholder: 'Note for the kitchen (optional)', style: 'flex:1' });
    let qty = 1, course = false;
    const qtyEl = h('span', { class: 'big' }, '1');
    const courseRow = h('div', { class: 'seg' });
    const drawCourse = () => setKids(courseRow, [[true, 'Starter'], [false, 'Main course']].map(([v, l]) =>
      h('button', { class: course === v ? 'on' : '', onclick: () => { course = v; drawCourse(); } }, l)));
    drawCourse();
    const add = act(async () => {
      if (!name.value.trim()) return toast('Type the dish name', true);
      if (!price.value.trim()) return toast('Type the price (0 if it is free)', true);
      let cents;
      try { cents = parseMoney(price.value); } catch { return toast('Enter a price like 2 or 2.50', true); }
      order = await call('add_open_item', { orderId, name: name.value, priceCents: cents, qty, note: note.value.trim() || null, starter: course });
      editing = null; // a dish added: the totals and Send come back
      closeModal(); draw(); toast(`${name.value.trim()} added`);
    });
    showModal(h('h3', {}, 'Other dish (not on the menu)'),
      h('div', { class: 'row' }, name), h('div', { class: 'row' }, h('span', { class: 'muted' }, 'Price'), price,
        h('span', { class: 'grow' }), h('span', { class: 'muted' }, 'Qty'),
        h('div', { class: 'stepper' }, h('button', { onclick: () => { qty = Math.max(1, qty - 1); qtyEl.textContent = qty; } }, '−'), qtyEl,
          h('button', { onclick: () => { qty = Math.min(99, qty + 1); qtyEl.textContent = qty; } }, '+'))),
      order.order_type === 'dine_in' && courseRow,
      h('div', { class: 'row' }, note),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: add }, 'Add to order'), h('span', { class: 'grow' }), h('button', { onclick: closeModal }, 'Cancel')));
    for (const el of [name, price, note]) el.addEventListener('keydown', (e) => { if (e.key === 'Enter') add(e); });
  } }, '+ Other dish');
  const grid = h('div', { class: 'items' });
  const subBar = h('div', { class: 'subcats', hidden: true });
  const picker = h('div', { class: 'stack picker', hidden: true });
  const catColor = Object.fromEntries(state.cats.map((c) => [c.id, c.color]));
  const drawMenu = () => {
    // Main categories on the left; a main category's sub-categories as chips above the items.
    const known = new Set(state.cats.map((c) => c.id));
    const mains = state.cats.filter((c) => !c.parent_id || !known.has(c.parent_id));
    const subsOf = (id) => state.cats.filter((c) => c.parent_id === id);
    if (state.catId != null && !mains.some((c) => c.id === state.catId)) {
      // A sub-category picked before: open its main category on it.
      const sub = state.cats.find((c) => c.id === state.catId);
      state.subId = sub?.id ?? null;
      state.catId = sub?.parent_id ?? null;
    }
    setKids(cats, 
      h('button', { class: state.catId == null ? 'on' : '', style: catStyle('#344054'), onclick: () => { state.catId = null; state.subId = null; search.value = ''; drawMenu(); } },
        h('span', { class: 'dot' }), 'All items'),
      ...mains.map((c) => h('button', {
        class: state.catId === c.id ? 'on' : '', style: catStyle(c.color),
        onclick: () => { state.catId = c.id; state.subId = null; search.value = ''; closePicker(); drawMenu(); },
      }, h('span', { class: 'dot' }), c.name, subsOf(c.id).length > 0 && h('span', { class: 'has-subs' }, '›'))));
    const subs = state.catId != null ? subsOf(state.catId) : [];
    if (state.subId != null && !subs.some((c) => c.id === state.subId)) state.subId = null;
    const q = search.value.trim().toLowerCase();
    subBar.hidden = q !== '' || !subs.length;
    const mainCat = state.cats.find((c) => c.id === state.catId);
    setKids(subBar, subs.length > 0 && [
      h('button', { class: state.subId == null ? 'on' : '', style: catStyle(mainCat?.color), onclick: () => { state.subId = null; drawMenu(); } }, `All ${mainCat?.name ?? ''}`),
      ...subs.map((c) => h('button', { class: state.subId === c.id ? 'on' : '', style: catStyle(c.color || mainCat?.color), onclick: () => { state.subId = c.id; drawMenu(); } }, c.name)),
    ]);
    const shown = state.catId == null ? null : new Set(state.subId != null ? [state.subId] : [state.catId, ...subs.map((c) => c.id)]);
    const list = state.menu.filter((p) => q ? matches(p, q) : shown == null || shown.has(p.category_id));
    soldBtn.classList.toggle('on', soldMode);
    soldBtn.textContent = soldMode ? 'Done' : 'Sold out';
    grid.classList.toggle('sold-mode', soldMode);
    setKids(grid, ...list.slice(0, 300).map((p) => h('button', {
      class: `tile ${p.sold_out ? 'sold' : ''}`, style: catStyle(catColor[p.category_id]), onclick: soldMode ? toggleSold(p) : p.sold_out
        ? () => toast(`${p.name} is sold out today. Tap "Sold out" above to put it back on sale.`, true) : choose(p),
    }, h('span', { class: 'names' }, h('span', { class: 'name' }, p.name), p.name_alt && h('span', { class: 'alt', lang: 'th' }, p.name_alt)),
      h('span', { class: 'foot' },
        p.sold_out ? h('span', { class: 'tag sold' }, 'SOLD OUT') : h('span', { class: 'price' }, itemPrice(p, state.itemVariants[p.id])),
        p.is_set && h('span', { class: 'tag set' }, 'Set'),
        p.starter && h('span', { class: 'tag' }, 'Starter'),
        (p.is_set || state.itemGroups[p.id] || state.itemVariants[p.id]) && h('span', { class: 'more', title: 'Has choices' }, '•••')))));
    if (!list.length) grid.append(h('div', { class: 'muted pad' }, q ? 'No items match your search.' : 'No items in this category yet.'));
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
  const sendBtn = h('button', { class: 'send', onclick: act(() => send(allCourses.checked)) });
  // Mains waiting: send them. Mains already in the kitchen: call them again.
  const mainsBtn = h('button', { class: 'mains', onclick: act(async () => {
    if (order.items.some((i) => i.held && !i.sent_at)) return send(true, 'Mains called');
    reportPrint(await call('call_main_course', { orderId }), 'Main course called', true);
  }) });
  const payBtn = h('button', { class: 'go pay', onclick: () => finish() });
  // Phones: the menu and the ticket take turns on screen.
  let layout = null;
  const phoneBar = h('button', { class: 'phone-bar', onclick: () => { layout.classList.toggle('show-ticket'); draw(); } });
  // Dine-in: starters go first and the rest waits for "Call mains", unless ticked.
  const allCourses = h('input', { type: 'checkbox' });
  const extra = h('div');
  let editing = null;

  // `course`: true = starter, false = main, null = as on the menu.
  // `team`: the team that makes it (team id), null = the menu's.
  async function addItem(productId, optionIds = [], variantId = null, course = null, team = null) {
    order = await call('add_item', { orderId, productId, qty: 1, note: null, variantId, optionIds, starter: course, teamId: team });
    editing = null; // a dish added: the totals and Send come back
    draw();
  }
  // Tap on a menu item: add it straight away, or ask for its options first.
  // Dine-in, when Settings → Kitchen teams has teams to choose from and the
  // dish's team is one of them: "Which team makes this?" first. Otherwise a
  // starter on a dine-in order asks "Starter or Main?" (Settings → Orders).
  function choose(p) {
    return act(async () => {
      if (p.is_set) return openSetPicker(p);
      const groups = (state.itemGroups[p.id] || []).filter((g) => g.options.length);
      const variants = state.itemVariants[p.id] || [];
      const next = async (course, team = null) => {
        if (!groups.length && !variants.length) { await addItem(p.id, [], null, course, team); closePicker(); return; }
        openPicker(p, groups, variants, course, team);
      };
      // The team picker replaces "Starter or Main?"; the course stays as on the menu.
      if (order.order_type === 'dine_in' && state.teamChoice?.length) {
        const teams = teamsFor(p.id);
        if (!teams.length) return next(null);
        setKids(picker, h('h3', {}, p.name), h('div', { class: 'muted' }, 'Which team makes this?'),
          h('div', { class: 'course-pick team-pick' }, teams.map((t) => h('button', {
            class: t.id === p.team_id ? 'primary usual' : '',
            onclick: act(() => next(null, t.id)),
          }, t.name, t.name_alt && h('small', { lang: 'th' }, t.name_alt), t.id === p.team_id && h('small', {}, 'usual')))),
          h('div', { class: 'row' }, h('button', { onclick: closePicker }, 'Cancel')));
        [picker.hidden, grid.hidden, cats.hidden] = [false, true, true];
        return;
      }
      if (p.starter && order.order_type === 'dine_in' && state.starterChoice) {
        setKids(picker, h('h3', {}, p.name), h('div', { class: 'muted' }, 'Serve it as:'),
          h('div', { class: 'course-pick' },
            h('button', { class: 'primary', onclick: act(() => next(true)) }, 'Starter', h('small', {}, 'goes first')),
            h('button', { class: 'primary main', onclick: act(() => next(false)) }, 'Main course', h('small', {}, 'with the mains'))),
          h('div', { class: 'row' }, h('button', { onclick: closePicker }, 'Cancel')));
        [picker.hidden, grid.hidden, cats.hidden] = [false, true, true];
        return;
      }
      await next(null);
    });
  }
  function openPicker(p, groups, variants = [], course = null, team = null) {
    const chosen = new Map(groups.map((g) => [g.id, new Set(preselect(g))]));
    let variant = null;
    const body = h('div', { class: 'stack' });
    const price = h('strong');
    const redraw = () => {
      setKids(body, 
        variants.length > 0 && h('div', { class: 'stack' },
          h('div', {}, h('strong', {}, 'Choose'), ' ', h('span', { class: 'muted' }, 'required')),
          h('div', { class: 'opts' }, variants.map((v) => h('button', {
            class: variant?.id === v.id ? 'on' : '',
            onclick: () => { variant = v; redraw(); },
          }, v.name, h('small', {}, money(v.price_cents)))))),
        ...groups.map((g) => h('div', { class: 'stack' },
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
      const base = variants.length ? variant?.price_cents : p.price_cents;
      price.textContent = base == null ? `from ${itemPrice(p, variants).replace('from ', '')}` : money(base + extra);
    };
    const add = act(async () => {
      if (variants.length && !variant) return toast(`Choose which ${p.name}`, true);
      const missing = groups.find((g) => chosen.get(g.id).size < g.min_select);
      if (missing) return toast(`${missing.name}: ${groupRule(missing)}`, true);
      await addItem(p.id, groups.flatMap((g) => [...chosen.get(g.id)]), variant?.id ?? null, course, team);
      closePicker();
    });
    // With only choices (no option groups), one tap on a choice adds it.
    if (!groups.length) {
      setKids(picker, h('h3', {}, p.name),
        h('div', { class: 'opts' }, variants.map((v) => h('button', {
          onclick: act(async () => { await addItem(p.id, [], v.id, course, team); closePicker(); }),
        }, v.name, h('small', {}, money(v.price_cents))))),
        h('div', { class: 'row' }, h('button', { onclick: closePicker }, 'Cancel')));
      [picker.hidden, grid.hidden, cats.hidden] = [false, true, true];
      return;
    }
    setKids(picker, h('h3', {}, p.name), body,
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: add }, 'Add to order'),
        h('button', { onclick: closePicker }, 'Cancel'), h('span', { style: 'flex:1' }), price));
    redraw();
    [picker.hidden, grid.hidden, cats.hidden] = [false, true, true];
  }
  // A set menu: choose the dishes for each section, then add the set.
  async function openSetPicker(p) {
    const sections = await call('get_set', { setId: p.id });
    if (!sections.length) return toast(`${p.name} has no sections yet (Menu → tap the set)`, true);
    const chosen = new Map(sections.map((s) => [s.id, []]));
    const body = h('div', { class: 'stack' });
    const addBtn = h('button', { class: 'primary' }, 'Add to order');
    // "choose 1" stays grey until Add is tapped with something missing, then turns red; green once done.
    let tried = false, incomplete = true;
    // A chosen dish still needs its "comes with", its own choice or a required option.
    const entryMissing = (s, x) => (s.with_items.length > 0 && x.with_id == null)
      || ((state.itemVariants[x.product_id] ?? []).length > 0 && x.variant_id == null)
      || (state.itemGroups[x.product_id] ?? []).some((g) => g.options.length && x.option_ids.filter((id) => g.options.some((o) => o.id === id)).length < g.min_select);
    const sectionDone = (s) => chosen.get(s.id).length === s.pick && !chosen.get(s.id).some((x) => entryMissing(s, x));
    const redraw = () => {
      setKids(body, sections.map((s) => {
        const mine = chosen.get(s.id);
        const done = sectionDone(s);
        return h('div', { class: 'stack set-section' },
          h('div', {}, h('strong', {}, s.name), s.name_alt && h('span', { class: 'alt', lang: 'th' }, ' ', s.name_alt), ' ',
            h('span', { class: `badge ${done ? 'ok' : tried ? 'warn' : ''}` }, `${done ? '✓ ' : ''}choose ${s.pick}${s.pick > 1 ? ` (${mine.length}/${s.pick})` : ''}`),
            s.starter && h('span', { class: 'badge' }, 'starter'),
            mine.length > 0 && s.pick > 1 && h('button', { class: 'ghost small-btn', onclick: () => { mine.length = 0; redraw(); } }, 'Clear')),
          h('div', { class: 'opts' }, s.items.map((c) => {
            const n = mine.filter((x) => x.product_id === c.product_id).length;
            if (c.sold_out) return h('button', { class: 'sold', disabled: true, title: 'Sold out today' }, c.name, h('small', {}, 'sold out'));
            return h('button', { class: n ? 'on' : '', onclick: () => {
              const fresh = { product_id: c.product_id, with_id: null, variant_id: null,
                option_ids: (state.itemGroups[c.product_id] ?? []).filter((g) => g.options.length).flatMap(preselect) };
              if (s.pick === 1) mine.splice(0, 1, { ...fresh, with_id: mine[0]?.with_id ?? null });
              else if (mine.length < s.pick) mine.push(fresh);
              else if (n) mine.splice(mine.findIndex((x) => x.product_id === c.product_id), 1);
              else return toast(`${s.name}: choose ${s.pick}`, true);
              redraw();
            } }, c.name, c.name_alt && h('small', { lang: 'th' }, c.name_alt), n > 1 && h('small', {}, `× ${n}`));
          })),
          // What each chosen dish comes with (rice, noodles...), when the section has it.
          s.with_items.length > 0 && mine.map((entry) => h('div', { class: 'with-row' },
            h('span', { class: `with-label ${tried && entry.with_id == null ? 'need' : ''}` }, `${s.items.find((c) => c.product_id === entry.product_id)?.name ?? ''} with`),
            h('div', { class: 'opts' }, s.with_items.map((w) => h('button', {
              class: entry.with_id === w.product_id ? 'on' : '',
              onclick: () => { entry.with_id = w.product_id; redraw(); },
            }, w.name, w.name_alt && h('small', { lang: 'th' }, w.name_alt)))))),
          // Each chosen dish's own choice (chicken / beef...) and options.
          mine.map((entry) => {
            const vs = state.itemVariants[entry.product_id] ?? [];
            const gs = (state.itemGroups[entry.product_id] ?? []).filter((g) => g.options.length);
            if (!vs.length && !gs.length) return '';
            const dish = s.items.find((c) => c.product_id === entry.product_id)?.name ?? '';
            return h('div', { class: 'stack with-row dish-choices' },
              vs.length > 0 && h('div', { class: 'row' }, h('span', { class: `with-label ${tried && entry.variant_id == null ? 'need' : ''}` }, dish), h('div', { class: 'opts' }, vs.map((v) => h('button', {
                class: entry.variant_id === v.id ? 'on' : '', onclick: () => { entry.variant_id = v.id; redraw(); },
              }, v.name, v.name_alt && h('small', { lang: 'th' }, v.name_alt))))),
              gs.map((g) => h('div', { class: 'row' }, h('span', { class: `with-label ${tried && entry.option_ids.filter((id) => g.options.some((o) => o.id === id)).length < g.min_select ? 'need' : ''}` }, `${vs.length ? '' : dish + ' · '}${g.name}`, h('small', { class: 'muted' }, ` ${groupRule(g)}`)),
                h('div', { class: 'opts' }, g.options.map((o) => {
                  const on = entry.option_ids.includes(o.id);
                  return h('button', { class: on ? 'on' : '', onclick: () => {
                    const inGroup = entry.option_ids.filter((id) => g.options.some((x) => x.id === id));
                    if (on) entry.option_ids = entry.option_ids.filter((id) => id !== o.id);
                    else if (g.max_select === 1) entry.option_ids = [...entry.option_ids.filter((id) => !inGroup.includes(id)), o.id];
                    else if (inGroup.length >= g.max_select) return toast(`${g.name}: up to ${g.max_select}`, true);
                    else entry.option_ids = [...entry.option_ids, o.id];
                    redraw();
                  } }, o.name, o.price_cents ? h('small', {}, signed(o.price_cents)) : '');
                })))));
          }));
      }));
      const entries = sections.flatMap((s) => chosen.get(s.id));
      const extras = entries.reduce((n, x) => n + (state.itemGroups[x.product_id] ?? []).flatMap((g) => g.options).filter((o) => x.option_ids.includes(o.id)).reduce((a, o) => a + o.price_cents, 0), 0);
      addBtn.textContent = extras ? `Add to order · ${money(p.price_cents + extras)}` : 'Add to order';
      incomplete = !sections.every(sectionDone);
    };
    addBtn.onclick = act(async () => {
      if (incomplete) {
        tried = true;
        redraw();
        return toast('Finish the parts marked in red first', true);
      }
      const picks = sections.flatMap((s) => chosen.get(s.id).map((x) => ({ section_id: s.id, product_id: x.product_id, with_id: x.with_id, variant_id: x.variant_id, option_ids: x.option_ids })));
      order = await call('add_set', { orderId, setId: p.id, picks, note: null });
      editing = null;
      closePicker();
      draw();
    });
    setKids(picker, h('div', { class: 'row' }, h('h3', { class: 'grow' }, p.name), h('strong', {}, money(p.price_cents))), body,
      h('div', { class: 'row' }, addBtn, h('button', { onclick: closePicker }, 'Cancel')));
    redraw();
    [picker.hidden, grid.hidden, cats.hidden] = [false, true, true];
  }
  function closePicker() {
    [picker.hidden, grid.hidden, cats.hidden] = [true, false, false];
  }
  // Report a kitchen print attempt alongside `msg`.
  function reportPrint(p, msg, needPrinter = false) {
    if (p.error) return toast(`${msg}, but the kitchen ticket did not print (${p.error}). Use Reprint.`, true);
    if (!p.attempted && needPrinter) return toast('No kitchen printer is set up (Manage → Kitchen printer)', true);
    toast(p.attempted ? `${msg} · ticket printed` : msg);
  }
  async function setLine(it, changes) {
    // Fewer of a discounted dish: the discount shrinks with it (the same share off),
    // so − never gives more off or asks for a manager PIN.
    if (changes.qty > 0 && changes.qty < it.qty && it.discount_cents && !('discount_cents' in changes)) {
      changes = { ...changes, discount_cents: Math.floor(it.discount_cents * changes.qty / it.qty) };
    }
    const args = { orderId, itemId: it.id, update: { qty: it.qty, discount_cents: it.discount_cents, note: it.note, ...changes } };
    try {
      order = await invoke('update_item', args);
    } catch (e) {
      const msg = String(e?.message ?? e);
      // Over the staff discount limit (Settings → Orders → Discounts): a manager approves with their PIN.
      if (!/needs a manager/i.test(msg)) {
        if (msg === 'please log in') start();
        else if (isDeadOrder(msg)) leaveDeadOrder();
        // The dish is gone (removed or moved on another till): show the order as it is now.
        else if (msg === 'not found') { order = await call('get_order', { id: orderId }); draw(); toast(`${it.name} was changed on another till`, true); }
        else toast(msg, true);
        const err = new Error(msg); err.handled = true; throw err;
      }
      const ok = await pinDialog({
        title: 'Manager PIN', hint: `${it.name}: ${msg}. A manager or admin enters their PIN to approve it.`, okLabel: 'Approve discount',
        submit: async (pin) => {
          try { order = await invoke('update_item', { ...args, pin }); } catch (e2) {
            const m = String(e2?.message ?? e2);
            if (isDeadOrder(m)) { setTimeout(leaveDeadOrder); return 'This order was closed on another till'; }
            return /invalid pin/i.test(m) ? 'Wrong PIN, try again' : /needs a manager/i.test(m) ? 'That PIN cannot approve it: a manager or admin PIN is needed' : m;
          }
        },
      });
      if (!ok) { const err = new Error('Discount not approved'); err.handled = true; throw err; }
      toast('Discount approved');
    }
    // The dish now belongs to another bill (its order was joined onto it on another till).
    if (order.id !== orderId) { leaveDeadOrder(); return; }
    draw();
  }
  async function send(allCourses = false, what = 'Sent') {
    const r = await call('send_to_kitchen', { orderId, allCourses });
    order = await call('get_order', { id: orderId });
    draw();
    // Counted like the Send button: a set's dishes, not the set's own line.
    const n = r.items.filter((i) => !i.is_set).reduce((n, i) => n + i.qty, 0);
    const prep = (r.prep ?? []).reduce((n, i) => n + i.qty, 0);
    if (!n && !prep) return toast('Nothing new to send');
    const waiting = order.items.filter((i) => i.held && !i.sent_at).reduce((n, i) => n + i.qty, 0);
    const msg = `${what}: ${n} item(s)` + (prep ? ` · PREPARE NOW for ${prep}` : '') + (waiting ? ` · ${waiting} main(s) wait for "Call mains"` : '');
    reportPrint(r.prep_print?.error ? r.prep_print : r.print, msg);
  }
  // The order again from the server, e.g. after a payment was taken back in Finish & Pay.
  const reload = async () => {
    if (state.orderId !== orderId || !layout?.isConnected) return;
    order = await call('get_order', { id: orderId });
    if (order.status !== 'open') return renderShell();
    draw();
  };
  const finish = act(async () => {
    order = await call('get_order', { id: orderId });
    if (order.status !== 'open') return renderShell();
    draw();
    if (!order.items.length) return toast('The order is empty', true);
    let sentFirst = false;
    let printError = null;
    await payDialog(order, reload, async (payment) => {
      // Anything not yet sent (mains waiting included) goes to the kitchen with the first payment.
      if (!sentFirst && order.items.some((i) => !i.sent_at)) printError = (await call('send_to_kitchen', { orderId, allCourses: true })).print.error;
      sentFirst = true;
      const { order: res, print: receipt } = await call('add_payment', { orderId, payment });
      if (res.status !== 'paid') {
        order = res;
        draw();
        toast(`Paid ${money(res.payments.at(-1).amount_cents)} · ${money(res.total_cents - res.paid_cents)} left to pay`);
        return res;
      }
      const last = res.payments.at(-1);
      const msg = `Order #${res.id} paid: ${money(res.total_cents)}` + (res.tip_cents ? ` + tip ${money(res.tip_cents)}` : '') +
        (last?.change_cents ? ` · change ${money(last.change_cents)}` : '');
      closeOrder();
      // History is for managers and admins: staff ask one of them to reprint.
      const reprint = isManager() ? 'reprint it from History' : 'ask a manager to reprint it from History';
      if (printError) toast(`${msg}. Kitchen ticket did not print (${printError}); ${reprint}.`, true);
      else if (receipt.error) toast(`${msg}. Receipt/drawer did not work (${receipt.error}); ${reprint}.`, true);
      else toast(msg);
      return res;
    });
  });

  // One dish on the bill, edited in a pop-up in the middle of the screen:
  // how many, the price each, starter or main, the team, a note, a discount,
  // print again, remove. Nothing is saved until Save (Remove and the course
  // and team buttons act at once, as before).
  function openLineEditor(it) {
    const sent = !!it.sent_at;
    let qty = it.qty;
    const priceOk = !it.parent_id && !(order.paid_cents > 0);
    const price = h('input', { inputmode: 'decimal', value: money(it.unit_price_cents), 'aria-label': 'Price each', class: 'price-input' });
    const note = h('input', { value: it.note ?? '', placeholder: 'Note for the kitchen, e.g. no onions', 'aria-label': 'Note' });
    const disc = h('input', { value: it.discount_cents ? money(it.discount_cents) : '', placeholder: '0.00 or 10%', title: 'An amount (2.50) or a percentage (10%)', 'aria-label': 'Discount' });
    const qtyEl = h('strong', { class: 'qty-big' });
    const total = h('strong', { class: 'line-total' });
    const minus = h('button', { class: 'step', 'aria-label': 'One less', onclick: () => { qty = Math.max(1, qty - 1); redraw(); } }, '−');
    const plus = h('button', { class: 'step', 'aria-label': 'One more', onclick: () => { qty = Math.min(sent ? it.qty : 999, qty + 1); redraw(); } }, '+');
    const priceNow = () => { try { return parseMoney(price.value); } catch { return null; } };
    const redraw = () => {
      qtyEl.textContent = qty;
      plus.disabled = sent && qty >= it.qty;
      minus.disabled = qty <= 1;
      const p = priceNow();
      let d = 0;
      try { d = disc.value.trim() ? parseDiscount(disc.value, (p ?? it.unit_price_cents) * qty) : 0; } catch { d = 0; }
      total.textContent = p == null ? '—' : money(Math.max(0, p * qty - d));
      drawChips();
    };
    // Quick notes: tap to add to (or take off) the note.
    const chips = h('div', { class: 'quick-notes' });
    const drawChips = () => {
      const parts = note.value.split(',').map((x) => x.trim()).filter(Boolean);
      setKids(chips, (state.quickNotes ?? []).map((q) => h('button', {
        class: parts.some((x) => x.toLowerCase() === q.toLowerCase()) ? 'on' : '',
        onclick: () => {
          const has = parts.findIndex((x) => x.toLowerCase() === q.toLowerCase());
          if (has >= 0) parts.splice(has, 1); else parts.push(q);
          note.value = parts.join(', ').slice(0, 100);
          drawChips();
        },
      }, q)));
    };
    // Quick discounts (Settings → Orders → Discounts): fill the box; Save applies it.
    const presets = state.ui?.discountPresets ?? [];
    const discChips = presets.length > 0 && h('div', { class: 'quick-notes disc-chips' },
      presets.map((p) => h('button', { onclick: () => { disc.value = `${p}%`; redraw(); } }, `−${p}%`)),
      h('button', { onclick: () => { disc.value = ''; redraw(); } }, 'None'));
    for (const el of [price, disc, note]) el.addEventListener('input', redraw);
    // A change of course or team is saved at once; the pop-up opens again on the dish as it is now.
    const reopen = () => { const fresh = order.items.find((i) => i.id === it.id); if (fresh) openLineEditor(fresh); else closeModal(); };
    const courseRow = !sent && !it.is_set && order.order_type === 'dine_in' && h('div', { class: 'edit-field' }, h('label', {}, 'Serve as'),
      h('div', { class: 'seg' }, [[true, 'Starter'], [false, 'Main course']].map(([v, l]) => h('button', {
        class: !!it.starter === v ? 'on' : '',
        onclick: act(async () => { if (!!it.starter === v) return; order = await call('set_item_course', { itemId: it.id, starter: v }); draw(); reopen(); }),
      }, l))));
    const teamRow = !sent && !it.is_set && order.order_type === 'dine_in' && (() => {
      const teams = teamsFor(it.product_id);
      const cur = lineTeam(it);
      if (!teams.length && !(it.team_id != null && state.teamChoice?.length)) return '';
      return h('div', { class: 'edit-field' }, h('label', {}, 'Team'),
        h('div', { class: 'seg' }, (teams.length ? teams : state.teamChoice).map((t) => h('button', {
          class: cur === t.id ? 'on' : '',
          onclick: act(async () => { if (cur === t.id) return; order = await call('set_item_team', { itemId: it.id, teamId: t.id }); draw(); reopen(); }),
        }, t.name))));
    })();

    // Price first (it may need a manager's PIN), then quantity, note and discount.
    const save = act(async () => {
      const p = priceNow();
      if (priceOk && p == null) return toast('Price: enter an amount like 12 or 12.50', true);
      const newPrice = priceOk ? p : it.unit_price_cents;
      let cents = 0;
      try { cents = disc.value.trim() ? parseDiscount(disc.value, newPrice * qty) : 0; } catch { return toast('Discount: an amount like 2.50 or a percentage like 10%', true); }
      const newNote = note.value.trim() || null;
      closeModal();
      if (newPrice !== it.unit_price_cents) {
        const args = { orderId, itemId: it.id, priceCents: newPrice };
        try {
          order = await invoke('set_item_price', args);
        } catch (e) {
          const msg = String(e?.message ?? e);
          if (!/needs a manager/i.test(msg)) { toast(msg, true); return; }
          const ok = await pinDialog({
            title: 'Manager PIN', hint: `${it.name}: ${money(it.unit_price_cents)} → ${money(newPrice)}. A manager or admin enters their PIN to approve the new price.`, okLabel: 'Approve price',
            submit: async (pin) => {
              try { order = await invoke('set_item_price', { ...args, pin }); } catch (e2) {
                const m = String(e2?.message ?? e2);
                return /invalid pin/i.test(m) ? 'Wrong PIN, try again' : /needs a manager/i.test(m) ? 'That PIN cannot approve it: a manager or admin PIN is needed' : m;
              }
            },
          });
          if (!ok) { draw(); return toast('Price not changed', true); }
        }
        draw();
        toast(`${it.name}: ${money(newPrice)} each`);
      }
      const now = order.items.find((i) => i.id === it.id) ?? it;
      if (qty !== now.qty || cents !== now.discount_cents || newNote !== (now.note ?? null)) {
        await setLine(now, { qty, note: newNote, discount_cents: cents });
        if (sent && qty < it.qty) toast(`${it.name} −${it.qty - qty} · CANCELLED ticket sent to the kitchen`);
        else if (sent && newNote !== (it.note ?? null)) toast('Note changed · CHANGED ticket sent to the kitchen');
        else if (cents !== it.discount_cents) toast(cents ? `${it.name}: −${money(cents)} discount` : `${it.name}: discount removed`);
        else if (newNote !== (it.note ?? null)) toast(newNote ? `Note saved: “${newNote}”` : 'Note removed');
      }
    });
    for (const el of [price, note, disc]) el.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(e); });
    const remove = h('button', { class: 'danger', onclick: act(async () => {
      closeModal();
      await setLine(it, { qty: 0 });
      toast(sent ? `${it.name} removed · CANCELLED ticket sent to the kitchen` : `${it.name} removed`);
    }) }, icon('x', 18), 'Remove');
    showModal(
      h('div', { class: 'edit-head' },
        h('div', { class: 'grow' }, h('h3', {}, it.name), it.name_alt && h('div', { class: 'alt', lang: 'th' }, it.name_alt),
          it.options.length > 0 && h('div', { class: 'muted small' }, it.options.map((o) => o.name).join(', '))),
        h('button', { class: 'icon-btn ghost', 'aria-label': 'Close', title: 'Close (Esc)', onclick: closeModal }, icon('x'))),
      sent && h('div', { class: 'warn-box' }, 'Already in the kitchen. Fewer or Remove prints a CANCELLED ticket; a new note prints a CHANGED ticket.'),
      h('div', { class: 'edit-grid' },
        h('div', { class: 'edit-field' }, h('label', {}, 'How many'), h('div', { class: 'qty-pick' }, minus, qtyEl, plus)),
        h('div', { class: 'edit-field' }, h('label', {}, 'Price each'),
          priceOk ? price : h('div', { class: 'muted small price-locked' }, money(it.unit_price_cents), ' · ', it.parent_id ? 'priced by its set' : 'locked: part of the bill is paid')),
        h('div', { class: 'edit-field edit-total' }, h('label', {}, 'Line total'), total)),
      courseRow, teamRow,
      h('div', { class: 'edit-field' }, h('label', {}, 'Note'), chips, note),
      h('div', { class: 'edit-field' }, h('label', {}, 'Discount'), discChips, disc),
      h('div', { class: 'edit-actions' },
        remove,
        sent && h('button', { title: 'Print this dish on a kitchen ticket again', onclick: act(async () =>
          reportPrint(await call('reprint_kitchen_items', { orderId, itemIds: [it.id] }), `${it.name} printed again`, true)) }, icon('printer', 18), 'Print again'),
        h('span', { class: 'grow' }),
        h('button', { onclick: closeModal }, 'Cancel'),
        h('button', { class: 'primary save-line', onclick: save }, 'Save')));
    document.querySelector('#modal .dialog')?.classList.add('line-editor');
    // Esc closes; Enter in a box saves (above).
    document.removeEventListener('keydown', state.modalKeys);
    state.modalKeys = (e) => { if (e.key === 'Escape') closeModal(); };
    document.addEventListener('keydown', state.modalKeys);
    redraw();
  }

  function draw() {
    const hasDetails = ['customer_name', 'customer_phone', 'delivery_address', 'notes', 'external_ref', 'customer_id'].some((k) => order[k]);
    state.orderEmpty = !order.items.length && !order.payments?.length && !hasDetails ? orderId : null;
    setKids(lines, h('table', { class: 'ticket-lines' }, order.items.map((it) => {
      const sent = !!it.sent_at;
      // A dish inside a set: shown under the set, no price or stepper.
      if (it.parent_id) {
        return h('tr', { class: 'set-dish' }, h('td', { colspan: 4 },
          it.section_name && h('span', { class: 'muted' }, `${it.section_name}: `), it.qty > 1 ? `${it.qty} × ${it.name}` : it.name,
          it.with_name && h('span', { class: 'muted' }, ` with ${it.with_name}`),
          it.options.length > 0 && h('span', { class: 'muted' }, ` · ${it.options.map((o) => o.name).join(', ')}`),
          sent ? h('span', { class: 'sent' }, ' ✓') : '',
          it.held && !sent ? h('span', { class: 'badge held' }, 'waits for mains') : '',
          it.starter && !sent ? h('span', { class: 'badge' }, 'starter') : '',
          it.name_alt && h('div', { class: 'alt', lang: 'th' }, it.name_alt)));
      }
      const toggleEdit = () => openLineEditor(it);
      const row = h('tr', {},
        h('td', { class: 'line-name', title: 'Note, discount, remove', onclick: toggleEdit }, h('div', {}, it.name, sent ? h('span', { class: 'sent' }, ' ✓') : '',
          it.held && !sent ? h('span', { class: 'badge held' }, 'waits for mains') : '',
          it.starter && !sent ? h('span', { class: 'badge' }, 'starter') : '',
          it.prep && !sent ? h('span', { class: 'badge prep' }, it.prep_sent_at ? 'preparing' : 'prep ahead') : '',
          it.team_id != null && teamName(it.team_id) ? h('span', { class: 'badge team', title: 'Made by this team on this order' }, teamName(it.team_id)) : ''),
          it.name_alt && h('div', { class: 'alt', lang: 'th' }, it.name_alt),
          it.options.length > 0 && h('div', { class: 'muted' }, it.options.map((o) => o.name).join(', ')),
          it.note && h('div', { class: 'muted' }, '“', it.note, '”'),
          it.qty > 1 && h('div', { class: 'muted' }, `${it.qty} × ${money(it.unit_price_cents)}`),
          it.discount_cents ? h('div', { class: 'muted' }, `discount -${money(it.discount_cents)}`) : ''),
        h('td', { class: 'qty' }, h('div', { class: 'stepper' },
          h('button', { onclick: act(async () => {
            await setLine(it, { qty: it.qty - 1 });
            if (sent) toast(`${it.name} −1 · CANCELLED ticket sent to the kitchen`);
          }) }, '−'),
          h('span', {}, it.qty),
          h('button', { onclick: act(() => (sent
            ? (it.is_set ? openSetPicker(state.menu.find((p) => p.id === it.product_id) ?? { id: it.product_id, name: it.name, price_cents: it.unit_price_cents })
              : addItem(it.product_id, it.options.map((o) => o.option_id), it.variant_id, null, it.team_id ?? null))
            : setLine(it, { qty: it.qty + 1 }))) }, '+'))),
        h('td', { class: 'num' }, money(it.line_total_cents)),
        h('td', {}, h('button', { class: 'icon-btn edit-btn', title: 'Change: how many, price, note, discount, remove', 'aria-label': `Change ${it.name}`,
          onclick: toggleEdit }, '✎')));
      return row;
    })));
    if (!order.items.length) lines.append(h('div', { class: 'empty small' }, icon('orders', 36), h('p', {}, 'Tap menu items to add them')));
    setKids(totals, 
      h('tr', {}, h('td', {}, 'Subtotal'), h('td', { class: 'num' }, money(order.subtotal_cents))),
      order.discount_cents ? h('tr', {}, h('td', {}, 'Discount'), h('td', { class: 'num' }, '-' + money(order.discount_cents))) : '',
      h('tr', { class: 'service-row', onclick: showTax, title: 'Change the tax on this order' },
        h('td', {}, (order.tax_bp != null ? `Tax ${pct(order.tax_bp)}% ` : 'Tax ') + (order.tax_incl ? '(included) ' : ''), h('span', { class: 'edit-link' }, 'change')),
        h('td', { class: 'num' }, money(order.tax_cents))),
      (order.service_bp || state.service?.rate_bp) ? h('tr', { class: 'service-row', onclick: showService, title: 'Change or remove the service charge' },
        h('td', {}, `${state.service?.label || 'Service charge'} ${order.service_bp ? pct(order.service_bp) + '%' : '(none)'} `, h('span', { class: 'edit-link' }, 'change')),
        h('td', { class: 'num' }, money(order.service_cents))) : '',
      h('tr', { class: 'grand' }, h('td', {}, 'Total'), h('td', { class: 'num' }, money(order.total_cents))),
      order.paid_cents ? h('tr', {}, h('td', {}, 'Paid so far'), h('td', { class: 'num' }, '-' + money(order.paid_cents))) : '',
      order.paid_cents ? h('tr', { class: 'grand' }, h('td', {}, 'Left to pay'), h('td', { class: 'num' }, money(order.total_cents - order.paid_cents))) : '');
    // A set's own line is for the bill; its dishes are what the kitchen gets.
    const qty = (f) => order.items.filter((i) => !i.is_set).filter(f).reduce((n, i) => n + i.qty, 0);
    const unsent = qty((i) => !i.sent_at && !i.held);
    const waiting = qty((i) => !i.sent_at && i.held);
    const starters = qty((i) => !i.sent_at && !i.held && i.starter);
    const byCourse = order.order_type === 'dine_in' && !allCourses.checked && (starters || waiting);
    sendBtn.textContent = !unsent ? 'Send to kitchen'
      : byCourse && starters ? `Send starters (${starters})` : `Send to kitchen (${unsent + (allCourses.checked ? waiting : 0)})`;
    sendBtn.disabled = !unsent && !(allCourses.checked && waiting);
    sendBtn.classList.toggle('ready', !sendBtn.disabled);
    const mainsSent = order.order_type === 'dine_in' && order.items.some((i) => i.sent_at && !i.starter);
    mainsBtn.textContent = waiting ? `Call mains (${waiting})` : 'Call mains again';
    mainsBtn.title = waiting ? 'Send the mains to the kitchen' : 'Print a short "MAIN COURSE" ticket so the kitchen fires the mains now';
    mainsBtn.hidden = !waiting && !mainsSent;
    setKids(payBtn, h('span', {}, order.paid_cents ? 'Pay the rest' : 'Finish & Pay'), h('strong', {}, money(order.total_cents - (order.paid_cents ?? 0))));
    const count = order.items.filter((i) => !i.parent_id).reduce((n, i) => n + i.qty, 0);
    setKids(phoneBar, layout?.classList.contains('show-ticket') ? '← Back to menu' : `View order · ${count} item(s)`,
      h('strong', {}, money(order.total_cents)));
  }
  allCourses.onchange = () => draw();

  // --- secondary actions: move table, customer details, cancel ---
  // Move: the whole order to another table (joining its bill if busy), or some dishes.
  // A dialog with every table as a big button, floor by floor: pick one, then Move.
  const showMove = act(async () => {
    const floors = await call('list_floors');
    const statuses = await Promise.all(floors.map((f) => call('floor_status', { floorId: f.id })));
    const bill = order.items.filter((i) => !i.parent_id);
    const pick = new Map();
    let target = null, some = false;
    const goTo = (o, msg) => { closeModal(); toast(msg); state.orderId = o.id; renderShell(); };
    const mode = h('div', { class: 'seg' });
    const dishes = h('div', { class: 'stack dish-pick' });
    const tables = h('div', { class: 'stack move-tables' });
    const ok = h('button', { class: 'primary' });
    const drawMove = () => {
      setKids(mode, [[false, 'Whole order'], [true, 'Some dishes']].map(([v, l]) =>
        h('button', { class: some === v ? 'on' : '', onclick: () => { some = v; drawMove(); } }, l)));
      dishes.hidden = !some;
      setKids(dishes, some && bill.map((it) => {
        const q = pick.get(it.id) ?? 0;
        const set = (v) => { v = Math.max(0, Math.min(it.qty, v)); if (v) pick.set(it.id, v); else pick.delete(it.id); drawMove(); };
        return h('div', { class: `row dish-row ${q ? 'on' : ''}`, onclick: () => set(q < it.qty ? q + 1 : 0) },
          h('span', { style: 'flex:1' }, it.name),
          h('div', { class: 'stepper', onclick: (e) => e.stopPropagation() },
            h('button', { onclick: () => set(q - 1) }, '−'), h('span', {}, `${q}/${it.qty}`), h('button', { onclick: () => set(q + 1) }, '+')));
      }));
      const groups = floors.map((f, i) => [f, statuses[i].filter((t) => t.id !== order.table_id && t.is_table !== false)]).filter(([, ts]) => ts.length);
      setKids(tables, groups.map(([f, ts]) => [
        groups.length > 1 && h('div', { class: 'pay-label' }, f.name),
        h('div', { class: 'table-pick' }, ts.map((t) => h('button', {
          class: `${t.order_id ? 'busy' : ''} ${target?.id === t.id ? 'on' : ''}`,
          title: t.order_id ? 'Has a bill: the dishes join it' : 'Free table',
          onclick: () => { target = t; drawMove(); },
        }, h('b', {}, t.name), h('small', {}, t.order_id ? `bill ${money(t.total_cents)}` : 'free')))),
      ]));
      if (!groups.length) tables.append(h('div', { class: 'muted' }, 'There is no other table to move to.'));
      const n = [...pick.values()].reduce((a, b) => a + b, 0);
      ok.disabled = !target || (some && !n);
      ok.textContent = !target ? 'Tap a table' : some && !n ? 'Tap the dishes to move'
        : some ? `Move ${n} dish(es) to ${target.name}` : target.order_id ? `Join ${target.name}'s bill` : `Move to ${target.name}`;
    };
    ok.onclick = act(async () => {
      if (!some) {
        const o = await call('merge_into_table', { orderId, tableId: target.id });
        // The answer says what happened (the table may have got a bill meanwhile).
        return goTo(o, o.id !== orderId ? `Joined onto ${o.table_name}: one bill` : `Moved to ${o.table_name}`);
      }
      const all = bill.every((i) => (pick.get(i.id) ?? 0) >= i.qty);
      const o = await call('move_items', { orderId, tableId: target.id, items: [...pick.entries()] });
      if (all) return goTo(o, `Everything moved to ${o.table_name}`);
      closeModal();
      order = await call('get_order', { id: orderId });
      draw();
      toast(`Moved to ${o.table_name} (nothing reprinted)`);
    });
    drawMove();
    showModal(h('h3', {}, 'Move to another table'), mode, dishes, tables,
      h('div', { class: 'muted small' }, 'A table that already has a bill (yellow): the dishes join that bill. Dishes already sent stay sent.'),
      h('div', { class: 'row' }, ok, h('span', { class: 'grow' }), h('button', { onclick: closeModal }, 'Cancel')));
    document.querySelector('#modal .dialog')?.classList.add('move-dialog');
  });
  const showDetails = () => {
    const f = customerFields(order.order_type, order);
    setKids(extra, h('div', { class: 'stack card' }, f.el, h('div', { class: 'row' },
      h('button', { class: 'primary', onclick: act(async () => {
        order = await call('update_order_details', { id: orderId, details: f.value() });
        setKids(extra); header(); toast('Saved');
      }) }, 'Save'),
      h('button', { onclick: () => setKids(extra) }, 'Close'))));
  };
  // Service charge on this order only: remove it, pick a rate or type one.
  const showService = () => {
    const presets = [...new Set([state.service?.rate_bp || 0, 1000, 1250, 1500].filter((b) => b > 0))].sort((a, b) => a - b);
    const custom = h('input', { inputmode: 'decimal', placeholder: '%', style: 'width:80px', value: order.service_bp ? pct(order.service_bp) : '' });
    const set = (bp) => act(async () => {
      order = await call('set_service_charge', { orderId, rateBp: bp });
      setKids(extra); draw();
      toast(bp ? `Service charge ${pct(bp)}%` : 'Service charge removed');
    });
    setKids(extra, h('div', { class: 'stack card' },
      h('strong', {}, state.service?.label || 'Service charge'),
      h('div', { class: 'row' },
        h('button', { class: order.service_bp === 0 ? 'on' : 'danger', onclick: set(0) }, 'Remove'),
        presets.map((bp) => h('button', { class: order.service_bp === bp ? 'on' : '', onclick: set(bp) }, `${pct(bp)}%`))),
      h('div', { class: 'row' }, 'Other', custom, '%',
        h('button', { class: 'primary', onclick: (e) => {
          const v = Number(custom.value.replace(',', '.'));
          if (!Number.isFinite(v) || v < 0 || v > 30) return toast('Enter a percentage from 0 to 30', true);
          return set(Math.round(v * 100))(e);
        } }, 'Apply'),
        h('button', { onclick: () => setKids(extra) }, 'Close'))));
  };
  // Tax on this order only: menu rates, a rate for every line, or none.
  const showTax = () => {
    const presets = [...new Set([0, state.defaultTax || 0, 500, 2000])].sort((a, b) => a - b);
    const custom = h('input', { inputmode: 'decimal', placeholder: '%', style: 'width:80px', value: order.tax_bp != null ? pct(order.tax_bp) : '' });
    const set = (bp) => act(async () => {
      order = await call('set_order_tax', { orderId, rateBp: bp });
      setKids(extra); draw();
      toast(bp == null ? 'Tax back to each item\'s menu rate' : bp ? `Tax ${pct(bp)}% on every item` : 'No tax on this order');
    });
    setKids(extra, h('div', { class: 'stack card' },
      h('strong', {}, 'Tax on this order'),
      h('div', { class: 'row' },
        h('button', { class: order.tax_bp == null ? 'on' : '', onclick: set(null) }, 'Menu rates'),
        presets.map((bp) => h('button', { class: order.tax_bp === bp ? 'on' : '', onclick: set(bp) }, bp ? `${pct(bp)}%` : 'No tax (0%)'))),
      h('div', { class: 'row' }, 'Other', custom, '%',
        h('button', { class: 'primary', onclick: (e) => {
          const v = Number(custom.value.replace(',', '.'));
          if (!Number.isFinite(v) || v < 0 || v > 100) return toast('Enter a percentage from 0 to 100', true);
          return set(Math.round(v * 100))(e);
        } }, 'Apply'),
        h('button', { onclick: () => setKids(extra) }, 'Close')),
      h('div', { class: 'muted small' }, '"Menu rates" uses each item\'s own tax. A rate here applies to every item on this order, including ones added later.')));
  };
  // Cancel: admins and managers just confirm (no PIN). Staff type a PIN in
  // the same box: their own, or a manager's or admin's once the kitchen has
  // the order. The server checks the same rules (cancel_order).
  const cancelBtn = () => h('button', { class: 'danger', onclick: act(async () => {
    const sent = order.items.some((i) => i.sent_at);
    const noPin = isManager();
    // Settings → Orders → Cancelling an order: CANCELLED ticket on (default) or off.
    const prints = await invoke('get_setting', { key: 'print_cancelled_order' }).then((v) => v !== '0').catch(() => true);
    const kitchenNote = prints ? 'A CANCELLED ticket is printed for any dishes still cooking.' : 'Nothing is printed for the kitchen.';
    let r;
    const submit = async (pin) => {
      try { r = await invoke('cancel_order', { orderId, reason: '', pin }); } catch (e) {
        const msg = String(e?.message ?? e);
        if (isDeadOrder(msg, { orderId })) { setTimeout(leaveDeadOrder); return 'This order was closed on another till'; }
        if (/permission/i.test(msg)) return noPin ? 'You can no longer cancel without a PIN: log in again' : sent ? 'This order is already in the kitchen: a manager or admin PIN is needed' : 'A PIN is needed';
        return /invalid pin/i.test(msg) ? 'Wrong PIN, try again' : msg;
      }
    };
    const title = `Cancel order #${order.id}?`, okLabel = 'Cancel order', okClass = 'danger paybig', keepLabel = 'Keep the order';
    const lines = order.items.filter((i) => i.parent_id == null);
    const count = lines.reduce((n, i) => n + i.qty, 0);
    const done = noPin
      ? await confirmDialog({
        title, okLabel, okClass, keepLabel, submit: () => submit(null),
        hint: [
          h('div', {}, count ? `${count} item${count === 1 ? '' : 's'} · ${money(order.total_cents)}` : 'Nothing on it yet.'),
          h('div', { class: sent ? 'cancel-warn' : '' }, sent
            ? `The kitchen already has this order. ${kitchenNote}`
            : 'The kitchen has not got this order yet: nothing is printed.'),
          h('div', { class: 'muted small' }, 'This cannot be undone.'),
        ],
      })
      : await pinDialog({
        title, okLabel, okClass, keepLabel, submit,
        hint: sent ? `The kitchen already has this order. Enter a manager or admin PIN to cancel it. ${kitchenNote}` : 'Enter your PIN to confirm.',
      });
    if (!done) return;
    closeOrder();
    // Dishes still cooking get a CANCELLED ticket (when switched on; else nothing is printed).
    if (r?.error) toast(`Order #${order.id} cancelled, but the CANCELLED ticket did not print (${r.error}). Tell the kitchen.`, true);
    else toast(`Order #${order.id} cancelled` + (r?.attempted ? ' · CANCELLED ticket sent to the kitchen' : ''));
  }) }, icon('x', 20), h('span', {}, 'Cancel'));

  // The header names the order the way staff say it: the table for dine-in,
  // the platform or customer otherwise. The order number goes under it.
  const head = h('div', { class: 'ticket-head' });
  function header() {
    const dine = order.order_type === 'dine_in';
    const title = orderTitle(order);
    setKids(head, h('div', { class: 'row nowrap' },
      h('button', { class: 'icon-btn back-ticket', onclick: closeOrder, title: 'Back', 'aria-label': 'Back' }, icon('back')),
      h('div', { class: 'grow' },
        h('div', { class: 'row nowrap head-title' }, h('strong', { class: 'big-id' }, title), title !== TYPE[order.order_type] && typeChip(order.order_type)),
        h('div', { class: 'muted small head-sub' }, [
          `#${order.id}`, dine && order.floor_name, order.customer_phone, order.delivery_address, order.notes,
          `opened ${ago(order.opened_at)} ago by ${order.opened_by_name}`,
        ].filter(Boolean).join(' · '))),
      // Phones: the ticket fills the screen; this goes back to the menu to add more.
      h('button', { class: 'to-menu', title: 'Back to the menu to add dishes', onclick: () => { layout.classList.remove('show-ticket'); draw(); } },
        '+ Add', h('span', { class: 'wide-only' }, ' dishes'))));
  }
  header();
  draw();
  drawMenu();
  // Sold-out marks may have changed on another device: refresh them.
  call('menu').then((m) => { state.menu = m; drawMenu(); }).catch(() => {});
  layout = h('div', { class: 'order' },
    h('div', { class: 'menu' }, h('div', { class: 'row menu-top' }, h('div', { class: 'searchbox' }, icon('search', 20), search), otherBtn, soldBtn), cats, subBar, grid, picker),
    h('div', { class: 'ticket' },
      head,
      // What you do to the order sits under its name; the foot is the money and the kitchen.
      h('div', { class: 'tools' },
        order.order_type === 'dine_in'
          ? h('button', { onclick: showMove }, icon('move', 20), h('span', {}, 'Move'))
          : h('button', { onclick: showDetails }, icon('user', 20), h('span', {}, 'Customer')),
        h('button', { title: 'Print the kitchen ticket again', onclick: act(async () => reportPrint(await call('reprint_kitchen', { orderId }), 'Kitchen ticket reprinted', true)) }, icon('printer', 20), h('span', {}, 'Reprint')),
        h('button', { title: 'Print the bill', onclick: act(async () => printResult(await call('print_receipt', { orderId }), 'Bill printed')) }, icon('receipt', 20), h('span', {}, 'Bill')),
        h('button', { title: 'Open the cash drawer', onclick: act(async () => printResult(await call('open_drawer'), 'Drawer opened')) }, icon('drawer', 20), h('span', {}, 'Drawer')),
        cancelBtn()),
      lines, extra,
      h('div', { class: 'ticket-foot' },
        totals,
        order.order_type === 'dine_in' && h('label', { class: 'switch', title: 'Otherwise starters go first and the rest waits for "Call mains"' },
          allCourses, h('span', {}), 'All courses together'),
        h('div', { class: 'row nowrap send-row' }, sendBtn, mainsBtn),
        payBtn)),
    phoneBar);
  screen(layout);
  draw();
  if (!matchMedia('(pointer: coarse)').matches) search.focus();
}

// ---------- payment dialog ----------

function closeModal() {
  const m = document.getElementById('modal');
  m.hidden = true;
  setKids(m);
  document.removeEventListener('keydown', state.modalKeys);
  // Run once, however the dialog was closed (see payDialog).
  const closed = state.onModalClose;
  state.onModalClose = null;
  closed?.();
}

function showModal(...kids) {
  const m = document.getElementById('modal');
  setKids(m, h('div', { class: 'dialog card stack' }, ...kids));
  m.hidden = false;
}

/// A PIN keypad in a dialog, e.g. a manager approving something on this
/// till (nobody is logged in or out). `submit(pin)` does the action and
/// returns nothing when it worked, or a message to show (wrong PIN…).
/// Resolves true once it worked, false if the dialog was closed.
function pinDialog({ title, hint, okLabel, okClass = 'primary paybig', keepLabel = 'Cancel', submit }) {
  return new Promise((resolve) => {
    let pin = '', busy = false;
    const dots = h('div', { class: 'pin-dots' });
    const err = h('div', { class: 'change short' });
    const ok = h('button', { class: okClass }, okLabel);
    const drawPin = () => { setKids(dots, ...Array.from({ length: Math.max(4, pin.length) }, (_, i) => h('span', { class: i < pin.length ? 'on' : '' }))); ok.disabled = pin.length < 4; };
    const key = (k) => { if (k === '⌫') pin = pin.slice(0, -1); else if (k === 'C') pin = ''; else if (pin.length < 8) pin += k; err.textContent = ''; drawPin(); };
    const go = async () => {
      if (pin.length < 4 || busy) return;
      busy = true;
      let msg;
      try { msg = await submit(pin); } catch (e) { msg = String(e?.message ?? e); } finally { busy = false; }
      if (msg) { pin = ''; drawPin(); err.textContent = msg; return; }
      state.onModalClose = null;
      closeModal();
      resolve(true);
    };
    ok.onclick = go;
    showModal(h('h3', {}, title), h('div', { class: 'muted' }, hint), dots, err,
      h('div', { class: 'keypad' }, ['1', '2', '3', '4', '5', '6', '7', '8', '9', 'C', '0', '⌫'].map((k) => h('button', { onclick: () => key(k) }, k))),
      ok, h('div', { class: 'row' }, h('span', { class: 'grow' }), h('button', { onclick: closeModal }, keepLabel)));
    drawPin();
    state.onModalClose = () => resolve(false);
    document.removeEventListener('keydown', state.modalKeys);
    document.activeElement?.blur();
    state.modalKeys = (e) => {
      if (/^[0-9]$/.test(e.key)) key(e.key);
      else if (e.key === 'Backspace') key('⌫');
      else if (e.key === 'Enter') { e.preventDefault(); go(); }
      else if (e.key === 'Escape') closeModal();
    };
    document.addEventListener('keydown', state.modalKeys);
  });
}

/// A yes / no box in the same look as pinDialog, for an action that needs
/// no PIN. `submit()` does the action and returns nothing when it worked, or
/// a message to show. Resolves true once it worked, false if closed. Taps on
/// the OK button in the first moment after it opens are ignored (a double tap
/// on the button that opened it), as are taps while it is working.
function confirmDialog({ title, hint, okLabel, okClass = 'primary paybig', keepLabel = 'Cancel', submit }) {
  return new Promise((resolve) => {
    let busy = false;
    const armedAt = Date.now() + 500;
    const err = h('div', { class: 'change short' });
    const ok = h('button', { class: okClass }, okLabel);
    const go = async () => {
      if (busy || Date.now() < armedAt) return;
      busy = true;
      ok.disabled = true;
      err.textContent = '';
      let msg;
      try { msg = await submit(); } catch (e) { msg = String(e?.message ?? e); }
      if (msg) { busy = false; ok.disabled = false; err.textContent = msg; return; }
      state.onModalClose = null;
      closeModal();
      resolve(true);
    };
    ok.onclick = go;
    const keep = h('button', { onclick: () => { if (!busy) closeModal(); } }, keepLabel);
    showModal(h('h3', {}, title), h('div', { class: 'stack confirm-hint' }, hint), err,
      ok, h('div', { class: 'row' }, h('span', { class: 'grow' }), keep));
    state.onModalClose = () => resolve(false);
    document.removeEventListener('keydown', state.modalKeys);
    document.activeElement?.blur();
    // Enter confirms only on its own: not the Enter at the end of a
    // barcode scan (a burst of keys), and not while another button
    // (Keep the order) has the focus.
    let lastKey = 0;
    state.modalKeys = (e) => {
      if (e.key === 'Escape') { if (!busy) closeModal(); return; }
      if (e.key !== 'Enter') { lastKey = Date.now(); return; }
      e.preventDefault();
      if (e.repeat || Date.now() - lastKey < 300) return;
      const f = document.activeElement;
      if (f === keep) { if (!busy) closeModal(); return; }
      if (f instanceof HTMLButtonElement && f !== ok) return;
      go();
    };
    document.addEventListener('keydown', state.modalKeys);
  });
}

/// Finish & Pay settings (Manage → Bills & receipts → Payments). Not set =
/// the defaults below, which are how the app always worked.
const PAY_KEYS = ['tip_presets', 'tips_on', 'cash_quick', 'split_max', 'pay_default_method', 'exact_cash_pays', 'pay_name_cash', 'pay_name_card'];
const PAY_NAME_MAX = 24;
const PAY_DEFAULTS = { tips: [5, 10, 15], notes: [5, 10, 20, 50, 100], splitMax: 6 };
/// "5, 10,15" → [5, 10, 15]; null if empty or any part is not a number.
function numberList(v) {
  const parts = String(v ?? '').split(/[,;\s]+/).filter(Boolean);
  const nums = parts.map(Number);
  return parts.length && nums.every(Number.isFinite) ? nums : null;
}
/// At most two decimals (12.5 or 0.25, not 0.125).
const twoDecimals = (x) => Math.abs(x * 100 - Math.round(x * 100)) < 1e-6;
const goodTips = (l) => l?.length === 3 && l.every((p) => p > 0 && p <= 100 && twoDecimals(p));
const goodNotes = (l) => l?.length >= 1 && l.length <= 8 && l.every((x) => x > 0 && x <= 1e6 && twoDecimals(x));
const goodSplit = (n) => Number.isInteger(n) && n >= 2 && n <= 12;
function paySettings(raw = {}) {
  const tips = numberList(raw.tip_presets), notes = numberList(raw.cash_quick), n = Number(raw.split_max ?? NaN);
  return {
    tips: goodTips(tips) ? tips : PAY_DEFAULTS.tips,
    tipsOn: raw.tips_on !== '0',
    /// In cents, smallest first.
    notes: [...new Set((goodNotes(notes) ? notes : PAY_DEFAULTS.notes).map((x) => Math.round(x * 100)))].sort((a, b) => a - b),
    splitMax: goodSplit(n) ? n : PAY_DEFAULTS.splitMax,
    method: raw.pay_default_method === 'card' ? 'card' : 'cash',
    exactPays: raw.exact_cash_pays === '1',
  };
}

/// "Finish & Pay": choose Cash or Card (or the delivery platform); for cash
/// enter what the customer handed over and see the change. `pay(method,
/// tenderedCents)` takes the payment and returns the paid order.
/// Finish & Pay on one screen: how much (everything, split evenly, an
/// amount or chosen dishes), an optional tip, the payment method, and for
/// cash what was handed over. One button takes the payment.
/// `pay(input)` takes one payment and returns the order (still open after a
/// part payment). `refresh()` reloads the order screen: after a payment is
/// taken back and when the dialog closes, however it closes.
async function payDialog(order, refresh, pay) {
  const cfg = paySettings(await invoke('get_settings', { keys: PAY_KEYS }).catch(() => ({})));
  const reload = () => { Promise.resolve(refresh()).catch(() => {}); };
  let busy = false;
  let mode = 'all'; // all | even | amount | dishes
  let even = { n: 2, done: 0 };
  let tipMode = 0; // 0 | basis points | 'other'
  let method = cfg.method;
  const typed = { cash: '', amount: '', tip: '' };
  let focus = 'cash';
  const pick = new Map();
  const parse = (t) => { try { return t ? parseMoney(t) : null; } catch { return null; } };
  const left = () => Math.max(0, order.total_cents - (order.paid_cents ?? 0));
  const lineDue = (it) => it.line_total_cents + (order.tax_incl ? 0 : it.tax_cents);
  const bill = () => order.items.filter((i) => !i.parent_id && i.qty - i.paid_qty > 0);
  const share = () => {
    if (!pick.size) return 0;
    const all = order.items.filter((i) => !i.parent_id).reduce((n, i) => n + lineDue(i), 0);
    let sel = 0;
    for (const [id, q] of pick) { const it = order.items.find((i) => i.id === id); sel += Math.floor(lineDue(it) * q / it.qty); }
    const everything = bill().every((i) => (pick.get(i.id) ?? 0) >= i.qty - i.paid_qty);
    return everything ? left() : Math.min(left(), Math.round(order.total_cents * sel / (all || 1)));
  };
  const evenAmount = () => (even.n - even.done <= 1 ? left() : Math.round(left() / (even.n - even.done)));
  const due = () => ({ all: left(), even: evenAmount(), amount: Math.min(parse(typed.amount) ?? 0, left()), dishes: share() }[mode]);
  const tip = () => (tipMode === 'other' ? parse(typed.tip) ?? 0 : Math.round(due() * tipMode / 10000));
  const target = () => due() + tip();
  const cash = () => (typed.cash ? parse(typed.cash) : target());
  const setKeys = (fn) => {
    document.removeEventListener('keydown', state.modalKeys);
    state.modalKeys = fn;
    document.addEventListener('keydown', fn);
  };

  const key = (k) => {
    let t = typed[focus];
    if (k === '⌫') t = t.slice(0, -1);
    else if (k === 'C') t = '';
    else if (k === '.' && t.includes('.')) return;
    else if (/\.\d\d$/.test(t)) return;
    else if (t.replace('.', '').length >= 7) return;
    else t += k;
    typed[focus] = t;
    draw();
  };
  const seg = (items, current, onPick) => h('div', { class: 'seg' }, items.map(([v, label, sub]) =>
    h('button', { class: current === v ? 'on' : '', onclick: () => { onPick(v); draw(); } }, label, sub && h('small', {}, sub))));
  // A value the keypad types into; tap it to type there.
  const field = (name, label, value, hint, bad = false) => h('button', { class: `pay-field ${focus === name ? 'focus' : ''}`, onclick: () => { focus = name; draw(); } },
    h('span', { class: 'muted' }, label), h('strong', {}, value), hint && h('small', { class: bad ? 'bad' : '' }, hint));

  const take = async () => {
    if (busy) return;
    const d = due();
    if (d <= 0) return toast(mode === 'dishes' ? 'Tap the dishes this person pays for' : 'Enter an amount to pay', true);
    if (method === 'cash' && (cash() ?? 0) < target()) return toast(`Not enough cash: ${money(target())} to pay`, true);
    busy = true;
    try {
      const input = { method, tendered_cents: method === 'cash' ? cash() : 0, tip_cents: tip(),
        amount_cents: mode === 'all' || mode === 'dishes' ? null : d, items: mode === 'dishes' ? [...pick.entries()] : null };
      const before = order.payments?.length ?? 0;
      const res = await pay(input);
      const p = res.payments?.[before] ?? res.payments?.at(-1);
      order = res;
      // A fast second tap right after a part payment must not pay the rest too.
      if (res.status === 'paid') busy = false; else setTimeout(() => { busy = false; }, 700);
      if (mode === 'even') even.done += 1; else mode = 'all';
      if (mode === 'even' && even.done >= even.n) mode = 'all';
      pick.clear();
      typed.cash = ''; typed.amount = ''; typed.tip = '';
      tipMode = 0; focus = 'cash';
      const next = () => (res.status === 'paid' ? closeModal() : draw());
      if (method === 'cash' && p?.change_cents > 0) {
        showModal(h('div', { class: 'center', style: 'margin:0' },
          h('div', { class: 'muted' }, 'Give change'), h('div', { class: 'huge' }, money(p.change_cents)),
          h('div', { class: 'muted' }, `${payName('cash')} ${money(p.tendered_cents)} − ${money(p.amount_cents + p.tip_cents)}`),
          res.status !== 'paid' && h('div', { class: 'muted' }, `Left to pay: ${money(left())}`),
          h('button', { class: 'primary pay', onclick: next }, res.status === 'paid' ? 'Done' : 'Next payment')));
        setKeys((e) => { if (e.key === 'Enter' || e.key === 'Escape') next(); });
      } else next();
    } catch (e) {
      busy = false;
      if (!e?.handled) toast(String(e?.message ?? e), true);
    }
  };

  function draw() {
    if (method !== 'cash' && focus === 'cash') focus = mode === 'amount' ? 'amount' : tipMode === 'other' ? 'tip' : 'cash';
    const d = due(), t = tip(), tg = d + t, c = cash();
    const methods = [['cash', payName('cash')], ['card', payName('card')], ...(order.channel_id ? [['other', order.channel_name]] : [])];
    // Default names keep the old wording; a renamed method shows its name.
    const takeCash = payName('cash') === 'Cash' ? 'Take cash' : payName('cash');
    const keypadOn = method === 'cash' || focus === 'amount' || focus === 'tip';
    const quick = [...new Set([...cfg.notes, Math.ceil(tg / 100) * 100, Math.ceil(tg / 500) * 500, Math.ceil(tg / 1000) * 1000]
      .filter((x) => x > tg))].sort((a, b) => a - b).slice(0, 4);
    const enough = method !== 'cash' || (c != null && c >= tg);
    const okLabel = d <= 0 ? (mode === 'dishes' ? 'Tap the dishes this person pays for' : 'Enter an amount')
      : method === 'cash' ? (c > tg ? `${takeCash} ${money(tg)} · change ${money(c - tg)}` : `${takeCash} ${money(tg)}`)
      : method === 'card' ? `${payName('card')} payment taken · ${money(tg)}` : `Paid by ${order.channel_name} · ${money(tg)}`;
    showModal(h('div', { class: 'pay-one' },
      h('div', { class: 'pay-head' },
        h('div', { class: 'grow' }, h('h3', {}, orderTitle(order)),
          h('div', { class: 'muted small' }, [`Order #${order.id}`, orderTitle(order) !== TYPE[order.order_type] && TYPE[order.order_type]].filter(Boolean).join(' · '))),
        h('div', { class: 'pay-total' }, h('span', { class: 'muted' }, 'Total'), h('span', { class: 'big' }, money(order.total_cents))),
        h('button', { class: 'icon-btn ghost', title: 'Close (Esc)', 'aria-label': 'Close', onclick: closeModal }, icon('x'))),
      h('div', { class: 'pay-left stack' },
        order.paid_cents > 0 && h('div', { class: 'pay-parts' },
          order.payments.map((p) => h('div', { class: 'row pay-part' },
            h('span', { style: 'flex:1' }, `✓ ${payName(p.method, order.channel_name)} ${money(p.amount_cents)}`,
              p.tip_cents ? h('span', { class: 'muted' }, ` + tip ${money(p.tip_cents)}`) : ''),
            h('button', { class: 'link-btn', title: 'Take this payment back (a mistake)', onclick: act(async () => {
              order = await call('remove_payment', { orderId: order.id, paymentId: p.id });
              mode = 'all'; even.done = 0; draw();
              reload();
            }) }, 'undo'))),
          h('div', { class: 'row' }, h('strong', { style: 'flex:1' }, 'Left to pay'), h('span', { class: 'big' }, money(left())))),
        h('div', { class: 'pay-label' }, 'Pay'),
        seg([['all', 'Everything', money(left())], ['even', 'Split evenly'], ['amount', 'Amount'], ['dishes', 'Dishes']], mode, (v) => {
          mode = v;
          if (v === 'amount') focus = 'amount';
          else if (focus === 'amount') focus = 'cash';
        }),
        mode === 'even' && h('div', { class: 'row split-n' }, seg(Array.from({ length: cfg.splitMax - 1 }, (_, i) => i + 2).map((n) => [n, `÷ ${n}`]), even.n, (n) => { even = { n, done: 0 }; }),
          h('span', { class: 'muted' }, `Person ${even.done + 1} of ${even.n}: ${money(evenAmount())}`)),
        mode === 'amount' && field('amount', 'Amount to pay now', typed.amount || '0.00', `of ${money(left())} left`),
        mode === 'dishes' && h('div', { class: 'stack dish-pick' }, bill().map((it) => {
          const q = pick.get(it.id) ?? 0;
          const max = it.qty - it.paid_qty;
          const set = (v) => { v = Math.max(0, Math.min(max, v)); if (v) pick.set(it.id, v); else pick.delete(it.id); draw(); };
          return h('div', { class: `row dish-row ${q ? 'on' : ''}`, onclick: () => set(q < max ? q + 1 : 0) },
            h('span', { style: 'flex:1' }, it.name, max < it.qty ? h('span', { class: 'muted' }, ` (${it.paid_qty} paid)`) : ''),
            h('div', { class: 'stepper', onclick: (e) => e.stopPropagation() },
              h('button', { onclick: () => set(q - 1) }, '−'), h('span', {}, `${q}/${max}`), h('button', { onclick: () => set(q + 1) }, '+')));
        })),
        cfg.tipsOn && h('div', { class: 'pay-label' }, 'Tip'),
        cfg.tipsOn && seg([[0, 'No tip'], ...cfg.tips.map((p) => [Math.round(p * 100), `${p}%`]), ['other', 'Other']], tipMode, (v) => {
          tipMode = v;
          if (v === 'other') focus = 'tip'; else if (focus === 'tip') focus = 'cash';
        }),
        cfg.tipsOn && tipMode === 'other' && field('tip', 'Tip', typed.tip || '0.00'),
        // One clear total; the amount and the tip are listed above it only when there is a tip.
        h('div', { class: 'pay-sum' },
          t > 0 && h('div', { class: 'row' }, h('span', { style: 'flex:1' }, 'To pay now'), h('strong', {}, money(d))),
          t > 0 && h('div', { class: 'row' }, h('span', { style: 'flex:1' }, 'Tip'), h('strong', {}, money(t))),
          h('div', { class: 'row grand' }, h('span', { style: 'flex:1' }, t > 0 ? 'Total to take' : 'To pay now'), h('strong', {}, money(tg))))),
      h('div', { class: 'pay-right stack' },
        h('div', { class: 'pay-label' }, 'Payment method'),
        h('div', { class: 'seg methods' }, methods.map(([v, label]) => h('button', { class: method === v ? 'on' : '', onclick: () => { method = v; if (v === 'cash') focus = 'cash'; draw(); } },
          icon({ cash: 'cash', card: 'card' }[v] ?? 'bike', 26), label))),
        method === 'cash' && field('cash', `${payName('cash')} received`, typed.cash || `${money(tg)} (exact)`,
          c == null ? 'Enter an amount like 20 or 12.50' : c >= tg ? `Change ${money(c - tg)}` : `Short by ${money(tg - c)}`, c == null || c < tg),
        method === 'cash' && h('div', { class: 'row quick' }, h('button', { onclick: () => { typed.cash = ''; if (!cfg.exactPays || due() > 0) focus = 'cash'; draw(); if (cfg.exactPays) take(); } }, cfg.exactPays ? 'Pay exact' : 'Exact'),
          quick.map((x) => h('button', { onclick: () => { typed.cash = String(x / 100); focus = 'cash'; draw(); } }, money(x)))),
        method !== 'cash' && !keypadOn && h('div', { class: 'muted pay-hint' }, method === 'card'
          ? `Take ${money(tg)} on the card machine, then confirm below.` : `${order.channel_name} has collected the payment.`),
        keypadOn && h('div', { class: 'keypad' }, ['7', '8', '9', '4', '5', '6', '1', '2', '3', '.', '0', '⌫'].map((k) => h('button', { onclick: () => key(k) }, k))),
        h('button', { class: 'primary paybig pay-ok', disabled: d <= 0 || !enough, onclick: take }, okLabel))));
    document.querySelector('#modal .dialog')?.classList.add('pay-wide');
    setKeys((e) => {
      if (/^[0-9.]$/.test(e.key)) key(e.key);
      else if (e.key === 'Backspace') key('⌫');
      else if (e.key === 'Enter') take();
      else if (e.key === 'Escape') closeModal();
    });
  }
  draw();
  state.onModalClose = reload;
}

/// Inputs for customer details, shown according to order type.
function customerFields(type, o = {}) {
  const name = h('input', { value: o.customer_name ?? '', placeholder: 'Customer name' + (type === 'takeaway' ? ' (optional)' : '') });
  const phone = h('input', { value: o.customer_phone ?? '', placeholder: 'Phone' + (type === 'delivery' ? '' : ' (optional)'), inputmode: 'tel' });
  const addr = h('textarea', { placeholder: 'Delivery address', rows: 2, style: 'width:100%' });
  addr.value = o.delivery_address ?? '';
  const notes = h('input', { value: o.notes ?? '', placeholder: 'Notes (optional)', style: 'flex:1' });
  const ref = h('input', { value: o.external_ref ?? '', placeholder: `${o.channel_name ?? 'Platform'} order number` });
  return {
    el: h('div', { class: 'stack' }, o.channel_id && h('div', { class: 'row' }, ref),
      h('div', { class: 'row' }, name, phone), type === 'delivery' && addr, h('div', { class: 'row' }, notes)),
    value: () => ({
      customer_name: name.value || null, customer_phone: phone.value || null,
      delivery_address: type === 'delivery' ? addr.value || null : null, notes: notes.value || null,
      external_ref: o.channel_id ? ref.value || null : null, customer_id: o.customer_id ?? null,
    }),
  };
}

/// Saved-customer search + editable details. Typing a postcode lists the
/// addresses already saved there (door numbers), all offline.
/// Used for new delivery/collection orders and on the Customers screen.
function customerEditor({ type = 'delivery', customer = null, search = true, saveToggle = true, onPick = () => {} } = {}) {
  let selected = customer;
  const input = (ph, attrs = {}) => h('input', { placeholder: ph, ...attrs });
  const f = {
    name: input('Customer name'),
    phone: input('Phone', { inputmode: 'tel' }),
    line1: input('House number and street', { style: 'flex:1; min-width:220px' }),
    line2: input('Flat, building (optional)'),
    town: input('Town'),
    postcode: input('Postcode', { style: 'width:120px; text-transform:uppercase' }),
    notes: input('Customer notes, e.g. gate code (saved)', { style: 'flex:1' }),
  };
  const picked = h('div');
  const results = h('div', { class: 'stack' });
  const suggestions = h('div', { class: 'chips' });
  const save = h('input', { type: 'checkbox', checked: true });
  const find = input('Find saved customer: phone, name or postcode', { style: 'flex:1' });

  const fill = (c, addressOnly = false) => {
    if (!addressOnly) {
      selected = c;
      f.name.value = c?.name ?? '';
      f.phone.value = c?.phone ?? '';
      f.notes.value = c?.notes ?? '';
      onPick(c);
    }
    f.line1.value = c?.address_line1 ?? '';
    f.line2.value = c?.address_line2 ?? '';
    f.town.value = c?.town ?? '';
    f.postcode.value = c?.postcode ?? '';
    drawPicked();
  };
  const drawPicked = () => setKids(picked, selected
    ? h('div', { class: 'row muted' }, `Saved customer · ${selected.order_count} order(s)`,
      search && h('button', { onclick: () => { selected = null; drawPicked(); } }, 'Treat as new customer'))
    : '');
  const row = (c) => h('tr', { class: 'click', onclick: () => { fill(c); setKids(results); find.value = ''; } },
    h('td', {}, h('strong', {}, c.name)), h('td', {}, c.phone ?? ''), h('td', { class: 'muted' }, addressText(c)));

  let t1, t2;
  find.addEventListener('input', () => {
    clearTimeout(t1);
    t1 = setTimeout(async () => {
      const q = find.value.trim();
      if (!q) return setKids(results);
      const list = await call('search_customers', { query: q, limit: 8 });
      setKids(results, list.length ? h('table', {}, list.map(row)) : h('div', { class: 'muted' }, 'No saved customer found: fill in the details below.'));
    }, 150);
  });
  f.postcode.addEventListener('input', () => {
    clearTimeout(t2);
    t2 = setTimeout(async () => {
      const list = f.postcode.value.trim().length >= 5 ? await call('customers_at_postcode', { postcode: f.postcode.value }) : [];
      setKids(suggestions, ...(list.length ? [h('span', { class: 'muted' }, 'Saved at this postcode:')] : []),
        ...list.map((c) => h('button', {
          title: c.name,
          // With no name typed yet, pick the whole customer; otherwise just the address.
          onclick: () => fill(c, f.name.value.trim() !== '' && selected?.id !== c.id),
        }, [c.address_line1, c.address_line2].filter(Boolean).join(', '), h('small', { class: 'muted' }, ` ${c.name}`))));
    }, 150);
  });
  if (customer) fill(customer);
  const delivery = type === 'delivery';

  return {
    el: h('div', { class: 'stack' },
      search && h('div', { class: 'row' }, find), results, picked,
      h('div', { class: 'row' }, f.name, f.phone),
      delivery && h('div', { class: 'row' }, f.postcode, f.line1),
      delivery && suggestions,
      delivery && h('div', { class: 'row' }, f.line2, f.town),
      h('div', { class: 'row' }, f.notes),
      saveToggle && h('label', { class: 'check' }, save, ' Save / update this customer for next time')),
    focus: () => (search ? find : f.name).focus(),
    details: () => ({
      name: f.name.value, phone: f.phone.value || null, address_line1: f.line1.value || null,
      address_line2: f.line2.value || null, town: f.town.value || null, postcode: f.postcode.value || null,
      notes: f.notes.value || null,
    }),
    selected: () => selected,
    /// Save (create or update) when asked to; returns the customer, if any.
    async commit(force = false) {
      if (force || (saveToggle && save.checked)) {
        const d = this.details();
        selected = selected
          ? await call('update_customer', { id: selected.id, customer: d })
          : await call('create_customer', { customer: d });
        drawPicked();
      }
      return selected;
    },
  };
}

/// Open a delivery/collection order for the details in a customerEditor.
async function startCustomerOrder(type, ce, orderNotes) {
  const c = await ce.commit();
  const d = ce.details();
  const o = await call('open_order', { order: {
    order_type: type,
    customer_name: d.name || null,
    customer_phone: d.phone,
    delivery_address: type === 'delivery' ? addressText(d) || null : null,
    notes: orderNotes || d.notes || null,
    customer_id: c?.id ?? null,
  } });
  openOrder(o.id);
}

/// Read-only view of a closed order, with void for admins.
function orderDetail(o, onChange) {
  const reason = h('input', { placeholder: 'Void reason' });
  return h('div', { class: 'card stack' },
    h('h3', {}, `#${o.id} ${TYPE[o.order_type]}${where(o) ? ` · ${where(o)}` : ''} — ${o.status}`),
    h('div', { class: 'muted' }, `Opened ${fmtTime(o.opened_at)} by ${o.opened_by_name}` +
      (o.paid_at ? ` · paid ${fmtTime(o.paid_at)}` : '')),
    (o.customer_phone || o.delivery_address) && h('div', { class: 'muted' }, [o.customer_phone, o.delivery_address].filter(Boolean).join(' · ')),
    h('table', {}, o.items.map((i) => h('tr', {},
      h('td', {}, i.name, i.options.length > 0 && h('div', { class: 'muted' }, i.options.map((x) => x.name).join(', ')),
        i.note && h('div', { class: 'muted' }, i.note)),
      h('td', { class: 'num' }, `${i.qty} × ${money(i.unit_price_cents)}`),
      h('td', { class: 'num' }, i.discount_cents ? '-' + money(i.discount_cents) : ''),
      h('td', { class: 'num' }, money(i.line_total_cents))))),
    h('div', {}, `Tax ${o.tax_incl ? 'included ' : ''}${money(o.tax_cents)} · `, h('strong', {}, `Total ${money(o.total_cents)}`),
      o.tip_cents ? ` · Tip ${money(o.tip_cents)}` : ''),
    o.payments?.length > 0 && h('div', { class: 'muted' }, o.payments.map((p) =>
      `${payName(p.method, o.channel_name)} ${money(p.amount_cents)}` + (p.tip_cents ? ` + tip ${money(p.tip_cents)}` : '') +
      (p.change_cents ? ` (tendered ${money(p.tendered_cents)}, change ${money(p.change_cents)})` : '') + ` · ${p.paid_by_name}`).join('  |  ')),
    o.close_reason && h('div', {}, `Reason: ${o.close_reason}`),
    o.status !== 'cancelled' && h('div', {}, h('button', { onclick: act(async () => {
      printResult(await call('print_receipt', { orderId: o.id }), o.status === 'paid' ? 'Receipt printed' : 'Printed');
    }) }, o.status === 'paid' ? 'Reprint receipt' : 'Print'),
      o.status === 'paid' && h('button', { onclick: act(async () => printResult(await call('print_staff_copy', { orderId: o.id }), 'Staff copy printed')) }, 'Print staff copy')),
    o.status === 'paid' && h('div', {}, h('button', { onclick: act(async () => {
      const p = await call('reprint_kitchen', { orderId: o.id });
      if (p.error) toast(`Did not print: ${p.error}`, true);
      else toast(p.attempted ? 'Kitchen ticket reprinted' : 'No kitchen printer is set up', !p.attempted);
    }) }, 'Reprint kitchen ticket')),
    isManager() && o.status === 'paid' && h('div', { class: 'row' }, reason,
      h('button', { class: 'danger', onclick: act(async () => {
        await call('void_order', { orderId: o.id, reason: reason.value });
        toast('Order voided');
        onChange();
      }) }, 'Void / refund')),
    // A mistake to be done again: remove the order from history and reports.
    isAdmin() && h('div', { class: 'row delete-order' },
      h('span', { class: 'muted grow' }, 'Entered by mistake? Delete removes this order from history and reports for good' +
        (o.status === 'paid' ? ' (stock is put back). To refund a real sale use Void instead.' : '.')),
      confirmBtn('Delete order', `Tap again to delete #${o.id}`, async () => {
        await call('delete_order', { orderId: o.id, reason: reason.value || null });
        toast(`Order #${o.id} deleted`);
        onChange('deleted');
      })));
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
    const notes = h('input', { placeholder: 'Notes for this order (optional)', style: 'flex:1' });
    const ce = customerEditor({ type });
    setKids(form, h('div', { class: 'card stack' }, h('h3', {}, `New ${TYPE[type].toLowerCase()} order`), ce.el,
      h('div', { class: 'row' }, notes),
      h('div', { class: 'row' },
        h('button', { class: 'primary', onclick: act(() => startCustomerOrder(type, ce, notes.value)) }, 'Start order'),
        h('button', { onclick: () => setKids(form) }, 'Cancel'))));
    ce.focus();
  });
  const platformOrder = (ch) => act(async () => {
    const ref = h('input', { placeholder: `${ch.name} order number (optional)` });
    const name = h('input', { placeholder: 'Customer name (optional)' });
    const collects = h('input', { type: 'checkbox' });
    const start = act(async () => {
      const o = await call('open_order', { order: {
        order_type: collects.checked ? 'collection' : 'delivery', channel_id: ch.id,
        external_ref: ref.value || null, customer_name: name.value || null,
      } });
      openOrder(o.id);
    });
    setKids(form, h('div', { class: 'card stack' }, h('h3', {}, `New ${ch.name} order`),
      h('div', { class: 'row' }, ref, name), h('label', { class: 'check' }, collects, ' Customer collects (no platform rider)'),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: start }, 'Start order'),
        h('button', { onclick: () => setKids(form) }, 'Cancel'))));
    ref.focus();
  });
  const channels = await call('list_channels');
  const count = h('span', { class: 'count' });
  const draw = async () => {
    const orders = await call('list_open_orders');
    count.textContent = orders.length;
    setKids(board, orders.length ? h('div', { class: 'cards' },
      orders.map((o) => h('button', { class: `order-card ${o.unsent_count ? 'unsent' : ''}`, onclick: () => openOrder(o.id) },
        h('div', { class: 'row nowrap' }, h('strong', { class: 'big-id' }, orderTitle(o)), orderTitle(o) !== TYPE[o.order_type] && typeChip(o.order_type), h('span', { class: 'spacer' }),
          h('span', { class: 'time' }, ago(o.opened_at))),
        h('div', { class: 'muted small' }, [`#${o.id}`, o.order_type === 'dine_in' && o.floor_name].filter(Boolean).join(' · ')),
        o.customer_phone && h('div', { class: 'muted small' }, o.customer_phone),
        h('div', { class: 'row nowrap foot' },
          h('span', { class: 'muted small' }, `${o.item_count} item(s) · ${o.opened_by_name}`), h('span', { class: 'spacer' }),
          h('strong', { class: 'amt' }, money(o.total_cents))),
        (o.unsent_count > 0 || o.held_count > 0) && h('div', { class: 'row' },
          o.unsent_count > 0 && h('span', { class: 'badge warn' }, `${o.unsent_count} not sent`),
          o.held_count > 0 && h('span', { class: 'badge held' }, 'mains waiting')))))
      : h('div', { class: 'empty small' }, icon('orders', 36), h('p', {}, 'No open orders. Dine-in orders start from Tables.')));
  };
  const newTile = (label, ico, onclick, color = null) =>
    h('button', { class: 'new-tile', style: color ? catStyle(color) : '', onclick }, icon(ico, 26), h('span', {}, label));
  screen(h('div', { class: 'page' },
    h('div', { class: 'new-tiles' },
      // Settings → Orders → Order types can hide the kinds a restaurant never uses.
      typeShown('takeaway') && newTile('Takeaway', 'bag', newOrder('takeaway')),
      typeShown('collection') && newTile('Collection', 'walk', newOrder('collection')),
      typeShown('delivery') && newTile('Delivery', 'bike', newOrder('delivery')),
      channels.map((ch) => newTile(ch.name, 'bike', platformOrder(ch), ch.color || '#374151')),
      newTile('Open drawer', 'drawer', act(async () => printResult(await call('open_drawer'), 'Drawer opened')), '#475569')),
    form, h('div', { class: 'section-title' }, h('h3', {}, 'Open orders'), count), board));
  await draw();
  live(draw);
}

// ---------- history ----------

async function renderHistory() {
  const date = h('input', { type: 'date', value: today() });
  const list = h('div');
  const span = h('div', { class: 'muted small history-span' });
  const detail = h('div', { 'data-hint': 'Tap an order to see its details, reprint or void it.' });
  // Quick dates follow the business day (header), which runs from the last
  // day close: "Today" is the open business day, so orders after midnight
  // before the day is closed are still today. "Yesterday" is the business
  // day closed last, "Last 7 days" the business days dated in the last week.
  // The Day picker shows calendar days (midnight to midnight).
  let mode = 'today'; // today | yesterday | week | day
  let day = null;
  const OPEN_END = 4102444800; // 2100: "up to now", whatever this device's clock says
  const daysAgo = (n) => { const d = new Date(); d.setDate(d.getDate() - n); return d; };
  const quick = h('div', { class: 'seg quick-dates' });
  const drawQuick = () => setKids(quick, [['today', 'Today'], ['yesterday', 'Yesterday'], ['week', 'Last 7 days']].map(([k, label]) => h('button', {
    class: mode === k ? 'on' : '',
    onclick: () => { mode = k; load(); },
  }, label)));
  let loads = 0; // a newer tap wins over a slow earlier load
  const load = act(async () => {
    drawQuick();
    const mine = ++loads;
    if (mode !== 'day') {
      day = await call('business_day');
      if (mine !== loads) return;
    }
    // Not closed for a long time (or only once, long ago): business days
    // would mean weeks, so the quick dates fall back to calendar days.
    const stale = mode !== 'day' && Date.now() / 1000 - day.since > 36 * 3600;
    if (mode !== 'day') {
      date.value = stale ? isoDate(daysAgo(mode === 'yesterday' ? 1 : 0))
        : mode === 'yesterday' ? (day.prev_date ?? isoDate(daysAgo(1))) : day.date;
    }
    if (!date.value) return;
    const [from, to] = stale
      ? (mode === 'week' ? [dayRange(isoDate(daysAgo(6)))[0], OPEN_END] : mode === 'today' ? [dayRange(date.value)[0], OPEN_END] : dayRange(date.value))
      : { today: () => [day.since, OPEN_END], yesterday: () => [day.prev_since, day.since],
        week: () => [day.week_since, OPEN_END], day: () => dayRange(date.value) }[mode]();
    const hm = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    span.textContent = stale ? `The day has not been closed since ${friendlyTime(day.since)}: showing calendar days (close the day in Reports)` : {
      today: () => `Business day ${dayLabel(day.date)}: since ${friendlyTime(day.since)}${day.prev_date ? ' (the last day close)' : ''}`,
      yesterday: () => `Previous business day${day.prev_date ? ` ${dayLabel(day.prev_date)}` : ''}: ${friendlyTime(day.prev_since)} to ${friendlyTime(day.since)}`,
      week: () => `Last 7 business days: since ${friendlyTime(day.week_since)}`,
      day: () => `Calendar day ${dayLabel(date.value)}, midnight to midnight`,
    }[mode]();
    // The server sends at most 500 at a time: a busy week takes several pages.
    const PAGE = 500, MAX = 5000;
    const orders = [];
    for (let offset = 0; offset < MAX; offset += PAGE) {
      const page = await call('list_orders', { from, to, limit: PAGE, offset });
      if (mine !== loads) return;
      orders.push(...page);
      if (page.length < PAGE) break;
    }
    // Exactly 5000 is not "more than shown": look one past the end.
    const more = orders.length >= MAX && (await call('list_orders', { from, to, limit: 1, offset: MAX })).length > 0;
    if (mine !== loads) return;
    shown = orders;
    // Orders from more than one calendar day: the date goes with the time.
    const end = Math.min(to, Date.now() / 1000);
    const severalDays = mode !== 'day' && new Date(from * 1000).toDateString() !== new Date(end * 1000).toDateString();
    const opened = (t) => (severalDays ? friendlyTime(t) : hm(t));
    setKids(list, h('table', {},
      h('tr', {}, h('th', {}, '#'), h('th', {}, 'Opened'), h('th', {}, 'Type'), h('th', {}, 'Table / customer'), h('th', {}, 'Status'), h('th', { class: 'num' }, 'Total')),
      orders.map((o) => h('tr', { class: 'click', onclick: act(() => show(o.id)) },
        h('td', {}, h('strong', {}, `#${o.id}`)), h('td', {}, opened(o.opened_at)),
        h('td', {}, typeChip(o.order_type)), h('td', {}, where(o)), h('td', {}, h('span', { class: `status s-${o.status}` }, o.status)),
        h('td', { class: 'num' }, money(o.total_cents))))));
    if (more) list.prepend(h('div', { class: 'badge warn' }, `Showing the latest ${MAX} orders: pick one day to see the rest.`));
    if (!orders.length) setKids(list, h('div', { class: 'empty small' }, icon('history', 36), h('p', {}, {
      today: 'No closed orders yet in this business day.', yesterday: 'No closed orders in the previous business day.',
      week: 'No closed orders in the last 7 days.', day: 'No closed orders on this day.' }[mode])));
  });
  async function show(id) {
    setKids(detail, orderDetail(await call('get_order', { id }), async (what) => {
      if (what === 'deleted') setKids(detail);
      else await show(id);
      await load();
    }));
  }
  date.onchange = (e) => { mode = 'day'; load(e); };
  let shown = [];
  const bulk = () => bulkDelete('Delete orders', shown.filter((o) => o.status !== 'open').map((o) => ({
    id: o.id, label: [`#${o.id} ${TYPE[o.order_type] ?? ''}`, where(o)].filter(Boolean).join(' · '), sub: `${o.status} · ${money(o.total_cents)}`,
  })), (ids) => deleteMany('order', ids, 'order(s)'), async () => { setKids(detail); await load(); });
  screen(h('div', { class: 'page split' },
    h('div', { class: 'stack' }, h('div', { class: 'toolbar' }, quick, h('label', { class: 'field' }, 'Day', date), h('span', { class: 'grow' }),
      isAdmin() && h('button', { class: 'danger-outline', onclick: bulk }, 'Delete several…')), span, h('div', { class: 'panel' }, list)),
    h('div', { class: 'stack sticky' }, detail)));
  await load();
}

// ---------- menu (admin) ----------

/// A pop-up for an editor: put a card in it with setKids(host, card) and
/// empty it to close. Adds a close button; Escape closes it too.
function popupHost() {
  const host = h('div', { class: 'popup' });
  new MutationObserver(() => {
    const card = host.firstElementChild;
    if (card && !card.querySelector(':scope > .modal-x')) {
      card.prepend(h('button', { class: 'modal-x ghost', title: 'Close (Esc)', onclick: () => setKids(host) }, '×'));
    }
  }).observe(host, { childList: true });
  if (!state.popupEscBound) {
    state.popupEscBound = true;
    document.addEventListener('keydown', (e) => {
      const open = [...document.querySelectorAll('.popup')].find((m) => m.firstChild);
      if (e.key === 'Escape' && open && document.getElementById('modal').hidden) setKids(open);
    });
  }
  return host;
}

async function renderMenuAdmin() {
  const [cats, groups, feed, allVariants, defaultTaxRaw, teams] = await Promise.all([
    call('list_categories', { includeInactive: true }),
    call('list_option_groups', { includeInactive: true }),
    call('menu_options'),
    call('list_variants', { productId: null }),
    call('get_setting', { key: 'default_tax_bp' }),
    call('list_teams'),
  ]);
  const teamById = Object.fromEntries(teams.map((t) => [t.id, t]));
  const teamSelect = (value, none) => {
    const sel = h('select', {}, h('option', { value: '' }, none), teams.map((t) => h('option', { value: t.id }, t.name)));
    sel.value = value ?? '';
    return sel;
  };
  const defaultTax = Math.max(0, parseInt(defaultTaxRaw ?? '0', 10) || 0);
  const taxIncluded = (await call('get_setting', { key: 'prices_include_tax' })) !== '0';
  const variantsOf = {};
  for (const v of allVariants) (variantsOf[v.product_id] ??= []).push(v);
  const activeGroups = groups.filter((g) => g.active);
  const catById = Object.fromEntries(cats.map((c) => [c.id, c]));
  // Main categories, each followed by its sub-categories (one level).
  const isMain = (c) => !c.parent_id || !catById[c.parent_id];
  const catTree = cats.filter(isMain).flatMap((m) => [m, ...cats.filter((c) => c.parent_id === m.id)]);
  const catLabel = (c) => (isMain(c) ? c.name : `${catById[c.parent_id].name} › ${c.name}`);
  // What a new item in category `id` starts with.
  // A sub-category without its own tax or team uses its main category's, then the restaurant's.
  // Starter, prepare ahead, drink, dessert: a sub-category's own "on" wins, its
  // "off" (flags_off) turns its main category's off, otherwise it takes the main's.
  const flagOf = (c, m, key) => !!c?.[key] || (!!m?.[key] && !((c?.flags_off ?? 0) & CAT_FLAG_OFF[key]));
  const defaultsFor = (id) => {
    const c = catById[id], m = catById[c?.parent_id];
    return { tax: c?.tax_rate_bp ?? m?.tax_rate_bp ?? defaultTax, team: c?.team_id ?? m?.team_id ?? null,
      starter: flagOf(c, m, 'starter'), prep: flagOf(c, m, 'prep'), drink: flagOf(c, m, 'drink'), dessert: flagOf(c, m, 'dessert') };
  };
  const form = popupHost();
  const list = h('div');
  const search = h('input', { type: 'search', placeholder: 'Search items' });
  const showInactive = h('input', { type: 'checkbox' });
  const tabs = h('div', { class: 'cat-tabs' });
  state.menuTab ??= 'all';
  if (state.menuTab !== 'all' && state.menuTab !== 'none' && !catById[state.menuTab]?.active) state.menuTab = 'all';
  const changed = async (keepForm = false) => { state.menu = null; if (!keepForm) setKids(form); await renderMenuAdmin(); };
  const tabCat = () => (typeof state.menuTab === 'number' ? state.menuTab : null);

  // ---- error under a field (instead of a pop-up) ----
  const fieldErr = (input, msg) => {
    let el = input.parentNode?.querySelector(`.field-error[data-for="${input.dataset.f}"]`);
    if (!msg) { el?.remove(); input.classList.remove('bad'); return; }
    input.classList.add('bad');
    // The message goes as soon as the user starts fixing it.
    input.addEventListener('input', () => fieldErr(input), { once: true });
    if (!el) {
      el = h('div', { class: 'field-error', 'data-for': input.dataset.f });
      input.insertAdjacentElement('afterend', el);
    }
    el.textContent = msg;
  };

  // ---- the list ----
  const loadProducts = act(async () => {
    // Not the hidden entry behind "Other dish" on orders.
    let products = (await call('list_products', { includeInactive: showInactive.checked, limit: 500, offset: 0 })).filter((x) => x.sku !== '__open_item__');
    state.menuShown = products;
    const q = search.value.trim().toLowerCase();
    if (q) products = products.filter((p) => matches(p, q));
    else if (state.menuTab === 'none') products = products.filter((p) => p.category_id == null);
    else if (tabCat()) products = products.filter((p) => p.category_id === tabCat());
    // Dragging sets the order inside one category, so it needs a category tab.
    const canDrag = !q && tabCat() != null;
    const table = h('table', { class: 'menu-list' },
      h('tr', {}, canDrag && h('th', {}), h('th', {}, 'Item'), h('th', {}, 'Category'), teams.length > 0 && h('th', { title: 'The kitchen team whose ticket this item prints on' }, 'Team'), h('th', { title: 'Starters go to the kitchen first on dine-in orders' }, 'Starter'),
        h('th', { title: 'Prepare ahead: a PREPARE NOW ticket prints while the mains wait' }, 'Prep'),
        h('th', { title: 'Drink: goes straight away, on its own DRINKS ticket' }, 'Drink'),
        h('th', { title: 'Dessert: printed on its own DESSERTS ticket' }, 'Dessert'),
        h('th', { class: 'num' }, 'Price'), h('th', { class: 'num' }, 'Tax %'), h('th', { class: 'num' }, 'Stock')),
      products.map((p) => h('tr', { class: 'click', 'data-id': p.id, style: p.active ? '' : 'opacity:.5',
        onclick: (e) => { if (!e.currentTarget.dataset.dragged) editProduct(p); } },
        canDrag && h('td', { class: 'grip', title: 'Drag to reorder', onclick: (e) => e.stopPropagation() }, '⠿'),
        h('td', {}, p.name, p.is_set && h('span', { class: 'tag set' }, ' SET'), p.set_only && h('span', { class: 'badge', title: 'Only sold inside set menus' }, 'set only'),
          p.name_alt && h('div', { class: 'alt', lang: 'th' }, p.name_alt),
          variantsOf[p.id] ? h('div', { class: 'muted' }, variantsOf[p.id].map((v) => v.name).join(' / ')) : ''),
        h('td', {}, catById[p.category_id]?.name ?? '—'),
        teams.length > 0 && h('td', {}, teamById[p.team_id]?.name ?? '—'),
        h('td', { onclick: (e) => e.stopPropagation() }, h('input', {
          type: 'checkbox', checked: p.starter, title: 'Starter: sent to the kitchen first',
          onchange: act(async (e) => {
            await call('set_starter', { id: p.id, starter: e.target.checked });
            p.starter = e.target.checked;
            state.menu = null;
            toast(`${p.name}: ${p.starter ? 'starter' : 'not a starter'}`);
          }),
        })),
        h('td', { onclick: (e) => e.stopPropagation() }, h('input', {
          type: 'checkbox', checked: p.prep, title: 'Prepare ahead: the kitchen gets a PREPARE NOW ticket while the mains wait',
          onchange: act(async (e) => {
            await call('set_prep', { id: p.id, prep: e.target.checked });
            p.prep = e.target.checked;
            toast(`${p.name}: ${p.prep ? 'prepared ahead' : 'not prepared ahead'}`);
          }),
        })),
        h('td', { onclick: (e) => e.stopPropagation() }, h('input', {
          type: 'checkbox', checked: p.drink, title: 'Drink: goes straight away, on its own DRINKS ticket',
          onchange: act(async (e) => {
            await call('set_drink', { id: p.id, drink: e.target.checked });
            p.drink = e.target.checked;
            toast(`${p.name}: ${p.drink ? 'drink (own ticket)' : 'not a drink'}`);
          }),
        })),
        h('td', { onclick: (e) => e.stopPropagation() }, h('input', {
          type: 'checkbox', checked: p.dessert, title: 'Dessert: printed on its own DESSERTS ticket',
          onchange: act(async (e) => {
            await call('set_dessert', { id: p.id, dessert: e.target.checked });
            p.dessert = e.target.checked;
            toast(`${p.name}: ${p.dessert ? 'dessert (own ticket)' : 'not a dessert'}`);
          }),
        })),
        h('td', { class: 'num' }, itemPrice(p, variantsOf[p.id])),
        h('td', { class: 'num' }, pct(p.tax_rate_bp)), h('td', { class: 'num' }, p.track_stock ? p.stock_qty : '—'))));
    setKids(list, table,
      !products.length && h('div', { class: 'empty small' }, q ? 'No items match your search.' : 'No items here yet: tap "+ Menu item" to add one, or "Paste a menu" to add many.'),
      canDrag && products.length > 1 && h('div', { class: 'muted small pad' }, 'Drag ⠿ to change the order items appear on the order screen.'));
    if (canDrag) sortable(table, { item: 'tr[data-id]', handle: '.grip', onDrop: act(async (ids) => { await call('reorder_products', { ids }); state.menu = null; toast('Order saved'); }) });
  });

  // ---- categories (with defaults and drag to reorder) ----
  function editCategory(c) {
    const name = h('input', { value: c?.name ?? '', placeholder: 'Category name', 'data-f': 'cname' });
    const alt = h('input', { value: c?.name_alt ?? '', placeholder: 'Thai name (optional)', lang: 'th', 'data-f': 'calt' });
    const color = h('input', { type: 'color', value: c?.color ?? '#9a9484' });
    const active = h('input', { type: 'checkbox', checked: c ? c.active : true });
    const tax = h('input', { value: c?.tax_rate_bp != null ? pct(c.tax_rate_bp) : '', placeholder: `Same as restaurant (${pct(defaultTax)}%)`, inputmode: 'decimal', style: 'width:230px', 'data-f': 'ctax' });
    // Each default: a tick for a main category; for a sub-category "Same as <main>", On or Off.
    const flagBox = (key, label, title, guess) => {
      const box = h('input', { type: 'checkbox', checked: c ? c[key] : guess });
      const pick = h('select', { 'aria-label': label });
      const wrap = h('span', { class: 'flag-pick' });
      const sub = () => (parent.value ? catById[Number(parent.value)] : null);
      const draw = () => {
        const m = sub();
        if (!m) return setKids(wrap, h('label', { class: 'check', title }, box, ` ${label}`));
        const was = pick.value;
        setKids(pick, h('option', { value: 'inherit' }, `Same as main: ${m[key] ? 'on' : 'off'}`),
          h('option', { value: 'on' }, 'On'), h('option', { value: 'off' }, 'Off'));
        pick.value = was || (c?.[key] ? 'on' : (c?.flags_off ?? 0) & CAT_FLAG_OFF[key] ? 'off' : box.checked ? 'on' : 'inherit');
        setKids(wrap, h('label', { class: 'field', title }, label, pick));
      };
      return { el: wrap, draw,
        own: () => (sub() ? pick.value === 'on' : box.checked),
        off: () => (sub() && pick.value === 'off' ? CAT_FLAG_OFF[key] : 0),
        // What new items here get, the main category's default included.
        effective: () => { const m = sub(); return m ? pick.value === 'on' || (pick.value === 'inherit' && !!m[key]) : box.checked; } };
    };
    const team = teamSelect(c?.team_id, 'No team');
    const moveItems = h('input', { type: 'checkbox', checked: true });
    // Under a main category (a sub-category), or a main category itself.
    const hasSubs = !!c && cats.some((x) => x.parent_id === c.id);
    const parent = h('select', { disabled: hasSubs, title: hasSubs ? 'It has sub-categories, so it stays a main category' : '' },
      h('option', { value: '' }, '— None: a main category —'),
      cats.filter((x) => isMain(x) && x.active && x.id !== c?.id).map((x) => h('option', { value: x.id }, x.name)));
    parent.value = c?.parent_id ?? (state.newSubOf ?? '');
    state.newSubOf = null;
    const starter = flagBox('starter', 'Items here are starters', '', /^starter/i.test(c?.name ?? ''));
    const prep = flagBox('prep', 'Items here are prepared ahead', '', false);
    const drink = flagBox('drink', 'Drinks (own ticket)', 'Sent straight away and printed on their own DRINKS ticket', /drink|beverage|เครื่องดื่ม/i.test(c?.name ?? ''));
    const dessert = flagBox('dessert', 'Desserts (own ticket)', 'Printed on their own DESSERTS ticket', /dessert|sweet|ของหวาน/i.test(c?.name ?? ''));
    const flags = [starter, prep, drink, dessert];
    const flagsHint = h('div', { class: 'muted small' });
    const drawFlags = () => {
      flags.forEach((f) => f.draw());
      const m = parent.value ? catById[Number(parent.value)] : null;
      flagsHint.textContent = m ? `"Same as main" follows ${m.name} for new dishes. On or Off here always wins.` : '';
    };
    parent.addEventListener('change', drawFlags);
    drawFlags();
    // Drinks and desserts: the items already here follow what the category now gives them.
    const wasDrink = c ? flagOf(c, catById[c.parent_id], 'drink') : false;
    const wasDessert = c ? flagOf(c, catById[c.parent_id], 'dessert') : false;
    const save = act(async () => {
      fieldErr(name); fieldErr(tax);
      if (!name.value.trim()) return fieldErr(name, 'Type a name');
      let taxBp = null;
      if (tax.value.trim()) {
        const v = Number(tax.value.replace(',', '.').replace('%', ''));
        if (!Number.isFinite(v) || v < 0 || v > 100) return fieldErr(tax, 'Enter a tax rate from 0 to 100');
        taxBp = Math.round(v * 100);
      }
      const args = { name: name.value, color: color.value };
      const saved = c
        ? await call('update_category', { id: c.id, ...args, sortOrder: c.sort_order, active: active.checked })
        : await call('create_category', { ...args, sortOrder: cats.length + 1 });
      await call('set_category_defaults', { id: saved.id, taxRateBp: taxBp, starter: starter.own(), prep: prep.own(), nameAlt: alt.value.trim() || null });
      // Drinks and desserts: the items already here follow the category (its main category's included),
      // then the category keeps its own setting.
      for (const [f, was, cmd, key] of [[drink, wasDrink, 'set_category_drink', 'drink'], [dessert, wasDessert, 'set_category_dessert', 'dessert']]) {
        if (f.effective() !== was) await call(cmd, { id: saved.id, [key]: f.effective(), moveItems: true });
        if (f.own() !== f.effective() || (f.effective() === was && f.own() !== !!c?.[key])) await call(cmd, { id: saved.id, [key]: f.own(), moveItems: false });
      }
      const parentId = parent.value ? Number(parent.value) : null;
      if (parentId !== (c?.parent_id ?? null)) await call('set_category_parent', { id: saved.id, parentId });
      const off = flags.reduce((n, f) => n | f.off(), 0);
      if (off !== (c?.flags_off ?? 0)) await call('set_category_flags_off', { id: saved.id, flagsOff: off });
      const teamId = team.value ? Number(team.value) : null;
      if (teamId !== (c?.team_id ?? null) || (teamId != null && moveItems.checked)) {
        await call('set_category_team', { id: saved.id, teamId, moveItems: moveItems.checked });
      }
      toast('Saved');
      if (!c) state.menuTab = saved.id;
      await changed();
    });
    name.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(e); });
    setKids(form, h('div', { class: 'card stack' }, h('h3', {}, c ? `Edit ${c.name}` : 'New category'),
      h('div', { class: 'row' }, name, h('label', { class: 'field' }, 'Colour', color), c && h('label', { class: 'check' }, active, ' Active')),
      h('div', { class: 'field-col', style: 'display:flex;gap:4px' }, alt, h('div', { class: 'muted small' }, 'Printed next to the heading on kitchen tickets')),
      h('div', { class: 'muted' }, 'Defaults for new items added to this category:'),
      h('div', { class: 'row' }, h('label', { class: 'field' }, 'Tax %', tax), flags.map((f) => f.el)),
      flagsHint,
      h('div', { class: 'row' }, h('label', { class: 'field' }, 'Main category', parent),
        h('span', { class: 'muted small' }, hasSubs ? 'It has sub-categories, so it stays a main category.'
          : 'Pick one to make this a sub-category: on the order screen it shows as a button after you tap the main category.'),
        c && isMain(c) && h('button', { onclick: () => { state.newSubOf = c.id; editCategory(null); } }, '+ Sub-category here')),
      teams.length > 0 && h('div', { class: 'row' }, h('label', { class: 'field' }, 'Kitchen team', team),
        h('label', { class: 'check' }, moveItems, ' Also move the items already here to this team'),
        h('span', { class: 'muted small' }, 'Each team gets its own ticket with its name as a big header.')),
      h('div', { class: 'row' },
        h('button', { class: 'primary', onclick: save }, 'Save'),
        h('button', { onclick: () => setKids(form) }, 'Cancel'),
        h('span', { style: 'flex:1' }),
        c && isAdmin() && confirmBtn('Delete category', `Tap again to delete "${c.name}"`, async () => {
          await call('delete_category', { id: c.id });
          toast(`${c.name} deleted; its items now have no category`);
          state.menuTab = 'all';
          await changed();
        }))));
    name.focus();
  }

  // ---- option groups ----
  function editGroup(g) {
    const name = h('input', { value: g?.name ?? '', placeholder: 'Group name, e.g. Burger size' });
    const min = h('input', { type: 'number', value: g?.min_select ?? 0, min: 0, max: 20, style: 'width:70px' });
    const max = h('input', { type: 'number', value: g?.max_select ?? 1, min: 1, max: 20, style: 'width:70px' });
    const active = h('input', { type: 'checkbox', checked: g ? g.active : true });
    const rows = (g?.options ?? []).filter((o) => o.active).map((o) => ({ id: o.id, name: o.name, alt: o.name_alt ?? '', price: o.price_cents ? signed(o.price_cents) : '' }));
    const optList = h('div', { class: 'stack' });
    const drawRows = () => setKids(optList, ...rows.map((r, i) => h('div', { class: 'row' },
      h('input', { value: r.name, placeholder: 'Option, e.g. Large', style: 'width:160px', oninput: (e) => (r.name = e.target.value) }),
      h('input', { value: r.alt, placeholder: 'Thai (optional)', lang: 'th', class: 'alt-input', oninput: (e) => (r.alt = e.target.value) }),
      h('input', { value: r.price, placeholder: '+0.00', style: 'width:90px', inputmode: 'decimal', oninput: (e) => (r.price = e.target.value) }),
      h('button', { class: 'danger', onclick: () => { rows.splice(i, 1); drawRows(); } }, '×'))));
    const addRow = () => { rows.push({ id: null, name: '', alt: '', price: '' }); drawRows(); optList.lastChild.querySelector('input').focus(); };
    const save = act(async () => {
      const args = { name: name.value, minSelect: parseInt(min.value, 10) || 0, maxSelect: parseInt(max.value, 10) || 1 };
      const options = rows.filter((r) => r.name.trim()).map((r) => ({ id: r.id, name: r.name, name_alt: r.alt.trim() || null, price_cents: parseSignedMoney(r.price) }));
      const saved = g
        ? await call('update_option_group', { id: g.id, ...args, sortOrder: g.sort_order, active: active.checked })
        : await call('create_option_group', args);
      await call('save_group_options', { groupId: saved.id, options });
      toast('Saved'); await changed();
    });
    setKids(form, h('div', { class: 'card stack' },
      h('h3', {}, g ? `Edit ${g.name}` : 'New option group'),
      h('div', { class: 'row' }, name, g && h('label', { class: 'check' }, active, ' In use')),
      h('div', { class: 'row' }, 'Customer must choose at least', min, 'and at most', max),
      h('div', { class: 'muted' }, 'Sizes: at least 1, at most 1. Extras: at least 0, at most e.g. 5. Prices are added to the item (use -1.00 for a cheaper option). ' +
        'Untick "In use" to hide it for now; Delete removes it from every item (past orders are not affected).'),
      optList,
      h('div', {}, h('button', { onclick: addRow }, '+ Option')),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Save'), h('button', { onclick: () => setKids(form) }, 'Cancel'),
        h('span', { style: 'flex:1' }),
        g && isAdmin() && confirmBtn('Delete group', `Tap again to delete "${g.name}"`, async () => {
          await call('delete_option_group', { id: g.id });
          toast(`${g.name} deleted`); await changed();
        }))));
    drawRows();
    if (!rows.length) addRow();
    name.focus();
  }

  // ---- 1, 3, 7, 9, 10, 11: the item editor ----
  function editProduct(p, preset = {}) {
    if (p?.is_set) return editSet(p);
    const startCat = p ? p.category_id : (preset.category_id !== undefined ? preset.category_id : tabCat());
    const d0 = defaultsFor(startCat);
    const f = {
      name: h('input', { value: p?.name ?? '', placeholder: 'Item name', 'data-f': 'name' }),
      alt: h('input', { value: p?.name_alt ?? '', placeholder: 'Thai name (optional), e.g. ผัดไทย', lang: 'th', 'data-f': 'alt' }),
      cat: h('select', {}, h('option', { value: '' }, 'No category'), catTree.filter((c) => c.active || c.id === p?.category_id).map((c) => h('option', { value: c.id }, catLabel(c)))),
      price: h('input', { value: p ? money(p.price_cents) : '', placeholder: '0.00', inputmode: 'decimal', style: 'width:110px', 'data-f': 'price' }),
      tax: h('input', { value: pct(p ? p.tax_rate_bp : d0.tax), inputmode: 'decimal', style: 'width:80px', 'data-f': 'tax' }),
      cost: h('input', { value: p ? money(p.cost_cents) : '0', inputmode: 'decimal', style: 'width:100px', 'data-f': 'cost' }),
      barcode: h('input', { value: p?.barcode ?? '', placeholder: 'Barcode (optional)' }),
      sku: h('input', { value: p?.sku ?? '', placeholder: 'Code / PLU (optional)' }),
      track: h('input', { type: 'checkbox', checked: p ? p.track_stock : false }),
      stock: h('input', { value: '0', inputmode: 'numeric', style: 'width:80px' }),
      active: h('input', { type: 'checkbox', checked: p ? p.active : true }),
      starter: h('input', { type: 'checkbox', checked: p ? p.starter : d0.starter }),
      prep: h('input', { type: 'checkbox', checked: p ? p.prep : d0.prep }),
      team: teamSelect(p ? p.team_id : d0.team, 'No team'),
      drink: h('input', { type: 'checkbox', checked: p ? p.drink : d0.drink }),
      dessert: h('input', { type: 'checkbox', checked: p ? p.dessert : d0.dessert }),
      setOnly: h('input', { type: 'checkbox', checked: !!p?.set_only }),
    };
    f.cat.value = startCat ?? '';
    // A new item follows its category's defaults until the user changes them.
    let taxTouched = !!p, starterTouched = !!p, prepTouched = !!p, teamTouched = !!p;
    f.team.addEventListener('change', () => (teamTouched = true));
    let drinkTouched = !!p;
    f.drink.addEventListener('change', () => (drinkTouched = true));
    let dessertTouched = !!p;
    f.dessert.addEventListener('change', () => (dessertTouched = true));
    f.tax.addEventListener('input', () => (taxTouched = true));
    f.starter.addEventListener('change', () => (starterTouched = true));
    f.prep.addEventListener('change', () => (prepTouched = true));
    f.cat.addEventListener('change', () => {
      const d = defaultsFor(f.cat.value ? Number(f.cat.value) : null);
      if (!taxTouched) f.tax.value = pct(d.tax);
      if (!starterTouched) f.starter.checked = d.starter;
      if (!prepTouched) f.prep.checked = d.prep;
      if (!teamTouched) f.team.value = d.team ?? '';
      if (!drinkTouched) f.drink.checked = d.drink;
      if (!dessertTouched) f.dessert.checked = d.dessert;
      preview();
    });
    const linked = new Set(feed.links.filter((l) => l.product_id === p?.id).map((l) => l.group_id));
    const choices = (variantsOf[p?.id] ?? []).map((v) => ({ id: v.id, name: v.name, alt: v.name_alt ?? '', price: money(v.price_cents) }));
    const choiceList = h('div', { class: 'stack' });
    const priceHint = h('span', { class: 'muted' });
    const drawChoices = () => {
      priceHint.textContent = choices.length ? '(the item price is not used)' : '';
      setKids(choiceList, ...choices.map((c, i) => h('div', { class: 'row' },
        h('input', { value: c.name, placeholder: 'e.g. Chicken', style: 'width:150px', oninput: (e) => { c.name = e.target.value; preview(); } }),
        h('input', { value: c.alt, placeholder: 'Thai, e.g. ไก่', lang: 'th', class: 'alt-input', oninput: (e) => (c.alt = e.target.value) }),
        h('input', { value: c.price, placeholder: 'Price', inputmode: 'decimal', style: 'width:100px', 'data-f': `choice${i}`, oninput: (e) => { c.price = e.target.value; preview(); } }),
        h('button', { class: 'danger', onclick: () => { choices.splice(i, 1); drawChoices(); preview(); } }, '×'))));
    };
    const addChoice = () => { choices.push({ id: null, name: '', alt: '', price: '' }); drawChoices(); choiceList.lastChild.querySelector('input').focus(); };
    drawChoices();
    const groupBoxes = activeGroups.map((g) => ({ g, box: h('input', { type: 'checkbox', checked: linked.has(g.id) }) }));
    const groupRow = h('div', { class: 'row' });
    const drawGroups = () => setKids(groupRow, groupBoxes.map(({ g, box }) => h('label', { class: 'check' }, box, ` ${g.name} `, h('span', { class: 'muted' }, `(${groupRule(g)})`))),
      !groupBoxes.length && h('span', { class: 'muted' }, 'No option groups yet.'));
    drawGroups();

    // 9: new option group without leaving the item.
    const newGroup = h('div');
    const openNewGroup = () => {
      const gname = h('input', { placeholder: 'e.g. Spice level' });
      const rule = h('select', {}, h('option', { value: '1-1' }, 'Customer picks exactly 1'), h('option', { value: '0-1' }, 'Optional, up to 1'), h('option', { value: '0-5' }, 'Optional, up to 5'));
      const opts = h('textarea', { rows: 3, placeholder: 'One option per line, with an extra price if any, and Thai after | if you like:\nMild | เผ็ดน้อย\nHot | เผ็ด\nExtra hot | เผ็ดมาก +0.50', style: 'width:100%' });
      setKids(newGroup, h('div', { class: 'card stack inset' },
        h('strong', {}, 'New option group'),
        h('div', { class: 'row' }, gname, rule), opts,
        h('div', { class: 'row' },
          h('button', { class: 'primary', onclick: act(async () => {
            const [mn, mx] = rule.value.split('-').map(Number);
            const options = opts.value.split('\n').map((l) => l.trim()).filter(Boolean).map((l) => {
              const m = l.match(/^(.*?)\s*([+-]\s*[£$€]?\d+(?:[.,]\d{1,2})?)$/);
              const [name, alt = ''] = (m ? m[1] : l).split('|').map((x) => x.trim());
              return { id: null, name, name_alt: alt || null, price_cents: m ? parseSignedMoney(m[2].replace(/\s|[£$€]/g, '')) : 0 };
            });
            if (!options.length) return toast('Add at least one option, one per line', true);
            const g = await call('create_option_group', { name: gname.value, minSelect: mn, maxSelect: mx });
            const saved = await call('save_group_options', { groupId: g.id, options });
            groupBoxes.push({ g: saved, box: h('input', { type: 'checkbox', checked: true }) });
            activeGroups.push(saved);
            drawGroups();
            setKids(newGroup);
            toast(`${saved.name} created and ticked; Save the item to keep it`);
          }) }, 'Create group'),
          h('button', { onclick: () => setKids(newGroup) }, 'Cancel'))));
      gname.focus();
    };

    // 10: live preview of the order-screen tile.
    const tile = h('div', { class: 'tile preview' });
    const preview = () => {
      const c = catById[Number(f.cat.value)];
      let price;
      try { price = parseMoney(f.price.value || '0'); } catch { price = null; }
      const named = choices.filter((x) => x.name.trim());
      const prices = named.map((x) => { try { return parseMoney(x.price); } catch { return null; } }).filter((x) => x != null);
      const label = named.length && prices.length
        ? (prices.every((x) => x === prices[0]) ? money(prices[0]) : `from ${money(Math.min(...prices))}`)
        : price == null ? '?' : money(price);
      tile.style.setProperty('--c', c?.color || '#9a9484');
      setKids(tile, h('span', { class: 'names' }, h('span', { class: 'name' }, f.name.value.trim() || 'Item name'), f.alt.value.trim() && h('span', { class: 'alt', lang: 'th' }, f.alt.value.trim())),
        h('span', { class: 'foot' }, h('span', { class: 'price' }, label), f.starter.checked && h('span', { class: 'tag' }, 'Starter'),
          (named.length || groupBoxes.some((x) => x.box.checked)) && h('span', { class: 'more' }, '•••')));
    };
    for (const el of [f.name, f.alt, f.price]) el.addEventListener('input', preview);
    f.starter.addEventListener('change', preview);
    groupRow.addEventListener('change', preview);
    preview();

    // 11: check everything, showing problems next to the field.
    const validate = () => {
      let ok = true;
      const err = (el, msg) => { fieldErr(el, msg); if (msg) ok = false; };
      err(f.name, f.name.value.trim() ? null : 'Type a name');
      const hasChoices = choices.some((c) => c.name.trim());
      let price = 0;
      if (!(hasChoices && !f.price.value.trim())) {
        try { price = parseMoney(f.price.value); err(f.price, null); } catch (e) { err(f.price, e.message); }
      } else err(f.price, null);
      const tax = Number(String(f.tax.value).replace(',', '.').replace('%', '') || 0);
      err(f.tax, Number.isFinite(tax) && tax >= 0 && tax <= 100 ? null : 'From 0 to 100');
      let cost = 0;
      try { cost = parseMoney(f.cost.value || '0'); err(f.cost, null); } catch (e) { err(f.cost, e.message); }
      const variants = [];
      choices.forEach((c, i) => {
        if (!c.name.trim()) return;
        const el = choiceList.querySelector(`[data-f="choice${i}"]`);
        try { variants.push({ id: c.id, name: c.name, name_alt: c.alt.trim() || null, price_cents: parseMoney(c.price) }); err(el, null); } catch (e) { err(el, e.message); }
      });
      if (!ok) { more.open = more.open || !!form.querySelector('details .bad'); return null; }
      return {
        product: {
          name: f.name.value, name_alt: f.alt.value.trim() || null, category_id: f.cat.value ? Number(f.cat.value) : null,
          barcode: f.barcode.value || null, sku: f.sku.value || null, price_cents: price, cost_cents: cost,
          tax_rate_bp: Math.round(tax * 100), track_stock: f.track.checked, starter: f.starter.checked, prep: f.prep.checked,
          team_id: f.team.value ? Number(f.team.value) : null,
        },
        variants,
      };
    };
    const save = (again) => act(async () => {
      const v = validate();
      if (!v) return;
      const saved = p
        ? await call('update_product', { id: p.id, product: { ...v.product, active: f.active.checked } })
        : await call('create_product', { product: { ...v.product, stock_qty: parseInt(f.stock.value || '0', 10) } });
      const groupIds = groupBoxes.filter((x) => x.box.checked).map((x) => x.g.id);
      if (p || groupIds.length) await call('set_product_option_groups', { productId: saved.id, groupIds });
      if (p || v.variants.length) await call('set_variants', { productId: saved.id, variants: v.variants });
      if (saved.drink !== f.drink.checked) await call('set_drink', { id: saved.id, drink: f.drink.checked });
      if (saved.dessert !== f.dessert.checked) await call('set_dessert', { id: saved.id, dessert: f.dessert.checked });
      if (!!saved.set_only !== f.setOnly.checked) await call('set_set_only', { id: saved.id, setOnly: f.setOnly.checked });
      state.menu = null;
      if (again) {
        // 1: keep going in the same category.
        toast(`${saved.name} saved. Next item:`);
        await renderMenuAdmin();
        state.openEditor?.(null, { category_id: v.product.category_id });
      } else {
        toast('Saved');
        await changed();
      }
    });
    for (const el of [f.name, f.alt, f.price]) el.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(!p)(e); });

    const delta = h('input', { placeholder: '+/- qty', style: 'width:90px', inputmode: 'numeric' });
    const reason = h('input', { placeholder: 'Reason (delivery, count…)' });
    // 3: the rarely needed fields wait behind "More options".
    const more = h('details', { class: 'more-options' },
      h('summary', {}, 'More options', h('span', { class: 'muted' }, ' · tax, starter, drink, dessert, choices, extras, stock, codes')),
      h('div', { class: 'stack' },
        h('div', { class: 'row' }, h('label', { class: 'field' }, 'Tax %', f.tax), h('label', { class: 'field' }, 'Cost', f.cost),
          h('label', { class: 'check' }, f.starter, ' Starter ', h('span', { class: 'muted' }, '(sent to the kitchen first)')),
          h('label', { class: 'check' }, f.prep, ' Prepare ahead ', h('span', { class: 'muted' }, '(kitchen gets it early while mains wait)')),
          h('label', { class: 'check' }, f.drink, ' Drink ', h('span', { class: 'muted' }, '(sent straight away, own DRINKS ticket)')),
          h('label', { class: 'check' }, f.dessert, ' Dessert ', h('span', { class: 'muted' }, '(own DESSERTS ticket)')),
          h('label', { class: 'check' }, f.setOnly, ' Only in set menus ', h('span', { class: 'muted' }, '(no tile of its own on the order screen)'))),
        h('div', { class: 'stack' }, h('div', {}, h('strong', {}, 'Choices with their own price'), ' ', priceHint),
          h('div', { class: 'muted' }, 'For one item sold in versions, e.g. a soup with Chicken 6.50 / Beef 7.50. Staff pick one when ordering.'),
          choiceList, h('div', {}, h('button', { onclick: addChoice }, '+ Choice'))),
        h('div', { class: 'stack' }, h('strong', {}, 'Options (sizes, extras…)'), groupRow,
          h('div', {}, h('button', { onclick: openNewGroup }, '+ New option group')), newGroup),
        h('div', { class: 'row' }, f.barcode, f.sku),
        h('div', { class: 'row' }, h('label', { class: 'check' }, f.track, ' Count stock (e.g. bottled drinks)'),
          !p && h('label', { class: 'field' }, 'Opening stock', f.stock), p && h('label', { class: 'check' }, f.active, ' On the menu')),
        p && p.track_stock && h('div', { class: 'row' }, `Stock: ${p.stock_qty}`, delta, reason,
          h('button', { onclick: act(async () => {
            await call('adjust_stock', { id: p.id, delta: parseInt(delta.value, 10) || 0, reason: reason.value });
            toast('Stock updated'); await changed();
          }) }, 'Adjust'))));
    more.open = !!p && (choices.length > 0 || linked.size > 0 || !!p.sku || !!p.barcode || p.track_stock || !p.active);
    setKids(form, h('div', { class: 'card stack' },
      h('div', { class: 'row nowrap' }, h('h3', { class: 'grow' }, p ? `Edit ${p.name}` : 'New menu item'), tile),
      h('div', { class: 'row' }, h('div', { class: 'grow field-col' }, f.name), f.cat, teams.length > 0 && h('label', { class: 'field', title: 'The kitchen team whose ticket this prints on' }, 'Team', f.team)),
      h('div', { class: 'field-col', style: 'display:flex;gap:4px' }, f.alt, h('div', { class: 'muted small' }, 'Printed under the name on kitchen tickets')),
      h('div', { class: 'row' }, h('label', { class: 'field' }, taxIncluded ? 'Price (tax included)' : 'Price (before tax)', h('span', { class: 'field-col' }, f.price)),
        taxIncluded && h('span', { class: 'muted small' }, 'What the customer pays; the tax is worked out from it.')),
      more,
      h('div', { class: 'row' },
        !p && h('button', { class: 'primary', onclick: save(true), title: 'Enter' }, 'Save & add another'),
        h('button', { class: p ? 'primary' : '', onclick: save(false) }, 'Save'),
        h('button', { onclick: () => setKids(form) }, 'Cancel'),
        h('span', { style: 'flex:1' }),
        p && h('button', { onclick: act(async () => {
          const copy = await call('duplicate_product', { id: p.id });
          toast(`Copied as "${copy.name}": change the name and price`);
          await renderMenuAdmin();
          const fresh = (await call('list_products', { includeInactive: true, limit: 500, offset: 0 })).find((x) => x.id === copy.id);
          state.openEditor?.(fresh ?? copy);
        }) }, 'Duplicate'),
        p && isAdmin() && confirmBtn('Delete item', `Tap again to delete "${p.name}" for good`, async () => {
          await call('delete_product', { id: p.id });
          toast(`${p.name} deleted`);
          await changed();
        }))));
    (p ? f.name : f.name).focus();
    if (!p) f.name.select();
  }
  // ---- set menus: price, sections (Starter, Main, Side...) and their dishes ----
  async function editSet(p) {
    // Any dish can go in a set; its choices and options are picked with it on the order.
    const all = (await call('list_products', { includeInactive: false, limit: 500, offset: 0 })).filter((x) => !x.is_set);
    const byId = Object.fromEntries(all.map((x) => [x.id, x]));
    const sections = p
      ? (await call('get_set', { setId: p.id })).map((s) => ({ name: s.name, alt: s.name_alt ?? '', pick: s.pick, starter: s.starter,
        items: s.items.map((c) => c.product_id), with: s.with_items.map((c) => c.product_id) }))
      : [['Starter', true], ['Main', false], ['Side', false]].map(([name, starter]) => ({ name, alt: '', pick: 1, starter, items: [], with: [] }));
    // Anything plain can be what a dish comes with (rice, noodles...).
    const withable = (await call('list_products', { includeInactive: false, limit: 500, offset: 0 })).filter((x) => !x.is_set);
    for (const x of withable) byId[x.id] ??= x;
    const name = h('input', { value: p?.name ?? '', placeholder: 'Set name, e.g. Dinner set for 1', 'data-f': 'sname' });
    const alt = h('input', { value: p?.name_alt ?? '', placeholder: 'Thai name (optional)', lang: 'th' });
    const cat = h('select', {}, h('option', { value: '' }, 'No category'), cats.filter((c) => c.active || c.id === p?.category_id).map((c) => h('option', { value: c.id }, c.name)));
    cat.value = p?.category_id ?? (tabCat() ?? '');
    const price = h('input', { value: p ? money(p.price_cents) : '', placeholder: '0.00', inputmode: 'decimal', style: 'width:110px', 'data-f': 'sprice' });
    const tax = h('input', { value: pct(p ? p.tax_rate_bp : defaultsFor(cat.value ? Number(cat.value) : null).tax), inputmode: 'decimal', style: 'width:80px' });
    const active = h('input', { type: 'checkbox', checked: p ? p.active : true });
    const list = h('div', { class: 'stack' });
    // Searchable dish picker: tap a dish to add it, tap again to take it out.
    // `rule` keeps to the dishes that suit the section, unless "Show all" is ticked.
    const dishPicker = (chosen, pool, ui, rule, onChange) => {
      const q = h('input', { type: 'search', placeholder: 'Search dishes (English or Thai)', value: ui.q ?? '' });
      const tabsEl = h('div', { class: 'cat-tabs' });
      const grid = h('div', { class: 'pick-grid' });
      const showAll = h('input', { type: 'checkbox', checked: !!ui.all });
      const drawList = () => {
        const term = q.value.trim().toLowerCase();
        const fits = pool.filter((x) => ui.all || !rule || rule(x));
        const here = [...new Set(fits.map((x) => x.category_id ?? 0))];
        if (ui.cat != null && !here.includes(ui.cat)) ui.cat = null;
        setKids(tabsEl, h('button', { class: ui.cat == null ? 'on' : '', onclick: () => { ui.cat = null; drawList(); } }, 'All'),
          here.map((c) => h('button', { class: ui.cat === c ? 'on' : '', style: catStyle(catById[c]?.color),
            onclick: () => { ui.cat = c; drawList(); } }, catById[c]?.name ?? 'No category')));
        const shown = fits.filter((x) => (ui.cat == null || (x.category_id ?? 0) === ui.cat) && (!term || matches(x, term)));
        setKids(grid, shown.slice(0, 120).map((x) => h('button', {
          class: chosen.includes(x.id) ? 'on' : '',
          onclick: () => {
            const at = chosen.indexOf(x.id);
            if (at >= 0) chosen.splice(at, 1); else chosen.push(x.id);
            onChange();
            drawList();
          },
        }, h('span', {}, x.name), x.name_alt && h('small', { lang: 'th' }, x.name_alt), x.set_only && h('small', {}, 'set only'))),
        !shown.length && h('span', { class: 'muted' }, term ? 'No dish matches your search.' : 'No dishes here.'));
      };
      q.addEventListener('input', () => { ui.q = q.value; drawList(); });
      showAll.onchange = () => { ui.all = showAll.checked; drawList(); };
      drawList();
      return h('div', { class: 'stack dish-picker' },
        h('div', { class: 'row' }, h('div', { class: 'searchbox grow' }, icon('search', 18), q),
          rule && h('label', { class: 'check', title: 'Also list dishes that do not normally belong in this section' }, showAll, ' Show all dishes')),
        tabsEl, grid);
    };
    const draw = () => setKids(list, sections.map((sec, i) => {
      sec.ui ??= { open: !sec.items.length };
      sec.withUi ??= { open: false };
      const pick = h('input', { type: 'number', min: 1, max: 10, value: sec.pick, style: 'width:64px', oninput: (e) => (sec.pick = Math.max(1, Math.min(10, parseInt(e.target.value, 10) || 1))) });
      const chips = h('div', { class: 'opts' });
      const drawChips = () => setKids(chips, sec.items.length ? sec.items.map((id) => h('button', { class: 'chip-btn', title: 'Remove from this section',
        onclick: () => { sec.items.splice(sec.items.indexOf(id), 1); draw(); } }, byId[id]?.name ?? `#${id}`, ' ×'))
        : h('span', { class: 'muted' }, 'No dishes yet: tap "Add dishes".'));
      drawChips();
      const withChips = h('div', { class: 'opts' });
      const drawWith = () => setKids(withChips, sec.with.map((id) => h('button', { class: 'chip-btn', title: 'Remove',
        onclick: () => { sec.with.splice(sec.with.indexOf(id), 1); draw(); } }, byId[id]?.name ?? `#${id}`, ' ×')));
      drawWith();
      const newName = h('input', { placeholder: 'New dish just for sets' });
      const newAlt = h('input', { placeholder: 'Thai (optional)', lang: 'th' });
      const addNew = h('div', { class: 'sec-add' }, newName, newAlt, h('button', { onclick: act(async () => {
        if (!newName.value.trim()) return toast('Type the new dish name', true);
        const made = await call('create_product', { product: {
          name: newName.value.trim(), name_alt: newAlt.value.trim() || null, category_id: null, price_cents: 0, cost_cents: 0,
          tax_rate_bp: Math.round(Number(tax.value || 0) * 100), stock_qty: 0, track_stock: false, starter: sec.starter, prep: false, team_id: null,
        } });
        await call('set_set_only', { id: made.id, setOnly: true });
        made.set_only = true;
        all.push(made); withable.push(made); byId[made.id] = made; sec.items.push(made.id);
        toast(`${made.name} added (only sold in sets; give it a team in the menu list if needed)`);
        draw();
      }) }, '+ New dish'));
      return h('div', { class: 'card stack inset set-edit' },
        h('div', { class: 'sec-head' },
          h('input', { value: sec.name, placeholder: 'Section, e.g. Main', oninput: (e) => (sec.name = e.target.value) }),
          h('input', { value: sec.alt, placeholder: 'Thai name', lang: 'th', oninput: (e) => (sec.alt = e.target.value) }),
          h('label', { class: 'field' }, 'Choose', pick),
          h('label', { class: 'check', title: 'Goes to the kitchen with the starters; the rest waits for "Call mains"' },
            h('input', { type: 'checkbox', checked: sec.starter, onchange: (e) => { sec.starter = e.target.checked; draw(); } }), ' Starter course'),
          h('span', { class: 'sec-tools' }, h('button', { title: 'Move up', disabled: i === 0, onclick: () => { [sections[i - 1], sections[i]] = [sections[i], sections[i - 1]]; draw(); } }, '↑'),
          h('button', { title: 'Move down', disabled: i === sections.length - 1, onclick: () => { [sections[i + 1], sections[i]] = [sections[i], sections[i + 1]]; draw(); } }, '↓'),
          h('button', { class: 'danger', title: 'Remove this section', onclick: () => { sections.splice(i, 1); draw(); } }, '×'))),
        h('div', { class: 'row' }, chips, h('span', { style: 'flex:1' }),
          h('button', { class: sec.ui.open ? 'on' : '', onclick: () => { sec.ui.open = !sec.ui.open; draw(); } }, sec.ui.open ? 'Done' : '+ Add dishes')),
        sec.ui.open && dishPicker(sec.items, all, sec.ui, null, drawChips),
        sec.ui.open && addNew,
        // Optional: each dish here comes with one of these (e.g. rice or noodles).
        h('div', { class: 'with-edit' },
          h('span', { class: 'muted small' }, sec.with.length ? 'Each dish comes with one of:' : 'Comes with (e.g. rice or noodles): none'),
          withChips, h('span', { style: 'flex:1' }),
          h('button', { class: sec.withUi.open ? 'on' : '', onclick: () => { sec.withUi.open = !sec.withUi.open; draw(); } },
            sec.withUi.open ? 'Done' : 'Choose what it comes with')),
        sec.withUi.open && dishPicker(sec.with, withable, sec.withUi, null, drawWith));
    }));
    draw();
    const save = act(async () => {
      fieldErr(name); fieldErr(price);
      if (!name.value.trim()) return fieldErr(name, 'Type a name');
      let cents;
      try { cents = parseMoney(price.value); } catch (e) { return fieldErr(price, e.message); }
      const t = Number(String(tax.value).replace(',', '.').replace('%', '') || 0);
      if (!Number.isFinite(t) || t < 0 || t > 100) return toast('Tax: from 0 to 100', true);
      if (!sections.length) return toast('Add at least one section', true);
      const bad = sections.find((s) => !s.name.trim() || !s.items.length);
      if (bad) return toast(`${bad.name || 'A section'}: give it a name and at least one dish`, true);
      const fields = { name: name.value, name_alt: alt.value.trim() || null, category_id: cat.value ? Number(cat.value) : null,
        price_cents: cents, cost_cents: 0, tax_rate_bp: Math.round(t * 100), track_stock: false, starter: false, prep: false, team_id: null };
      const saved = p
        ? await call('update_product', { id: p.id, product: { ...fields, sku: p.sku, barcode: p.barcode, active: active.checked } })
        : await call('create_product', { product: { ...fields, stock_qty: 0 } });
      try {
        await call('save_set', { setId: saved.id, sections: sections.map((s) => ({
          name: s.name.trim(), name_alt: s.alt.trim() || null, pick: s.pick, starter: s.starter, product_ids: s.items, with_ids: s.with })) });
      } catch (e) {
        // A new set that could not be saved must not stay behind as a plain item.
        if (!p) await call('delete_product', { id: saved.id }).catch(() => {});
        throw e;
      }
      toast(`${saved.name} saved`);
      await changed();
    });
    setKids(form, h('div', { class: 'card stack' },
      h('h3', {}, p ? `Edit set menu: ${p.name}` : 'New set menu'),
      h('div', { class: 'row' }, h('div', { class: 'grow field-col' }, name), cat),
      h('div', { class: 'row' }, alt,
        h('label', { class: 'field' }, taxIncluded ? 'Price (tax included)' : 'Price (before tax)', h('span', { class: 'field-col' }, price)),
        h('label', { class: 'field' }, 'Tax %', tax), p && h('label', { class: 'check' }, active, ' On the menu')),
      h('div', { class: 'muted small' }, 'The bill shows only the set and its price. Each section\'s dishes go to the kitchen under the set\'s name; '
        + '"Starter course" sections go first on dine-in orders. Dishes with choices (e.g. chicken or beef) or options are fine: staff pick them with the dish; options with a price add to the set price.'),
      list,
      h('div', {}, h('button', { onclick: () => { sections.push({ name: '', alt: '', pick: 1, starter: false, items: [], with: [] }); draw(); } }, '+ Section')),
      h('div', { class: 'row' },
        h('button', { class: 'primary', onclick: save }, 'Save set'),
        h('button', { onclick: () => setKids(form) }, 'Cancel'),
        h('span', { style: 'flex:1' }),
        p && isAdmin() && confirmBtn('Delete set', `Tap again to delete "${p.name}"`, async () => {
          await call('delete_product', { id: p.id });
          toast(`${p.name} deleted`);
          await changed();
        }))));
    name.focus();
  }

  state.openEditor = editProduct;

  // ---- 6: paste a whole menu from a spreadsheet ----
  function pasteImport() {
    const text = h('textarea', { rows: 10, style: 'width:100%', placeholder: 'Name\tPrice\tCategory\tThai name\nPad Thai\t9.50\tMains\tผัดไทย\nSpring rolls\t4.50\tStarters\tปอเปี๊ยะ\nCola\t2.50\tDrinks' });
    const out = h('div', { class: 'stack' });
    let rows = [];
    const parse = () => {
      rows = text.value.split(/\r?\n/).map((l) => l.trim()).filter(Boolean).map((line, i) => {
        // Spreadsheets paste tab-separated; typed lines use commas ("quoted, names" allowed).
        const cells = line.includes('\t') ? line.split('\t') : splitCsv(line);
        const [name = '', priceText = '', category = '', alt = ''] = cells.map((c) => c.trim());
        let price = null, error = null;
        try { price = parseMoney(priceText); } catch { error = priceText ? `"${priceText}" is not a price` : 'no price'; }
        if (!name) error = 'no name';
        return { line: i + 1, name, alt, price, category, error };
      });
      // A header line ("Name, Price, Category") is skipped.
      if (rows[0]?.error && /price/i.test(text.value.split(/\r?\n/)[0])) rows.shift();
      const bad = rows.filter((r) => r.error);
      const byCat = (r) => (r.category ? cats.find((c) => c.active && c.name.toLowerCase() === r.category.toLowerCase()) : null);
      const fresh = [...new Map(rows.filter((r) => r.category && !byCat(r)).map((r) => [r.category.toLowerCase(), r.category])).values()];
      setKids(out, rows.length ? h('div', { class: 'panel' }, h('table', {},
        h('tr', {}, h('th', {}, '#'), h('th', {}, 'Item'), h('th', { class: 'num' }, 'Price'), h('th', {}, 'Category'), h('th', {}, '')),
        rows.slice(0, 200).map((r) => h('tr', { class: r.error ? 'bad-row' : '' },
          h('td', { class: 'muted' }, r.line), h('td', {}, r.name, r.alt && h('div', { class: 'alt', lang: 'th' }, r.alt)), h('td', { class: 'num' }, r.price != null ? money(r.price) : ''),
          h('td', {}, r.category || h('span', { class: 'muted' }, 'none'), r.category && !byCat(r) ? h('span', { class: 'badge' }, 'new') : ''),
          h('td', { class: 'field-error' }, r.error ?? ''))))) : '',
        rows.length > 200 && h('div', { class: 'muted' }, `…and ${rows.length - 200} more`),
        fresh.length > 0 && h('div', { class: 'muted' }, `New categories will be created: ${fresh.join(', ')}`),
        h('div', { class: 'row' },
          h('button', { class: 'primary', disabled: !rows.length || bad.length > 0, onclick: act(async () => {
            const r = await call('import_items', { rows: rows.map((x) => ({ name: x.name, name_alt: x.alt || null, price_cents: x.price, category: x.category || null })) });
            toast(`${r.items} items imported` + (r.new_categories.length ? `, ${r.new_categories.length} new categories` : ''));
            await changed();
          }) }, bad.length ? `Fix ${bad.length} line(s) first` : `Import ${rows.length} item(s)`),
          h('button', { onclick: () => setKids(form) }, 'Cancel')));
    };
    text.addEventListener('input', parse);
    setKids(form, h('div', { class: 'card stack' },
      h('h3', {}, 'Paste a menu'),
      h('div', { class: 'muted' }, 'Copy rows from Excel or Google Sheets (columns: name, price, category, Thai name) and paste them here, or type one item per line like "Pad Thai, 9.50, Mains, ผัดไทย". ' +
        'Missing categories are created; each item gets its category\'s tax and starter defaults. Check the preview, then import.'),
      text, out));
    parse();
    text.focus();
  }

  // ---- 8: category tabs + drag to reorder categories ----
  const drawTabs = () => setKids(tabs,
    h('button', { class: state.menuTab === 'all' ? 'on' : '', onclick: () => { state.menuTab = 'all'; drawTabs(); loadProducts(); } }, 'All'),
    catTree.filter((c) => c.active).map((c) => h('button', { class: `${state.menuTab === c.id ? 'on' : ''} ${isMain(c) ? '' : 'sub-tab'}`, style: catStyle(c.color),
      onclick: () => { state.menuTab = c.id; search.value = ''; drawTabs(); loadProducts(); } }, h('span', { class: 'dot' }), isMain(c) ? c.name : `↳ ${c.name}`)),
    h('button', { class: state.menuTab === 'none' ? 'on' : '', onclick: () => { state.menuTab = 'none'; drawTabs(); loadProducts(); } }, 'No category'));
  drawTabs();
  let searchTimer;
  search.addEventListener('input', () => { clearTimeout(searchTimer); searchTimer = setTimeout(loadProducts, 150); });
  showInactive.onchange = loadProducts;

  const catChips = h('div', { class: 'row cat-chips' }, catTree.map((c) => h('button', {
    'data-id': c.id, style: `border-left:6px solid ${c.color || '#d9dce1'}${c.active ? '' : ';opacity:.5'}`,
    onclick: (e) => { if (!e.target.closest('.grip') && !e.currentTarget.dataset.dragged) editCategory(c); },
    title: [c.tax_rate_bp != null ? `tax ${pct(c.tax_rate_bp)}%` : '', c.starter ? 'starters' : ''].filter(Boolean).join(' · '),
  }, h('span', { class: 'grip', title: 'Drag to reorder' }, '⠿'), isMain(c) ? c.name : h('span', {}, h('span', { class: 'muted' }, '↳ '), c.name),
    c.starter ? h('small', { class: 'muted' }, ' starters') : '')));
  sortable(catChips, { item: 'button[data-id]', handle: '.grip', axis: 'x', onDrop: act(async (ids) => {
    await call('reorder_categories', { ids });
    state.menu = null;
    toast('Category order saved');
    await changed(true);
  }) });

  screen(h('div', { class: 'page' },
    h('div', { class: 'stack' },
      h('div', { class: 'grid2' }, h('div', { class: 'card stack' },
        h('div', { class: 'row' }, h('h3', { style: 'flex:1' }, 'Categories'),
          isAdmin() && h('button', { class: 'danger-outline', onclick: () => bulkDelete('Delete categories', cats.map((c) => ({ id: c.id, label: c.name })),
            (ids) => deleteMany('category', ids, 'categor(ies)'), async () => { state.menuTab = 'all'; await changed(); }) }, 'Delete…'),
          h('button', { onclick: () => editCategory(null) }, '+ Category')),
        catChips,
        cats.length > 1 && h('div', { class: 'muted small' }, 'Drag ⠿ to change the order. Tap a category to set its colour and defaults (tax, starters).')),
      h('div', { class: 'card stack' },
        h('div', { class: 'row' }, h('h3', { style: 'flex:1' }, 'Options (sizes, extras…)'),
          isAdmin() && h('button', { class: 'danger-outline', onclick: () => bulkDelete('Delete option groups', groups.map((g) => ({ id: g.id, label: g.name, sub: groupRule(g) })),
            (ids) => deleteMany('option_group', ids, 'group(s)'), () => changed()) }, 'Delete…'),
          h('button', { onclick: () => editGroup(null) }, '+ Option group')),
        h('div', { class: 'row' }, groups.length ? groups.map((g) => h('button', {
          style: g.active ? '' : 'opacity:.5', onclick: () => editGroup(g), title: g.options.filter((o) => o.active).map((o) => o.name).join(', '),
        }, g.name, h('small', { class: 'muted' }, ` ${groupRule(g)}`)))
          : h('span', { class: 'muted' }, 'Create a group like "Size" or "Extras", then tick it on the menu items that use it.')))),
      tabs,
      h('div', { class: 'toolbar' },
        h('button', { class: 'primary', onclick: () => editProduct(null) }, icon('plus', 18), 'Menu item'),
        h('button', { onclick: act(() => editSet(null)) }, icon('plus', 18), 'Set menu'),
        h('button', { onclick: pasteImport }, 'Paste a menu'),
        h('div', { class: 'searchbox grow' }, icon('search', 18), search),
        h('label', { class: 'check' }, showInactive, ' Show removed'),
        isAdmin() && h('button', { class: 'danger-outline', onclick: () => bulkDelete('Delete menu items', (state.menuShown ?? []).map((p) => ({
          id: p.id, label: p.name, sub: [catById[p.category_id]?.name, money(p.price_cents)].filter(Boolean).join(' · '),
        })), (ids) => deleteMany('product', ids, 'item(s)'), () => changed()) }, 'Delete several…')),
      h('div', { class: 'panel' }, list)),
    form));
  await loadProducts();
}

/// Search match on the English or Thai name (`q` is lower case).
function matches(p, q) {
  return p.name.toLowerCase().includes(q) || (p.name_alt ?? '').toLowerCase().includes(q);
}

/// Split one CSV line, honouring "quoted, values".
function splitCsv(line) {
  const out = [];
  let cur = '', quoted = false;
  for (const ch of line) {
    if (ch === '"') quoted = !quoted;
    else if ((ch === ',' || ch === ';') && !quoted) { out.push(cur); cur = ''; }
    else cur += ch;
  }
  out.push(cur);
  return out;
}

// ---------- floor plan editor (admin) ----------

async function renderPlan() {
  const floors = await call('list_floors');
  if (!floors.some((f) => f.id === state.floorId)) state.floorId = floors[0]?.id ?? null;
  // Changes on this floor must be saved or discarded before going elsewhere.
  const busy = () => { if (state.planDirty) { toast('Save or discard changes on this floor first', true); return true; } return false; };
  const newFloor = () => {
    if (busy()) return;
    const name = h('input', { placeholder: 'e.g. Ground floor, Terrace', 'aria-label': 'Floor name' });
    const add = act(async () => {
      const f = await call('create_floor', { name: name.value });
      closeModal();
      state.floorId = f.id;
      renderPlan();
    });
    name.addEventListener('keydown', (e) => { if (e.key === 'Enter') add(e); });
    showModal(h('h3', {}, 'New floor'), name,
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: add }, 'Add floor'), h('span', { class: 'grow' }), h('button', { onclick: closeModal }, 'Cancel')));
    name.focus();
  };
  if (!floors.length) {
    return screen(h('div', { class: 'page narrow card stack' },
      h('h3', {}, 'Floor plan'), h('p', { class: 'muted' }, 'Create your first floor (for example "Ground floor"), then add tables to it.'),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: newFloor }, icon('plus', 18), 'Add a floor'))));
  }
  const floor = floors.find((f) => f.id === state.floorId);
  const { w: PW, h: PH } = state.plan;
  const SNAP = 10;
  let tables = await call('list_tables', { floorId: floor.id });
  let sel = null;
  const plan = planBox(250);
  plan.classList.add('edit');
  const panel = h('div', { class: 'plan-side' });
  // One Save and one Discard, in the tool row above the plan.
  const saveBtn = h('button', { class: 'primary', onclick: act(save) });
  const discardBtn = h('button', { onclick: () => { setDirty(false); renderPlan(); } }, 'Discard');
  const setDirty = (d) => {
    state.planDirty = d;
    saveBtn.textContent = d ? 'Save layout •' : 'Saved';
    saveBtn.disabled = !d;
    discardBtn.disabled = !d;
  };
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
  const snap = (v) => Math.round(v / SNAP) * SNAP;

  function draw() {
    drawTables();
    drawPanel();
  }
  // Only the shapes on the plan (the side card and its fields stay as they are).
  function drawTables() {
    setKids(plan, ...tables.map((t) => {
      const el = tableEl(t, `${t === sel ? 'sel' : ''} ${t.is_table === false ? 'deco' : ''}`);
      el.append(h('b', {}, t.name));
      el.addEventListener('pointerdown', (e) => {
        e.preventDefault();
        sel = t;
        const rect = plan.getBoundingClientRect();
        const scale = PW / rect.width;
        const sx = e.clientX, sy = e.clientY, x0 = t.x, y0 = t.y;
        let moved = false;
        try { el.setPointerCapture(e.pointerId); } catch { /* not a live pointer */ }
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
  }

  function nextName(prefix = 'T') {
    const used = new Set(tables.map((t) => t.name.toLowerCase()));
    for (let n = 1; ; n++) if (!used.has(`${prefix}${n}`.toLowerCase())) return `${prefix}${n}`;
  }
  function freeSpot(w, hh) {
    const hit = (x, y) => tables.some((t) => x < t.x + t.w + SNAP && t.x < x + w + SNAP && y < t.y + t.h + SNAP && t.y < y + hh + SNAP);
    for (let y = 20; y + hh <= PH; y += 20) for (let x = 20; x + w <= PW; x += 20) if (!hit(x, y)) return { x, y };
    return { x: 0, y: 0 };
  }
  // A table, or an area that only shows where something is (bar, window, door).
  const addShape = (shape, isTable = true) => () => {
    const w = isTable ? (shape === 'round' ? 90 : 100) : 160;
    const hh = isTable ? w : 70;
    const t = { id: null, name: nextName(isTable ? 'T' : 'Area '), shape, w, h: hh, is_table: isTable, ...freeSpot(w, hh) };
    tables.push(t);
    sel = t;
    setDirty(true);
    draw();
  };

  // The right-hand card: the chosen shape's name, shape, size and kind.
  function drawPanel() {
    if (!sel) {
      const n = tables.filter((t) => t.is_table !== false).length;
      setKids(panel, h('div', { class: 'card stack plan-empty' },
        icon('plan', 36), h('strong', {}, 'Tap a table to change it'),
        h('div', { class: 'muted small' }, 'Drag tables to move them. Tap one to rename it, change its shape or size, or mark it as not a table.'),
        h('div', { class: 'muted small' }, `${floor.name}: ${n} table${n === 1 ? '' : 's'}${tables.length > n ? ` and ${tables.length - n} other shape${tables.length - n === 1 ? '' : 's'}` : ''}.`)));
      return;
    }
    const t = sel;
    const name = h('input', { value: t.name, 'aria-label': 'Name' });
    const shape = segmented([['rect', 'Square'], ['round', 'Round']], t.shape, (v) => { t.shape = v; apply(); drawPanel(); });
    const w = h('input', { type: 'number', value: t.w, step: SNAP, min: 30, max: 400, 'aria-label': 'Width' });
    const hh = h('input', { type: 'number', value: t.h, step: SNAP, min: 30, max: 400, 'aria-label': 'Depth' });
    const label = () => (t.is_table === false ? t.name : `Table ${t.name}`);
    const title = h('h3', {}, label());
    // Off: a shape that only shows where something is (the bar, a window, the door).
    const isTable = h('input', { type: 'checkbox', checked: t.is_table !== false });
    const kindHint = h('div', { class: 'muted small' });
    const drawKind = () => { kindHint.textContent = isTable.checked
      ? 'Takes orders and is counted on the Tables screen.'
      : 'Only shows where something is (bar, window, door). It never takes an order and is not counted.'; };
    drawKind();
    // Applied as soon as something is typed or picked, so Save can be tapped straight away.
    const apply = () => {
      t.name = name.value.trim() || t.name;
      const size = (el, old) => { const v = Number(el.value); return v >= 30 ? clamp(snap(v), 30, 400) : old; };
      t.w = size(w, t.w);
      t.h = size(hh, t.h);
      t.x = clamp(t.x, 0, PW - t.w);
      t.y = clamp(t.y, 0, PH - t.h);
      title.textContent = label();
      setDirty(true);
      drawTables();
    };
    isTable.addEventListener('change', () => { t.is_table = isTable.checked; drawKind(); apply(); });
    [name, w, hh].forEach((el) => el.addEventListener('input', apply));
    setKids(panel, h('div', { class: 'card stack plan-card' },
      h('div', { class: 'row nowrap' }, title, h('span', { class: 'grow' }),
        h('button', { class: 'icon-btn ghost', title: 'Done', 'aria-label': 'Done', onclick: () => { sel = null; draw(); } }, icon('x', 20))),
      h('div', { class: 'edit-field' }, h('label', {}, 'Name'), name),
      h('div', { class: 'edit-field' }, h('label', {}, 'Shape'), shape),
      h('div', { class: 'plan-size' },
        h('div', { class: 'edit-field' }, h('label', {}, 'Width'), w),
        h('div', { class: 'edit-field' }, h('label', {}, 'Depth'), hh)),
      h('label', { class: 'switch' }, isTable, h('span', {}), 'This is a table'),
      kindHint,
      h('div', { class: 'row plan-card-actions' },
        h('button', { onclick: () => { const c = { ...t, id: null, name: nextName(t.is_table === false ? 'Area ' : 'T'), ...freeSpot(t.w, t.h) }; tables.push(c); sel = c; setDirty(true); draw(); } }, 'Duplicate'),
        h('button', { class: 'danger', onclick: () => { tables = tables.filter((x) => x !== t); sel = null; setDirty(true); draw(); } }, 'Remove'))));
  }

  // This floor: rename it or delete it, in a small pop-up.
  const floorSettings = () => {
    if (busy()) return;
    const rename = h('input', { value: floor.name, 'aria-label': 'Floor name' });
    const doRename = act(async () => {
      await call('update_floor', { id: floor.id, name: rename.value, sortOrder: floor.sort_order });
      closeModal(); toast('Renamed'); renderPlan();
    });
    rename.addEventListener('keydown', (e) => { if (e.key === 'Enter') doRename(e); });
    showModal(h('div', { class: 'row nowrap' }, h('h3', { class: 'grow' }, floor.name), h('button', { class: 'icon-btn ghost', 'aria-label': 'Close', onclick: closeModal }, icon('x'))),
      h('div', { class: 'edit-field' }, h('label', {}, 'Name'), h('div', { class: 'row nowrap' }, rename, h('button', { class: 'primary', onclick: doRename }, 'Rename'))),
      isAdmin() && h('div', { class: 'row' }, confirmBtn('Delete this floor', `Tap again to delete ${floor.name}`, async () => {
        await call('delete_floor', { id: floor.id });
        closeModal(); setDirty(false); state.floorId = null; toast('Floor removed'); renderPlan();
      })));
  };

  async function save() {
    const saved = await call('save_floor_layout', {
      floorId: floor.id,
      tables: tables.map(({ id, name, shape, x, y, w, h: th, is_table }) => ({ id: id ?? null, name, shape, x, y, w, h: th, is_table: is_table !== false })),
    });
    const selName = sel?.name;
    tables = saved;
    sel = tables.find((t) => t.name === selName) ?? null;
    setDirty(false);
    draw();
    toast('Floor plan saved');
  }

  setDirty(false);
  // Floors across the top, the tools under them, then the plan with its card at the side.
  screen(h('div', { class: 'page plan-page' },
    h('div', { class: 'plan-bar' },
      segmented(floors.map((f) => [f.id, f.name]), floor.id, (id) => { if (busy()) return; state.floorId = id; renderPlan(); }),
      h('button', { onclick: newFloor }, icon('plus', 18), 'Floor'),
      h('span', { class: 'grow' }),
      h('button', { onclick: floorSettings }, icon('settings', 18), 'This floor'),
      isAdmin() && floors.length > 1 && h('button', { class: 'danger-outline', onclick: () => {
        if (busy()) return;
        bulkDelete('Delete floors', floors.map((f) => ({ id: f.id, label: f.name })),
          (ids) => deleteMany('floor', ids, 'floor(s)'), () => { setDirty(false); state.floorId = null; return renderPlan(); });
      } }, 'Delete floors…')),
    h('div', { class: 'plan-tools' },
      h('button', { onclick: addShape('rect') }, h('span', { class: 'shape-ico sq' }), 'Square table'),
      h('button', { onclick: addShape('round') }, h('span', { class: 'shape-ico rd' }), 'Round table'),
      h('button', { onclick: addShape('rect', false) }, h('span', { class: 'shape-ico area' }), 'Area (not a table)'),
      h('span', { class: 'grow' }),
      discardBtn, saveBtn),
    h('div', { class: 'plan-editor' }, plan, panel)));
  draw();
}

// ---------- users (admin) ----------

async function renderUsers() {
  const list = h('div');
  const form = popupHost();
  const load = async () => {
    const users = await call('list_users');
    setKids(list, h('table', {},
      h('tr', {}, h('th', {}, 'Name'), h('th', {}, 'Role'), h('th', {}, 'Status'), h('th', {}, 'Last login')),
      users.map((u) => h('tr', { class: 'click', onclick: () => edit(u) },
        h('td', {}, h('span', { class: 'row nowrap' }, h('span', { class: 'avatar sm' }, initials(u.name)), u.name)),
        h('td', {}, h('span', { class: `chip ${u.role === 'staff' ? '' : 'info'}` }, ROLE_LABEL[u.role] ?? 'Staff')),
        h('td', {}, h('span', { class: `status ${u.active ? 's-paid' : 's-cancelled'}` }, u.active ? 'active' : 'disabled')),
        h('td', {}, u.last_login_at ? fmtTime(u.last_login_at) : '—')))));
  };
  function edit(u) {
    const name = h('input', { value: u?.name ?? '', placeholder: 'Name' });
    const role = h('select', { title: 'Staff: take orders and payments. Manager: also the menu, floor plan, voids, reports and kitchen settings (cannot delete or manage staff). Admin: everything.' },
      h('option', { value: 'staff' }, 'Staff'), h('option', { value: 'manager' }, 'Manager'), h('option', { value: 'admin' }, 'Admin'));
    role.value = u?.role ?? 'staff';
    const active = h('input', { type: 'checkbox', checked: u ? u.active : true });
    const pin = h('input', { type: 'password', inputmode: 'numeric', placeholder: u ? 'New PIN (optional)' : 'PIN (4-8 digits)' });
    for (const el of [name, pin]) el.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(e); });
    // Shared PINs are allowed, but say so before saving.
    const shared = h('div', { class: 'warn-box', hidden: true });
    let pinTimer;
    pin.addEventListener('input', () => {
      clearTimeout(pinTimer);
      const v = pin.value;
      if (!/^\d{4,8}$/.test(v)) { shared.hidden = true; return; }
      pinTimer = setTimeout(async () => {
        const names = await invoke('pin_shared', { pin: v, exceptUserId: u?.id ?? null }).catch(() => []);
        if (pin.value !== v) return;
        shared.hidden = !names.length;
        shared.textContent = names.length ? `${names.join(', ')} already ${names.length > 1 ? 'use' : 'uses'} this PIN. Better give each person their own: when someone else types a shared PIN to approve, it counts as the lowest role among them.` : '';
      }, 200);
    });
    const save = act(async () => {
      if (u) {
        await call('update_user', { id: u.id, name: name.value, role: role.value, active: active.checked });
        if (pin.value) await call('reset_pin', { id: u.id, pin: pin.value });
      } else {
        await call('create_user', { name: name.value, role: role.value, pin: pin.value });
      }
      toast('Saved');
      setKids(form);
      await start(); // own role/name may have changed
    });
    setKids(form, h('div', { class: 'card stack' },
      h('h3', {}, u ? `Edit ${u.name}` : 'New staff member'),
      h('label', { class: 'field-col', style: 'display:flex;gap:4px' }, 'Name', name),
      h('div', { class: 'row' }, h('label', { class: 'field' }, 'Role', role), u && h('label', { class: 'check' }, active, ' Active')),
      h('label', { class: 'field-col', style: 'display:flex;gap:4px' }, u ? 'New PIN' : 'PIN', pin,
        h('span', { class: 'muted small' }, u ? 'Leave empty to keep the current PIN.' : '4 to 8 digits; used to log in.'), shared),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Save'), h('button', { onclick: () => setKids(form) }, 'Cancel'))));
    name.focus();
  }
  screen(h('div', { class: 'page' },
    h('div', { class: 'stack' }, h('div', { class: 'toolbar' }, h('button', { class: 'primary', onclick: () => edit(null) }, icon('plus', 18), 'Staff member'),
      h('span', { class: 'muted' }, 'Tap a staff member to edit them or reset their PIN.')),
      h('div', { class: 'panel' }, list)),
    form));
  await load();
}

// ---------- reports (admin) ----------

/// Kitchen screen settings (Manage → Kitchen printer → Kitchen screen; managers
/// may change them). Not set = the defaults, how the screen always worked.
/// kds_paid_hours is used by the POS itself (kitchen_board), the rest here.
const KDS_KEYS = ['kds_slow_mins', 'kds_late_mins', 'kds_refresh_secs', 'kds_paid_hours', 'kds_beep'];
const KDS_LIMITS = { slow: [1, 240, 10], late: [1, 240, 20], refresh: [2, 30, 4], paid: [0, 24, 6] };
function kdsSettings(raw = {}) {
  const int = (v, [lo, hi, def]) => { const n = Number(v ?? NaN); return Number.isInteger(n) && n >= lo && n <= hi ? n : def; };
  const c = { slow: int(raw.kds_slow_mins, KDS_LIMITS.slow), late: int(raw.kds_late_mins, KDS_LIMITS.late),
    refresh: int(raw.kds_refresh_secs, KDS_LIMITS.refresh), paid: int(raw.kds_paid_hours, KDS_LIMITS.paid), beep: raw.kds_beep === '1' };
  if (c.late <= c.slow) c.late = c.slow + 1;
  return c;
}

/// Kitchen screen sound: a short two-tone beep made with WebAudio (no sound file).
/// Browsers only play sound after a tap on the page, so the screen asks for one.
const kdsAudio = { ctx: null };
function kdsAudioCtx() {
  const AC = window.AudioContext || window.webkitAudioContext;
  if (!AC) return null;
  try { kdsAudio.ctx ??= new AC(); } catch { return null; }
  return kdsAudio.ctx;
}
function kdsBeep() {
  const ctx = kdsAudio.ctx;
  if (!ctx || ctx.state !== 'running') return false;
  const t = ctx.currentTime;
  [[0, 880], [0.2, 1175]].forEach(([d, f]) => {
    const o = ctx.createOscillator(), g = ctx.createGain();
    o.type = 'sine'; o.frequency.value = f;
    g.gain.setValueAtTime(0.0001, t + d);
    g.gain.exponentialRampToValueAtTime(0.5, t + d + 0.015);
    g.gain.exponentialRampToValueAtTime(0.0001, t + d + 0.17);
    o.connect(g); g.connect(ctx.destination);
    o.start(t + d); o.stop(t + d + 0.18);
  });
  return true;
}

/// Kitchen screen: one card per order with the dishes still to cook.
/// Tap a dish when it is done; "Done" clears the whole card. Refreshes by itself.
async function renderKitchen() {
  const loadCfg = () => invoke('get_settings', { keys: KDS_KEYS }).then(kdsSettings);
  let [teams, cfg] = await Promise.all([call('list_teams').catch(() => []), loadCfg().catch(() => kdsSettings())]);
  let filter = 'all';
  try { filter = localStorage.getItem('pos.kdsFilter') || 'all'; } catch {}
  let lastDone = null;
  // New tickets beep (Settings → Kitchen screen). Dishes seen so far: anything else is new.
  let seen = null;
  const soundBtn = h('button', { class: 'kds-sound', hidden: true, title: 'Browsers play sound only after a tap' }, 'Tap to turn on the sound');
  const drawSound = () => { soundBtn.hidden = !cfg.beep || kdsAudio.ctx?.state === 'running'; };
  const armSound = () => {
    if (!cfg.beep) return;
    const ctx = kdsAudioCtx();
    if (!ctx) return;
    ctx.onstatechange = drawSound;
    if (ctx.state !== 'running') ctx.resume().then(drawSound).catch(() => {});
    drawSound();
  };
  // On pointerdown: the button hides as soon as the sound is allowed, before a click would land.
  let tried = 0;
  const trySound = () => {
    if (Date.now() - tried < 800) return;
    tried = Date.now();
    armSound();
    setTimeout(() => { if (kdsBeep()) toast('Sound on: the screen beeps when a new ticket arrives'); }, 150);
  };
  soundBtn.addEventListener('pointerdown', trySound);
  soundBtn.onclick = trySound;
  const filters = [['all', 'Everything'], ['food', 'Food'], ...teams.map((t) => [`team:${t.id}`, t.name]), ['drinks', 'Drinks'], ['desserts', 'Desserts']];
  const bar = h('div', { class: 'row kds-bar' });
  const board = h('div', { class: 'kds' });
  const undo = h('button', { hidden: true, onclick: act(async () => {
    if (!lastDone) return;
    await call('kitchen_done', { itemIds: lastDone, done: false });
    lastDone = null; undo.hidden = true;
    await refresh();
  }) }, 'Undo last done');
  const drawBar = () => setKids(bar, filters.map(([v, label]) => h('button', { class: filter === v ? 'on' : '', onclick: () => {
    filter = v;
    try { localStorage.setItem('pos.kdsFilter', v); } catch {}
    seen = null; // another filter: what shows is not new
    drawBar(); refresh();
  } }, label)), h('span', { style: 'flex:1' }), undo, h('button', { onclick: act(toggleFullscreen) }, icon('expand', 18), 'Full screen'));
  const mins = (t) => Math.max(0, Math.floor((Date.now() / 1000 - t) / 60));
  const done = (ids) => act(async () => {
    await call('kitchen_done', { itemIds: ids, done: true });
    lastDone = ids; undo.hidden = false;
    await refresh();
  });
  async function refresh() {
    const orders = await call('kitchen_board', { filter });
    const ids = orders.flatMap((o) => o.items.map((i) => i.id));
    if (seen && cfg.beep && ids.some((id) => !seen.has(id))) kdsBeep();
    if (!seen || seen.size > 5000) seen = new Set(ids); else ids.forEach((id) => seen.add(id));
    setKids(board, orders.length ? orders.map((o) => {
      const age = mins(o.since);
      const todo = o.items.filter((i) => !i.done).map((i) => i.id);
      return h('div', { class: `kds-card ${age >= cfg.late ? 'late' : age >= cfg.slow ? 'slow' : ''}` },
        h('div', { class: 'kds-head' }, h('strong', {}, o.heading), h('span', { class: 'kds-age' }, ageText(age))),
        (o.customer_name || o.paid) && h('div', { class: 'kds-sub' }, [o.customer_name, o.paid ? 'PAID' : ''].filter(Boolean).join(' · ')),
        h('div', { class: 'kds-items' }, o.items.map((i) => h('button', { class: `kds-item ${i.done ? 'done' : ''}`,
          onclick: i.done ? act(async () => { await call('kitchen_done', { itemIds: [i.id], done: false }); await refresh(); }) : done([i.id]) },
          h('span', { class: 'kds-qty' }, `${i.qty}×`),
          h('span', { class: 'kds-name' }, i.name, i.with_name && ` + ${i.with_name}`,
            i.name_alt && h('small', { lang: 'th' }, i.name_alt),
            i.set_name && h('small', {}, `set: ${i.set_name}`),
            i.options.length > 0 && h('small', {}, i.options.join(', ')),
            i.note && h('small', { class: 'kds-note' }, `“${i.note}”`))))),
        h('button', { class: 'primary kds-done', onclick: done(todo) }, `Done (${todo.length})`));
    }) : h('div', { class: 'empty' }, icon('check', 40), h('p', {}, 'Nothing to cook right now')));
  }
  drawBar();
  const page = h('div', { class: 'page kds-page' }, bar, soundBtn, board);
  // Any tap on the screen also allows the sound.
  page.addEventListener('pointerdown', armSound);
  screen(page);
  armSound();
  await refresh();
  // A tablet left on this screen picks up changed settings (colours, refresh) about once a minute.
  let ticks = 0;
  const tick = async () => {
    if (++ticks % Math.max(1, Math.round(60 / cfg.refresh)) === 0) {
      const fresh = await loadCfg().catch(() => null);
      if (!board.isConnected) return;
      if (fresh) {
        const restart = fresh.refresh !== cfg.refresh;
        cfg = fresh;
        armSound(); drawSound();
        if (restart) { clearInterval(refreshTimer); live(tick, cfg.refresh * 1000); }
      }
    }
    await refresh();
  };
  // Left the screen while it was loading: don't start a timer for a screen that is gone.
  if (!board.isConnected) return;
  live(tick, cfg.refresh * 1000);
}

/// Shift & cash: open with a float, money in / out, close by counting.
/// `shown`: a report to show straight away (the shift just closed).
async function renderShift(shown = null) {
  const [cur, past] = await Promise.all([call('current_shift'), call('list_shifts', { limit: 20 }).catch(() => [])]);
  const report = h('pre', { class: 'report', hidden: !shown }, shown ?? '');
  const moneyIn = (placeholder) => h('input', { inputmode: 'decimal', placeholder, style: 'width:140px' });
  const cents = (el) => { try { return el.value.trim() ? parseMoney(el.value) : null; } catch { return null; } };
  let main;
  if (!cur) {
    const float = moneyIn('e.g. 100');
    main = h('div', { class: 'card stack' },
      h('h3', {}, 'No shift open'),
      h('div', { class: 'muted' }, 'Count the cash in the drawer (the float) and open the shift. At the end, count again: the POS shows if anything is missing.'),
      h('div', { class: 'row' }, h('label', { class: 'field' }, 'Cash in the drawer now', float),
        h('button', { class: 'primary lg', onclick: act(async () => {
          const c = cents(float);
          if (c == null) return toast('Enter the cash in the drawer (0 if empty)', true);
          await call('open_shift', { floatCents: c });
          toast('Shift open');
          renderShift();
        }) }, 'Open shift')));
  } else {
    const box = h('div', {});
    const move = (kind) => {
      const amt = moneyIn('Amount');
      const why = h('input', { placeholder: kind === 'in' ? 'Reason (optional), e.g. more change' : 'What for? e.g. milk, supplier', style: 'flex:1' });
      setKids(box, h('div', { class: 'card stack' }, h('strong', {}, kind === 'in' ? 'Put money in the drawer' : 'Take money out of the drawer'),
        h('div', { class: 'row' }, amt, why,
          h('button', { class: 'primary', onclick: act(async () => {
            const c = cents(amt);
            if (!c) return toast('Enter an amount', true);
            const r = await call('cash_move', { kind, amountCents: c, reason: why.value });
            toast(`${kind === 'in' ? 'Money in' : 'Money out'}: ${money(c)}` + (r.print.error ? ` (drawer did not open: ${r.print.error})` : ''));
            renderShift();
          }) }, 'Save'), h('button', { onclick: () => setKids(box) }, 'Cancel'))));
      amt.focus();
    };
    const count = moneyIn('Counted');
    const note = h('input', { placeholder: 'Note (optional)', style: 'flex:1' });
    const diff = h('strong', {});
    const showDiff = () => {
      const c = cents(count);
      diff.textContent = c == null ? '' : c === cur.expected_cents ? 'Exactly right' : c < cur.expected_cents ? `Short ${money(cur.expected_cents - c)}` : `Over ${money(c - cur.expected_cents)}`;
      diff.className = c != null && c < cur.expected_cents ? 'short' : '';
    };
    count.addEventListener('input', showDiff);
    const line = (label, c, cls = '') => h('tr', { class: cls }, h('td', {}, label), h('td', { class: 'num' }, money(c)));
    main = h('div', { class: 'grid2' },
      h('div', { class: 'card stack' },
        h('h3', {}, 'Shift open'),
        h('div', { class: 'muted' }, `Since ${fmtTime(cur.opened_at)} · opened by ${cur.opened_by_name} · ${cur.payment_count} payment(s)`),
        h('table', {},
          line('Float', cur.float_cents), line(`${payName('cash')} sales`, cur.cash_cents),
          cur.cash_tip_cents ? line(`${payName('cash')} tips`, cur.cash_tip_cents) : '',
          cur.paid_in_cents ? line('Money in', cur.paid_in_cents) : '', cur.paid_out_cents ? line('Money out', -cur.paid_out_cents) : '',
          cur.refund_cents ? line(`${payName('cash')} refunds (voids)`, -cur.refund_cents) : '',
          line('Cash that should be in the drawer', cur.expected_cents, 'grand'),
          line(payName('card'), cur.card_cents), cur.card_tip_cents ? line(`${payName('card')} tips`, cur.card_tip_cents) : '',
          cur.other_cents ? line('Platforms / other', cur.other_cents) : ''),
        cur.moves.length > 0 && h('div', { class: 'muted small' }, cur.moves.map((m) => `${m.kind === 'in' ? '+' : '−'}${money(m.amount_cents)} ${m.reason ?? ''} (${m.user_name})`).join(' · ')),
        h('div', { class: 'row' }, h('button', { onclick: () => move('in') }, 'Money in'), h('button', { onclick: () => move('out') }, 'Money out')),
        box),
      h('div', { class: 'card stack' },
        h('h3', {}, 'Close the shift'),
        h('div', { class: 'muted' }, 'Count all the cash in the drawer and type the total.'),
        h('div', { class: 'row' }, h('label', { class: 'field' }, 'Cash counted', count), diff),
        h('div', { class: 'row' }, note),
        h('button', { class: 'primary lg', onclick: act(async () => {
          const c = cents(count);
          if (c == null) return toast('Type the cash you counted', true);
          const r = await call('close_shift', { countedCents: c, note: note.value });
          toast('Shift closed' + (r.print.error ? ` (report did not print: ${r.print.error})` : r.print.attempted ? ' · report printed' : ''), !!r.print.error);
          await renderShift(r.text);
          document.querySelector('pre.report')?.scrollIntoView({ block: 'nearest' });
        }) }, 'Close shift & print')));
  }
  const closed = past.filter((p) => p.shift.closed_at);
  screen(h('div', { class: 'page' }, main,
    closed.length > 0 && h('div', { class: 'card stack' }, h('h3', {}, 'Past shifts'), h('table', {},
      closed.map((p) => h('tr', {},
        h('td', {}, `${fmtTime(p.shift.opened_at)} – ${fmtTime(p.shift.closed_at)}`), h('td', { class: 'muted' }, p.shift.closed_by_name),
        h('td', { class: 'num' }, `expected ${money(p.shift.expected_cents)}`),
        h('td', { class: `num ${p.shift.difference_cents < 0 ? 'short' : ''}` }, p.shift.difference_cents === 0 ? 'OK' : `${p.shift.difference_cents < 0 ? 'short' : 'over'} ${money(Math.abs(p.shift.difference_cents))}`),
        h('td', {}, h('button', { onclick: () => { report.textContent = p.text; report.hidden = false; } }, 'View'),
          h('button', { onclick: act(async () => printResult(await call('print_shift', { id: p.shift.id }), 'Shift report printed')) }, 'Print')))))),
    report));
}

async function renderReports() {
  const date = h('input', { type: 'date', value: today() });
  const out = h('div');
  const eod = h('div', { class: 'card stack' });
  const none = () => h('div', { class: 'muted', style: 'margin-top:6px' }, 'No sales yet');
  const rows = (pairs) => pairs.length ? h('table', {}, pairs.map(([k, v]) => h('tr', {}, h('td', {}, k), h('td', { class: 'num' }, v)))) : none();

  // End of day: the open business day so far, the close button, past closes.
  const drawEod = async (shown = null) => {
    const day = await call('business_day');
    const now = Math.floor(Date.now() / 1000) + 1;
    const [s, closes] = await Promise.all([call('sales_summary', { from: day.since, to: now }), call('day_closes', { limit: 30 })]);
    const report = h('pre', { class: 'report', hidden: !shown }, shown?.text ?? '');
    const closeBtn = h('button', { class: 'primary', disabled: day.open_orders > 0 }, 'Close day & print summary');
    closeBtn.onclick = act(async () => {
      if (!closeBtn.dataset.sure) {
        closeBtn.dataset.sure = '1';
        closeBtn.textContent = `Tap again to close ${dayLabel(day.date)}`;
        return;
      }
      closeBtn.disabled = true;
      const r = await call('close_day');
      await drawBusinessDay();
      const c = r.view.close;
      const msg = `${dayLabel(c.business_date)} closed: ${c.summary.order_count} orders, ${money(c.summary.total_cents)}. Now on ${dayLabel(state.day.date)}`;
      if (r.empty) toast(`${dayLabel(c.business_date)} closed. No sales today, so nothing was printed. Now on ${dayLabel(state.day.date)}`);
      else if (r.print.error) toast(`${msg}. The summary did not print (${r.print.error}); reprint it below.`, true);
      else toast(r.print.attempted ? `${msg} · summary printed` : `${msg} · no receipt printer, summary shown below`);
      await drawEod(r.view);
    });
    setKids(eod, 
      h('div', { class: 'row' }, h('h3', { style: 'flex:1' }, `End of day · ${dayLabel(day.date)}`),
        h('span', { class: 'muted' }, `since ${friendlyTime(day.since)}`)),
      h('div', { class: 'row' },
        h('span', {}, `${s.order_count} paid orders · `, h('strong', {}, money(s.total_cents))),
        ...s.by_payment.map((g) => h('span', { class: 'badge' }, `${payName(g.key, 'platform')} ${money(g.total_cents)}`)),
        h('span', { style: 'flex:1' }),
        day.open_orders > 0 && h('span', { class: 'badge warn' }, `${day.open_orders} open order(s): pay or cancel them first`),
        closeBtn),
      h('div', { class: 'muted' }, 'Closing prints the day\'s sales summary on the receipt printer and moves the business day to the next date.'),
      report,
      closes.length ? h('details', {}, h('summary', {}, 'Previous days'), h('table', {},
        closes.map((v) => h('tr', {},
          h('td', {}, dayLabel(v.close.business_date)), h('td', { class: 'muted' }, `closed ${fmtTime(v.close.to)} by ${v.close.closed_by_name}`),
          h('td', { class: 'num' }, v.close.summary.order_count), h('td', { class: 'num' }, money(v.close.summary.total_cents)),
          h('td', {}, h('button', { onclick: () => { report.textContent = v.text; report.hidden = false; } }, 'View'),
            h('button', { onclick: act(async () => printResult(await call('reprint_day_close', { id: v.close.id }), 'Summary printed')) }, 'Print')))))) : '');
  };

  const load = act(async () => {
    const [from, to] = dayRange(date.value);
    const [s, top] = await Promise.all([call('sales_summary', { from, to }), call('top_products', { from, to, limit: 20 })]);
    setKids(out, h('div', { class: 'grid2' },
      h('div', { class: 'stack' },
        h('div', { class: 'card' }, h('h3', {}, 'Summary'), rows([
          ['Paid orders', s.order_count], [s.tax_included_cents ? 'Sales (menu prices)' : 'Gross', money(s.subtotal_cents)], ['Discounts', money(s.discount_cents)],
          ...(s.tax_cents - s.tax_included_cents ? [['Tax added on top', money(s.tax_cents - s.tax_included_cents)]] : []),
          ['Service charge', money(s.service_cents)], ['Total', money(s.total_cents)],
          ...(s.tip_cents ? [['Tips (not in total)', money(s.tip_cents)]] : []),
          ...(s.tax_included_cents ? [['Tax included in prices', money(s.tax_included_cents)]] : []),
          ['Voided / refunded', `${s.voided_count} (${money(s.voided_total_cents)})`], ['Cancelled', s.cancelled_count]])),
        h('div', { class: 'card' }, h('h3', {}, 'By order type'),
          rows(s.by_type.map((g) => [TYPE[g.key] ?? g.key, `${g.count} · ${money(g.total_cents)}`]))),
        h('div', { class: 'card' }, h('h3', {}, 'By platform'),
          rows(s.by_channel.map((g) => [g.key, `${g.count} · ${money(g.total_cents)}`]))),
        h('div', { class: 'card' }, h('h3', {}, 'By payment'),
          rows(s.by_payment.map((g) => [payName(g.key, 'Other / platform'), `${g.count} · ${money(g.total_cents)}`])))),
      h('div', { class: 'stack' },
        h('div', { class: 'card' }, h('h3', {}, 'By category (before tax)'),
          rows(s.by_category.map((g) => [g.key, `${g.count} · ${money(g.total_cents)}`]))),
        h('div', { class: 'card' }, h('h3', {}, 'Top items'), top.length ? h('table', {},
          top.map((t) => h('tr', {}, h('td', {}, t.name), h('td', { class: 'num' }, t.qty), h('td', { class: 'num' }, money(t.revenue_cents))))) : none()))));
  });
  date.onchange = load;
  screen(h('div', { class: 'page' }, eod, h('div', { class: 'section-title' }, h('h3', {}, 'Sales by calendar day'), h('span', { class: 'spacer' }), date), out));
  await Promise.all([drawEod(), load()]);
}

// ---------- settings (admin) ----------

// ---------- customers ----------

async function renderCustomers() {
  const q = h('input', { placeholder: 'Search: phone, name or postcode', style: 'flex:1' });
  const list = h('div');
  const panel = h('div', { 'data-hint': 'Find a customer to edit them, see their orders or start a new order.' });
  let timer;
  let found = [];
  const load = async () => {
    found = await call('search_customers', { query: q.value, limit: 100 });
    setKids(list, found.length ? h('table', {},
      h('tr', {}, h('th', {}, 'Name'), h('th', {}, 'Phone'), h('th', {}, 'Address'), h('th', { class: 'num' }, 'Orders')),
      found.map((c) => h('tr', { class: 'click', onclick: () => edit(c) },
        h('td', {}, c.name), h('td', {}, c.phone ?? ''), h('td', { class: 'muted' }, addressText(c)),
        h('td', { class: 'num' }, c.order_count))))
      : h('div', { class: 'empty small' }, icon('customers', 36), h('p', {}, q.value ? 'No customers match.' : 'No saved customers yet. They are added when you take delivery or collection orders, or with + Customer.')));
  };
  q.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(() => load().catch(() => {}), 150); });

  async function edit(c) {
    const ce = customerEditor({ customer: c, search: false, saveToggle: false });
    const history = h('div');
    const orderFor = (type) => act(async () => {
      await ce.commit(true);
      await startCustomerOrder(type, ce, null);
    });
    setKids(panel, h('div', { class: 'card stack' },
      h('h3', {}, c ? c.name : 'New customer'), ce.el,
      h('div', { class: 'row' },
        h('button', { class: 'primary', onclick: act(async () => { const saved = await ce.commit(true); toast('Saved'); await load(); await edit(saved); }) }, 'Save'),
        typeShown('delivery') && h('button', { onclick: orderFor('delivery') }, 'New delivery order'),
        typeShown('collection') && h('button', { onclick: orderFor('collection') }, 'New collection order'),
        c && isAdmin() && h('button', { class: 'danger', onclick: act(async () => {
          await call('delete_customer', { id: c.id }); toast('Customer removed'); setKids(panel); await load();
        }) }, 'Delete')),
      history));
    if (c) {
      const orders = await call('customer_orders', { id: c.id });
      setKids(history, orders.length ? h('div', { class: 'stack' }, h('strong', {}, 'Recent orders'), h('table', {},
        orders.map((o) => h('tr', { class: 'click', onclick: () => openOrder(o.id) },
          h('td', {}, `#${o.id}`), h('td', {}, fmtTime(o.opened_at)), h('td', {}, TYPE[o.order_type]),
          h('td', {}, o.status), h('td', { class: 'num' }, money(o.total_cents)))))) : '');
    }
    if (!c) ce.focus();
  }

  screen(h('div', { class: 'page split' },
    h('div', { class: 'stack' }, h('div', { class: 'toolbar' }, h('div', { class: 'searchbox grow' }, icon('search', 20), q),
      isAdmin() && h('button', { class: 'danger-outline', onclick: () => bulkDelete('Delete customers', found.map((c) => ({ id: c.id, label: c.name, sub: c.phone ?? '' })),
        (ids) => deleteMany('customer', ids, 'customer(s)'), async () => { setKids(panel); await load(); }) }, 'Delete several…'),
      h('button', { class: 'primary', onclick: () => edit(null) }, icon('plus', 18), 'Customer')), h('div', { class: 'panel' }, list)),
    h('div', { class: 'stack sticky' }, panel)));
  await load();
  q.focus();
}

/// Settings card for the delivery platform buttons (Uber Eats...).
function platformsCard() {
  const card = h('div', { class: 'card stack' });
  let rows = [];
  const list = h('div', { class: 'stack' });
  const draw = () => setKids(list, ...rows.map((r, i) => h('div', { class: 'row' },
    h('input', { value: r.name, placeholder: 'Platform name', oninput: (e) => (r.name = e.target.value) }),
    h('input', { type: 'color', value: r.color || '#374151', oninput: (e) => (r.color = e.target.value) }),
    h('button', { disabled: i === 0, onclick: () => { [rows[i - 1], rows[i]] = [rows[i], rows[i - 1]]; draw(); } }, '↑'),
    h('button', { class: 'danger', onclick: () => { rows.splice(i, 1); draw(); } }, '×'))));
  const load = async () => {
    rows = (await call('list_channels')).map((c) => ({ id: c.id, name: c.name, color: c.color }));
    draw();
  };
  card.append(
    h('h3', {}, 'Delivery platforms'),
    h('div', { class: 'muted' }, 'Each platform gets a quick button on the Orders screen, its own heading on kitchen tickets and a line in reports.'),
    list,
    h('div', { class: 'row' },
      h('button', { onclick: () => { rows.push({ id: null, name: '', color: '#374151' }); draw(); list.lastChild.querySelector('input').focus(); } }, '+ Platform'),
      h('button', { class: 'primary', onclick: act(async () => {
        await call('save_channels', { channels: rows.filter((r) => r.name.trim()).map((r) => ({ id: r.id, name: r.name, color: r.color })) });
        toast('Platforms saved');
        await load();
      }) }, 'Save platforms')));
  load().catch(() => {});
  return card;
}

/// Printer "Image mode" choices (saved value, label), numbered as on the
/// Thai test page so "the one marked 2" is the one to pick. Standard is the default.
const IMAGE_MODES = [['column', '1. Compatible (older printers)'], ['raster_small', '2. Small pieces'], ['raster', '3. Standard (default)']];

/// "Test Thai printing" for a printer form: one page with the Thai sample in every image mode.
const thaiTestButton = (form) => h('button', { onclick: act(async () => {
  if (form.none()) return toast('Choose a printer first', true);
  printResult(await call('test_thai_printer', { config: form.config() }), 'Thai test page sent: choose the mode whose Thai prints clearly');
}) }, 'Test Thai printing');

/// Connection, paper and copies for one printer (kitchen or receipt).
/// `names`: [[value, label]...] for "Names on tickets"; the first is the default.
/// `courses`: also offer copies for starters and mains tickets (kitchen).
function printerForm(cfg, systemPrinters, noneLabel, names, courses = false, simple = false) {
  const kind = h('select', {},
    h('option', { value: 'none' }, noneLabel),
    h('option', { value: 'network' }, 'Network printer (IP address)'),
    h('option', { value: 'windows' }, 'Printer installed in Windows (USB)'),
    h('option', { value: 'file' }, 'Serial port or shared printer path'));
  kind.value = cfg.kind;
  const host = h('input', { value: cfg.host ?? '', placeholder: '192.168.1.50' });
  const port = h('input', { value: cfg.port ?? 9100, inputmode: 'numeric', style: 'width:90px' });
  const winName = systemPrinters.length
    ? h('select', {}, systemPrinters.map((n) => h('option', { value: n }, n)))
    : h('input', { placeholder: 'Printer name as shown in Windows' });
  if (cfg.name) {
    if (systemPrinters.length && !systemPrinters.includes(cfg.name)) winName.append(h('option', { value: cfg.name }, cfg.name));
    winName.value = cfg.name;
  }
  const path = h('input', { value: cfg.path ?? '', placeholder: 'COM3 or \\\\PC-NAME\\Kitchen', style: 'width:260px' });
  const width = h('select', {}, h('option', { value: 42 }, '80 mm paper'), h('option', { value: 32 }, '58 mm paper'), h('option', { value: 48 }, '80 mm, small font (48)'));
  width.value = String(cfg.width ?? 42);
  const copies = h('input', { type: 'number', min: 1, max: 9, value: cfg.copies ?? 1, style: 'width:70px' });
  const delay = h('input', { type: 'number', min: 0, max: 30, value: cfg.copy_delay_secs ?? 2, style: 'width:70px', title: 'Seconds to wait between copies' });
  const bold = h('input', { type: 'checkbox', checked: cfg.bold ?? true });
  const large = h('input', { type: 'checkbox', checked: cfg.large_text ?? true });
  const tableOnly = h('input', { type: 'checkbox', checked: cfg.call_table_only ?? true });
  const appFont = h('input', { type: 'checkbox', checked: cfg.app_font ?? true });
  // On unless switched off (older saved settings too): cheap printers lose data sent all at once.
  const slow = h('input', { type: 'checkbox', checked: cfg.slow_send ?? true });
  const slowRow = h('div', { class: 'stack', style: 'gap:2px' },
    h('label', { class: 'check' }, slow, ' Send slowly (for printers that print junk or skip lines)'),
    h('div', { class: 'muted small' }, 'Sends each ticket in small pieces so the printer keeps up. The till does not wait; a long ticket finishes a second or two later. Network and serial printers only.'));
  const courseCopies = (v) => h('input', { type: 'number', min: 1, max: 9, value: v ?? '', placeholder: 'same', style: 'width:80px' });
  const starterCopies = courseCopies(cfg.starter_copies), mainsCopies = courseCopies(cfg.mains_copies), prepCopies = courseCopies(cfg.prep_copies);
  const additionalCopies = h('input', { type: 'number', min: 1, max: 9, value: cfg.additional_copies ?? '', placeholder: '1', style: 'width:80px' });
  const copiesOf = (el) => (el.value.trim() ? Math.min(9, Math.max(1, parseInt(el.value, 10) || 1)) : null);
  const lang = h('select', {}, names.map(([v, label]) => h('option', { value: v }, label)));
  lang.value = names.some(([v]) => v === cfg.names) ? cfg.names : names[0][0];
  // How Thai text (drawn as an image) is sent; "Test Thai printing" prints every mode.
  const imageMode = h('select', { 'aria-label': 'Image mode (for Thai text)' }, IMAGE_MODES.map(([v, label]) => h('option', { value: v }, label)));
  imageMode.value = IMAGE_MODES.some(([v]) => v === cfg.image_mode) ? cfg.image_mode : 'raster';
  const fields = h('div', { class: 'row' });
  const drawFields = () => {
    setKids(fields, ...{
      none: [],
      network: [h('label', { class: 'field' }, 'IP address', host), h('label', { class: 'field' }, 'Port', port)],
      windows: [h('label', { class: 'field' }, 'Printer', winName)],
      file: [h('label', { class: 'field' }, 'Port or path', path)],
    }[kind.value]);
    // Windows printers are paced by the spooler and USB already.
    slowRow.hidden = !['network', 'file'].includes(kind.value);
  };
  kind.onchange = drawFields;
  drawFields();
  return {
    el: h('div', { class: 'stack' }, h('div', { class: 'row' }, kind, width, h('label', { class: 'field' }, 'Copies', copies),
        !simple && h('label', { class: 'field' }, 'Delay between copies', delay, 'sec')), fields,
      courses && h('div', { class: 'row' }, h('label', { class: 'field' }, 'Starters ticket copies', starterCopies),
        h('label', { class: 'field' }, 'Mains ticket copies', mainsCopies),
        h('label', { class: 'field' }, 'Prepare ticket copies', prepCopies),
        h('span', { class: 'muted small' }, 'Dine-in. Starters: when starters are sent first, or the table orders only starters for now. ' +
          'Mains: "Call mains", and the first mains sent after the starters (also when the mains were ordered later). Empty = same as Copies.')),
      courses && h('div', { class: 'row' }, h('label', { class: 'field' }, 'Additional ticket copies', additionalCopies),
        h('span', { class: 'muted small' }, 'More dishes ordered after the mains went (ADDITIONAL); empty = 1.')),
      !simple && h('div', { class: 'row' }, h('label', { class: 'field' }, 'Item names', lang),
        h('span', { class: 'muted small' }, 'Thai names are set on each menu item; items without one print in English.')),
      !simple && h('label', { class: 'check' }, bold, ' Bold text (easier to read; matches the Thai)'),
      // Label above the select: stays readable on a phone.
      h('label', { class: 'field-col', style: 'gap:4px;align-self:flex-start;max-width:100%;color:var(--ink-2)' }, 'Image mode (for Thai text)', imageMode),
      h('div', { class: 'muted small' }, 'Thai text (and all kitchen text in the app\'s font) prints as pictures. If it doesn\'t print, tap Test Thai printing and choose the number that prints clearly.'),
      slowRow,
      courses && h('label', { class: 'check' }, appFont, ' Print tickets in the app\'s font (the same as the team names); off = the printer\'s own font'),
      courses && h('label', { class: 'check' }, tableOnly, ' "Call mains again" prints only the table, in huge letters (no dishes)'),
      courses && h('label', { class: 'check' }, large, ' Large text for dishes, options and notes (fewer letters per line)')),
    none: () => kind.value === 'none',
    config: () => ({
      kind: kind.value, width: Number(width.value), copies: Math.min(9, Math.max(1, parseInt(copies.value, 10) || 1)),
      copy_delay_secs: Math.min(30, Math.max(0, parseInt(delay.value, 10) || 0)),
      names: lang.value,
      bold: bold.checked,
      image_mode: imageMode.value,
      slow_send: slow.checked,
      ...(courses && { starter_copies: copiesOf(starterCopies), mains_copies: copiesOf(mainsCopies), prep_copies: copiesOf(prepCopies), additional_copies: copiesOf(additionalCopies), large_text: large.checked, call_table_only: tableOnly.checked, app_font: appFont.checked }),
      ...(kind.value === 'network' && { host: host.value.trim(), port: Number(port.value) || 9100 }),
      ...(kind.value === 'windows' && { name: winName.value }),
      ...(kind.value === 'file' && { path: path.value.trim() }),
    }),
  };
}

/// Settings card: defaults for new menu items.
function menuDefaultsCard() {
  const tax = h('input', { inputmode: 'decimal', style: 'width:90px', placeholder: '0' });
  const fill = h('input', { type: 'checkbox', checked: true });
  const incl = h('input', { type: 'checkbox', checked: true });
  invoke('get_setting', { key: 'prices_include_tax' }).then((v) => (incl.checked = v !== '0')).catch(() => {});
  incl.onchange = act(async () => {
    await call('set_setting', { key: 'prices_include_tax', value: incl.checked ? '1' : '0' });
    state.menu = null;
    toast(incl.checked ? 'New orders: menu prices include the tax' : 'New orders: tax is added on top of menu prices');
  });
  const untaxed = h('span', { class: 'muted' });
  const count = () => invoke('untaxed_items').then((n) => {
    untaxed.textContent = n ? `(${n} item${n === 1 ? '' : 's'} at 0% now)` : '';
  }).catch(() => {});
  invoke('get_setting', { key: 'default_tax_bp' }).then((v) => (tax.value = v ? pct(Number(v)) : '')).catch(() => {});
  count();
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Menu defaults'),
    h('label', { class: 'switch' }, incl, h('span', {}, ''), 'Menu prices include tax'),
    h('div', { class: 'muted' }, 'On: the price on the menu is what the customer pays and the tax is worked out inside it (10.00 at 20% = 8.33 + 1.67 tax). ' +
      'Off: the tax is added on top (10.00 + 2.00 = 12.00). Orders already open keep the way they were rung up.'),
    h('div', { class: 'muted' }, 'Tax rate for menu items (e.g. 20 for 20% VAT). A category can use its own rate instead (Menu → tap the category); ' +
      'set a category to 0% for items that are really tax-free.'),
    h('div', { class: 'row' }, h('label', { class: 'field' }, 'Default tax', tax, '%'),
      h('button', { class: 'primary', onclick: act(async () => {
        const v = Number((tax.value || '0').replace(',', '.').replace('%', ''));
        if (!Number.isFinite(v) || v < 0 || v > 100) return toast('Enter a tax rate from 0 to 100', true);
        await call('set_setting', { key: 'default_tax_bp', value: String(Math.round(v * 100)) });
        const n = fill.checked ? await call('apply_default_tax') : 0;
        state.menu = null;
        toast(n ? `Default tax ${v}% saved and applied to ${n} item${n === 1 ? '' : 's'} that had 0%` : `Default tax ${v}% saved`);
        count();
      }) }, 'Save')),
    h('label', { class: 'check' }, fill, ' Also apply it to existing items that are at 0% ', untaxed));
}

/// Settings card: kitchen teams (a separate ticket with a big header for each).
function teamsCard() {
  const on = h('input', { type: 'checkbox', checked: true });
  invoke('get_setting', { key: 'teams_on' }).then((v) => { on.checked = v !== '0'; body.hidden = !on.checked; }).catch(() => {});
  on.onchange = act(async () => {
    await call('set_setting', { key: 'teams_on', value: on.checked ? '1' : '0' });
    body.hidden = !on.checked;
    toast(on.checked ? 'Kitchen teams on' : 'Kitchen teams off: tickets print without team headers (your teams are kept)');
  });
  const split = h('input', { type: 'checkbox', checked: true });
  invoke('get_setting', { key: 'teams_split' }).then((v) => (split.checked = v !== '0')).catch(() => {});
  split.onchange = act(async () => {
    await call('set_setting', { key: 'teams_split', value: split.checked ? '1' : '0' });
    toast(split.checked ? 'Each team gets its own ticket' : 'One ticket for the whole kitchen');
  });
  let rows = [];
  const list = h('div', { class: 'stack' });
  const draw = () => setKids(list, rows.map((r, i) => h('div', { class: 'row team-row', 'data-team': i },
    h('input', { value: r.name, placeholder: 'Team name, e.g. Curry', style: 'width:180px', oninput: (e) => (r.name = e.target.value) }),
    h('input', { value: r.name_alt ?? '', placeholder: 'Thai, e.g. แกง', lang: 'th', class: 'alt-input', oninput: (e) => (r.name_alt = e.target.value) }),
    r.items != null && h('span', { class: 'muted small' }, `${r.items} item${r.items === 1 ? '' : 's'}`),
    h('button', { title: 'Move up', disabled: i === 0, onclick: () => { [rows[i - 1], rows[i]] = [rows[i], rows[i - 1]]; draw(); } }, '↑'),
    h('button', { title: 'Move down', disabled: i === rows.length - 1, onclick: () => { [rows[i + 1], rows[i]] = [rows[i], rows[i + 1]]; draw(); } }, '↓'),
    h('button', { class: 'danger', title: 'Remove this team', onclick: () => { rows.splice(i, 1); draw(); } }, '×'))),
    !rows.length && h('div', { class: 'muted' }, 'No teams: the kitchen gets one ticket.'));
  const load = () => invoke('list_teams').then((t) => { rows = t.map((x) => ({ ...x })); draw(); }).catch((e) => toast(String(e?.message ?? e), true));
  load();
  // Everything below the main switch hides while teams are off.
  const body = h('div', { class: 'stack' });
  const card = h('div', { class: 'card stack' },
    h('h3', {}, 'Kitchen teams'),
    h('label', { class: 'switch' }, on, h('span', {}, ''), 'Use kitchen teams'),
    body);
  // Language of the team's bar on tickets (setting team_names).
  const lang = h('select', {}, [['both', 'English / Thai'], ['english', 'English only'], ['thai', 'Thai only']].map(([v, t]) => h('option', { value: v }, t)));
  invoke('get_setting', { key: 'team_names' }).then((v) => (lang.value = v || 'both')).catch(() => {});
  lang.onchange = act(async () => {
    await call('set_setting', { key: 'team_names', value: lang.value });
    toast(`Team names on tickets: ${lang.selectedOptions[0].textContent}`);
  });
  setKids(body,
    h('label', { class: 'switch' }, split, h('span', {}, ''), 'Print a separate ticket for each team'),
    h('label', { class: 'field' }, 'Team name on tickets', lang),
    h('div', { class: 'muted' }, 'When on, each team prints its own ticket with its name as a big white-on-black header, so Starter, Main, Curry and Rice each get just their dishes; off = one ticket for everything, still with each team\'s header over its dishes. ' +
      'Choose a team for each category (Menu → tap the category) or for single items. Items without a team print on a plain ticket last. ' +
      'A team is where a ticket prints; "Starter" on an item is when it is sent.'),
    list,
    h('div', { class: 'row' },
      h('button', { onclick: () => { rows.push({ id: null, name: '', name_alt: '', items: null }); draw(); list.querySelector('.team-row:last-child input')?.focus(); } }, '+ Team'),
      h('button', { class: 'primary', onclick: act(async () => {
        if (rows.some((r) => !r.name.trim())) return toast('Give every team a name', true);
        await call('save_teams', { teams: rows.map((r) => ({ id: r.id, name: r.name.trim(), name_alt: (r.name_alt ?? '').trim() || null })) });
        state.menu = null;
        await load();
        reloadTeamChoice?.();
        toast('Teams saved');
      }) }, 'Save teams'),
      h('button', { onclick: act(async () => { printResult(await call('test_teams_print'), 'Sample tickets printed'); }) }, 'Print sample tickets')));
  return card;
}

/// Settings card: what tickets and bills call each kind of order.
function ticketLabelsCard() {
  const defs = [['label_dine_in', 'Dine-in', 'DINE-IN', 'Printed before the table, e.g. DINE-IN T3. Leave empty to print the table alone.'],
    ['label_takeaway', 'Takeaway', 'TAKEAWAY'], ['label_collection', 'Collection', 'COLLECTION'], ['label_delivery', 'Delivery', 'DELIVERY'],
    ['label_table', 'Table (main course call)', 'TABLE', 'Before the number on the "Call mains again" ticket. Leave empty for the number alone.']];
  const inputs = defs.map(([key, , def]) => {
    const el = h('input', { placeholder: def, style: 'width:200px' });
    invoke('get_setting', { key }).then((v) => { if (v != null) el.value = v; }).catch(() => {});
    return el;
  });
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Words on tickets and bills'),
    h('div', { class: 'muted' }, 'What kitchen tickets, bills and receipts call each kind of order. Empty = the standard word.'),
    defs.map(([, label, , hint], i) => h('div', { class: 'row' }, h('label', { class: 'field' }, label, inputs[i]), hint && h('span', { class: 'muted small' }, hint))),
    h('div', { class: 'row' }, h('button', { class: 'primary', onclick: act(async () => {
      for (const [i, [key]] of defs.entries()) await call('set_setting', { key, value: inputs[i].value.trim() });
      toast('Saved');
    }) }, 'Save')));
}

/// Settings card: the quick notes staff tap onto an order line.
function quickNotesCard() {
  const text = h('textarea', { rows: 6, style: 'width:100%', placeholder: 'One note per line, e.g.\nNo onions\nExtra spicy\nNo ice' });
  invoke('get_setting', { key: QUICK_NOTES_KEY }).then((v) => (text.value = quickNotes(v).join('\n'))).catch(() => {});
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Quick notes'),
    h('div', { class: 'muted' }, 'Buttons shown when staff tap ✎ on an order line, so common requests are one tap (they print on the kitchen ticket). One per line.'),
    text,
    h('div', { class: 'row' }, h('button', { class: 'primary', onclick: act(async () => {
      const list = text.value.split('\n').map((n) => n.trim()).filter(Boolean);
      if (list.some((n) => n.length > 40)) return toast('Keep each quick note to 40 characters', true);
      if (list.length > 30) return toast('Up to 30 quick notes', true);
      await call('set_setting', { key: QUICK_NOTES_KEY, value: list.join('\n') });
      state.menu = null;
      text.value = list.join('\n');
      toast(list.length ? `${list.length} quick notes saved` : 'Quick notes removed');
    }) }, 'Save')));
}

/// Settings card: the screen staff see right after logging in.
function startScreenCard() {
  const seg = h('div', { class: 'seg' });
  const draw = (cur) => setKids(seg, START_SCREENS.map(([v, label]) => h('button', {
    class: v === cur ? 'on' : '',
    onclick: act(async () => {
      await call('set_setting', { key: 'start_screen', value: v });
      draw(v);
      toast(`After login: ${label}`);
    }),
  }, label)));
  draw(state.ui?.startScreen ?? 'tables');
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Start screen after login'),
    seg,
    h('div', { class: 'muted small' }, 'New takeaway opens a new takeaway order at once; leaving it empty drops it.'));
}

/// Settings card: hide the new-order buttons for kinds of order a restaurant never takes.
function orderTypesCard() {
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Order types'),
    h('div', { class: 'muted small' }, 'Buttons for new orders on the Orders screen. Dine-in (from Tables) always shows; open orders of a hidden type still work.'),
    settingSwitch('show_takeaway', 'Takeaway', 'Takeaway button shown', 'Takeaway button hidden'),
    settingSwitch('show_collection', 'Collection', 'Collection button shown', 'Collection button hidden'),
    settingSwitch('show_delivery', 'Delivery (your own drivers)', 'Delivery button shown', 'Delivery button hidden'));
}

/// Settings card: quick discount buttons and how much staff may give without a manager.
function discountCard() {
  const card = h('div', { class: 'card stack' });
  invoke('get_settings', { keys: ['discount_presets', 'discount_staff_max_pct'] }).then((raw) => {
    const c = uiSettings(raw);
    const presets = h('input', { value: c.discountPresets.join(', '), placeholder: 'e.g. 5, 10, 15, 20', style: 'width:200px' });
    const limit = numBox(c.discountLimit == null ? '' : String(c.discountLimit), { inputmode: 'numeric', placeholder: 'none' });
    setKids(card,
      h('h3', {}, 'Discounts'),
      h('div', { class: 'muted small' }, 'Tap ✎ on an order line for these buttons. Staff can also type an amount or a percentage, e.g. 10%.'),
      h('label', { class: 'field' }, 'Buttons', presets, '%'),
      h('label', { class: 'field' }, 'Staff can give up to', limit, '% without a manager PIN'),
      h('div', { class: 'muted small' }, 'Empty = no limit. Managers and admins can always give more.'),
      h('div', {}, h('button', { class: 'primary', onclick: act(async () => {
        const list = presets.value.split(/[,;\s]+/).filter(Boolean).map((x) => Number(x.replace('%', '')));
        if (list.length > 8 || list.some((n) => !Number.isInteger(n) || n < 1 || n > 100)) return toast('Buttons: up to 8 whole percentages from 1 to 100, e.g. 5, 10, 15', true);
        const max = limit.value.trim();
        if (max !== '' && !(Number.isInteger(Number(max)) && Number(max) >= 0 && Number(max) <= 100)) return toast('Limit: a whole percentage from 0 to 100, or empty', true);
        await call('set_setting', { key: 'discount_presets', value: list.join(',') });
        await call('set_setting', { key: 'discount_staff_max_pct', value: max });
        toast((list.length ? `Discount buttons ${list.join(' / ')} %` : 'No discount buttons') + (max !== '' ? ` · over ${max}% needs a manager` : ''));
      }) }, 'Save discounts')));
  }).catch((e) => setKids(card, h('div', { class: 'muted' }, String(e?.message ?? e))));
  return card;
}

/// Settings card: bigger menu tiles and order buttons, on every device.
function buttonSizeCard() {
  const seg = h('div', { class: 'seg' });
  const draw = (big) => setKids(seg, [[false, 'Normal'], [true, 'Large']].map(([v, label]) => h('button', {
    class: v === big ? 'on' : '',
    onclick: act(async () => {
      await call('set_setting', { key: 'button_size', value: v ? 'large' : 'normal' });
      document.body.classList.toggle('big-buttons', v);
      draw(v);
      toast(`Menu buttons: ${label.toLowerCase()}`);
    }),
  }, label)));
  draw(!!state.ui?.bigButtons);
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Menu button size'),
    seg,
    h('div', { class: 'muted small' }, 'Large: bigger menu tiles, categories and order buttons, easier to hit. Fewer tiles fit on the screen. Applies to every device.'));
}

/// Settings card: log out after a few minutes without activity.
function autoLogoutCard() {
  const sel = h('select', {},
    [[0, 'Never'], [1, '1 minute'], [2, '2 minutes'], [5, '5 minutes'], [10, '10 minutes'], [15, '15 minutes'], [30, '30 minutes'], [60, '1 hour']]
      .map(([v, label]) => h('option', { value: v }, label)));
  const current = state.autoLogout ?? AUTO_LOGOUT_DEFAULT;
  if (![...sel.options].some((o) => Number(o.value) === current)) sel.append(h('option', { value: current }, `${current} minutes`));
  sel.value = String(current);
  sel.onchange = act(async () => {
    await call('set_setting', { key: AUTO_LOGOUT_KEY, value: sel.value });
    state.autoLogout = Number(sel.value);
    toast(state.autoLogout ? `Auto logout after ${sel.selectedOptions[0].textContent} without activity` : 'Auto logout switched off');
  });
  const warn = h('select', {}, [[0, 'No warning'], [10, '10 seconds'], [20, '20 seconds'], [30, '30 seconds'], [60, '1 minute']]
    .map(([v, label]) => h('option', { value: v }, label)));
  const warnNow = state.ui?.warnSecs ?? AUTO_LOGOUT_WARN_DEFAULT;
  if (![...warn.options].some((o) => Number(o.value) === warnNow)) warn.append(h('option', { value: warnNow }, `${warnNow} seconds`));
  warn.value = String(warnNow);
  warn.onchange = act(async () => {
    await call('set_setting', { key: 'auto_logout_warn_secs', value: warn.value });
    toast(Number(warn.value) ? `"Logging out in ${warn.value} s" shows before auto logout` : 'Auto logout without a warning');
  });
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Login and auto logout'),
    h('div', { class: 'muted' }, 'Log out by itself when nobody touches the screen or presses a key for this long, so the next person must enter their own PIN. ' +
      'Open orders are kept. Applies to this till and every device using the local server.'),
    h('label', { class: 'field' }, 'Log out after', sel),
    h('label', { class: 'field' }, 'Warn before', warn),
    h('div', { class: 'muted small' }, 'A big "Logging out in … s" covers the screen; any tap or key keeps you logged in.'),
    pinAutoLogin());
}

/// Log in without OK once the PIN is complete (setting pin_auto_login).
function pinAutoLogin() {
  const box = h('input', { type: 'checkbox', checked: true });
  invoke('get_setting', { key: 'pin_auto_login' }).then((v) => (box.checked = v !== '0')).catch(() => {});
  box.onchange = act(async () => {
    await call('set_setting', { key: 'pin_auto_login', value: box.checked ? '1' : '0' });
    toast(box.checked ? 'Login as soon as the PIN is typed' : 'Press OK to log in');
  });
  return h('div', { class: 'stack' },
    h('label', { class: 'switch' }, box, h('span', {}, ''), 'Log in as soon as the PIN is typed (no OK)'),
    h('div', { class: 'muted small' }, 'Works once every staff member has logged in once after this update (the till learns how long each PIN is, never the PIN). ' +
      'If PINs have different lengths, the shorter ones still press OK.'));
}

/// Settings card for the service charge.
function serviceCard() {
  const card = h('div', { class: 'card stack' });
  call('get_service_settings').then((cfg) => {
    const rate = h('input', { inputmode: 'decimal', value: cfg.rate_bp ? pct(cfg.rate_bp) : '', placeholder: '0', style: 'width:90px' });
    const label = h('input', { value: cfg.label, style: 'width:200px' });
    const types = Object.entries(TYPE).map(([k, name]) => ({ k, box: h('input', { type: 'checkbox', checked: cfg.order_types.includes(k) }), name }));
    setKids(card,
      h('h3', {}, 'Service charge'),
      h('div', { class: 'muted' }, 'Added to the bill as a percentage of the food (after discounts, no tax on it). Leave the rate empty or 0 for no service charge. ' +
        'Staff can change or remove it on any order by tapping it under the totals.'),
      h('div', { class: 'row' }, h('label', { class: 'field' }, 'Rate', rate, '%'), h('label', { class: 'field' }, 'Name on the bill', label)),
      h('div', { class: 'row' }, h('span', { class: 'muted' }, 'Add it to new:'),
        types.map((t) => h('label', { class: 'check' }, t.box, ` ${t.name}`))),
      h('div', {}, h('button', { class: 'primary', onclick: act(async () => {
        const v = Number((rate.value || '0').replace(',', '.'));
        if (!Number.isFinite(v) || v < 0 || v > 30) return toast('Enter a rate from 0 to 30 %', true);
        await call('set_service_settings', { settings: {
          rate_bp: Math.round(v * 100), label: label.value, order_types: types.filter((t) => t.box.checked).map((t) => t.k),
        } });
        state.menu = null; // reload on the next order screen
        toast(v ? `Service charge ${v}% saved (applies to new orders)` : 'Service charge switched off');
      }) }, 'Save service charge')));
  }).catch(() => {});
  return card;
}

/// Settings card for full screen (desktop window only).
/// Light, dark or the same as the device; this device only (the moon / sun button at the top does the same).
function themeCard() {
  const box = h('div');
  const draw = () => setKids(box, segmented([['light', 'Light'], ['dark', 'Dark'], ['auto', 'Same as this device']], themePref(), (v) => { setTheme(v); draw(); }));
  draw();
  return h('div', { class: 'card stack' }, h('h3', {}, 'Light or dark'), box,
    h('div', { class: 'muted small' }, 'Saved on this device only, so a till and a tablet can differ.'));
}

function displayCard() {
  const on = h('input', { type: 'checkbox' });
  invoke('get_setting', { key: 'start_fullscreen' }).then((v) => (on.checked = v === '1')).catch(() => {});
  on.onchange = act(async () => {
    await call('set_setting', { key: 'start_fullscreen', value: on.checked ? '1' : '0' });
    toast(on.checked ? 'The POS will open in full screen' : 'The POS will open in a window');
  });
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Display'),
    h('label', { class: 'switch' }, on, h('span', {}), 'Open in full screen when the POS starts'),
    h('div', { class: 'row' },
      h('button', { onclick: act(toggleFullscreen) }, icon('expand', 18), 'Full screen on / off'),
      h('span', { class: 'muted' }, 'Shortcut: F11, or the full-screen button at the top right.')));
}

/// Settings card for the local network server (desktop window only).
function serverCard() {
  const card = h('div', { class: 'card stack' });
  const draw = (st) => {
    const port = h('input', { type: 'number', min: 1024, max: 65535, value: st.settings.port, style: 'width:110px' });
    const auto = h('input', { type: 'checkbox', checked: st.settings.auto_start });
    const settings = () => ({ port: Number(port.value) || 8080, auto_start: auto.checked });
    const apply = (run) => act(async () => {
      const next = await call('set_server', { settings: settings(), run });
      draw(next);
      toast(run ? 'Local server running' : next.running ? 'Saved' : 'Local server stopped');
    });
    setKids(card,
      h('div', { class: 'row' }, h('h3', { class: 'grow' }, 'Local server'),
        h('span', { class: `status ${st.running ? 's-paid' : 's-cancelled'}` }, st.running ? 'running' : 'stopped')),
      h('div', { class: 'muted' }, 'Use this POS from other devices on the same network (tablets, laptops, phones) in a web browser. ' +
        'Each device logs in with its own PIN and shares the same orders, menu and printers. This PC must stay on with the POS open.'),
      st.running && h('div', { class: 'server-urls' }, h('div', { class: 'muted small' }, 'Open this address on the other device:'),
        st.urls.map((u) => h('code', {}, u))),
      st.error && !st.running && h('div', { class: 'badge warn' }, st.error),
      h('div', { class: 'row' }, h('label', { class: 'field' }, 'Port', port),
        h('label', { class: 'switch' }, auto, h('span', {}), 'Start automatically when the POS opens')),
      h('div', { class: 'row' },
        st.running
          ? [h('button', { class: 'primary', onclick: apply(true) }, 'Save & restart'), h('button', { class: 'danger', onclick: apply(false) }, 'Stop server')]
          : h('button', { class: 'primary', onclick: apply(true) }, 'Start server'),
        !st.running && h('button', { onclick: apply(false) }, 'Save')),
      h('div', { class: 'muted small' }, 'The first time, Windows may ask whether to allow the POS on the network: choose Allow on private networks.'));
  };
  call('server_status').then(draw).catch(() => {});
  return card;
}

const printResult = (r, ok) => toast(r.error ? `Did not print: ${r.error}` : ok, !!r.error);

async function renderSettings() {
  const tab = settingsGroups().find(([id]) => id === state.settingsTab);
  if (!tab) return renderManageHome();
  const store = h('input', { value: state.storeName ?? '', placeholder: 'Restaurant name' });
  // Printers are admin settings (managers only see the kitchen cards).
  const [kitchenCfg, receiptCfg, systemPrinters] = isAdmin() ? await Promise.all([
    call('get_kitchen_printer'), call('get_receipt_settings'), call('list_system_printers'),
  ]) : [{}, {}, []];

  const kitchen = isAdmin() && printerForm(kitchenCfg, systemPrinters, 'No kitchen printer',
    [['both', 'English with Thai under it'], ['english', 'English only'], ['thai', 'Thai only']], true);

  const receipt = isAdmin() && printerForm(receiptCfg, systemPrinters, 'No receipt printer',
    [['english', 'English only'], ['both', 'English with Thai under it']]);
  const autoPrint = h('input', { type: 'checkbox', checked: receiptCfg.auto_print });
  const drawer = h('input', { type: 'checkbox', checked: receiptCfg.drawer_on_cash });
  const shopName = h('input', { value: receiptCfg.shop_name ?? '', placeholder: `${state.storeName || 'Restaurant name'} (the restaurant name above)`, style: 'flex:1' });
  const staffCopy = h('input', { type: 'checkbox', checked: receiptCfg.staff_copy !== false });
  const taxBreakdown = h('input', { type: 'checkbox', checked: receiptCfg.tax_breakdown === true });
  const showStaff = h('input', { type: 'checkbox', checked: receiptCfg.show_staff !== false });
  const header = h('textarea', { rows: 3, style: 'width:100%', placeholder: 'Address, phone, VAT number… (one per line)' });
  header.value = receiptCfg.header ?? '';
  const footer = h('input', { value: receiptCfg.footer ?? '', placeholder: 'e.g. Thank you for your visit!', style: 'flex:1' });
  const currency = h('input', { value: receiptCfg.currency ?? '£', style: 'width:60px' });
  const receiptSettings = () => ({
    ...receipt.config(), auto_print: autoPrint.checked, drawer_on_cash: drawer.checked, staff_copy: staffCopy.checked,
    tax_breakdown: taxBreakdown.checked, show_staff: showStaff.checked,
    shop_name: shopName.value.trim() || null,
    header: header.value.trim() || null, footer: footer.value.trim() || null, currency: currency.value,
  });

  const restaurantCard = () => h('div', { class: 'card stack' },
    h('h3', {}, 'Restaurant'),
    h('div', { class: 'row' }, store, h('button', { class: 'primary', onclick: act(async () => {
      await call('set_setting', { key: 'store_name', value: store.value });
      toast('Saved');
      await start();
    }) }, 'Save')));
  const kitchenPrinterCard = () => h('div', { class: 'card stack' },
    h('h3', {}, 'Kitchen printer'),
    h('div', { class: 'muted' }, 'Tickets print on "Send to kitchen" on any ESC/POS thermal printer (Epson, Star, Xprinter…). ' +
      'For USB printers choose "installed in Windows"; for network printers use the IP address from its self-test page.'),
    kitchen.el,
    h('div', { class: 'row' },
      h('button', { onclick: act(async () => {
        if (kitchen.none()) return toast('Choose a printer first', true);
        printResult(await call('test_kitchen_printer', { config: kitchen.config() }), 'Test ticket sent');
      }) }, 'Print test ticket'),
      thaiTestButton(kitchen),
      h('button', { class: 'primary', onclick: act(async () => {
        await call('set_kitchen_printer', { config: kitchen.config() });
        toast('Kitchen printer saved');
      }) }, 'Save kitchen printer')));
  const receiptCard = () => h('div', { class: 'card stack' },
    h('h3', {}, 'Receipt printer & cash drawer'),
    h('div', { class: 'muted' }, 'Customer receipts and bills. The cash drawer plugs into this printer. ' +
      'It can be the same printer as the kitchen one.'),
    receipt.el,
    h('label', { class: 'check' }, autoPrint, ' Print a receipt automatically after payment'),
    h('label', { class: 'check' }, drawer, ' Open the cash drawer for cash payments'),
    h('label', { class: 'check' }, staffCopy, ' Also print a staff copy after payment, with the tax by class (net, VAT, gross)'),
    h('label', { class: 'check' }, taxBreakdown, ' Show the VAT / tax breakdown on customer bills and receipts'),
    h('label', { class: 'check' }, showStaff, ' Show "Served by" and the staff name on customer bills and receipts'),
    h('div', { class: 'row' }, 'Currency symbol', currency),
    h('label', { class: 'field' }, 'Name at the top of the bill', h('div', { class: 'row' }, shopName)),
    h('label', { class: 'field' }, 'Address, phone, VAT number (under the name, one per line)', header),
    h('label', { class: 'field' }, 'Message at the bottom', h('div', { class: 'row' }, footer)),
    h('div', { class: 'row' },
      h('button', { onclick: act(async () => {
        if (receipt.none()) return toast('Choose a printer first', true);
        printResult(await call('test_receipt_printer', { settings: receiptSettings(), drawerOnly: false }), 'Test receipt sent');
      }) }, 'Print test receipt'),
      thaiTestButton(receipt),
      h('button', { onclick: act(async () => {
        if (receipt.none()) return toast('Choose a printer first', true);
        printResult(await call('test_receipt_printer', { settings: receiptSettings(), drawerOnly: true }), 'Drawer opened');
      }) }, 'Test cash drawer'),
      h('button', { class: 'primary', onclick: act(async () => {
        await call('set_receipt_settings', { settings: receiptSettings() });
        toast('Receipt settings saved');
      }) }, 'Save receipt settings')));
  // Product name, version and credits: shown at the top of About & updates.
  const creditsCard = () => h('div', { class: 'card about-credits' },
    h('div', { class: 'about-head' },
      h('h3', { class: 'about-name' }, 'Limehouse POS'),
      state.version && h('span', { class: 'chip info' }, `Version ${state.version}`)),
    h('div', { class: 'about-by' }, 'Developed by ', h('strong', {}, 'Pheeraphat Bunliang')),
    h('div', { class: 'muted small' }, 'Designed and developed with AI-assisted programming.'),
    h('div', { class: 'about-copy muted small' }, '\u00a9 2026 Pheeraphat Bunliang. All rights reserved.'));
  const aboutCard = () => h('div', { class: 'card stack' },
    h('h3', {}, 'Updates'),
    state.remote
      ? h('div', { class: 'muted' }, 'This browser uses the POS running on the main till. Updates are installed there.')
      : [h('div', { class: 'muted' }, 'The app checks for updates every few hours while an admin is logged in on this till. Installing one takes about a minute: ' +
        'the database is backed up, the app closes, updates and reopens. Orders, menu and settings are kept.'),
      h('div', { class: 'row' }, h('button', { onclick: act(async () => {
        const info = await checkForUpdate(false);
        if (info && !info.available) toast(`You have the latest version (${info.current})`);
      }) }, 'Check for updates'))]);
  const backupCard = () => {
    const list = h('div', { class: 'backup-list' });
    const admin = isAdmin();
    let files = [];
    const load = async () => {
      files = await call('list_full_backups').catch(() => []);
      setKids(list, files.length ? files.map((f) => h('div', { class: 'backup-row' },
        h('div', { class: 'grow' }, h('div', {}, fmtTime(f.modified)),
          h('div', { class: 'muted small' }, `${backupKind(f.name)} · ${f.name} · ${fileSize(f.size)}`)),
        admin && h('div', { class: 'row backup-acts' },
          h('button', { onclick: act(async () => restoreDialog(f, await call('inspect_backup', { name: f.name }))) }, 'Restore'),
          state.remote && h('button', { onclick: act(() => downloadBackup(f.name)) }, 'Download'),
          confirmBtn('Delete', 'Tap again to delete', async () => {
            await call('delete_backups', { names: [f.name] }); toast('Backup deleted'); await load();
          }, 'danger-outline'))))
        : h('div', { class: 'muted' }, 'No backups yet.'));
    };
    load();
    // How many full backups to keep (setting backups_kept; not set = 14).
    const [KEEP_MIN, KEEP_MAX, KEEP_DEF] = [3, 90, 14];
    const keep = numBox('', { inputmode: 'numeric', placeholder: String(KEEP_DEF), 'aria-label': 'Backups kept' });
    const keepText = h('span', {}, `the last ${KEEP_DEF} are kept`);
    invoke('get_setting', { key: 'backups_kept' }).then((v) => {
      const n = Number(v);
      if (v != null && Number.isInteger(n) && n >= KEEP_MIN && n <= KEEP_MAX) { keep.value = String(n); keepText.textContent = `the last ${n} are kept`; }
    }).catch(() => {});
    // Where the files are, to copy one to a USB stick (moving to a new PC).
    const folder = h('div', { class: 'muted small backup-folder' });
    if (admin) invoke('backup_folder').then((dir) => setKids(folder, state.remote ? 'Backup folder on the main till: ' : 'Backup folder: ', h('code', {}, dir))).catch(() => {});
    const status = h('div', { class: 'muted', hidden: true });
    const fileInput = h('input', { type: 'file', accept: '.db', hidden: true });
    const fromFile = h('button', { onclick: () => fileInput.click() }, 'Restore everything from a file…');
    fileInput.onchange = act(async () => {
      const f = fileInput.files[0];
      fileInput.value = '';
      if (!f) return;
      fromFile.disabled = true;
      status.hidden = false;
      try {
        const r = await uploadBackup(f, (pct) => { status.textContent = `Loading ${f.name}… ${pct}%`; });
        status.textContent = '';
        await load();
        restoreDialog(r.file, r.summary);
      } finally {
        fromFile.disabled = false;
        status.hidden = true;
      }
    });
    return h('div', { class: 'card stack' },
      h('h3', {}, 'Backup'),
      h('p', { class: 'muted' }, 'Everything (orders, menu, customers, staff and PINs, settings) is backed up automatically once a day at startup; ', keepText,
        '. The copies made before an update, a reset or a restore, and files loaded from elsewhere, are kept apart (the last 3 of each, and any from the last 7 days).'),
      admin && h('div', { class: 'row' }, h('label', { class: 'field' }, 'Keep the last', keep, 'backups'),
        h('button', { onclick: act(async () => {
          const raw = keep.value.trim(), n = Number(raw);
          if (raw && (!Number.isInteger(n) || n < KEEP_MIN || n > KEEP_MAX)) return toast(`Backups kept: a whole number from ${KEEP_MIN} to ${KEEP_MAX}`, true);
          await call('set_setting', { key: 'backups_kept', value: raw });
          keepText.textContent = `the last ${raw || KEEP_DEF} are kept`;
          toast(`Keeping the last ${raw || KEEP_DEF} backups. Older ones are deleted at the next backup.`);
        }) }, 'Save')),
      admin && h('div', { class: 'muted small' }, `${KEEP_MIN} to ${KEEP_MAX} (empty = ${KEEP_DEF}). Daily backups only: safety copies and files loaded from elsewhere are kept apart. Each backup is a full copy of the database.`),
      h('div', { class: 'row' }, h('button', { class: 'primary', onclick: act(async () => { toast('Backup saved: ' + (await call('backup_now'))); await load(); }) }, 'Back up everything now'),
        admin && fromFile, fileInput,
        admin && !state.remote && h('button', { onclick: act(() => invoke('open_backup_folder')) }, 'Open backup folder'),
        h('span', { class: 'grow' }),
        admin && h('button', { class: 'danger-outline', onclick: () => bulkDelete('Delete backups', files.map((f) => ({ id: f.name, label: fmtTime(f.modified), sub: `${f.name} · ${fileSize(f.size)}` })),
          async (names) => `${await call('delete_backups', { names })} backup(s) deleted`, load) }, 'Delete several…')),
      status,
      admin && h('div', { class: 'muted small' }, h('strong', {}, 'Moving to a new PC? '),
        state.remote ? 'Download a backup here, ' : 'Back up, open the backup folder and copy the newest backup to a USB stick, ',
        'then on the new PC open Backup & data and use Restore everything from a file…'),
      admin && folder,
      list);
  };

  const cards = {
    shop: () => [restaurantCard(), serviceCard(), menuDefaultsCard()],
    bills: () => [receiptCard(), paymentsCard(), ticketLabelsCard()],
    kitchen: () => [isAdmin() && kitchenPrinterCard(), isAdmin() && routesCard(systemPrinters), kdsCard()],
    tickets: () => [splitCard()],
    orders: () => [isAdmin() && orderTypesCard(), isAdmin() && discountCard(), courseCard(), cancelTicketCard(), isAdmin() && fixSentCard(), quickNotesCard()],
    teams: () => [teamsCard(), teamChoiceCard()],
    login: () => [startScreenCard(), autoLogoutCard()],
    delivery: () => [platformsCard()],
    // Full screen and the local server are this PC's own (not on a phone or tablet).
    screen: () => [themeCard(), buttonSizeCard(), !state.remote && displayCard(), !state.remote && serverCard()],
    data: () => [backupCard(), menuBackupCard(), resetCard()],
    // Checking for and installing updates is for admins only.
    about: () => [creditsCard(), isAdmin() && aboutCard()],
  };
  const group = { id: tab[0], icon: tab[1], title: tab[2], cards: cards[tab[0]] };
  drawTitle(group.title, 'Manage');
  const back = () => { state.settingsTab = null; renderSettings().catch((e) => toast(String(e?.message ?? e), true)); };
  screen(h('div', { class: 'page settings' },
    h('div', { class: 'settings-head' },
      h('button', { class: 'back-btn', onclick: back }, icon('back', 22), 'Manage'),
      h('h2', {}, icon(group.icon, 26), group.title)),
    group.cards()));
}

/// The settings areas this user may open: all for admins, the kitchen ones
/// and About for managers, only About (credits and version) for staff.
function settingsGroups() {
  const kitchen = ['kitchen', 'tickets', 'orders', 'teams'];
  return SETTINGS_GROUPS.filter(([id]) => isAdmin() || id === 'about' || (isManager() && kitchen.includes(id)));
}

/// Manage: big tiles for the other screens (History, Menu, Staff…) and,
/// for admins, every settings area.
function renderManageHome() {
  state.settingsTab = null;
  drawTitle('Manage');
  const tiles = [];
  const tile = (id, ic, title, sub, words, onclick) => {
    const el = h('button', { class: 'settings-tile', 'data-group': id, onclick }, icon(ic, 32), h('strong', {}, title), sub && h('small', {}, sub));
    el.dataset.find = `${title} ${sub ?? ''} ${words ?? ''}`.toLowerCase();
    tiles.push(el);
    return el;
  };
  const pages = VIEWS.filter(([id, , need]) => !['floor', 'orders', 'settings'].includes(id) && canSee(need));
  const groups = settingsGroups();
  const open = (id) => () => { state.settingsTab = id; renderSettings().catch((e) => toast(String(e?.message ?? e), true)); };
  const daily = h('div', { class: 'settings-tiles' }, pages.map(([id, label]) => tile(id, id, label, MANAGE_SUB[id], PAGE_WORDS[id], () => go(id))));
  const setTiles = groups.length > 0 && h('div', { class: 'settings-tiles' }, groups.map(([id, ic, title, sub]) =>
    tile(id, ic, title, id === 'about' ? `Version ${state.version}` : sub, SETTING_WORDS[id], open(id))));
  const dailyHead = h('h3', { class: 'manage-title' }, 'Daily work');
  const setHead = groups.length > 0 && h('h3', { class: 'manage-title' }, 'Settings');
  const none = h('div', { class: 'empty small', hidden: true }, icon('search', 36), h('p', {}, 'Nothing matches. Try another word, like "printer", "tax" or "PIN".'));
  // Every word typed must appear in the tile's name, its line or its search words.
  const find = h('input', { type: 'search', placeholder: 'What are you looking for? e.g. printer, tax, PIN, backup', 'aria-label': 'Find a page or setting' });
  const filter = () => {
    const words = find.value.toLowerCase().split(/\s+/).filter(Boolean);
    for (const t of tiles) t.hidden = !words.every((w) => t.dataset.find.includes(w));
    dailyHead.hidden = [...daily.children].every((t) => t.hidden);
    if (setTiles) setHead.hidden = [...setTiles.children].every((t) => t.hidden);
    none.hidden = tiles.some((t) => !t.hidden);
  };
  find.addEventListener('input', filter);
  // Enter with a single match opens it.
  find.addEventListener('keydown', (e) => { if (e.key === 'Enter') { const shown = tiles.filter((t) => !t.hidden); if (shown.length === 1) shown[0].click(); } });
  screen(h('div', { class: 'page settings-home' },
    h('div', { class: 'searchbox manage-find' }, icon('search', 22), find),
    dailyHead, daily, setHead, setTiles, none));
  return Promise.resolve();
}
/// Extra words each Manage tile is found by (what people call things).
const PAGE_WORDS = {
  history: 'paid orders reprint receipt void refund past old bills delete',
  customers: 'customer phone address postcode delivery saved',
  reports: 'sales money totals end of day close day summary takings',
  shift: 'shift float cash count drawer money in out till',
  kitchen: 'kitchen screen tablet kds cook chef done display',
  menuAdmin: 'menu items dishes food drinks categories prices sets options sizes extras sold out thai names paste',
  plan: 'floor plan tables layout rooms terrace seats',
  users: 'staff users people pin password roles manager admin waiter',
};
const SETTING_WORDS = {
  shop: 'restaurant name service charge tax vat rate prices included',
  bills: 'receipt printer bill cash drawer footer header address tips payments split card cash names',
  kitchen: 'kitchen printer tickets copies font size large bold thai kitchen screen starters mains',
  tickets: 'drinks desserts bar tickets own printer',
  orders: 'order types takeaway delivery collection discounts cancel cancelling quick notes courses starter',
  teams: 'kitchen teams station curry rice grill',
  login: 'login pin auto logout lock start screen staff timeout',
  delivery: 'uber eats just eat deliveroo platform delivery apps',
  screen: 'button size big full screen display phones tablets wifi local server network theme dark light mode',
  data: 'backup restore reset delete data menu copy export move new pc',
  about: 'version update updates credits licence',
};

/// Settings card: more kitchen printers (bar, desserts...) and what each prints.
function routesCard(systemPrinters) {
  const card = h('div', { class: 'card stack' });
  Promise.all([call('get_kitchen_routes'), call('list_teams').catch(() => [])]).then(([routes, teams]) => {
    let rows = routes.map((r) => ({ ...r }));
    const list = h('div', { class: 'stack' });
    const forms = [];
    const draw = () => {
      forms.length = 0;
      setKids(list, rows.map((r, i) => {
        const label = h('input', { value: r.label ?? '', placeholder: 'Name, e.g. Bar', style: 'width:160px' });
        const pf = printerForm({ ...r, kind: r.kind ?? 'network' }, systemPrinters, 'Choose…', [['both', 'English with Thai under it']], false, true);
        const drinks = h('input', { type: 'checkbox', checked: !!r.drinks });
        const desserts = h('input', { type: 'checkbox', checked: !!r.desserts });
        const teamBoxes = teams.map((t) => ({ t, box: h('input', { type: 'checkbox', checked: (r.team_ids ?? []).includes(t.id) }) }));
        const value = () => {
          const c = pf.config();
          return { label: label.value.trim(), kind: c.kind, host: c.host, port: c.port, name: c.name, path: c.path, width: c.width, copies: c.copies, image_mode: c.image_mode, slow_send: c.slow_send,
            drinks: drinks.checked, desserts: desserts.checked, team_ids: teamBoxes.filter((x) => x.box.checked).map((x) => x.t.id) };
        };
        forms.push(value);
        return h('div', { class: 'card stack route-card' },
          h('div', { class: 'row' }, label, h('span', { style: 'flex:1' }),
            h('button', { onclick: act(async () => printResult(await call('test_route_printer', { route: value() }), 'Test ticket sent')) }, 'Print test'),
            thaiTestButton(pf),
            h('button', { class: 'danger', onclick: () => { rows = forms.map((f) => f()); rows.splice(i, 1); draw(); } }, 'Remove')),
          pf.el,
          h('div', { class: 'row' }, h('strong', {}, 'Prints:'),
            h('label', { class: 'check' }, drinks, ' Drinks'), h('label', { class: 'check' }, desserts, ' Desserts'),
            teamBoxes.map((x) => h('label', { class: 'check' }, x.box, ` ${x.t.name}`))));
      }), !rows.length && h('div', { class: 'muted' }, 'No other printers: every kitchen ticket prints on the kitchen printer above.'));
    };
    draw();
    setKids(card, h('h3', {}, 'Other kitchen printers'),
      h('div', { class: 'muted' }, 'For example a printer at the bar for drinks, or one at the dessert station. Tick what each one prints; everything else prints on the kitchen printer above. ' +
        'Names, font and large text follow the kitchen printer.'),
      list,
      h('div', { class: 'row' },
        h('button', { onclick: () => { rows = forms.map((f) => f()); rows.push({ label: '', kind: 'network', port: 9100, width: 42, copies: 1, team_ids: [] }); draw(); } }, '+ Printer'),
        h('button', { class: 'primary', onclick: act(async () => {
          const routes = forms.map((f) => f());
          await call('set_kitchen_routes', { routes });
          rows = routes;
          toast(routes.length ? `${routes.length} other kitchen printer(s) saved` : 'All tickets print on the kitchen printer');
        }) }, 'Save printers')));
  }).catch((e) => setKids(card, h('div', { class: 'muted' }, String(e?.message ?? e))));
  return card;
}

/// A small number box for the settings cards.
const numBox = (value, attrs = {}) => h('input', { inputmode: 'decimal', value, style: 'width:70px', ...attrs });

/// Settings card: tips, cash quick buttons, split evenly, how Finish & Pay opens (admins).
function paymentsCard() {
  const card = h('div', { class: 'card stack' });
  invoke('get_settings', { keys: PAY_KEYS }).then((raw) => {
    const c = paySettings(raw);
    const tips = c.tips.map((p) => numBox(String(p), { 'aria-label': 'Tip %' }));
    const notes = h('input', { value: c.notes.map((x) => String(x / 100)).join(', '), placeholder: '5, 10, 20, 50, 100', style: 'width:240px' });
    const split = numBox(String(c.splitMax), { inputmode: 'numeric' });
    const method = h('select', {}, h('option', { value: 'cash' }, payName('cash')), h('option', { value: 'card' }, payName('card')));
    method.value = c.method;
    const nameBox = (v, def) => h('input', { value: v ?? '', placeholder: def, maxlength: PAY_NAME_MAX, style: 'width:200px' });
    const cashName = nameBox(raw.pay_name_cash, 'Cash'), cardName = nameBox(raw.pay_name_card, 'Card');
    setKids(card,
      h('h3', {}, 'Payments'),
      h('div', { class: 'muted' }, 'How Finish & Pay looks. Switches save at once; the rest with the Save button.'),
      settingSwitch('tips_on', 'Ask for tips (a Tip row in Finish & Pay)', 'Finish & Pay asks for tips', 'No Tip row in Finish & Pay'),
      h('div', { class: 'row' }, h('span', {}, 'Tip buttons'),
        tips.map((t) => h('label', { class: 'field' }, t, '%'))),
      h('label', { class: 'field' }, 'Cash quick buttons', notes),
      h('div', { class: 'muted small' }, 'Note values, separated by commas. The total rounded up is offered too; the four smallest amounts above the total show.'),
      h('label', { class: 'field' }, 'Split evenly: up to', split, 'people (2 to 12)'),
      h('label', { class: 'field' }, 'Payment method when Finish & Pay opens', method),
      h('div', { class: 'row' }, h('label', { class: 'field' }, 'Call Cash', cashName), h('label', { class: 'field' }, 'Call Card', cardName)),
      h('div', { class: 'muted small' }, 'How the payment buttons, receipts, History, reports and the shift report name them, e.g. เงินสด or QR / Card. ' +
        'Empty = Cash and Card. Reports still count them the same way.'),
      settingSwitch('exact_cash_pays', 'Tapping "Exact" takes the cash payment at once (no second tap)',
        '"Exact" takes the payment at once', '"Exact" fills in the amount, then tap Take cash', false),
      h('div', {}, h('button', { class: 'primary', onclick: act(async () => {
        const tipList = numberList(tips.map((t) => t.value.replace(',', '.')).join(','));
        if (!goodTips(tipList)) return toast('Enter three tip percentages, e.g. 5, 10 and 15 (more than 0, at most 100)', true);
        const noteList = numberList(notes.value);
        if (!goodNotes(noteList)) return toast('Enter 1 to 8 cash amounts separated by commas, e.g. 5, 10, 20, 50, 100', true);
        const n = Number(split.value);
        if (!goodSplit(n)) return toast('Split evenly: enter a number of people from 2 to 12', true);
        const names = [cashName.value.trim(), cardName.value.trim()];
        if (names.some((x) => x.length > PAY_NAME_MAX)) return toast(`Payment names: up to ${PAY_NAME_MAX} characters`, true);
        if ((names[0] || 'Cash').toLowerCase() === (names[1] || 'Card').toLowerCase()) return toast('Give Cash and Card different names', true);
        const values = { tip_presets: tipList.join(','), cash_quick: noteList.join(','), split_max: String(n), pay_default_method: method.value,
          pay_name_cash: names[0], pay_name_card: names[1] };
        for (const [key, value] of Object.entries(values)) await call('set_setting', { key, value });
        await loadUiSettings();
        toast(`Payments saved: tips ${tipList.join(' / ')} %, split up to ${n}, opens on ${payName(method.value)}`);
      }) }, 'Save payments')));
  }).catch((e) => setKids(card, h('div', { class: 'muted' }, String(e?.message ?? e))));
  return card;
}

/// Settings card: kitchen screen colours, refresh, how long paid orders stay (managers too).
function kdsCard() {
  const card = h('div', { class: 'card stack' });
  invoke('get_settings', { keys: KDS_KEYS }).then((raw) => {
    const c = kdsSettings(raw);
    const box = (v) => numBox(String(v), { inputmode: 'numeric' });
    const slow = box(c.slow), late = box(c.late), refresh = box(c.refresh), paid = box(c.paid);
    const dot = (cls) => h('span', { class: `kds-dot ${cls}` });
    setKids(card,
      h('h3', {}, 'Kitchen screen'),
      h('div', { class: 'muted' }, 'The Kitchen screen (Manage → Kitchen screen) on a tablet: when a card changes colour, how often it refreshes, and how long a paid order with dishes still to cook stays on it.'),
      h('div', { class: 'row' },
        h('label', { class: 'field' }, dot('slow'), 'Orange after', slow, 'min'),
        h('label', { class: 'field' }, dot('late'), 'Red after', late, 'min')),
      h('label', { class: 'field' }, 'Refresh every', refresh, 'seconds (2 to 30)'),
      h('label', { class: 'field' }, 'Keep paid orders for', paid, 'hours (0 to 24)'),
      h('div', { class: 'muted small' }, '0 hours: a paid order leaves the screen at once, even if dishes are still to cook.'),
      settingSwitch('kds_beep', 'Beep when a new ticket arrives', 'The kitchen screen beeps for new tickets', 'The kitchen screen is silent', false),
      h('div', { class: 'muted small' }, 'A short beep on the kitchen screen\'s device. Browsers play sound only after a tap: if the screen shows "Tap to turn on the sound", tap it once.'),
      h('div', {}, h('button', { class: 'primary', onclick: act(async () => {
        const v = { slow: Number(slow.value), late: Number(late.value), refresh: Number(refresh.value), paid: Number(paid.value) };
        const ok = (n, [lo, hi]) => Number.isInteger(n) && n >= lo && n <= hi;
        if (!ok(v.slow, KDS_LIMITS.slow) || !ok(v.late, KDS_LIMITS.late)) return toast('Orange and red: whole minutes from 1 to 240', true);
        if (v.late <= v.slow) return toast('Red must come after orange (more minutes)', true);
        if (!ok(v.refresh, KDS_LIMITS.refresh)) return toast('Refresh: whole seconds from 2 to 30', true);
        if (paid.value.trim() === '' || !ok(v.paid, KDS_LIMITS.paid)) return toast('Paid orders: whole hours from 0 to 24', true);
        const values = { kds_slow_mins: v.slow, kds_late_mins: v.late, kds_refresh_secs: v.refresh, kds_paid_hours: v.paid };
        for (const [key, value] of Object.entries(values)) await call('set_setting', { key, value: String(value) });
        toast(`Kitchen screen saved: orange after ${v.slow} min, red after ${v.late} min, refresh every ${v.refresh} s`);
      }) }, 'Save kitchen screen')));
  }).catch((e) => setKids(card, h('div', { class: 'muted' }, String(e?.message ?? e))));
  return card;
}

/// Settings card: whether drinks and desserts get their own tickets.
function splitCard() {
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Own tickets for drinks and desserts'),
    settingSwitch('drinks_split', 'Print drinks on their own DRINKS ticket', 'Drinks on their own ticket', 'Drinks print with the food'),
    settingSwitch('desserts_split', 'Print desserts on their own DESSERTS ticket', 'Desserts on their own ticket', 'Desserts print with the food'),
    h('div', { class: 'muted' }, 'Which items are drinks or desserts: Menu → tick "Drink" or "Dessert" on the item, or tap a category and tick "Drinks" / "Desserts" for all of it. ' +
      'Off = they print on the normal ticket with the rest. Drinks still go to the kitchen straight away.'));
}

/// Settings card: ask "Starter or Main?" when a starter is tapped.
function courseCard() {
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Starter or main course'),
    settingSwitch('starter_choice', 'When a starter is tapped on a dine-in order, ask "Starter or Main course?"',
      'Starters ask Starter or Main', 'Starters are added as starters'),
    h('div', { class: 'muted' }, 'Main course: the dish waits and goes with the mains. A dish not yet sent can also be changed later: tap ✎ on it, then Starter or Main course.'),
    h('div', { class: 'muted small' }, 'When teams are ticked under Settings → Kitchen teams → "Which team makes this?", that question is asked instead of this one.'));
}

/// Reloads the "Which team makes this?" card on screen (after the teams are saved).
let reloadTeamChoice = null;
/// Settings card: "Which team makes this?" — the teams a dish can be moved
/// to when it is ordered (setting team_choice: team ids, empty = off).
function teamChoiceCard() {
  const card = h('div', { class: 'card stack' });
  // The saved list itself (team_choice reads empty while kitchen teams are switched off).
  const load = () => Promise.all([call('list_teams'), call('get_setting', { key: 'team_choice' })]).then(([teams, raw]) => {
    const chosen = (raw ?? '').split(',').map((x) => parseInt(x, 10)).filter((x) => !Number.isNaN(x));
    const boxes = teams.map((t) => ({ t, box: h('input', { type: 'checkbox', checked: chosen.includes(t.id) }) }));
    setKids(card,
      h('h3', {}, 'Which team makes this?'),
      h('div', { class: 'muted' }, 'Tick the teams staff can choose between. On a dine-in order, tapping a dish of one of these teams asks "Which team makes this?": ' +
        'the dish then prints under that team (and on its printer) for this order only. Dishes of other teams are added straight away. ' +
        'This replaces "Starter or Main course?"; a dish stays a starter or a main as the menu says. Only while kitchen teams are on.'),
      teams.length ? h('div', { class: 'row' }, boxes.map((x) => h('label', { class: 'check' }, x.box, ` ${x.t.name}`)))
        : h('div', { class: 'muted' }, 'Add teams above first.'),
      h('div', { class: 'muted small' }, 'Tick two or more, or none to switch it off. A dish not yet sent can also be moved later: tap ✎ on it, then Team.'),
      h('div', {}, h('button', { class: 'primary', onclick: act(async () => {
        const ids = boxes.filter((x) => x.box.checked).map((x) => x.t.id);
        if (ids.length === 1) return toast('Tick at least two teams, or none to switch it off', true);
        await call('set_setting', { key: 'team_choice', value: ids.join(',') });
        toast(ids.length ? `Staff choose between ${boxes.filter((x) => x.box.checked).map((x) => x.t.name).join(', ')}` : 'Team choice off');
      }) }, 'Save')));
  }).catch((e) => toast(String(e?.message ?? e), true));
  load();
  reloadTeamChoice = () => { if (card.isConnected) load(); };
  return card;
}

/// Settings card: whether cancelling a whole order prints a CANCELLED kitchen ticket.
function cancelTicketCard() {
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Cancelling an order'),
    settingSwitch('print_cancelled_order', 'Print a CANCELLED ticket for the kitchen when an order is cancelled',
      'Cancelling an order prints a CANCELLED ticket', 'Cancelling an order prints nothing for the kitchen'),
    h('div', { class: 'muted' }, 'On: the ticket lists the dishes still cooking (not yet marked done on the kitchen screen). Off: the order is cancelled the same way, but nothing is printed, so tell the kitchen yourself. ' +
      'Taking one dish off an order (− or Remove) still prints a CANCELLED ticket for that dish.'));
}

/// Settings card: who may fix a dish that was already sent.
function fixSentCard() {
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Fixing a dish already sent'),
    settingSwitch('staff_cancel_sent', 'Staff can take off or reduce dishes already sent', 'Staff can fix sent dishes', 'Only an admin can take off sent dishes'),
    h('div', { class: 'muted' }, 'Wrong dish sent? On the order tap ✎ on it: − or Remove prints a CANCELLED ticket for the kitchen, a new note prints a CHANGED ticket, ' +
      '"Print again" prints that dish again. Then add the right dish and send it.'));
}

/// An on/off switch for setting `key` ("1" / "0"; not set = `def`).
function settingSwitch(key, label, onMsg, offMsg, def = true) {
  const box = h('input', { type: 'checkbox', checked: def });
  invoke('get_setting', { key }).then((v) => (box.checked = v == null ? def : v !== '0')).catch(() => {});
  box.onchange = act(async () => {
    await call('set_setting', { key, value: box.checked ? '1' : '0' });
    toast(box.checked ? onMsg : offMsg);
  });
  return h('label', { class: 'switch', 'data-key': key }, box, h('span', {}, ''), label);
}

/// Tick several things and delete them in one go (admins only). `items`:
/// [{ id, label, sub }]; `del(ids)` deletes and returns a message.
function bulkDelete(title, items, del, done) {
  if (!items.length) return toast('Nothing to delete here', true);
  const picked = new Set();
  const list = h('div', { class: 'stack bulk-list' });
  const all = h('input', { type: 'checkbox' });
  const btn = h('button', { class: 'danger' });
  let sure = false;
  const draw = () => {
    setKids(list, items.map((it) => {
      const box = h('input', { type: 'checkbox', checked: picked.has(it.id) });
      box.onchange = () => { if (box.checked) picked.add(it.id); else picked.delete(it.id); sure = false; drawBtn(); };
      return h('label', { class: `bulk-row ${picked.has(it.id) ? 'on' : ''}` }, box, h('span', { class: 'grow' }, it.label), it.sub && h('span', { class: 'muted small' }, it.sub));
    }));
    all.checked = picked.size === items.length;
    drawBtn();
  };
  const drawBtn = () => {
    btn.textContent = !picked.size ? 'Tick what to delete' : sure ? `Tap again: delete ${picked.size} for good` : `Delete ${picked.size}`;
    btn.disabled = !picked.size;
  };
  all.onchange = () => { picked.clear(); if (all.checked) items.forEach((it) => picked.add(it.id)); sure = false; draw(); };
  btn.onclick = act(async () => {
    if (!sure) { sure = true; drawBtn(); return; }
    const msg = await del([...picked]);
    closeModal();
    toast(msg);
    await done?.();
  });
  showModal(h('div', { class: 'row' }, h('h3', { class: 'grow' }, title), h('label', { class: 'check' }, all, ' All')),
    list, h('div', { class: 'row' }, btn, h('span', { class: 'grow' }), h('button', { onclick: closeModal }, 'Close')));
  draw();
}

/// Delete several of one kind through `delete_many`; the message says what happened.
async function deleteMany(kind, ids, what) {
  const r = await call('delete_many', { kind, ids });
  return [`${r.deleted} ${what} deleted`, r.hidden && `${r.hidden} hidden (still on past orders)`, r.errors.length && `${r.errors.length} not deleted: ${r.errors[0]}`]
    .filter(Boolean).join(' · ');
}

/// Two taps to run `fn`: the first changes the label to `sure`.
function confirmBtn(label, sure, fn, cls = 'danger') {
  const b = h('button', { class: cls });
  b.textContent = label;
  b.onclick = act(async () => {
    if (!b.dataset.sure) {
      b.dataset.sure = '1';
      b.textContent = sure;
      setTimeout(() => { delete b.dataset.sure; b.textContent = label; }, 5000);
      return;
    }
    delete b.dataset.sure;
    b.textContent = label;
    await fn();
  });
  return b;
}

// ---------- full backups: restore, load from a file, download ----------

const fileSize = (b) => (b >= 1048576 ? `${(b / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(b / 1024))} KB`);
/// What a full backup file is, from its name (see prune_backups in commands.rs).
function backupKind(name) {
  const kinds = [['pos-before-update-', 'Before an update'], ['pos-before-reset-', 'Before a reset'],
    ['pos-before-restore-', 'Before a restore'], ['pos-imported-', 'Loaded from a file']];
  return kinds.find(([p]) => name.startsWith(p))?.[1] ?? 'Backup';
}
/// Bytes as base64, in slices (one big String.fromCharCode call overflows the stack).
function base64(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(s);
}
/// Send a .db file to the till in pieces (each request stays small enough
/// for the local server), then have it checked. Resolves to { file, summary }.
const UPLOAD_PIECE = 512 * 1024;
async function uploadBackup(f, progress) {
  if (!f.size) throw new Error('This file is empty: it is not a POS backup.');
  const id = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, '0')).join('');
  for (let offset = 0; offset < f.size; offset += UPLOAD_PIECE) {
    progress(Math.floor((offset * 100) / f.size));
    const piece = new Uint8Array(await f.slice(offset, offset + UPLOAD_PIECE).arrayBuffer());
    await call('upload_backup_chunk', { upload: id, offset, data: base64(piece) });
  }
  progress(100);
  return call('finish_backup_upload', { upload: id });
}
/// Browsers: save a full backup from the main till to this device.
async function downloadBackup(name) {
  let r;
  try {
    r = await fetch('/backup-file', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Terminal': terminalId }, body: JSON.stringify({ name }) });
  } catch { throw new Error('Cannot reach the POS. Check the main till is on and its server is running.'); }
  if (!r.ok) throw new Error((await r.json().catch(() => null)) || `server error ${r.status}`);
  const a = h('a', { href: URL.createObjectURL(await r.blob()), download: name });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 60000);
}
/// Show what a backup holds, then RESTORE typed and the admin's PIN replace
/// all data with it. `f`: the backup file (name, modified); `s`: inspect_backup.
function restoreDialog(f, s) {
  const word = h('input', { placeholder: 'Type RESTORE', autocomplete: 'off', autocapitalize: 'characters', spellcheck: 'false', style: 'width:170px' });
  const go = h('button', { class: 'danger solid', disabled: true }, 'Restore…');
  word.oninput = () => (go.disabled = word.value.trim() !== 'RESTORE');
  const fact = (k, v) => [h('span', { class: 'muted' }, k), h('strong', {}, String(v))];
  const when = s.last_used_at ?? f.modified;
  go.onclick = act(async () => {
    let r;
    const ok = await pinDialog({
      title: 'Enter your PIN to restore', hint: 'All data on this till will be replaced by the backup.', okLabel: 'Restore now', okClass: 'danger solid paybig',
      submit: async (pin) => {
        try { r = await invoke('restore_backup', { name: f.name, pin, confirm: word.value }); } catch (e) { return String(e?.message ?? e); }
      },
    });
    if (!ok) return;
    const n = r.summary;
    const msg = `Restored backup from ${fmtTime(when)}: ${n.orders} orders, ${n.menu_items} menu items, ${n.customers} customers, ${n.staff} staff. ` +
      'Log in with a PIN from the backup.';
    // Reload like every other till and tablet will, and say what happened after.
    try { sessionStorage.setItem(FLASH_KEY, msg); } catch {}
    forgetSession();
    location.reload();
  });
  showModal(h('h3', {}, 'Restore this backup?'),
    h('div', { class: 'restore-facts' },
      fact('Restaurant', s.store_name ?? '(no name)'), fact('Last used', fmtTime(when)),
      fact('Orders', s.open_orders ? `${s.orders} (${s.open_orders} open)` : s.orders), fact('Menu items', s.menu_items),
      fact('Customers', s.customers), fact('Staff', s.staff)),
    h('div', { class: 'muted small' }, `${backupKind(f.name)} · ${f.name}`,
      s.schema_version < s.app_schema_version ? ' · from an older version of the app: it is brought up to date' : ''),
    h('div', { class: 'warn-box' }, h('strong', {}, 'Everything on this till is replaced: '),
      'orders, menu, customers, staff and PINs, and settings all come from the backup. ',
      'A safety copy of the current data is saved first ("Before a restore" in the list), so you can go back. ',
      'Every till, tablet and kitchen screen is logged out: log in with a PIN from the backup.'),
    h('div', { class: 'row' }, word, go, h('span', { class: 'grow' }), h('button', { onclick: closeModal }, 'Cancel')));
  word.focus();
}

/// Menu only: save, list, restore, and restore from a file (another till).
function menuBackupCard() {
  const list = h('div', { class: 'stack' });
  let menuFiles = [];
  const restored = async (n) => {
    state.menu = null;
    toast(`Menu restored: ${n.items} items, ${n.categories} categories, ${n.option_groups} option groups`);
  };
  const load = async () => {
    const files = await call('list_menu_backups');
    menuFiles = files;
    setKids(list, files.length
      ? h('table', {}, files.slice(0, 10).map((f) => h('tr', {},
        h('td', {}, fmtTime(f.saved_at)), h('td', { class: 'muted' }, `${f.items} items`),
        h('td', { class: 'num' }, confirmBtn('Restore', 'Tap again: replace the menu', async () => restored(await call('restore_menu_backup', { name: f.name })), ''),
          isAdmin() && confirmBtn('Delete', 'Tap again to delete', async () => { await call('delete_menu_backups', { names: [f.name] }); toast('Menu backup deleted'); await load(); }, 'danger-outline')))))
      : h('div', { class: 'muted' }, 'No menu backups yet.'));
  };
  const file = h('input', { type: 'file', accept: '.json,application/json', hidden: true });
  const picked = h('div');
  file.onchange = act(async () => {
    const f = file.files[0];
    file.value = '';
    if (!f) return;
    let menu;
    try { menu = JSON.parse(await f.text()); } catch { return toast('This file is not a POS menu backup', true); }
    if (menu?.format !== 'pos-menu') return toast('This file is not a POS menu backup', true);
    setKids(picked, h('div', { class: 'row card' },
      h('span', { class: 'grow' }, `${f.name}: ${menu.items.length} items, ${menu.categories.length} categories`),
      confirmBtn('Replace menu with this file', 'Tap again to replace the menu', async () => {
        restored(await call('import_menu', { menu }));
        setKids(picked);
      }, 'primary'),
      h('button', { onclick: () => setKids(picked) }, 'Cancel')));
  });
  const save = act(async () => {
    const r = await call('backup_menu');
    toast(`Menu saved (${r.file.items} items)`);
    if (state.remote) {
      // In a browser, also give the user the file.
      const a = h('a', { href: URL.createObjectURL(new Blob([JSON.stringify(r.menu, null, 2)], { type: 'application/json' })), download: r.file.name });
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    }
    await load();
  });
  load().catch(() => {});
  return h('div', { class: 'card stack' },
    h('h3', {}, 'Menu backup'),
    h('p', { class: 'muted' }, 'Save just the menu (categories, items, choices, options and prices), then restore it later or load it on another till. ' +
      'Restoring replaces the current menu; past orders are not changed.'),
    h('div', { class: 'row' }, h('button', { class: 'primary', onclick: save }, 'Back up menu'),
      h('button', { onclick: () => file.click() }, 'Restore from a file…'), file, h('span', { class: 'grow' }),
      isAdmin() && h('button', { class: 'danger-outline', onclick: () => bulkDelete('Delete menu backups', menuFiles.map((f) => ({ id: f.name, label: fmtTime(f.saved_at), sub: `${f.items} items` })),
        async (names) => `${await call('delete_menu_backups', { names })} menu backup(s) deleted`, load) }, 'Delete several…')),
    picked, list);
}

/// Danger zone: delete chosen data (admin only, PIN + RESET, full backup first).
function resetCard() {
  const box = (label, hint) => {
    const input = h('input', { type: 'checkbox' });
    return { input, el: h('label', { class: 'check' }, input, h('span', {}, h('strong', {}, label), hint && h('span', { class: 'muted' }, ` · ${hint}`))) };
  };
  const sales = box('Sales history', 'orders, payments, day closes');
  const salesWhen = h('select', {}, h('option', { value: 'all' }, 'All sales, including open orders'), h('option', { value: 'before' }, 'Only sales before a date'));
  const salesDate = h('input', { type: 'date', value: today(), hidden: true });
  salesWhen.onchange = () => (salesDate.hidden = salesWhen.value !== 'before');
  const salesOpts = h('div', { class: 'row', style: 'padding-left:28px', hidden: true }, salesWhen, salesDate);
  sales.input.onchange = () => (salesOpts.hidden = !sales.input.checked);
  const customers = box('Customers', 'saved names, phones, addresses');
  const menu = box('Menu', 'categories, items, options, stock');
  const floor = box('Floor plan', 'floors and tables');
  const staff = box('Staff', 'everyone except you');
  const settings = box('Settings', 'restaurant name, printers, platforms, display, local server');
  const pin = h('input', { type: 'password', inputmode: 'numeric', placeholder: 'Your PIN', style: 'width:140px' });
  const confirm = h('input', { placeholder: 'Type RESET', style: 'width:140px' });
  const go = act(async () => {
    const options = {
      sales: sales.input.checked ? (salesWhen.value === 'all' ? { when: 'all' } : { when: 'before', date: salesDate.value }) : null,
      customers: customers.input.checked, menu: menu.input.checked, floor_plan: floor.input.checked,
      staff: staff.input.checked, settings: settings.input.checked,
    };
    if (!options.sales && !Object.entries(options).some(([k, v]) => k !== 'sales' && v)) return toast('Tick what you want to delete', true);
    const r = await call('reset_data', { options, pin: pin.value, confirm: confirm.value });
    pin.value = confirm.value = '';
    const n = r.report;
    toast(`Deleted: ${n.orders} orders, ${n.customers} customers, ${n.items} items, ${n.tables} tables, ${n.staff} staff. A full backup was saved first.`);
    Object.assign(state, { menu: null, cats: null });
    await start();
  });
  return h('div', { class: 'card stack danger-zone' },
    h('h3', {}, 'Danger zone · Reset data'),
    h('p', { class: 'muted' }, 'Tick only what you want to DELETE. Everything left unticked is kept. ' +
      'A full backup is saved automatically first, so a mistake can be undone from the backups folder.'),
    sales.el, salesOpts, customers.el, menu.el, floor.el, staff.el, settings.el,
    h('div', { class: 'row' }, pin, confirm, h('button', { class: 'danger solid', onclick: go }, 'Delete selected data')));
}

// ---------- full screen ----------

/// Desktop: the app window. Browser: the page (tablets have no F11).
async function toggleFullscreen() {
  if (TAURI) return invoke('toggle_fullscreen');
  if (document.fullscreenElement) await document.exitFullscreen();
  else await document.documentElement.requestFullscreen?.();
}
document.addEventListener('keydown', (e) => {
  // Browsers handle F11 themselves; the desktop window needs it done here.
  if (e.key === 'F11' && TAURI) {
    e.preventDefault();
    toggleFullscreen().catch((err) => toast(String(err?.message ?? err), true));
  }
});
document.getElementById('fullscreen').append(icon('expand', 20));
// On-screen keyboard: on by default on the main till, off on other devices (they have their own).
{
  const kb = document.getElementById('keyboard');
  kb.append(icon('keyboard', 22));
  kb.onclick = () => { OSK.enabled = !OSK.enabled; toast(OSK.enabled ? 'On-screen keyboard on: tap a field to type' : 'On-screen keyboard off'); };
  OSK.onchange = () => kb.classList.toggle('on', OSK.enabled);
  OSK.init(!!TAURI);
}
document.getElementById('fullscreen').onclick = act(toggleFullscreen);

document.getElementById('logout').prepend(icon('logout', 20));
document.getElementById('help').append(icon('help', 22), h('span', { class: 'help-label' }, 'Help'));
document.getElementById('help').onclick = showHelp;
document.getElementById('theme').onclick = toggleTheme;
applyTheme();
start().catch((e) => toast(String(e?.message ?? e), true));
// After a restore this page reloaded: say what was restored.
try {
  const flash = sessionStorage.getItem(FLASH_KEY);
  if (flash) { sessionStorage.removeItem(FLASH_KEY); toast(flash, false, 9000); }
} catch {}
// A till or tablet left on the login screen notices a restore on another
// one (its list of names would be the old staff) and reloads.
setInterval(() => { if (!state.session) invoke('app_status').then(dataChanged).catch(() => {}); }, 15000);
/// A ticket sent slowly (or a later copy) that failed after the till was told
/// it was sent: say so on every till, so it can be printed again.
let printFailAfter = null;
setInterval(() => {
  if (!state.session) return;
  invoke('print_failures', { after: printFailAfter }).then(({ failures: list, last }) => {
    printFailAfter = last;
    if (!list.length) return;
    toast(list.length > 1 ? `${list.at(-1).message} (and ${list.length - 1} more)` : list[0].message, true, 12000);
  }).catch(() => {});
}, 10000);
