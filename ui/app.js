'use strict';
// POS terminal UI. Every rule (prices, tax, permissions, stock, table
// occupancy) is enforced in Rust; this file only calls commands and draws.
// It runs in the desktop window and, through the optional local server, in
// any browser on the network.

const TAURI = window.__TAURI__?.core;
/// Commands only the desktop window has (they act on this PC itself).
const DESKTOP_ONLY = new Set(['check_update', 'install_update', 'server_status', 'set_server', 'toggle_fullscreen', 'open_backup_folder',
  'get_autostart', 'set_autostart', 'pick_backup_copy_folder', 'pick_recovery_backup', 'restart_app']);
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
  // This device's name (Devices & display), so the log and tickets can say "Bar tablet".
  const headers = { 'Content-Type': 'application/json', 'X-Terminal': terminalId };
  try { const n = localStorage.getItem('pos.deviceName')?.trim(); if (n) headers['X-Device'] = encodeURIComponent(n); } catch {}
  try {
    r = await fetch(`/api/${cmd}`, { method: 'POST', headers, body: JSON.stringify(args) });
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
  menu: null, cats: null, catId: null, dirty: false,
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
  chev: '<path d="M9 18l6-6-6-6"/>',
  grip: '<circle cx="9" cy="6" r="1.2"/><circle cx="15" cy="6" r="1.2"/><circle cx="9" cy="12" r="1.2"/><circle cx="15" cy="12" r="1.2"/><circle cx="9" cy="18" r="1.2"/><circle cx="15" cy="18" r="1.2"/>',
  more: '<circle cx="5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="19" cy="12" r="1.2"/>',
  minus: '<path d="M5 12h14"/>',
  alert: '<circle cx="12" cy="12" r="9"/><path d="M12 7v6M12 16.5h.01"/>',
  calendar: '<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M3 10h18M8 3v4M16 3v4"/>',
  chat: '<path d="M21 12a8 8 0 0 1-11.6 7.1L3 21l1.9-6.4A8 8 0 1 1 21 12z"/><path d="M8 11h.01M12 11h.01M16 11h.01"/>',
  send: '<path d="M22 2L11 13"/><path d="M22 2l-7 20-4-9-9-4z"/>',
  paperclip: '<path d="M21 11.5l-8.6 8.6a5 5 0 0 1-7-7l8.6-8.6a3.3 3.3 0 0 1 4.7 4.7l-8.6 8.6a1.7 1.7 0 0 1-2.4-2.4l7.9-7.9"/>',
  activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  ban: '<circle cx="12" cy="12" r="9"/><path d="M5.6 5.6l12.8 12.8"/>',
  key: '<circle cx="8" cy="15" r="4"/><path d="M10.8 12.2L20 3M16 7l3 3M14 9l2 2"/>',
  coin: '<circle cx="12" cy="12" r="9"/><path d="M14.5 9.5c-.5-1-1.5-1.5-2.5-1.5-1.7 0-2.5 1-2.5 2s1 1.6 2.5 2 2.5 1 2.5 2-1 2-2.5 2c-1 0-2-.5-2.5-1.5M12 6v2M12 16v2"/>',
  trash: '<path d="M3 6h18M8 6V4h8v2M6 6l1 15h10l1-15"/>',
  download: '<path d="M12 3v12M7 10l5 5 5-5M4 21h16"/>',
};
// Manage pages are drawn with their own id as the icon.
ICONS.tickets = ICONS.chat;
ICONS.log = ICONS.activity;

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
  // E.g. the Theme row in Devices & display shows the new choice.
  document.dispatchEvent(new Event('themechange'));
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
/// `ms`: how long it stays (longer messages need longer). `action`: { label, onclick },
/// one button in the toast (e.g. Open), which also hides it.
/// `top`: at the top of the screen, wherever the page is (a note not about what is being done).
function toast(msg, err = false, ms = err ? 4000 : 2500, action = null, { top = false } = {}) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.className = err ? 'err' : '';
  t.dataset.top = top ? '1' : '';
  if (action) {
    t.classList.add('has-act');
    t.replaceChildren(h('span', {}, msg), h('button', { class: 'toast-act', onclick: () => { t.hidden = true; clearTimeout(toastTimer); action.onclick(); } }, action.label));
  }
  t.hidden = false;
  placeToast();
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), ms);
}

/// With a sheet or dialog open, a toast goes to the very top of the screen (over the
/// title), never on the sheet's body or foot (Send, Normal/Urgent, quick-text chips).
/// Notes not about the page (`toast(..., { top: true })`) go there too.
function placeToast() {
  const t = document.getElementById('toast');
  const top = !t.hidden && !document.body.classList.contains('kb-open') && (t.dataset.top === '1' || !document.getElementById('modal').hidden);
  t.classList.toggle('top', !!top);
}

/// Saving a setting can change the order screen ("Starter or Main?", tax,
/// service charge…): the next order screen loads the menu and settings again.
const SETTINGS_WRITES = new Set(['set_setting', 'set_service_settings', 'set_receipt_settings', 'save_teams']);
/// The server's answer for anything done to an order that is no longer open (orders.rs `require_open`).
const ORDER_CLOSED = 'this order is already closed';
/// A dish not as this screen showed it (pos_core::ITEM_CHANGED), and a pay dialog whose
/// "left to pay" is out of date (pos_core::BILL_CHANGED): nothing was saved or taken.
const ITEM_CHANGED = /dish changed on another till/i;
const ITEM_CHANGED_TEXT = 'This dish changed on another till';
const BILL_CHANGED = /bill changed on another till/i;
/// The answer when this device could not reach the main till at all (invoke): a
/// money action may or may not have arrived, so the till checks before a retry.
const UNREACHABLE = /cannot reach the pos/i;
/// A random id for one tap or one confirm (add_item, add_payment): sent again
/// after a lost answer, the main till does not do it twice.
const onceKey = () => [...crypto.getRandomValues(new Uint8Array(12))].map((b) => b.toString(16).padStart(2, '0')).join('');
/// True when `msg` (from a call made on the order screen) means the order on
/// screen is gone: paid, cancelled, or merged into another bill on another till.
function isDeadOrder(msg, args = {}, cmd = null) {
  if (!state.orderOpen || state.orderId == null) return false;
  if (msg === ORDER_CLOSED || SWEPT.test(msg)) return true;
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
/// The server's answer for an order its automatic sweep closed for having nothing on
/// it (pos_core::EMPTY_ORDER_SWEPT). An empty order another till cancelled still
/// answers ORDER_CLOSED.
const EMPTY_ORDER_SWEPT = 'this empty order was closed automatically';
const SWEPT = { test: (msg) => msg === EMPTY_ORDER_SWEPT };
const SWEPT_TEXT = 'This empty order was closed automatically';
/// Leave a dead order instead of failing on every tap. Says why: closed on
/// another till, or (empty for a long time) closed automatically.
function leaveDeadOrder(msg = '') {
  const gone = state.orderId;
  state.onModalClose = null;
  closeModal();
  state.orderEmpty = null;
  closeOrder();
  if (SWEPT.test(msg)) return toast(SWEPT_TEXT, true);
  // Gone (not closed): joined onto another table's bill, or all its dishes moved there.
  if (msg === 'not found' || msg === '') {
    return invoke('order_moved_to', { orderId: gone }).then((o) => {
      if (!o) return toast('This order was closed on another till', true);
      toast(`Moved to ${orderTitle(o)}`, false, 4000);
      if (state.session && state.orderId == null) openOrder(o.id);
    }, () => toast('This order was closed on another till', true));
  }
  toast('This order was closed on another till', true);
}
/// `quiet`: no toast for an error (the caller shows it, e.g. in a settings row).
async function call(cmd, args = {}, { quiet = false } = {}) {
  try {
    const r = await invoke(cmd, args);
    if (SETTINGS_WRITES.has(cmd)) { state.menu = null; loadUiSettings(); }
    return r;
  } catch (e) {
    const msg = String(e?.message ?? e);
    // Logged out on the server (idle, or a backup was restored on any till):
    // forget everything from the old session before showing the login.
    if (msg === 'please log in') { forgetSession(); start(); }
    // Damaged data on the main till: browsers stop with "The main till needs attention".
    // Not an admin (they may restore from Backup & data), and not "Back up now" (it says why).
    else if (DATA_DAMAGED.test(msg) && state.remote && !isAdmin() && cmd !== 'backup_now') renderAttention();
    else if (isDeadOrder(msg, args, cmd) || await deniedForDeadOrder(msg, args, cmd)) leaveDeadOrder(msg);
    else if (!quiet) toast(msg, true);
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
/// Put a page on screen. A first argument { action } is the page's one main
/// button, in its header (Manage pages).
function screen(...kids) {
  clearInterval(refreshTimer);
  closeInfoPop();
  const action = kids[0] && !(kids[0] instanceof Node) && 'action' in kids[0] ? kids.shift().action : null;
  // Pages opened from Manage open inside it (managers and admins); staff get
  // their one page (Kitchen screen) with a way back to Manage.
  if (state.session && !state.orderId && inManage(state.view)) {
    const id = state.view, name = VIEWS.find(([v]) => v === id)?.[1];
    if (useShell()) return manageScreen([dhead(id, name, action), ...kids]);
    return setKids(app, h('div', { class: 'dhead staff-head' }, h('button', { class: 'ghost back show', onclick: () => go('settings') }, icon('back', 22), h('span', {}, 'Manage')), h('h2', {}, name)), ...kids);
  }
  setKids(app, action, ...kids);
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
    quickNotes: null, starterChoice: null, day: null, unsentAsk: null,
  });
  resetManage();
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
/// The server's answer for any command while the data is damaged (commands.rs).
const DATA_DAMAGED = /POS data is damaged/i;

/// Browsers on the network while the main till's data is damaged: nothing works
/// here; the main till has the recovery screen.
function renderAttention() {
  forgetSession();
  document.getElementById('bar').hidden = true;
  lockScreen(h('div', { class: 'lock-card stack recovery' },
    h('span', { class: 'eic bad' }, icon('alert', 34)),
    h('h2', {}, 'The main till needs attention'),
    h('p', {}, 'The POS data on the main till is damaged, so this device cannot work. Ask a manager to check the main till: it shows how to restore a backup.'),
    h('button', { class: 'primary lg', onclick: () => location.reload() }, 'Try again')));
}

/// The recovery screen (main till only): the data is damaged or would not open.
/// "Restore the newest good backup" checks the newest backups and offers the first
/// good one; "Choose a backup…" picks any (the list here, or a file on a USB stick).
/// RESTORE and an admin PIN from that backup confirm it, in the usual danger sheet.
function renderRecovery(rec) {
  state.session = null;
  document.getElementById('bar').hidden = true;
  const status = h('div', { class: 'recovery-status', role: 'status', 'aria-live': 'polite' });
  const working = (text) => setKids(status, text && h('span', { class: 'spin', 'aria-hidden': 'true' }), text);
  const failed = (e) => { working(null); setKids(status, h('div', { class: 'warn-box' }, String(e?.message ?? e))); };
  const buttons = [];
  const busy = (on) => { for (const b of buttons) b.disabled = on; };
  const check = async (name = null) => {
    busy(true);
    working(name ? 'Checking that backup…' : 'Looking for the newest good backup… (this can take a few seconds)');
    try {
      const found = await invoke('recovery_check', name ? { name } : {});
      working(null);
      recoveryRestoreSheet(found.file, found.summary);
    } catch (e) { failed(e); } finally { busy(false); }
  };
  // The newest good backup, looked for as soon as the screen shows: the button and
  // the text say how old it is (what restoring it means for today's orders).
  let good = null;
  const newest = h('button', { class: 'primary lg', onclick: () => (good ? recoveryRestoreSheet(good.file, good.summary) : check()) },
    icon('database', 20), 'Restore the newest good backup');
  const age = h('p', { class: 'recovery-age' }, h('span', { class: 'spin', 'aria-hidden': 'true' }), ' Looking for the newest good backup…');
  invoke('recovery_check', {}).then((found) => {
    good = found;
    const at = found.summary.last_used_at ?? found.file.modified;
    setKids(newest, icon('database', 20), `Restore backup from ${friendlyTime(at)}`);
    setKids(age, h('strong', {}, `Newest good backup: ${friendlyTime(at)}`), ` (${ago(at)} ago). Orders after that are not in it. `,
      rec.reason === 'cannot_open' ? 'Nothing else works until a backup is restored.' : 'The damaged data is kept as a safety copy.');
  }, (e) => setKids(age, String(e?.message ?? e)));
  const choose = h('button', { class: 'lg', onclick: () => chooseBackup() }, 'Choose a backup…');
  buttons.push(newest, choose);
  async function chooseBackup() {
    let st;
    try { st = await invoke('recovery_status'); } catch (e) { return failed(e); }
    const fromFile = TAURI && h('button', { onclick: async () => {
      s.close();
      busy(true); working('Checking the file…');
      try {
        const found = await invoke('pick_recovery_backup');
        working(null);
        if (found) recoveryRestoreSheet(found.file, found.summary);
      } catch (e) { failed(e); } finally { busy(false); }
    } }, 'From a file (USB stick, second folder)…');
    const rows = st.backups.map((f) => srow({ label: friendlyTime(f.modified).replace(/^./, (c) => c.toUpperCase()),
      hint: `${backupKind(f.name)} · ${fileSize(f.size ?? 0)}`, cls: 'brow', ctl: h('button', { onclick: () => { s.close(); check(f.name); } }, 'Check')}));
    const s = sheet({ title: 'Choose a backup', body: [
      ssec(null, rows.length ? rows : [h('p', { class: 'snote' }, `No backups in ${st.folder}.`)]),
      fromFile && ssec('Somewhere else', [h('div', { class: 'row' }, fromFile)]),
    ] });
  }
  lockScreen(h('div', { class: 'lock-card stack recovery' },
    h('span', { class: 'eic bad' }, icon('alert', 34)),
    h('h2', {}, rec.message || 'The POS data is damaged'),
    age,
    newest, choose, status,
    rec.can_log_in && h('button', { class: 'ghost', onclick: () => { state.skipRecovery = true; renderLogin(); } }, 'Try logging in'),
    rec.detail && h('details', { class: 'muted small' }, h('summary', {}, 'Details for support'), rec.detail)));
}

/// What a backup holds, then RESTORE and an admin PIN from that backup (the danger sheet).
function recoveryRestoreSheet(f, sm) {
  const when = sm.last_used_at ?? f.modified;
  const word = h('input', { placeholder: 'RESTORE', autocomplete: 'off', autocapitalize: 'characters', spellcheck: 'false', 'aria-label': 'Type RESTORE' });
  const pin = h('input', { type: 'password', inputmode: 'numeric', autocomplete: 'off', 'aria-label': 'Admin PIN' });
  const fact = (k, v) => h('div', {}, h('span', { class: 'muted' }, k), h('strong', {}, String(v)));
  const whenText = when ? friendlyTime(when) : '?';
  const sh = sheet({ title: 'Restore this backup?', danger: true, saveLabel: 'Restore now', body: [
    h('div', { class: 'restore-facts' },
      fact('Restaurant', sm.store_name ?? '(no name)'), fact('Last used', whenText),
      fact('Orders', sm.open_orders ? `${sm.orders} (${sm.open_orders} open)` : sm.orders), fact('Menu items', sm.menu_items),
      fact('Customers', sm.customers), fact('Staff', sm.staff)),
    h('p', { class: 'snote' }, `${backupKind(f.name)} · ${fileSize(f.size ?? 0)}`),
    h('div', { class: 'warn-box' }, h('strong', {}, `Orders after ${whenText.replace(/^today /, '')} are not in this backup.`),
      ' The damaged data is replaced by it (kept as a safety copy first).'),
    ssec('Confirm', [srow({ label: 'Type RESTORE', cls: 'text-row', ctl: word }), srow({ label: 'Admin PIN', cls: 'text-row', ctl: pin })]),
    h('p', { class: 'snote' }, "An admin PIN from this backup (the PIN they had on the backup's date)."),
  ], onSave: async () => {
    let r;
    try { r = await invoke('recovery_restore', { name: f.name, pin: pin.value, confirm: word.value.trim() }); } catch (e) { return String(e?.message ?? e); }
    const n = r.summary;
    const msg = `Restored backup from ${whenText}: ${n.orders} orders, ${n.menu_items} menu items, ${n.staff} staff. Log in with a PIN from the backup.`;
    if (r.restart_needed) {
      setTimeout(() => lockScreen(h('div', { class: 'lock-card stack recovery' },
        h('span', { class: 'eic' }, icon('check', 34)), h('h2', {}, 'Restored. Restart POS to finish.'), h('p', {}, msg),
        h('button', { class: 'primary lg', onclick: act(async () => { try { sessionStorage.setItem(FLASH_KEY, msg); } catch {} await invoke('restart_app'); }) }, 'Restart POS'))));
      return true;
    }
    try { sessionStorage.setItem(FLASH_KEY, msg); } catch {}
    forgetSession();
    location.reload();
    return false;
  } });
  const ok = () => { sh.save.disabled = !(word.value.trim() === 'RESTORE' && /^\d{4,8}$/.test(pin.value)); };
  word.addEventListener('input', ok);
  pin.addEventListener('input', ok);
  ok();
}

async function start() {
  const st = await invoke('app_status');
  if (dataChanged(st)) return;
  state.session = st.session;
  state.storeName = st.store_name;
  state.plan = { w: st.plan_width, h: st.plan_height };
  state.version = st.version;
  state.remote = st.remote;
  document.title = st.store_name || 'PuppxPOS';
  // The main till only: the data is damaged (or would not open). Recovery first;
  // "Try logging in" (when it may still work) goes on to the login.
  if (st.recovery && !(st.recovery.can_log_in && state.skipRecovery)) return renderRecovery(st.recovery);
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
      h('div', { class: 'lock-store' }, state.storeName || 'PuppxPOS'),
      clock, date,
      h('div', { class: 'lock-foot' }, h('span', { class: 'grow' }, state.remote ? 'Connected to the main till' : `Version ${state.version ?? ''}`),
        h('button', { class: 'ghost icon-btn theme-btn', onclick: toggleTheme }))),
    // On small screens the side (and its theme button) is hidden: one in the corner instead.
    h('div', { class: 'lock-main' }, h('button', { class: 'ghost icon-btn theme-btn lock-theme', onclick: toggleTheme }), ...kids)));
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
      resetManage();
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
  }).catch((e) => { if (DATA_DAMAGED.test(String(e?.message ?? e)) && state.remote) renderAttention(); else pinStep(null); });
  pinStep(null);
}

// ---------- settings the screens read (start screen, order types, sizes…) ----------

/// Settings every screen may need, read once per screen change in one call.
/// Not set = the defaults in uiSettings(), which are how the app always worked.
const UI_KEYS = ['auto_logout_minutes', 'auto_logout_warn_secs', 'start_screen', 'show_takeaway', 'show_collection', 'show_delivery',
  'button_size', 'discount_presets', 'discount_staff_max_pct', 'pay_name_cash', 'pay_name_card'];
const DISCOUNT_PRESETS_DEFAULT = [5, 10, 15, 20];
/// The staff discount limit (%) until a manager or admin sets one (pos-core DISCOUNT_LIMIT_DEFAULT).
const DISCOUNT_LIMIT_DEFAULT = 10;
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
    /// Staff need a manager's PIN for more than this percentage off a line; null = no limit
    /// (stored as ''). Never set: DISCOUNT_LIMIT_DEFAULT (pos-core settings.rs discount_limit).
    discountLimit: raw.discount_staff_max_pct != null && String(raw.discount_staff_max_pct).trim() === '' ? null
      : whole(raw.discount_staff_max_pct, 0, 100, DISCOUNT_LIMIT_DEFAULT),
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
  ['tickets', 'Tickets', 'manager'],
  ['log', 'Activity log', 'manager'],
];
const inManage = (view) => !VIEWS.find(([id]) => id === view)?.[3];

function go(view) {
  if (!leaveOk()) return;
  // Leaving an order with dishes not yet sent: ask first.
  if (state.orderId != null && state.unsentAsk) {
    askUnsent().then((ok) => { if (ok) go(view); });
    return;
  }
  // The Manage tab: the last page or group opened (the list on a phone).
  if (view === 'settings') { state.settingsTab = null; clearBack(); }
  state.view = view;
  const left = state.orderId;
  state.orderId = null;
  dropEmptyOrder(left).finally(renderShell);
}

// ---------- automatic updates ----------

let updateTimer;
/// Check the update server for a newer signed version; show it in the header.
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
  // Logged out meanwhile (e.g. "The main till needs attention" took over): nothing to draw.
  if (!state.session) return;
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
  document.getElementById('brand').title = state.storeName || 'PuppxPOS';
  const navOn = inManage(state.view) ? 'settings' : state.view;
  document.getElementById('nav').replaceChildren(...VIEWS.filter(([, , need, rail]) => rail && canSee(need)).map(([id, label]) =>
    h('button', { class: !state.orderId && id === navOn ? 'on' : '', onclick: () => go(id), title: label, 'aria-label': label }, icon(id), h('span', { class: 'nav-label' }, label))));
  const name = VIEWS.find(([id]) => id === state.view)?.[1];
  drawTitle(state.orderId ? 'Order' : name, !state.orderId && inManage(state.view) ? 'Manage' : null);
  document.getElementById('who').title = `${state.session.name} · ${ROLE_LABEL[state.session.role] ?? 'Staff'}`;
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
    customers: renderCustomers, shift: renderShift, kitchen: renderKitchen, tickets: renderTickets, log: renderActivity,
  }[state.view];
  view().catch((e) => { if (!e?.handled) toast(String(e?.message ?? e), true); });
  drawTicketBadges();
  // Just logged in: badges, a reply toast and admins' backup alerts at once, not in 10 s.
  if (!tk.started) { tk.started = true; pollTickets(); }
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
    'Pick Today, Yesterday or 7 days, or tap the calendar for one day.',
    'Tap an order to see it, reprint its receipt or void it (a refund). ⋯ on a row reprints straight away.']],
  reports: ['Reports', [
    "See a day's sales, orders, average bill, tips and best sellers. Pick the day at the top.",
    'At closing time tap Close day: it prints the summary. All orders must be paid or cancelled first.']],
  settings: ['Manage', [
    'Pick a page or a settings group in the list. Search finds a setting by name, like "printer", "tax" or "PIN".',
    'Switches and choices save at once: a ✓ shows next to them. Printers, the service charge and other settings with several parts open a panel with Save.',
    'Lists (teams, delivery apps, the floor plan) keep changes in a bar at the bottom until you tap Save.',
    'Tap (i) next to a setting for a short explanation.']],
  menuAdmin: ['Menu', [
    'Tap + Item to add a dish: a name, a category and a price is enough.',
    'Tap any item to change it: kitchen flags, sold out, and under Advanced tax, choices, option groups and stock.',
    'Categories and option groups are on the left (chips on a phone). ⋯ has set menus and Paste a menu.']],
  plan: ['Floor plan', ['Pick a floor at the top, or add one with + Floor. This floor renames or deletes it.', 'Add a square or round table, or an Area for things that are not tables (the bar, a window).', 'Drag shapes to move them. Tap one to change its name, shape, size or kind in the card on the right.', 'Changes wait in the bar at the bottom: tap Save layout when you are done.']],
  users: ['Staff', ['Add each person with + Staff member: a name, a role and a PIN.', 'Tap a person to change their name, role, PIN or whether they can log in. Changes save at once.']],
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
    helpTickets(),
    h('button', { class: 'primary', onclick: closeModal }, 'Got it'));
  document.querySelector('#modal .dialog')?.classList.add('help-dialog');
  // Esc closes it, like the other dialogs.
  document.removeEventListener('keydown', state.modalKeys);
  state.modalKeys = (e) => { if (e.key === 'Escape') closeModal(); };
  document.addEventListener('keydown', state.modalKeys);
}

/// Top bar title: "Restaurant / Manage › Reports". `crumb` ("Manage") is left out on phones.
function drawTitle(title, crumb = null) {
  setKids(document.getElementById('title'), h('span', {}, state.storeName || 'PuppxPOS'), h('span', { class: 'sep' }, '/'),
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
  state.unsentAsk = null;
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
  state.unsentAsk = null;
  closeModal();
  hideIdleWarn();
  await dropEmptyOrder(state.orderId);
  try { await invoke('logout'); } catch { /* already logged out on the server */ }
  Object.assign(state, { session: null, orderId: null, view: 'floor', menu: null, cats: null });
  resetManage();
  document.getElementById('bar').hidden = true;
  renderLogin();
  if (message) toast(message);
}

document.getElementById('logout').onclick = act(async () => {
  // A list or form with a save bar (Delivery apps, Teams, the floor plan, a staff member or
  // customer being edited): log out only once its changes are let go.
  if (state.dirty && !await discardDialog()) return;
  if (!await askUnsent()) return;
  await logOut();
});

/// "Discard your changes?" before logging out: Keep editing is the big button. True = discard.
function discardDialog() {
  return new Promise((resolve) => {
    const done = (v) => { state.onModalClose = null; closeModal(); resolve(v); };
    showModal(h('h3', {}, 'Discard your changes?'), h('div', { class: 'stack confirm-hint' }, 'They are not saved yet.'),
      h('button', { class: 'primary paybig', onclick: () => done(false) }, 'Keep editing'),
      h('div', { class: 'row' }, h('span', { class: 'grow' }), h('button', { class: 'danger-outline', onclick: () => done(true) }, 'Discard and log out')));
    state.onModalClose = () => resolve(false);
    document.removeEventListener('keydown', state.modalKeys);
    state.modalKeys = (e) => { if (e.key === 'Escape') closeModal(); };
    document.addEventListener('keydown', state.modalKeys);
    document.querySelector('#modal .primary')?.focus();
  });
}

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
  // The floor plan editor with unsaved changes (its save bar holds), and the kitchen screen for staff, stay logged in.
  if (state.dirty === 'hold' || (state.view === 'kitchen' && !state.orderId && !isAdmin())) { lastActivity = Date.now(); return; }
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
  return h('div', { class: 'plan', style: `aspect-ratio:${w}/${ph}; width:min(100%, calc((100vh - ${reserve}px) * ${w / ph})); --plan-ratio:${ph / w}` });
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
        // The table as it is now: the floor may be a few seconds old (paid or
        // moved on another till), so an old bill is never opened again.
        const cur = await call('table_order', { tableId: t.id });
        if (cur) return openOrder(cur.id);
        try {
          const o = await invoke('open_order', { order: { order_type: 'dine_in', table_id: t.id } });
          openOrder(o.id);
        } catch (e) {
          // Another till opened it a moment ago: join that order, quietly.
          const o = await invoke('table_order', { tableId: t.id }).catch(() => null);
          if (o) return openOrder(o.id);
          throw e;
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
  // An empty order the server closed by itself (the table was tapped from an old
  // floor view): a table gets a new order, anything else goes back with a note.
  if (order.status === 'cancelled' && order.close_reason === 'empty' && !order.items.length) {
    const left = state.orderId;
    if (order.order_type === 'dine_in' && order.table_id != null) {
      let o = null;
      try { o = await invoke('open_order', { order: { order_type: 'dine_in', table_id: order.table_id } }); }
      catch { o = await invoke('table_order', { tableId: order.table_id }).catch(() => null); } // someone else just opened it
      if (o && state.orderId === left) { state.orderId = o.id; return renderOrder(); }
    }
    // Swept or cancelled empty on another till: the order data cannot tell which.
    if (state.orderId === left) { toast('This empty order was closed', true); closeOrder(); }
    return;
  }
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
      // Staff: a typed price needs a manager's or admin's PIN (orders.rs add_open_item_approved).
      const r = await managerRetry('add_open_item', { orderId, name: name.value, priceCents: cents, qty, note: note.value.trim() || null, starter: course }, {
        hint: `${name.value.trim()} × ${qty} at ${money(cents)}: a dish not on the menu. A manager or admin enters their PIN to approve it.`,
        okLabel: 'Approve and add' });
      if (!r) return;
      if (r.declined) return toast(`${name.value.trim()} not added (no manager PIN)`, true);
      order = r.result;
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
  const coursesSwitch = h('label', { class: 'switch', title: 'Otherwise starters go first and the rest waits for "Call mains"' },
    allCourses, h('span', {}), 'All courses together');
  // Customer details, service charge, tax: a card under the dishes.
  const extra = h('div', { class: 'ticket-extra' });
  let editing = null;
  // Changes sent from this screen: a refresh asked for before one of them is not shown.
  let changes = 0;

  // `course`: true = starter, false = main, null = as on the menu.
  // `team`: the team that makes it (team id), null = the menu's.
  async function addItem(productId, optionIds = [], variantId = null, course = null, team = null) {
    changes += 1;
    // A key per tap: asked again after a lost answer, the dish is not added twice.
    order = await call('add_item', { orderId, productId, qty: 1, note: null, variantId, optionIds, starter: course, teamId: team, idempotencyKey: onceKey() });
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
  // A dish changed, removed or moved on another till since this screen showed it:
  // nothing was saved; show the order as it is now and say what changed.
  async function lineChanged(it) {
    let now;
    try { now = await invoke('get_order', { id: orderId }); } catch (e) {
      const m = String(e?.message ?? e);
      if (m === 'please log in') { forgetSession(); start(); } else if (isDeadOrder(m, { id: orderId }, 'get_order')) leaveDeadOrder(m);
      return;
    }
    if (state.orderId !== orderId) return;
    if (now.status !== 'open') return leaveDeadOrder(ORDER_CLOSED);
    order = now;
    draw();
    const fresh = now.items.find((i) => i.id === it.id);
    toast(!fresh ? `${it.name} was removed or moved on another till: nothing changed`
      : `${it.name} changed on another till (now ${fresh.qty}${fresh.discount_cents ? `, −${money(fresh.discount_cents)}` : ''}): nothing changed. Check it and try again.`, true, 6000);
  }
  // `pin`: a manager's PIN already typed for this change (the price, in the dish pop-up).
  // `delta`: the order screen's + (1) and − (-1): one more or fewer than the dish has
  // on the server now, so taps made faster than the answers come back all count.
  // Otherwise the change is checked against the dish as this screen shows it: if
  // another till changed it meanwhile, nothing is saved and the order is shown again.
  // `noteOnly`: only the note changes (qtyDelta 0: the dish's quantity and discount stay as they are now).
  async function setLine(it, edit, pin = null, { delta = 0, noteOnly = false } = {}) {
    changes += 1;
    // Fewer of a discounted dish: the discount shrinks with it (the same share off),
    // so − never gives more off or asks for a manager PIN.
    if (edit.qty > 0 && edit.qty < it.qty && it.discount_cents && !('discount_cents' in edit)) {
      edit = { ...edit, discount_cents: Math.floor(it.discount_cents * edit.qty / it.qty) };
    }
    const args = { orderId, itemId: it.id, update: { qty: it.qty, discount_cents: it.discount_cents, note: it.note, ...edit } };
    if (delta) args.qtyDelta = delta;
    // A note alone: the discount sent is the one shown, and checked as such.
    if (noteOnly) Object.assign(args, { qtyDelta: 0, expectedDiscountCents: it.discount_cents });
    // A − on a dish the kitchen has (a CANCELLED ticket) is for the number on screen only.
    else if (!delta || (delta < 0 && it.sent_at)) Object.assign(args, { expectedQty: it.qty, expectedDiscountCents: it.discount_cents });
    try {
      order = await invoke('update_item', pin ? { ...args, pin } : args);
    } catch (e) {
      const msg = String(e?.message ?? e);
      // Over the staff discount limit (Settings → Orders → Discounts): a manager approves with their PIN.
      if (!/needs a manager/i.test(msg)) {
        if (msg === 'please log in') { forgetSession(); start(); }
        else if (isDeadOrder(msg)) leaveDeadOrder(msg);
        // The dish is gone or not as shown (changed, removed or moved on another till): show the order as it is now.
        else if (msg === 'not found' || ITEM_CHANGED.test(msg)) await lineChanged(it);
        else toast(msg, true);
        const err = new Error(msg); err.handled = true; throw err;
      }
      // Over the discount limit, more of a dish whose price was made cheaper, or fewer of a
      // dish already sent to the kitchen (orders.rs update_item_approved).
      const what = /changed price/i.test(msg) ? 'more' : /sent to the kitchen/i.test(msg) ? 'void' : 'discount';
      const q = edit.qty ?? it.qty;
      let stale = false;
      const ok = await pinDialog({
        title: 'Manager PIN',
        hint: what === 'more'
          ? `${it.name}: ${it.qty} → ${q} at ${money(it.unit_price_cents)} each, a price changed by a manager. A manager or admin enters their PIN to approve more of it.`
          : what === 'void'
            ? `${it.name}: ${q ? `${it.qty} → ${q}` : 'remove'} (already sent to the kitchen). A manager or admin enters their PIN to approve it.`
            : `${it.name}: ${msg}. A manager or admin enters their PIN to approve it.`,
        okLabel: what === 'more' ? `Approve ${q} at ${money(it.unit_price_cents)}` : what === 'void' ? (q ? `Approve ${it.qty} → ${q}` : 'Approve removing it') : 'Approve discount',
        submit: async (pin) => {
          try { order = await invoke('update_item', { ...args, pin }); } catch (e2) {
            const m = String(e2?.message ?? e2);
            if (m === 'please log in') { setTimeout(() => { forgetSession(); start(); }); return 'Logged out: log in again'; }
            if (isDeadOrder(m)) { setTimeout(() => leaveDeadOrder(m)); return 'This order is closed'; }
            if (m === 'not found' || ITEM_CHANGED.test(m)) { stale = true; setTimeout(closeModal); return 'Changed on another till'; }
            return /invalid pin/i.test(m) ? 'Wrong PIN, try again' : /needs a manager/i.test(m) ? 'That PIN cannot approve it: a manager or admin PIN is needed' : m;
          }
        },
      });
      if (stale) {
        await lineChanged(it);
        const err = new Error(ITEM_CHANGED_TEXT); err.handled = true; throw err;
      }
      if (!ok) {
        const err = new Error(what === 'more' || what === 'void' ? `Not changed: ${it.name} stays at ${it.qty}` : 'Discount not approved');
        err.handled = true;
        err.notApproved = true;
        toast(err.message, true);
        throw err;
      }
      toast(what === 'discount' ? 'Discount approved' : 'Approved');
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
  // After a payment whose answer never came: "Checking…", then the order as the main till
  // has it. Taken (one more payment than before, by this method): carry on as if the answer
  // had come. Not taken: the dialog shows the payments so far and Take can be tapped again
  // (with the same key, so it is still taken only once).
  // Messages go in the pay dialog's strip (err.note), never in a toast over it.
  const notSaved = (payment, text) => { const err = new Error('not saved'); err.handled = true; err.note = { kind: 'bad', text }; return err; };
  const card = (payment) => payment.method === 'card';
  async function checkPayment(payment, before) {
    for (let i = 0; i < 4; i++) {
      if (i) await new Promise((r) => setTimeout(r, 1500));
      let now;
      try { now = await invoke('get_order', { id: orderId }); } catch (e) {
        if (UNREACHABLE.test(String(e?.message ?? e))) continue;
        throw e;
      }
      order = now;
      const p = now.payments.at(-1);
      if (now.payments.length > before && p?.method === payment.method) return { order: now, print: { attempted: false } };
      if (now.status !== 'open') { const err = new Error('closed'); err.handled = true; leaveDeadOrder(ORDER_CLOSED); throw err; }
      draw();
      const err = notSaved(payment, card(payment)
        ? "Not saved: the main till didn't get it. Don't charge the card again. Tap Confirm to save it."
        : "Not saved: the main till didn't get it. Tap Confirm again.");
      err.order = now;
      throw err;
    }
    throw notSaved(payment, "Still cannot reach the main till. Check the Wi-Fi, then tap Confirm again (it won't be counted twice)."
      + (card(payment) ? " Don't charge the card again." : ''));
  }
  const finish = act(async () => {
    order = await call('get_order', { id: orderId });
    if (order.status !== 'open') return renderShell();
    draw();
    if (!order.items.length) return toast('The order is empty', true);
    let printError = null;
    await payDialog(order, reload, async (payment, { expectedLeftCents, idempotencyKey, closes, before, checking }) => {
      // Anything not yet sent (mains waiting included) goes to the kitchen with the payment
      // that closes the bill. A part payment sends nothing: mains held for "Call mains" stay held.
      if (closes && order.items.some((i) => !i.sent_at)) printError = (await call('send_to_kitchen', { orderId, allCourses: true })).print.error;
      // What the dialog showed as left to pay: if another till changed the bill since,
      // the server takes nothing ("The bill changed on another till") and the dialog shows it again.
      const args = { orderId, payment, expectedLeftCents, idempotencyKey };
      // Other on an order not from a delivery app: staff need a manager's or admin's PIN
      // (orders.rs add_payment_approved); the PIN pad takes the pay dialog's place for a moment.
      let paid;
      try {
        if (payment.method === 'other' && !order.channel_id) {
          const r = await managerRetry('add_payment', args, {
            hint: `${money(payment.amount_cents ?? order.total_cents - (order.paid_cents ?? 0))} by Other: no money comes into the till. A manager or admin enters their PIN to approve it.`,
            okLabel: 'Approve payment', before: () => { state.onModalClose = null; } });
          if (!r?.result) {
            const err = new Error('not paid'); err.handled = true; err.redraw = !!r?.declined;
            if (r?.declined) err.note = { kind: 'bad', text: 'Not paid (no manager PIN).' };
            throw err;
          }
          paid = r.result;
        } else paid = await call('add_payment', args, { quiet: true });
      } catch (e) {
        if (!UNREACHABLE.test(String(e?.message ?? e))) throw e;
        // The answer was lost (Wi-Fi): the payment may or may not have been taken. Look before any retry.
        checking();
        paid = await checkPayment(payment, before);
      }
      const { order: res, print: receipt } = paid;
      if (res.status !== 'paid') {
        order = res;
        draw();
        return res;
      }
      const last = res.payments.at(-1);
      const msg = !res.payments.length ? `Order #${res.id} closed (nothing to pay)`
        : `Order #${res.id} paid: ${money(res.total_cents)}` + (res.tip_cents ? ` + tip ${money(res.tip_cents)}` : '') +
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
  // `typed`: what was typed before (the note), kept when the pop-up opens again.
  function openLineEditor(it, typed = {}) {
    const sent = !!it.sent_at;
    let qty = it.qty;
    const priceOk = !it.parent_id && !(order.paid_cents > 0);
    const price = h('input', { inputmode: 'decimal', value: money(it.unit_price_cents), 'aria-label': 'Price each', class: 'price-input' });
    const note = h('input', { value: typed.note ?? it.note ?? '', placeholder: 'Note for the kitchen, e.g. no onions', 'aria-label': 'Note' });
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
    // One message at the end says what was saved and what was not.
    const save = act(async () => {
      const p = priceNow();
      if (priceOk && p == null) return toast('Price: enter an amount like 12 or 12.50', true);
      const newPrice = priceOk ? p : it.unit_price_cents;
      const discFor = (each) => (disc.value.trim() ? parseDiscount(disc.value, each * qty) : 0);
      try { discFor(newPrice); } catch { return toast('Discount: an amount like 2.50 or a percentage like 10%', true); }
      const newNote = note.value.trim() || null;
      closeModal();
      const said = [];
      let priceFailed = false, vanished = false;
      let pin = null;
      // The same answers as call() gives: logged out, or the order closed on another till.
      // "not found" is not enough to leave: it is also the answer when only the dish
      // went (removed or moved on another till) while the order stays open.
      const gone = (msg) => {
        if (msg === 'please log in') { forgetSession(); start(); return true; }
        if ((msg === ORDER_CLOSED || SWEPT.test(msg)) && isDeadOrder(msg, { orderId })) { leaveDeadOrder(msg); return true; }
        return false;
      };
      // After "not found": leave only if the order itself is no longer open; else show it as it is now.
      const dishGone = async () => {
        let now = null;
        try { now = await invoke('get_order', { id: orderId }); } catch (e) {
          if (String(e?.message ?? e) === 'please log in') { forgetSession(); start(); return; }
        }
        if (state.orderId !== orderId) return;
        if (!now || now.status !== 'open' || now.id !== orderId) { leaveDeadOrder(); return; }
        order = now;
        draw();
        toast(`${it.name} was changed on another till: nothing saved`, true);
      };
      if (newPrice !== it.unit_price_cents) {
        const args = { orderId, itemId: it.id, priceCents: newPrice };
        try {
          order = await invoke('set_item_price', args);
          said.push(`${it.name}: ${money(newPrice)} each`);
        } catch (e) {
          const msg = String(e?.message ?? e);
          if (gone(msg)) return;
          if (msg === 'not found') return dishGone();
          if (/needs a manager/i.test(msg)) {
            const ok = await pinDialog({
              title: 'Manager PIN', hint: `${it.name}: ${money(it.unit_price_cents)} → ${money(newPrice)}. A manager or admin enters their PIN to approve the new price.`, okLabel: 'Approve price',
              submit: async (typed) => {
                try { order = await invoke('set_item_price', { ...args, pin: typed }); pin = typed; } catch (e2) {
                  const m = String(e2?.message ?? e2);
                  if (m === 'please log in' || m === ORDER_CLOSED || SWEPT.test(m)) { setTimeout(() => gone(m)); return m === 'please log in' ? 'Logged out: log in again' : 'This order is closed'; }
                  // The dish (or the order) went while the PIN was typed: close this and look.
                  if (m === 'not found') { vanished = true; setTimeout(closeModal); return 'Changed on another till'; }
                  return /invalid pin/i.test(m) ? 'Wrong PIN, try again' : /needs a manager/i.test(m) ? 'That PIN cannot approve it: a manager or admin PIN is needed' : m;
                }
              },
            });
            if (!state.session || state.orderId !== orderId) return;
            if (vanished) return dishGone();
            if (ok) said.push(`${it.name}: ${money(newPrice)} each`);
            else { priceFailed = true; said.push('price not changed (no manager PIN)'); }
          } else { priceFailed = true; said.push(`price not changed: ${msg}`); }
        }
        draw();
      }
      // The discount as typed, on the price the dish has now (a percentage follows it).
      const now = order.items.find((i) => i.id === it.id) ?? it;
      const cents = discFor(now.unit_price_cents);
      if (qty !== now.qty || cents !== now.discount_cents || newNote !== (now.note ?? null)) {
        // Only the note: it never conflicts with a change of quantity on another till,
        // so it goes on the dish as it is now (qtyDelta 0), not checked against this screen.
        const noteOnly = qty === now.qty && cents === now.discount_cents;
        try {
          await setLine(now, { qty, note: newNote, discount_cents: cents }, pin, { noteOnly });
        } catch (e) {
          if (!state.session || state.orderId !== orderId) return;
          // Changed on another till: the dish pop-up opens again on it as it is now,
          // with the note as typed, to save again.
          const fresh = order.items.find((i) => i.id === it.id);
          if (ITEM_CHANGED.test(String(e?.message ?? e)) && fresh && newNote !== (fresh.note ?? null)) openLineEditor(fresh, { note: newNote ?? '' });
          // setLine has shown its own error; add what did or did not happen to the price.
          else if (said.length) toast(`${said.join(' · ')} · ${e?.message ?? e}`, true);
          return;
        }
        if (sent && qty < it.qty) said.push(`${it.name} −${it.qty - qty} · CANCELLED ticket sent to the kitchen`);
        else if (qty !== now.qty) said.push(`${it.name} × ${qty}`);
        if (sent && newNote !== (it.note ?? null)) said.push('note changed · CHANGED ticket sent to the kitchen');
        else if (newNote !== (now.note ?? null)) said.push(newNote ? `note saved: “${newNote}”` : 'note removed');
        if (cents !== now.discount_cents) said.push(cents ? `−${money(cents)} discount` : 'discount removed');
      }
      if (said.length) toast(said.join(' · ').replace(/^./, (c) => c.toUpperCase()), priceFailed);
    });
    for (const el of [price, note, disc]) el.addEventListener('keydown', (e) => { if (e.key === 'Enter') save(e); });
    const remove = h('button', { class: 'danger', onclick: act(async () => {
      closeModal();
      await setLine(it, { qty: 0 });
      toast(sent ? `${it.name} removed · CANCELLED ticket sent to the kitchen` : `${it.name} removed`);
    }) }, icon('x', 18), 'Remove');
    showModal(
      // The dish's name, × and the kitchen warning stay at the top while the rest scrolls (on-screen keyboard open).
      h('div', { class: 'edit-top' },
        h('div', { class: 'edit-head' },
          h('div', { class: 'grow' }, h('h3', {}, it.name), it.name_alt && h('div', { class: 'alt', lang: 'th' }, it.name_alt),
            it.options.length > 0 && h('div', { class: 'muted small' }, it.options.map((o) => o.name).join(', '))),
          h('button', { class: 'icon-btn ghost', 'aria-label': 'Close', title: 'Close (Esc)', onclick: closeModal }, icon('x'))),
        sent && h('div', { class: 'warn-box' }, 'Already in the kitchen. Fewer or Remove prints a CANCELLED ticket; a new note prints a CHANGED ticket.')),
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
            await setLine(it, { qty: it.qty - 1 }, null, { delta: -1 });
            if (sent) toast(`${it.name} −1 · CANCELLED ticket sent to the kitchen`);
          }) }, '−'),
          h('span', {}, it.qty),
          h('button', { onclick: act(() => (sent
            ? (it.is_set ? openSetPicker(state.menu.find((p) => p.id === it.product_id) ?? { id: it.product_id, name: it.name, price_cents: it.unit_price_cents })
              : addItem(it.product_id, it.options.map((o) => o.option_id), it.variant_id, null, it.team_id ?? null))
            : setLine(it, { qty: it.qty + 1 }, null, { delta: 1 }))) }, '+'))),
        h('td', { class: 'num' }, money(it.line_total_cents)),
        h('td', {}, h('button', { class: 'icon-btn edit-btn', title: 'Change: how many, price, note, discount, remove', 'aria-label': `Change ${it.name}`,
          onclick: toggleEdit }, '✎')));
      return row;
    })));
    if (!order.items.length) lines.append(h('div', { class: 'empty small' }, icon('orders', 36), h('p', {}, 'Tap menu items to add them')));
    setKids(totals, 
      // Subtotal and discount: the rows a short till drops first (style.css), the total and tax stay.
      h('tr', { class: 'sub-row' }, h('td', {}, 'Subtotal'), h('td', { class: 'num' }, money(order.subtotal_cents))),
      order.discount_cents ? h('tr', { class: 'sub-row' }, h('td', {}, 'Discount'), h('td', { class: 'num' }, '-' + money(order.discount_cents))) : '',
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
    // "All courses together" only matters with starters still to send or mains waiting.
    coursesSwitch.hidden = !(starters || waiting);
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
    // Onto a bill with a lower service charge or tax (the dishes take its rate): staff
    // need a manager's PIN (orders.rs move_approval); the PIN pad takes this dialog's place.
    const moveCall = async (cmd, args) => {
      const r = await managerRetry(cmd, args, {
        hint: `${target.name}'s bill has a lower service charge or tax: the dishes would take it. A manager or admin enters their PIN to approve.`,
        okLabel: 'Approve move' });
      if (r?.declined) toast('Not moved (no manager PIN)', true);
      return r?.result ?? null;
    };
    ok.onclick = act(async () => {
      if (!some) {
        const o = await moveCall('merge_into_table', { orderId, tableId: target.id });
        if (!o) return;
        // The answer says what happened (the table may have got a bill meanwhile).
        return goTo(o, o.id !== orderId ? `Joined onto ${o.table_name}: one bill` : `Moved to ${o.table_name}`);
      }
      const all = bill.every((i) => (pick.get(i.id) ?? 0) >= i.qty);
      const o = await moveCall('move_items', { orderId, tableId: target.id, items: [...pick.entries()] });
      if (!o) return;
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
    const was = f.value();
    setKids(extra, h('div', { class: 'stack card' }, f.el, h('div', { class: 'row' },
      h('button', { class: 'primary', onclick: act(async () => {
        // Only the details changed here: another till's edit of the others stays.
        const details = f.value();
        const fields = Object.keys(details).filter((k) => details[k] !== was[k]);
        if (!fields.length) { setKids(extra); return; }
        order = await call('update_order_details', { id: orderId, details, fields });
        setKids(extra); header(); toast('Saved');
      }) }, 'Save'),
      h('button', { onclick: () => setKids(extra) }, 'Close'))));
  };
  // Service charge on this order only: remove it, pick a rate or type one.
  const showService = () => {
    const presets = [...new Set([state.service?.rate_bp || 0, 1000, 1250, 1500].filter((b) => b > 0))].sort((a, b) => a - b);
    const custom = h('input', { inputmode: 'decimal', placeholder: '%', style: 'width:80px', value: order.service_bp ? pct(order.service_bp) : '' });
    const set = (bp) => act(async () => {
      // Removing or lowering it: staff need a manager's PIN.
      const r = await managerRetry('set_service_charge', { orderId, rateBp: bp }, {
        hint: `${state.service?.label || 'Service charge'}: ${order.service_bp ? `${pct(order.service_bp)}%` : 'none'} → ${bp ? `${pct(bp)}%` : 'none'}. A manager or admin enters their PIN to approve.`,
        okLabel: bp ? `Approve ${pct(bp)}%` : 'Remove service charge' });
      if (!r) return;
      if (r.declined) return toast('Service charge not changed (no manager PIN)', true);
      order = r.result;
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
      // A rate for the whole order, or "Menu rates" when that lowers the tax: staff need a manager's PIN.
      const label = (v) => (v == null ? 'menu rates' : `${pct(v)}%`);
      const r = await managerRetry('set_order_tax', { orderId, rateBp: bp }, {
        hint: `Tax on this order: ${label(order.tax_bp)} → ${label(bp)}. A manager or admin enters their PIN to approve.`,
        okLabel: `Approve ${label(bp)}` });
      if (!r) return;
      if (r.declined) return toast('Tax not changed (no manager PIN)', true);
      order = r.result;
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
    const sent = order.items.some((i) => i.sent_at || i.prep_sent_at);
    // Nothing sent and nothing paid (a mistake order): anyone confirms, with no PIN.
    const mistake = !sent && !order.payments?.length;
    const noPin = isManager() || mistake;
    // Settings → Orders → Cancelling an order: CANCELLED ticket on (default) or off.
    const prints = await invoke('get_setting', { key: 'print_cancelled_order' }).then((v) => v !== '0').catch(() => true);
    const kitchenNote = prints ? 'A CANCELLED ticket is printed for any dishes still cooking.' : 'Nothing is printed for the kitchen.';
    let r;
    const submit = async (pin) => {
      try { r = await invoke('cancel_order', { orderId, reason: '', pin }); } catch (e) {
        const msg = String(e?.message ?? e);
        if (isDeadOrder(msg, { orderId })) { setTimeout(() => leaveDeadOrder(msg)); return 'This order is closed'; }
        if (/permission/i.test(msg)) return isManager() ? 'You can no longer cancel without a PIN: log in again'
          : mistake ? 'This order was sent or paid on another till: close this and cancel it again with a PIN' : sent ? 'This order is already in the kitchen: a manager or admin PIN is needed' : 'A PIN is needed';
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
      h('button', { class: 'icon-btn back-ticket', onclick: act(async () => { if (await askUnsent()) closeOrder(); }), title: 'Back', 'aria-label': 'Back' }, icon('back')),
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
        order.order_type === 'dine_in' && coursesSwitch,
        h('div', { class: 'row nowrap send-row' }, sendBtn, mainsBtn),
        payBtn)),
    phoneBar);
  screen(layout);
  draw();
  if (!matchMedia('(pointer: coarse)').matches) search.focus();
  // Leaving with dishes not yet sent (Back, the side bar, Log out): "Send N dishes to the
  // kitchen?". Auto logout and a bill that closes do not ask (paying sends them).
  state.unsentAsk = async () => {
    if (state.orderId !== orderId || !layout.isConnected || order.status !== 'open') return true;
    const waiting = order.items.filter((i) => !i.is_set && !i.sent_at && !i.held);
    const n = waiting.reduce((x, i) => x + i.qty, 0);
    if (!n) return true;
    return new Promise((resolve) => {
      const done = (v) => { state.onModalClose = null; closeModal(); resolve(v); };
      showModal(h('h3', {}, `Send ${n} dish${n === 1 ? '' : 'es'} to the kitchen?`),
        h('div', { class: 'stack confirm-hint' }, `${orderTitle(order)}: ${n === 1 ? 'it has' : 'they have'} not been sent yet.`,
          n <= 3 && h('ul', { class: 'unsent-list' }, waiting.map((i) => h('li', {}, `${i.qty}× ${i.name}`)))),
        h('button', { class: 'send ready paybig', onclick: async () => {
          state.onModalClose = null; closeModal();
          try { await send(allCourses.checked); resolve(true); } catch (e) { if (!e?.handled) toast(String(e?.message ?? e), true); resolve(false); }
        } }, 'Send to kitchen'),
        h('div', { class: 'row unsent-acts' }, h('button', { onclick: () => done(false) }, 'Stay here'), h('span', { class: 'grow' }),
          h('button', { class: 'danger-outline', onclick: () => done(true) }, 'Leave unsent')));
      state.onModalClose = () => resolve(false);
      document.removeEventListener('keydown', state.modalKeys);
      state.modalKeys = (e) => { if (e.key === 'Escape') closeModal(); };
      document.addEventListener('keydown', state.modalKeys);
      document.querySelector('#modal .send')?.focus();
    });
  };
  // Changes made on other tills show here within a few seconds: every 5 s, while
  // nothing is open on top (a dialog, a picker, the details card) and no change of
  // this screen is on its way.
  live(async () => {
    if (state.orderId !== orderId || !layout.isConnected || document.hidden) return;
    if (!document.getElementById('modal').hidden || !picker.hidden || extra.childElementCount) return;
    const asked = changes;
    let now;
    try { now = await invoke('get_order', { id: orderId }); } catch (e) {
      const m = String(e?.message ?? e);
      if (m === 'please log in') { forgetSession(); start(); } else if (isDeadOrder(m, { id: orderId }, 'get_order')) leaveDeadOrder(m);
      return;
    }
    if (asked !== changes || state.orderId !== orderId || !layout.isConnected || !document.getElementById('modal').hidden) return;
    if (now.status !== 'open') return leaveDeadOrder(ORDER_CLOSED);
    if (JSON.stringify(now) === JSON.stringify(order)) return;
    const moved = now.table_id !== order.table_id;
    order = now;
    header();
    draw();
    if (moved) toast(`Moved to ${orderTitle(now)}`);
  }, 5000);
}

/// Before leaving the order on screen: true to go (see `state.unsentAsk`).
async function askUnsent() {
  const ask = state.unsentAsk;
  if (!ask || state.orderId == null) return true;
  const ok = await ask();
  if (ok) state.unsentAsk = null;
  return ok;
}

// ---------- payment dialog ----------

function closeModal() {
  const m = document.getElementById('modal');
  m.hidden = true;
  setKids(m);
  // A settings sheet (see sheet()) dresses the same host.
  m.removeAttribute('class');
  m.onclick = null;
  placeToast();
  state.sheetBack = null;
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

/// A change staff may make only with a manager's or admin's PIN (taking a payment
/// back, lowering the service charge, the tax on an order). Tries it; when the
/// server asks for a manager, the PIN pad asks and it is tried again with that PIN
/// (the PIN only approves: nobody is logged in or out). Gives { result } (with
/// `approved` after a PIN), { notFound }, { declined } (PIN pad closed), or null
/// when it failed for another reason (already shown, as call() does).
const pinError = (m) => (/invalid pin/i.test(m) ? 'Wrong PIN, try again' : /needs a manager/i.test(m) ? 'That PIN cannot approve it: a manager or admin PIN is needed' : m);
async function managerRetry(cmd, args, { hint, okLabel, before = null }) {
  // Only a payment can be gone while its order is open; for the order itself "not found" means it is gone.
  const missing = (m) => m === 'not found' && cmd === 'remove_payment';
  try { return { result: await invoke(cmd, args) }; } catch (e) {
    const msg = String(e?.message ?? e);
    if (missing(msg)) return { notFound: true };
    if (!/needs a manager/i.test(msg)) {
      if (msg === 'please log in') { forgetSession(); start(); }
      else if (isDeadOrder(msg, args, cmd) || await deniedForDeadOrder(msg, args, cmd)) leaveDeadOrder(msg);
      else toast(msg, true);
      return null;
    }
  }
  before?.();
  let result = null, notFound = false, gone = false;
  const ok = await pinDialog({ title: 'Manager PIN', hint, okLabel,
    submit: async (pin) => {
      try { result = await invoke(cmd, { ...args, pin }); } catch (e2) {
        const m = String(e2?.message ?? e2);
        if (m === 'please log in') { gone = true; setTimeout(() => { closeModal(); forgetSession(); start(); }); return 'Logged out: log in again'; }
        if (missing(m)) { notFound = true; setTimeout(closeModal); return 'Changed on another till'; }
        if (isDeadOrder(m, args, cmd)) { gone = true; setTimeout(() => { closeModal(); leaveDeadOrder(m); }); return 'This order is closed'; }
        return pinError(m);
      }
    } });
  if (gone) return null;
  if (notFound) return { notFound: true };
  return ok ? { result, approved: true } : { declined: true };
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
      ok, h('div', { class: 'row' }, h('span', { class: 'grow' }), h('button', { class: 'pin-keep', onclick: closeModal }, keepLabel)));
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
  let rightMin = 0;
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
  /// Nothing left to pay (every dish free or fully discounted, or already covered): one button closes it.
  const nothingToPay = () => left() === 0 && order.items.length > 0;
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

  // One key per payment: a retry of the same payment after a lost answer keeps it.
  let pending = null;
  // The message strip above Confirm ({ kind: 'ok' | 'warn' | 'bad', text }), until the next Confirm.
  let note = null;
  // The answer was lost: Confirm shows a spinner while the main till is asked.
  let checking = false;
  const take = async () => {
    if (busy) return;
    const d = due();
    const nothing = nothingToPay();
    if (d <= 0 && !nothing) return toast(mode === 'dishes' ? 'Tap the dishes this person pays for' : 'Enter an amount to pay', true);
    if (method === 'cash' && (cash() ?? 0) < target()) return toast(`Not enough cash: ${money(target())} to pay`, true);
    busy = true;
    note = null;
    try {
      // The amount shown is the amount sent; for dishes the server checks it is their share now.
      const input = nothing ? { method: 'cash', tendered_cents: 0, tip_cents: 0, amount_cents: null, items: null }
        : { method, tendered_cents: method === 'cash' ? cash() : 0, tip_cents: tip(),
          amount_cents: d, items: mode === 'dishes' ? [...pick.entries()] : null };
      const before = order.payments?.length ?? 0;
      const sig = JSON.stringify([order.id, before, left(), input]);
      if (pending?.sig !== sig) pending = { sig, key: onceKey() };
      const res = await pay(input, { expectedLeftCents: left(), idempotencyKey: pending.key, closes: nothing || d >= left(), before,
        checking: () => { checking = true; draw(); } });
      checking = false;
      pending = null;
      if (res.status !== 'paid' && res.payments.length > before) {
        note = { kind: 'ok', text: `Paid ${money(res.payments.at(-1).amount_cents)} · ${money(Math.max(0, res.total_cents - res.paid_cents))} left to pay` };
      }
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
      checking = false;
      const msg = String(e?.message ?? e);
      // Another till changed the bill since this dialog opened: nothing was taken.
      // Show the bill as it is now, to check and confirm again.
      if (BILL_CHANGED.test(msg)) {
        const was = left(), wasDue = due();
        try { order = await invoke('get_order', { id: order.id }); } catch { /* shown as it was */ }
        if (order.status !== 'open') { state.onModalClose = null; closeModal(); reload(); return; }
        if (mode === 'even' && even.done) even = { n: even.n, done: 0 };
        typed.amount = ''; typed.cash = '';
        if (mode === 'dishes') {
          // The same dishes stay picked where they still can be: their share now shows.
          for (const [id, q] of [...pick]) {
            const it = bill().find((i) => i.id === id);
            const n = it ? Math.min(q, it.qty - it.paid_qty) : 0;
            if (n > 0) pick.set(id, n); else pick.delete(id);
          }
          // Their share the same (another dish changed): say what is left to pay now instead.
          note = { kind: 'warn', text: !pick.size ? `Bill changed on another till: those dishes are no longer on it (was ${money(wasDue)}). Nothing taken.`
            : share() !== wasDue ? `Bill changed on another till: these dishes now come to ${money(share())} (was ${money(wasDue)}). Nothing taken.`
              : `Bill changed on another till: now ${money(left())} left to pay (was ${money(was)}). Nothing taken. Check and tap Confirm again.` };
        } else {
          pick.clear();
          note = { kind: 'warn', text: `Bill changed on another till: now ${money(left())} (was ${money(was)}). Nothing taken.` };
        }
        draw(); state.onModalClose = reload;
        return;
      }
      // Logged out or the order closed elsewhere: call() already left the dialog.
      if (!state.session || document.getElementById('modal').hidden) return;
      note = e?.note ?? (e?.redraw || msg === 'not paid' ? null : { kind: 'bad', text: `Not paid: ${msg}` });
      // Checked after a lost answer: the dialog shows the payments as they are now.
      if (e?.order) order = e.order;
      draw(); state.onModalClose = reload;
    }
  };

  const strip = () => note && h('div', { class: `pay-note ${note.kind}`, role: note.kind === 'ok' ? 'status' : 'alert' }, note.text);
  function draw() {
    if (nothingToPay()) {
      showModal(
        h('div', { class: 'pay-head' },
          h('div', { class: 'grow' }, h('h3', {}, orderTitle(order)), h('div', { class: 'muted small' }, `Order #${order.id}`)),
          h('div', { class: 'pay-total' }, h('span', { class: 'muted' }, 'Total'), h('span', { class: 'big' }, money(order.total_cents))),
          h('button', { class: 'icon-btn ghost', title: 'Close (Esc)', 'aria-label': 'Close', onclick: closeModal }, icon('x'))),
        h('p', { class: 'muted' }, order.paid_cents ? 'The payments already cover this bill.' : 'Nothing to pay on this bill (every dish is free or fully discounted).'),
        strip(),
        h('button', { class: 'primary paybig pay-ok', disabled: checking, onclick: take }, checking ? [h('span', { class: 'spin', 'aria-hidden': 'true' }), ' Checking…'] : 'Close bill (nothing to pay)'));
      setKeys((e) => { if (e.key === 'Enter') take(); else if (e.key === 'Escape') closeModal(); });
      return;
    }
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
      : method === 'card' ? `Confirm ${payName('card') === 'Card' ? 'card' : payName('card')} payment · ${money(tg)}` : `Paid by ${order.channel_name} · ${money(tg)}`;
    // One clear total; the amount and the tip are listed above it only when there is a tip.
    const sum = (where) => h('div', { class: `pay-sum ${where}` },
      t > 0 && h('div', { class: 'row' }, h('span', { style: 'flex:1' }, 'To pay now'), h('strong', {}, money(d))),
      t > 0 && h('div', { class: 'row' }, h('span', { style: 'flex:1' }, 'Tip'), h('strong', {}, money(t))),
      h('div', { class: 'row grand' }, h('span', { style: 'flex:1' }, t > 0 ? 'Total to take' : 'To pay now'), h('strong', {}, money(tg))));
    // Each tap draws the dialog again: keep where it was scrolled to (phones).
    const scrolled = document.querySelector('#modal .pay-one')?.scrollTop ?? 0;
    // The header (close) and the footer (the total and Take) stay in view;
    // only the part between them scrolls when the screen is short.
    showModal(
      h('div', { class: 'pay-head' },
        h('div', { class: 'grow' }, h('h3', {}, orderTitle(order)),
          h('div', { class: 'muted small' }, [`Order #${order.id}`, orderTitle(order) !== TYPE[order.order_type] && TYPE[order.order_type]].filter(Boolean).join(' · '))),
        h('div', { class: 'pay-total' }, h('span', { class: 'muted' }, 'Total'), h('span', { class: 'big' }, money(order.total_cents))),
        h('button', { class: 'icon-btn ghost', title: 'Close (Esc)', 'aria-label': 'Close', onclick: closeModal }, icon('x'))),
      h('div', { class: 'pay-one' },
        h('div', { class: 'pay-left stack' },
          // The payments so far scroll in their own list; "Left to pay" under them always shows.
          order.paid_cents > 0 && h('div', { class: 'pay-parts' }, h('div', { class: 'pay-part-list' },
            order.payments.map((p) => h('div', { class: 'row nowrap pay-part' },
              h('span', { style: 'flex:1' }, `✓ ${payName(p.method, order.channel_name)} ${money(p.amount_cents)}`,
                p.tip_cents ? h('span', { class: 'muted' }, ` + tip ${money(p.tip_cents)}`) : ''),
              h('button', { class: 'link-btn', title: 'Take this payment back (a mistake)', onclick: act(() => undoPayment(p)) }, 'undo')))),
            h('div', { class: 'row nowrap pay-left-row' }, h('strong', { style: 'flex:1' }, 'Left to pay'), h('span', { class: 'big' }, money(left())))),
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
          sum('in-body')),
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
          keypadOn && h('div', { class: 'keypad' }, ['7', '8', '9', '4', '5', '6', '1', '2', '3', '.', '0', '⌫'].map((k) => h('button', { onclick: () => key(k) }, k))))),
      h('div', { class: 'pay-foot' }, sum('in-foot'), strip(),
        h('button', { class: 'primary paybig pay-ok', disabled: checking || d <= 0 || !enough, onclick: take },
          checking ? [h('span', { class: 'spin', 'aria-hidden': 'true' }), ' Checking…'] : okLabel)));
    document.querySelector('#modal .dialog')?.classList.add('pay-wide');
    const body = document.querySelector('#modal .pay-one');
    // Phones: the method side keeps the height Cash needed, so switching to the
    // shorter Card does not pull the scrolled page (and the buttons) down.
    const right = document.querySelector('#modal .pay-right');
    if (right && matchMedia('(max-width: 760px)').matches) {
      rightMin = Math.max(rightMin, right.offsetHeight);
      right.style.minHeight = `${rightMin}px`;
    }
    if (body) body.scrollTop = scrolled;
    // Many part payments in a short list: the latest one shows.
    const parts = document.querySelector('#modal .pay-part-list');
    if (parts) parts.scrollTop = parts.scrollHeight;
    setKeys((e) => {
      if (/^[0-9.]$/.test(e.key)) key(e.key);
      else if (e.key === 'Backspace') key('⌫');
      else if (e.key === 'Enter') take();
      else if (e.key === 'Escape') closeModal();
    });
  }
  // Undo a payment: staff need a manager's PIN. The PIN pad takes the dialog's
  // place for a moment, then the pay dialog comes back.
  async function undoPayment(p) {
    const id = order.id;
    const r = await managerRetry('remove_payment', { orderId: id, paymentId: p.id }, {
      hint: `Take back ${payName(p.method, order.channel_name)} ${money(p.amount_cents)}? A manager or admin enters their PIN to approve.`,
      okLabel: 'Take payment back',
      before: () => { state.onModalClose = null; } });
    if (!state.session || document.body.classList.contains('locked')) return;
    if (!r) { if (document.getElementById('modal').hidden && state.orderId === id) { draw(); state.onModalClose = reload; } return; }
    // Already gone (taken back on another till): show the order as it is now.
    if (r.notFound) {
      order = await call('get_order', { id });
      toast('That payment was already taken back', true);
    } else if (r.declined) toast('Payment not taken back (no manager PIN)', true);
    else { order = r.result; if (r.approved) toast('Payment taken back'); }
    mode = 'all'; even.done = 0;
    draw();
    state.onModalClose = reload;
    reload();
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
/// `rows`: the fields as labelled setting rows (the Customers page), not a compact form.
function customerEditor({ type = 'delivery', customer = null, search = true, saveToggle = true, onPick = () => {}, rows = false } = {}) {
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
  if (rows) {
    // Labelled rows: the label says what the box is, so the boxes keep short hints and one width.
    const hints = { name: 'e.g. Somchai', phone: 'e.g. 081 234 5678', line1: 'House number and street', line2: 'Optional', town: '', postcode: 'e.g. SW1A 1AA', notes: 'e.g. gate code' };
    const labels = { name: 'Name', phone: 'Phone', postcode: 'Postcode', line1: 'Address', line2: 'Flat, building', town: 'Town', notes: 'Notes' };
    for (const [k, el] of Object.entries(f)) { el.removeAttribute('style'); el.placeholder = hints[k]; el.setAttribute('aria-label', labels[k]); }
    f.postcode.classList.add('upper');
  }

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

  const fieldRow = (k, hint) => srow({ id: `c_${k}`, label: { name: 'Name', phone: 'Phone', postcode: 'Postcode', line1: 'Address', line2: 'Flat, building', town: 'Town', notes: 'Notes' }[k], hint, cls: 'text-row', ctl: f[k] });
  return {
    el: rows ? h('div', { class: 'cust-rows' },
      ssec(null, [fieldRow('name'), fieldRow('phone')]),
      delivery && ssec('Address', [fieldRow('postcode'), h('div', { class: 'srow-strip cust-sugg' }, suggestions), fieldRow('line1'), fieldRow('line2'), fieldRow('town')]),
      ssec(null, [fieldRow('notes', 'Saved with the customer')]))
    : h('div', { class: 'stack' },
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
  const paid = o.status === 'paid';
  const printBtn = (label, fn) => h('button', { onclick: act(fn) }, icon('printer', 18), label);
  // Void (a refund) and Delete each ask in a sheet, with the reason.
  const voidSheet = () => {
    const reason = h('input', { placeholder: 'e.g. wrong dish, card refunded', 'aria-label': 'Reason', maxlength: 120 });
    sheet({ title: `Void #${o.id}?`, danger: true, saveLabel: 'Void / refund',
      body: [ssec(null, [srow({ label: 'Reason', cls: 'text-row', ctl: reason })], `${money(o.total_cents)} comes off the day's sales.`)],
      onSave: async () => {
        await call('void_order', { orderId: o.id, reason: reason.value }, { quiet: true });
        toast('Order voided');
        onChange();
      } });
  };
  const deleteSheet = () => {
    const reason = h('input', { placeholder: 'Optional', 'aria-label': 'Reason', maxlength: 120 });
    sheet({ title: `Delete #${o.id}?`, danger: true, saveLabel: 'Delete for good',
      body: [h('div', { class: 'warn-box' }, `Removes it from history and reports.${paid ? ' Stock is put back. To refund a real sale, use Void.' : ''}`),
        ssec(null, [srow({ label: 'Reason', cls: 'text-row', ctl: reason })])],
      onSave: async () => {
        await call('delete_order', { orderId: o.id, reason: reason.value || null }, { quiet: true });
        toast(`Order #${o.id} deleted`);
        onChange('deleted');
      } });
  };
  const opened = `${friendlyTime(o.opened_at)} · ${o.opened_by_name}`;
  return h('div', { class: 'rcpt' },
    h('div', { class: 'rcpt-head' },
      h('div', { class: 'rcpt-title' }, h('h3', {}, `#${o.id}`), typeChip(o.order_type), statusPill(o.status)),
      isAdmin() && moreMenu(`#${o.id}`, () => [h('button', { class: 'danger', onclick: deleteSheet }, 'Delete order…')])),
    h('div', { class: 'rcpt-meta' },
      where(o) && h('div', { class: 'rcpt-where' }, where(o)),
      h('div', {}, `Opened ${opened}${o.paid_at ? ` · paid ${friendlyTime(o.paid_at).replace(/^today /, '')}` : ''}`),
      // where() names the customer on all but dine-in orders.
      (o.customer_phone || o.delivery_address || (o.order_type === 'dine_in' && o.customer_name)) && h('div', {},
        [o.order_type === 'dine_in' && o.customer_name, o.customer_phone, o.delivery_address].filter(Boolean).join(' · '))),
    orderWho(o),
    h('div', { class: 'rcpt-lines' }, o.items.map((i) => h('div', { class: 'rline' },
      h('span', { class: 'rq' }, `${i.qty}×`),
      h('span', { class: 'rn' }, i.name,
        i.options.length > 0 && h('small', {}, i.options.map((x) => x.name).join(', ')),
        i.note && h('small', {}, i.note),
        (i.qty > 1 || i.discount_cents > 0) && h('small', {}, [i.qty > 1 && `${money(i.unit_price_cents)} each`, i.discount_cents > 0 && `−${money(i.discount_cents)}`].filter(Boolean).join(' · '))),
      // Before its discount, as on the printed receipt (the discount is under it and in the sum).
      h('span', { class: 'rt' }, money(i.unit_price_cents * i.qty)))),
    !o.items.length && h('div', { class: 'muted rline-none' }, 'No dishes')),
    // As on the printed receipt: subtotal, discount, tax, service charge, total.
    h('div', { class: 'rcpt-sum' },
      o.discount_cents > 0 && h('div', {}, h('span', {}, 'Subtotal'), h('span', {}, money(o.subtotal_cents))),
      o.discount_cents > 0 && h('div', {}, h('span', {}, 'Discount'), h('span', {}, '−' + money(o.discount_cents))),
      h('div', {}, h('span', {}, o.tax_incl ? 'Tax included' : 'Tax'), h('span', {}, money(o.tax_cents))),
      o.service_cents > 0 && h('div', {}, h('span', {}, `${state.service?.label || 'Service charge'} ${pct(o.service_bp)}%`), h('span', {}, money(o.service_cents))),
      h('div', { class: 'grand' }, h('span', {}, 'Total'), h('span', {}, money(o.total_cents))),
      // On top of the total, as on the receipt (with the payment).
      o.tip_cents > 0 && h('div', {}, h('span', {}, 'Tip'), h('span', {}, money(o.tip_cents)))),
    o.payments?.length > 0 && h('div', { class: 'rcpt-pays' }, o.payments.map((p) => h('div', {},
      h('span', {}, payName(p.method, o.channel_name), h('small', {}, [p.paid_by_name, p.tip_cents && `tip ${money(p.tip_cents)}`,
        p.change_cents && `paid ${money(p.tendered_cents)}, change ${money(p.change_cents)}`].filter(Boolean).join(' · '))),
      h('span', {}, money(p.amount_cents))))),
    o.close_reason && h('div', { class: 'rcpt-reason' }, `Reason: ${o.close_reason}`),
    orderActivity(o),
    o.status !== 'cancelled' && h('div', { class: 'rcpt-foot' },
      printBtn(paid ? 'Reprint receipt' : 'Print', async () => printResult(await call('print_receipt', { orderId: o.id }), paid ? 'Receipt printed' : 'Printed')),
      paid && printBtn('Staff copy', async () => printResult(await call('print_staff_copy', { orderId: o.id }), 'Staff copy printed')),
      paid && printBtn('Kitchen ticket', async () => {
        const p = await call('reprint_kitchen', { orderId: o.id });
        if (p.error) toast(`Did not print: ${p.error}`, true);
        else toast(p.attempted ? 'Kitchen ticket reprinted' : 'No kitchen printer is set up', !p.attempted);
      }),
      isManager() && paid && h('button', { class: 'danger-outline', onclick: voidSheet }, 'Void / refund')));
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
  // Quick dates follow the business day (header), which runs from the last
  // day close: "Today" is the open business day, so orders after midnight
  // before the day is closed are still today. "Yesterday" is the business
  // day closed last, "7 days" the business days dated in the last week.
  // A picked day is a calendar day (midnight to midnight).
  let mode = 'today'; // today | yesterday | week | day
  let day = null;
  let picked = today();
  const OPEN_END = 4102444800; // 2100: "up to now", whatever this device's clock says
  const daysAgo = (n) => { const d = new Date(); d.setDate(d.getDate() - n); return d; };
  const list = h('div', { class: 'olist', role: 'list', 'aria-label': 'Orders' });
  const span = h('div', { class: 'md-span' });
  const more = h('div', { class: 'srow-strip', hidden: true });
  const find = h('input', { type: 'search', placeholder: 'Order # or table', 'aria-label': 'Find an order', enterkeyhint: 'search' });
  const detail = h('div', { class: 'md-body' });
  const empty = () => setKids(detail, h('div', { class: 'sgroup md-empty' }, emptyState({ icon: 'history', title: 'Tap an order to see it' })));
  let shown = [];
  let openId = null;
  const dayBtn = h('button', { class: 'daybtn', 'aria-label': 'Pick a day', onclick: () => pickDay() });
  const seg = sseg([['today', 'Today'], ['yesterday', 'Yesterday'], ['week', '7 days']], mode, (v) => { mode = v; load(); }, 'Dates');
  const drawDayBtn = () => {
    dayBtn.classList.toggle('on', mode === 'day');
    setKids(dayBtn, icon('calendar', 20), mode === 'day' && h('span', {}, dayLabel(picked)));
    if (mode !== 'day') seg.set(mode); else seg.set(null);
  };
  // Pick a day: a small sheet with the date (a calendar day, midnight to midnight).
  function pickDay() {
    const date = h('input', { type: 'date', value: picked, max: today(), 'aria-label': 'Day' });
    const s = sheet({ title: 'Show one day', saveLabel: 'Show', body: ssec(null, [srow({ label: 'Day', ctl: date, cls: 'text-row' })]), onSave: () => {
      if (!date.value) return 'Pick a day';
      picked = date.value;
      mode = 'day';
      load();
    } });
    date.addEventListener('keydown', (e) => { if (e.key === 'Enter') s.save.click(); });
  }
  let loads = 0; // a newer tap wins over a slow earlier load
  const load = act(async () => {
    drawDayBtn();
    const mine = ++loads;
    if (mode !== 'day') {
      day = await call('business_day');
      if (mine !== loads) return;
    }
    // Not closed for a long time (or only once, long ago): business days
    // would mean weeks, so the quick dates fall back to calendar days.
    const stale = mode !== 'day' && Date.now() / 1000 - day.since > 36 * 3600;
    const date = mode === 'day' ? picked : stale ? isoDate(daysAgo(mode === 'yesterday' ? 1 : 0))
      : mode === 'yesterday' ? (day.prev_date ?? isoDate(daysAgo(1))) : day.date;
    const [from, to] = stale
      ? (mode === 'week' ? [dayRange(isoDate(daysAgo(6)))[0], OPEN_END] : mode === 'today' ? [dayRange(date)[0], OPEN_END] : dayRange(date))
      : { today: () => [day.since, OPEN_END], yesterday: () => [day.prev_since, day.since],
        week: () => [day.week_since, OPEN_END], day: () => dayRange(date) }[mode]();
    span.textContent = stale ? `Day not closed since ${friendlyTime(day.since)}: calendar days` : {
      today: () => `${dayLabel(day.date)} · since ${friendlyTime(day.since)}`,
      yesterday: () => `${day.prev_date ? dayLabel(day.prev_date) : 'Previous day'} · ${friendlyTime(day.prev_since)} to ${friendlyTime(day.since)}`,
      week: () => `Since ${friendlyTime(day.week_since)}`,
      day: () => `${dayLabel(date)} · midnight to midnight`,
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
    const over = orders.length >= MAX && (await call('list_orders', { from, to, limit: 1, offset: MAX })).length > 0;
    if (mine !== loads) return;
    shown = orders;
    more.hidden = !over;
    more.textContent = over ? `Showing the latest ${MAX}. Pick one day for the rest.` : '';
    // Orders from more than one calendar day: the date goes with the time.
    const end = Math.min(to, Date.now() / 1000);
    severalDays = mode !== 'day' && new Date(from * 1000).toDateString() !== new Date(end * 1000).toDateString();
    drawList();
  });
  let severalDays = false;
  const hm = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  // Rows are drawn 200 at a time: a busy week stays quick to open and scroll.
  const CHUNK = 200;
  // An order number typed in the search ("523" or "#523") finds that order on any date.
  let byId = null, lookups = 0;
  async function findNumber(q) {
    const mine = ++lookups;
    let o = null;
    try { o = await invoke('get_order', { id: Number(q) }); } catch { /* none, or not this user's */ }
    if (mine !== lookups) return;
    const fine = o && o.status !== 'open' && !(o.status === 'cancelled' && o.close_reason === 'empty');
    byId = fine ? { ...o, item_count: o.items.filter((i) => !i.parent_id).reduce((n, i) => n + i.qty, 0), unsent_count: 0, held_count: 0, elsewhere: true } : { id: Number(q), none: true };
    drawList();
  }
  function drawList() {
    const q = find.value.trim().toLowerCase().replace(/^#/, '');
    let rows = q ? shown.filter((o) => String(o.id).startsWith(q) || where(o).toLowerCase().includes(q) || (o.customer_phone ?? '').includes(q)) : shown;
    if (/^\d{1,9}$/.test(q) && !shown.some((o) => String(o.id) === q)) {
      if (byId?.id !== Number(q)) { findNumber(q); } else if (!byId.none) rows = [byId, ...rows];
    }
    const draw = (n) => {
      setKids(list, rows.slice(0, n).map(orderRow),
        rows.length > n && h('button', { class: 'ghost olist-more', onclick: () => draw(n + CHUNK) }, `Show more (${rows.length - n})`));
    };
    draw(CHUNK);
    if (!rows.length) setKids(list, emptyState({ icon: q ? 'search' : 'history', title: q ? 'Nothing found' : 'No orders', text: q ? 'Try an order number or table.' : {
      today: 'None yet this business day.', yesterday: 'None in the previous business day.',
      week: 'None in the last 7 days.', day: 'None on this day.' }[mode] }));
  }
  function orderRow(o) {
    const st = statusPill(o.status);
    const row = h('div', { class: `orow${o.id === openId ? ' on' : ''}`, role: 'listitem', tabindex: 0, 'data-id': o.id,
      onclick: (e) => { if (!e.target.closest('.more-btn')) show(o.id); },
      onkeydown: (e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === row) { e.preventDefault(); show(o.id); } } },
    h('span', { class: 'o-main' },
      h('span', { class: 'o-l1' }, h('strong', {}, `#${o.id}`), h('span', { class: 'o-time' }, severalDays || o.elsewhere ? friendlyTime(o.opened_at) : hm(o.opened_at)), typeChip(o.order_type)),
      h('span', { class: 'o-l2' }, st, h('span', { class: 'o-where' }, where(o)))),
    h('strong', { class: 'o-total' }, money(o.total_cents)),
    moreMenu(`#${o.id}`, () => [
      o.status !== 'cancelled' && h('button', { onclick: act(async () => printResult(await call('print_receipt', { orderId: o.id }), o.status === 'paid' ? 'Receipt printed' : 'Printed')) },
        o.status === 'paid' ? 'Reprint receipt' : 'Print'),
      h('button', { onclick: () => show(o.id) }, 'Open'),
    ]));
    return row;
  }
  async function show(id) {
    const o = await call('get_order', { id }).catch(() => null);
    if (!o) return;
    // The service charge's name (Settings), for its line on the receipt.
    if (o.service_cents) state.service = await call('get_service_settings').catch(() => state.service);
    openId = id;
    for (const r of list.querySelectorAll('.orow')) r.classList.toggle('on', Number(r.dataset.id) === id);
    setKids(detail, orderDetail(o, async (what) => {
      if (what === 'deleted') { openId = null; empty(); md.close(true); } else await show(id);
      await load();
    }));
    md.show();
  }
  let ft;
  find.addEventListener('input', () => { clearTimeout(ft); ft = setTimeout(drawList, 120); });
  const bulk = () => bulkDelete('Delete orders', shown.filter((o) => o.status !== 'open').map((o) => ({
    id: o.id, label: [`#${o.id} ${TYPE[o.order_type] ?? ''}`, where(o)].filter(Boolean).join(' · '), sub: `${STATUS_LABEL[o.status] ?? o.status} · ${money(o.total_cents)}`,
  })), (ids) => deleteMany('order', ids, 'order(s)'), async () => { openId = null; empty(); await load(); });
  const md = masterDetail({ cls: 'md-orders', back: 'History',
    list: [h('div', { class: 'md-tools' }, seg, dayBtn), h('div', { class: 'md-tools' }, h('label', { class: 'searchbox grow' }, icon('search', 20), find)),
      span, more, h('div', { class: 'md-list' }, list)],
    detail });
  empty();
  screen({ action: isAdmin() && h('button', { onclick: bulk }, 'Select…') }, md);
  await load();
  // Asked for from a ticket or the Activity log: shown whatever the dates.
  if (state.histOpen) { const id = state.histOpen; state.histOpen = null; await show(id); }
}
const STATUS_LABEL = { paid: 'Paid', voided: 'Voided', cancelled: 'Cancelled', open: 'Open' };
const statusPill = (s) => h('span', { class: `pill ${{ paid: 'ok', voided: 'bad', open: 'warn' }[s] ?? ''}` }, STATUS_LABEL[s] ?? s);

/// Master-detail (History, Staff, Customers): the list beside the detail on wide
/// screens; on phones the list, then the detail as its own screen (a back step).
function masterDetail({ cls = '', list, detail, back }) {
  // A page drawn again: a detail screen left open on the old one is gone.
  if (backStack.at(-1) === 'item') popBack('item');
  state.itemBack = null;
  const el = h('div', { class: `md ${cls}` },
    h('div', { class: 'md-side' }, list),
    h('div', { class: 'md-main' }, h('button', { class: 'ghost back md-dback', onclick: () => el.close() }, icon('back', 22), h('span', {}, back)), detail));
  const hide = () => { el.classList.remove('show-detail'); state.itemBack = null; };
  el.show = () => {
    if (!phoneQ.matches || el.classList.contains('show-detail')) return;
    el.classList.add('show-detail');
    app.scrollTop = 0;
    pushBack('item');
    state.itemBack = () => { if (!leaveOk()) { pushBack('item'); return; } hide(); };
  };
  /// `force`: the item is gone (deleted), whatever its form held.
  el.close = (force = false) => {
    if (!el.classList.contains('show-detail')) return;
    if (!force && !leaveOk()) return;
    if (backStack.at(-1) === 'item') popBack('item');
    hide();
  };
  return el;
}

// ---------- menu (admin) ----------

// Esc on a form with a save bar in a page (Customers): ask in the bar, as × does.
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape' || !state.dirty || !document.getElementById('modal').hidden || openPop) return;
  const bar = document.querySelector('.mdetail .savebar.askable:not([hidden])');
  if (bar) bar.ask(bar.closeTo);
});

async function renderMenuAdmin() {
  const [cats, groups, feed, allVariants, defaultTaxRaw, teams, taxInclRaw] = await Promise.all([
    call('list_categories', { includeInactive: true }),
    call('list_option_groups', { includeInactive: true }),
    call('menu_options'),
    call('list_variants', { productId: null }),
    call('get_setting', { key: 'default_tax_bp' }),
    call('list_teams'),
    call('get_setting', { key: 'prices_include_tax' }),
  ]);
  const teamById = Object.fromEntries(teams.map((t) => [t.id, t]));
  const teamSelect = (value, none) => {
    const sel = h('select', { 'aria-label': 'Kitchen team' }, h('option', { value: '' }, none), teams.map((t) => h('option', { value: t.id }, t.name)));
    sel.value = value ?? '';
    return sel;
  };
  const defaultTax = Math.max(0, parseInt(defaultTaxRaw ?? '0', 10) || 0);
  const taxIncluded = taxInclRaw !== '0';
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
  const list = h('div', { class: 'mlist', role: 'list', 'aria-label': 'Menu items' });
  const listHead = h('div', { class: 'mlist-head' });
  const catPane = h('nav', { class: 'mcats', 'aria-label': 'Categories' });
  const catChips = h('div', { class: 'mchips', role: 'tablist', 'aria-label': 'Categories' });
  const search = h('input', { type: 'search', placeholder: 'Search items', 'aria-label': 'Search items', enterkeyhint: 'search' });
  state.menuTab ??= 'all';
  if (state.menuTab !== 'all' && state.menuTab !== 'none' && !catById[state.menuTab]?.active) state.menuTab = 'all';
  const changed = async () => { state.menu = null; await renderMenuAdmin(); };
  const tabCat = () => (typeof state.menuTab === 'number' ? state.menuTab : null);
  let all = []; // every item (removed ones too when shown)

  // ---- error under a field (instead of a pop-up) ----
  const fieldErr = (input, msg) => {
    if (!input) return;
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
  const field = (label, ctl, o = {}) => srow({ label, ctl, cls: `text-row ${o.cls ?? ''}`, hint: o.hint, tip: o.tip });
  const pick = (tab) => { state.menuTab = tab; search.value = ''; drawProducts(); };

  // ---- the item list ----
  const loadProducts = act(async () => {
    // Not the hidden entry behind "Other dish" on orders.
    all = (await call('list_products', { includeInactive: !!state.menuRemoved, limit: 500, offset: 0 })).filter((x) => x.sku !== '__open_item__');
    state.menuShown = all;
    drawProducts();
  });
  const TAGS = [['starter', 'Starter'], ['prep', 'Prep'], ['drink', 'Drink'], ['dessert', 'Dessert']];
  function itemRow(p, canDrag) {
    const sub = [p.name_alt, variantsOf[p.id]?.map((v) => v.name).join(' / '), state.menuTab === 'all' || search.value ? catById[p.category_id]?.name : null].filter(Boolean).join(' · ');
    const row = h('div', { class: `mrow${p.active ? '' : ' removed'}${p.sold_out ? ' sold' : ''}`, role: 'listitem', tabindex: 0, 'data-id': p.id,
      onclick: (e) => { if (e.target.closest('.lgrip, .more-btn') || row.dataset.dragged) return; editProduct(p); },
      onkeydown: (e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === row) { e.preventDefault(); editProduct(p); } } },
    canDrag ? h('span', { class: 'lgrip', role: 'button', tabindex: 0, title: 'Drag to change the order', 'aria-label': `Drag ${p.name}` }, icon('grip', 20)) : h('span', { class: 'm-dot', style: `background:${catById[p.category_id]?.color || 'var(--line-2)'}` }),
    h('span', { class: 'm-text' },
      h('strong', {}, p.name),
      h('span', { class: 'm-sub' }, sub && h('small', { lang: 'th' }, sub),
      h('span', { class: 'm-tags' },
        p.sold_out && h('span', { class: 'ftag sold' }, 'Sold out'),
        !p.active && h('span', { class: 'ftag' }, 'Removed'),
        p.is_set && h('span', { class: 'ftag set' }, 'Set'),
        p.set_only && h('span', { class: 'ftag' }, 'Set only'),
        TAGS.filter(([k]) => p[k]).map(([k, l]) => h('span', { class: `ftag ${k}` }, l)),
        teams.length > 0 && teamById[p.team_id] && h('span', { class: 'ftag team' }, teamById[p.team_id].name)))),
    h('span', { class: 'm-price' }, h('strong', {}, itemPrice(p, variantsOf[p.id])), p.track_stock && h('small', {}, `${p.stock_qty} left`)),
    moreMenu(p.name, () => [
      h('button', { onclick: act(async () => {
        await call('set_sold_out', { id: p.id, soldOut: !p.sold_out });
        p.sold_out = !p.sold_out;
        state.menu = null;
        toast(p.sold_out ? `${p.name}: sold out today` : `${p.name}: back on sale`);
        drawProducts();
      }) }, p.sold_out ? 'Back on sale' : 'Sold out today'),
      h('button', { onclick: () => editProduct(p) }, 'Edit'),
      !p.is_set && h('button', { onclick: act(() => duplicate(p)) }, 'Duplicate'),
    ]));
    return row;
  }
  function drawProducts() {
    const q = search.value.trim().toLowerCase();
    let products = all;
    if (q) products = products.filter((p) => matches(p, q));
    else if (state.menuTab === 'none') products = products.filter((p) => p.category_id == null);
    else if (tabCat()) products = products.filter((p) => p.category_id === tabCat());
    // Dragging sets the order inside one category, so it needs a category tab.
    const canDrag = !q && tabCat() != null;
    const c = catById[tabCat()];
    setKids(listHead, h('span', { class: 'mlist-title' }, q ? `"${search.value.trim()}"` : c ? c.name : state.menuTab === 'none' ? 'No category' : 'All items',
      h('small', {}, `${products.length}`)),
    c && !q && h('button', { class: 'ghost', onclick: () => editCategory(c) }, 'Edit category'));
    setKids(list, products.map((p) => itemRow(p, canDrag)),
      !products.length && emptyState({ icon: q ? 'search' : 'menuAdmin', title: q ? 'Nothing found' : 'No items here yet', text: q ? 'Try another name.' : null,
        action: !q && h('button', { class: 'primary', onclick: () => editProduct(null) }, icon('plus', 18), 'Item') }));
    drawCats();
  }
  sortable(list, { item: '.mrow', handle: '.lgrip', onDrop: act(async (ids) => { await call('reorder_products', { ids }); state.menu = null; toast('Order saved'); }) });

  // ---- categories: the pane (wide) or chips (phones), with counts ----
  let reorder = !!state.menuReorder;
  // ↑ / ↓ on a row in reorder mode: one place up or down, saved at once. A main
  // category moves with its sub-categories past the next main one; a
  // sub-category moves only among its main category's.
  const moveCat = (c, dir) => act(async () => {
    const blocks = cats.filter(isMain).map((m) => ({ id: m.id, subs: cats.filter((x) => x.parent_id === m.id).map((x) => x.id) }));
    const swap = (list, i) => {
      const j = i + dir;
      if (i < 0 || j < 0 || j >= list.length) return false;
      [list[i], list[j]] = [list[j], list[i]];
      return true;
    };
    const subs = isMain(c) ? null : blocks.find((b) => b.id === c.parent_id).subs;
    if (!(subs ? swap(subs, subs.indexOf(c.id)) : swap(blocks, blocks.findIndex((b) => b.id === c.id)))) return;
    const ids = blocks.flatMap((b) => [b.id, ...b.subs]);
    await call('reorder_categories', { ids });
    state.menu = null;
    toast('Category order saved');
    await changed();
    document.querySelector(`.mcat-list [data-id="${c.id}"]`)?.focus();
  })();
  function drawCats() {
    const count = (id) => all.filter((p) => (id === 'all' ? true : id === 'none' ? p.category_id == null : p.category_id === id)).length;
    const on = (id) => !search.value.trim() && state.menuTab === id;
    // Reorder mode: a 44 px grip on each row (and no ⋯); otherwise the row picks the category.
    const catRow = (c) => h('div', { class: `mcat${on(c.id) && !reorder ? ' on' : ''}${isMain(c) ? '' : ' sub'}${c.active ? '' : ' removed'}`, 'data-id': c.id, role: 'button', tabindex: 0,
      onclick: (e) => { if (reorder || e.target.closest('.lgrip, .more-btn') || e.currentTarget.dataset.dragged) return; pick(c.id); },
      onkeydown: (e) => {
        if (reorder && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) { e.preventDefault(); moveCat(c, e.key === 'ArrowUp' ? -1 : 1); }
        else if (!reorder && (e.key === 'Enter' || e.key === ' ') && e.target === e.currentTarget) { e.preventDefault(); pick(c.id); }
      } },
    reorder && h('span', { class: 'lgrip', title: 'Drag to change the order', 'aria-hidden': 'true' }, icon('grip', 20)),
    h('span', { class: 'm-dot', style: `background:${c.color || 'var(--line-2)'}` }),
    h('span', { class: 'mcat-name' }, c.name), h('small', {}, count(c.id)),
    !reorder && moreMenu(c.name, () => [h('button', { onclick: () => editCategory(c) }, 'Edit'),
      isMain(c) && h('button', { onclick: () => { state.newSubOf = c.id; editCategory(null); } }, '+ Sub-category')]));
    const plain = (id, label) => h('div', { class: `mcat plain${on(id) ? ' on' : ''}`, role: 'button', tabindex: 0, onclick: () => pick(id),
      onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); pick(id); } } }, h('span', { class: 'mcat-name' }, label), h('small', {}, count(id)));
    const catList = h('div', { class: `mcat-list${reorder ? ' reorder' : ''}` }, catTree.map(catRow));
    sortable(catList, { item: '.mcat', handle: '.lgrip', onDrop: act(async (ids) => {
      await call('reorder_categories', { ids });
      state.menu = null;
      toast('Category order saved');
      await changed();
    }) });
    setKids(catPane,
      h('div', { class: 'mcats-title' }, h('span', {}, 'Categories'), reorder
        ? h('button', { class: 'ghost mcats-done', onclick: () => { reorder = state.menuReorder = false; drawCats(); } }, 'Done')
        : cats.length > 0 && moreMenu('categories', () => [cats.length > 1 && h('button', { onclick: () => { reorder = state.menuReorder = true; drawCats(); } }, 'Change order'),
          isAdmin() && h('button', { class: 'danger', onclick: () => bulkDelete('Delete categories',
            cats.map((c) => ({ id: c.id, label: c.name })), (ids) => deleteMany('category', ids, 'categor(ies)'), async () => { state.menuTab = 'all'; await changed(); }) }, 'Delete several…')])),
      !reorder && plain('all', 'All items'), catList, !reorder && plain('none', 'No category'),
      h('button', { class: 'ladd', onclick: () => editCategory(null) }, icon('plus', 18), 'Category'),
      h('div', { class: 'mcats-title opts' }, h('span', {}, 'Options'), isAdmin() && groups.length > 0 && moreMenu('option groups', () => [h('button', { class: 'danger', onclick: () => bulkDelete('Delete option groups',
        groups.map((g) => ({ id: g.id, label: g.name, sub: groupRule(g) })), (ids) => deleteMany('option_group', ids, 'group(s)'), () => changed()) }, 'Delete several…')])),
      groups.map((g) => h('div', { class: `mcat mgrp${g.active ? '' : ' removed'}`, role: 'button', tabindex: 0, onclick: () => editGroup(g),
        onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); editGroup(g); } } },
      h('span', { class: 'mcat-name' }, g.name, h('small', {}, groupRule(g))), h('span', { class: 'chev' }, icon('chev', 16)))),
      h('button', { class: 'ladd', onclick: () => editGroup(null) }, icon('plus', 18), 'Option group'));
    // Phones: the same as a row of chips (and Options › opens the groups).
    const chip = (id, label, color = null) => h('button', { class: `mchip${on(id) ? ' on' : ''}`, role: 'tab', 'aria-selected': String(on(id)), onclick: () => pick(id) },
      color && h('span', { class: 'm-dot', style: `background:${color}` }), label);
    setKids(catChips, chip('all', 'All'), catTree.filter((c) => c.active).map((c) => chip(c.id, c.name, c.color || 'var(--line-2)')), chip('none', 'No category'),
      h('button', { class: 'mchip', onclick: optionsSheet }, 'Options', icon('chev', 16)),
      h('button', { class: 'mchip add', 'aria-label': 'New category', onclick: () => editCategory(null) }, icon('plus', 18)));
    catChips.querySelector('.mchip.on')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }
  // Phones: the option groups in a sheet.
  function optionsSheet() {
    const s = sheet({ title: 'Options', body: [ssec(null, [
      ...groups.map((g) => navRow({ label: g.name, hint: groupRule(g), value: g.active ? null : 'Off', onOpen: () => { s.close(); setTimeout(() => editGroup(g)); } })),
      h('button', { class: 'ladd', onclick: () => { s.close(); setTimeout(() => editGroup(null)); } }, icon('plus', 18), 'Option group')])] });
  }

  // ---- a category, in a sheet ----
  function editCategory(c) {
    const name = h('input', { value: c?.name ?? '', placeholder: 'e.g. Curries', 'data-f': 'cname', maxlength: 40, 'aria-label': 'Name' });
    const alt = h('input', { value: c?.name_alt ?? '', placeholder: 'Optional', lang: 'th', 'data-f': 'calt', 'aria-label': 'Thai name' });
    const color = h('input', { type: 'color', value: c?.color ?? '#9a9484', 'aria-label': 'Colour', class: 'swatch' });
    const tax = h('input', { value: c?.tax_rate_bp != null ? pct(c.tax_rate_bp) : '', placeholder: `${pct(defaultTax)} (restaurant)`, inputmode: 'decimal', 'data-f': 'ctax', class: 'short-in', 'aria-label': 'Tax %' });
    // Under a main category (a sub-category), or a main category itself.
    const hasSubs = !!c && cats.some((x) => x.parent_id === c.id);
    const parent = h('select', { disabled: hasSubs, 'aria-label': 'Main category' },
      h('option', { value: '' }, 'None (main)'),
      cats.filter((x) => isMain(x) && x.active && x.id !== c?.id).map((x) => h('option', { value: x.id }, x.name)));
    parent.value = c?.parent_id ?? (state.newSubOf ?? '');
    state.newSubOf = null;
    // Each default: a switch for a main category; for a sub-category "Same as main", On or Off.
    const flagBox = (key, label, guess) => {
      const sw = switchRow({ label, value: c ? c[key] : guess, onChange: () => {} });
      const sel = h('select', { 'aria-label': label });
      const selRow = srow({ label, ctl: sel, cls: 'text-row' });
      const wrap = h('div', { class: 'flag-wrap' });
      const sub = () => (parent.value ? catById[Number(parent.value)] : null);
      const draw = () => {
        const m = sub();
        if (!m) return setKids(wrap, sw);
        const was = sel.value;
        setKids(sel, h('option', { value: 'inherit' }, `Same as main (${m[key] ? 'on' : 'off'})`), h('option', { value: 'on' }, 'On'), h('option', { value: 'off' }, 'Off'));
        sel.value = was || (c?.[key] ? 'on' : (c?.flags_off ?? 0) & CAT_FLAG_OFF[key] ? 'off' : sw.input.checked ? 'on' : 'inherit');
        setKids(wrap, selRow);
      };
      return { el: wrap, draw,
        own: () => (sub() ? sel.value === 'on' : sw.input.checked),
        off: () => (sub() && sel.value === 'off' ? CAT_FLAG_OFF[key] : 0),
        // What new items here get, the main category's default included.
        effective: () => { const m = sub(); return m ? sel.value === 'on' || (sel.value === 'inherit' && !!m[key]) : sw.input.checked; } };
    };
    const starter = flagBox('starter', 'Starters', /^starter/i.test(c?.name ?? ''));
    const prep = flagBox('prep', 'Prepared ahead', false);
    const drink = flagBox('drink', 'Drinks', /drink|beverage|เครื่องดื่ม/i.test(c?.name ?? ''));
    const dessert = flagBox('dessert', 'Desserts', /dessert|sweet|ของหวาน/i.test(c?.name ?? ''));
    const flags = [starter, prep, drink, dessert];
    const drawFlags = () => flags.forEach((f) => f.draw());
    parent.addEventListener('change', drawFlags);
    drawFlags();
    const team = teamSelect(c?.team_id, 'No team');
    const moveItems = switchRow({ label: 'Move its items too', value: true, onChange: () => {} });
    moveItems.hidden = !team.value;
    team.addEventListener('change', () => { moveItems.hidden = !team.value; });
    const active = switchRow({ label: 'On the menu', value: c ? c.active : true, onChange: () => {} });
    // Drinks and desserts: the items already here follow what the category now gives them.
    const wasDrink = c ? flagOf(c, catById[c.parent_id], 'drink') : false;
    const wasDessert = c ? flagOf(c, catById[c.parent_id], 'dessert') : false;
    const save = async () => {
      fieldErr(name); fieldErr(tax);
      if (!name.value.trim()) { fieldErr(name, 'Type a name'); return 'Type a name'; }
      let taxBp = null;
      if (tax.value.trim()) {
        const v = Number(tax.value.replace(',', '.').replace('%', ''));
        if (!Number.isFinite(v) || v < 0 || v > 100) { fieldErr(tax, 'From 0 to 100'); return 'Tax: from 0 to 100'; }
        taxBp = Math.round(v * 100);
      }
      const args = { name: name.value, color: color.value };
      const saved = c
        ? await call('update_category', { id: c.id, ...args, sortOrder: c.sort_order, active: active.input.checked }, { quiet: true })
        : await call('create_category', { ...args, sortOrder: cats.length + 1 }, { quiet: true });
      await call('set_category_defaults', { id: saved.id, taxRateBp: taxBp, starter: starter.own(), prep: prep.own(), nameAlt: alt.value.trim() || null }, { quiet: true });
      // Drinks and desserts: the items already here follow the category (its main category's included),
      // then the category keeps its own setting.
      for (const [f, was, cmd, key] of [[drink, wasDrink, 'set_category_drink', 'drink'], [dessert, wasDessert, 'set_category_dessert', 'dessert']]) {
        if (f.effective() !== was) await call(cmd, { id: saved.id, [key]: f.effective(), moveItems: true }, { quiet: true });
        if (f.own() !== f.effective() || (f.effective() === was && f.own() !== !!c?.[key])) await call(cmd, { id: saved.id, [key]: f.own(), moveItems: false }, { quiet: true });
      }
      const parentId = parent.value ? Number(parent.value) : null;
      if (parentId !== (c?.parent_id ?? null)) await call('set_category_parent', { id: saved.id, parentId }, { quiet: true });
      const off = flags.reduce((n, f) => n | f.off(), 0);
      if (off !== (c?.flags_off ?? 0)) await call('set_category_flags_off', { id: saved.id, flagsOff: off }, { quiet: true });
      const teamId = team.value ? Number(team.value) : null;
      if (teamId !== (c?.team_id ?? null) || (teamId != null && moveItems.input.checked)) {
        await call('set_category_team', { id: saved.id, teamId, moveItems: moveItems.input.checked }, { quiet: true });
      }
      toast('Saved');
      if (!c) state.menuTab = saved.id;
      setTimeout(changed);
    };
    const del = c && isAdmin() && confirmBtn('Delete', `Tap again to delete`, async () => {
      await call('delete_category', { id: c.id });
      s.close();
      toast(`${c.name} deleted; its items now have no category`);
      state.menuTab = 'all';
      await changed();
    }, 'ghost danger');
    const subBtn = c && isMain(c) && c.active && h('button', { class: 'ghost', onclick: () => { s.close(); state.newSubOf = c.id; setTimeout(() => editCategory(null)); } }, '+ Sub-category');
    const s = sheet({ title: c ? c.name : 'New category', wide: true, onSave: save, left: (del || subBtn) && h('span', { class: 'sheet-left' }, del, subBtn),
      body: [ssec(null, [field('Name', name), field('Thai name', alt, { hint: 'On kitchen tickets' }), srow({ label: 'Colour', ctl: color }), c && active]),
        ssec('Place', [field('Main category', parent, { hint: hasSubs ? 'Has sub-categories' : 'Pick one to make this a sub-category' }),
          teams.length > 0 && field('Kitchen team', team), teams.length > 0 && moveItems]),
        ssec('New items here', [field('Tax %', tax), flags.map((f) => f.el)], 'Defaults for items you add. Drinks and desserts also move the items already here.')] });
    name.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); if (name.value.trim()) s.save.click(); } });
    if (!c) tapFocus(name);
  }

  // ---- an option group, in a sheet ----
  function editGroup(g) {
    const name = h('input', { value: g?.name ?? '', placeholder: 'e.g. Size, Extras', maxlength: 40, 'aria-label': 'Name', 'data-f': 'gname' });
    const min = stepper({ value: g?.min_select ?? 0, min: 0, max: 20, label: 'At least' });
    const max = stepper({ value: g?.max_select ?? 1, min: 1, max: 20, label: 'At most' });
    const active = switchRow({ label: 'In use', hint: (v) => (v ? '' : 'Hidden on orders'), value: g ? g.active : true, onChange: () => {} });
    const rows = (g?.options ?? []).filter((o) => o.active).map((o) => ({ id: o.id, name: o.name, alt: o.name_alt ?? '', price: o.price_cents ? signed(o.price_cents) : '' }));
    if (!rows.length) rows.push({ id: null, name: '', alt: '', price: '' });
    const opts = listEditor({ rows, cls: 'opts3', addLabel: 'Option', onChange: () => {},
      make: () => ({ id: null, name: '', alt: '', price: '' }),
      render: (r) => [
        h('input', { value: r.name, placeholder: 'e.g. Large', 'aria-label': 'Option', oninput: (e) => (r.name = e.target.value) }),
        h('input', { value: r.alt, placeholder: 'Thai', lang: 'th', 'aria-label': 'Thai name', oninput: (e) => (r.alt = e.target.value) }),
        h('input', { value: r.price, placeholder: '+0.00', inputmode: 'decimal', 'aria-label': 'Extra price', class: 'price-in', oninput: (e) => (r.price = e.target.value) })] });
    const save = async () => {
      if (!name.value.trim()) { fieldErr(name, 'Type a name'); return 'Type a name'; }
      if (max.value() < min.value()) return 'At most must be at least as many as At least';
      let options;
      try { options = rows.filter((r) => r.name.trim()).map((r) => ({ id: r.id, name: r.name, name_alt: r.alt.trim() || null, price_cents: parseSignedMoney(r.price) })); }
      catch (e) { return `Price: ${e.message ?? e}`; }
      const args = { name: name.value, minSelect: min.value(), maxSelect: max.value() };
      const saved = g
        ? await call('update_option_group', { id: g.id, ...args, sortOrder: g.sort_order, active: active.input.checked }, { quiet: true })
        : await call('create_option_group', args, { quiet: true });
      await call('save_group_options', { groupId: saved.id, options }, { quiet: true });
      toast('Saved');
      setTimeout(changed);
    };
    const del = g && isAdmin() && confirmBtn('Delete', 'Tap again to delete', async () => {
      await call('delete_option_group', { id: g.id });
      s.close();
      toast(`${g.name} deleted`);
      await changed();
    }, 'ghost danger');
    const s = sheet({ title: g ? g.name : 'New option group', wide: true, onSave: save, left: del,
      body: [ssec(null, [field('Name', name), srow({ label: 'At least', hint: 'Size: 1. Extras: 0.', ctl: min }), srow({ label: 'At most', ctl: max }), g && active]),
        ssec('Options', [opts], 'Price is added to the item. Use −1.00 for a cheaper one.')] });
    if (!g) tapFocus(name);
  }

  // ---- an item, in a sheet: the main fields, the kitchen flags, then Advanced ----
  async function duplicate(p) {
    const copy = await call('duplicate_product', { id: p.id });
    toast(`Copied as "${copy.name}"`);
    await renderMenuAdmin();
    const fresh = (await call('list_products', { includeInactive: true, limit: 500, offset: 0 })).find((x) => x.id === copy.id);
    state.openEditor?.(fresh ?? copy);
  }
  function editProduct(p, preset = {}) {
    if (p?.is_set) return editSet(p);
    const startCat = p ? p.category_id : (preset.category_id !== undefined ? preset.category_id : tabCat());
    const d0 = defaultsFor(startCat);
    const flag = (label, hint, value) => switchRow({ label, hint, value, onChange: () => {} });
    const rows = {
      starter: flag('Starter', 'Sent to the kitchen first', p ? p.starter : d0.starter),
      prep: flag('Prepare ahead', 'Ticket while the mains wait', p ? p.prep : d0.prep),
      drink: flag('Drink', 'Own DRINKS ticket, sent at once', p ? p.drink : d0.drink),
      dessert: flag('Dessert', 'Own DESSERTS ticket', p ? p.dessert : d0.dessert),
      sold: flag('Sold out today', 'Back on sale at day close', !!p?.sold_out),
      setOnly: flag('Only in set menus', 'No tile of its own', !!p?.set_only),
      track: flag('Count stock', 'e.g. bottled drinks', p ? p.track_stock : false),
      active: flag('On the menu', 'Off: removed, kept on past orders', p ? p.active : true),
    };
    const f = {
      name: h('input', { value: p?.name ?? '', placeholder: 'e.g. Pad Thai', 'data-f': 'name', maxlength: 60, 'aria-label': 'Name', enterkeyhint: 'next' }),
      alt: h('input', { value: p?.name_alt ?? '', placeholder: 'e.g. ผัดไทย', lang: 'th', 'data-f': 'alt', 'aria-label': 'Thai name', enterkeyhint: 'next' }),
      cat: h('select', { 'aria-label': 'Category' }, h('option', { value: '' }, 'No category'), catTree.filter((c) => c.active || c.id === p?.category_id).map((c) => h('option', { value: c.id }, catLabel(c)))),
      price: h('input', { value: p ? money(p.price_cents) : '', placeholder: '0.00', inputmode: 'decimal', class: 'short-in', 'data-f': 'price', 'aria-label': 'Price' }),
      tax: h('input', { value: pct(p ? p.tax_rate_bp : d0.tax), inputmode: 'decimal', class: 'short-in', 'data-f': 'tax', 'aria-label': 'Tax %' }),
      cost: h('input', { value: p ? money(p.cost_cents) : '0', inputmode: 'decimal', class: 'short-in', 'data-f': 'cost', 'aria-label': 'Cost' }),
      barcode: h('input', { value: p?.barcode ?? '', placeholder: 'Optional', 'aria-label': 'Barcode' }),
      sku: h('input', { value: p?.sku ?? '', placeholder: 'Optional', 'aria-label': 'Code / PLU' }),
      stock: h('input', { value: '0', inputmode: 'numeric', class: 'short-in', 'aria-label': 'Opening stock' }),
      team: teamSelect(p ? p.team_id : d0.team, 'No team'),
    };
    for (const k of ['starter', 'prep', 'drink', 'dessert', 'setOnly', 'track', 'active', 'sold']) f[k] = rows[k].input;
    f.cat.value = startCat ?? '';
    // A new item follows its category's defaults until the user changes them.
    const touched = new Set(p ? ['tax', 'starter', 'prep', 'team', 'drink', 'dessert'] : []);
    for (const k of ['starter', 'prep', 'team', 'drink', 'dessert']) f[k].addEventListener('change', () => touched.add(k));
    f.tax.addEventListener('input', () => touched.add('tax'));
    f.cat.addEventListener('change', () => {
      const d = defaultsFor(f.cat.value ? Number(f.cat.value) : null);
      if (!touched.has('tax')) f.tax.value = pct(d.tax);
      for (const k of ['starter', 'prep', 'drink', 'dessert']) if (!touched.has(k)) f[k].checked = d[k];
      if (!touched.has('team')) f.team.value = d.team ?? '';
      preview();
    });
    // Choices with their own price (a soup with Chicken 6.50 / Beef 7.50).
    const choices = (variantsOf[p?.id] ?? []).map((v) => ({ id: v.id, name: v.name, alt: v.name_alt ?? '', price: money(v.price_cents) }));
    let choiceSeq = 0;
    const choiceList = listEditor({ rows: choices, cls: 'opts3', addLabel: 'Choice', onChange: () => preview(),
      make: () => ({ id: null, name: '', alt: '', price: '' }),
      render: (c) => {
        c.key ??= ++choiceSeq;
        return [h('input', { value: c.name, placeholder: 'e.g. Chicken', 'aria-label': 'Choice', oninput: (e) => { c.name = e.target.value; preview(); } }),
          h('input', { value: c.alt, placeholder: 'Thai', lang: 'th', 'aria-label': 'Thai name', oninput: (e) => (c.alt = e.target.value) }),
          h('input', { value: c.price, placeholder: 'Price', inputmode: 'decimal', class: 'price-in', 'aria-label': 'Price', 'data-f': `choice${c.key}`, oninput: (e) => { c.price = e.target.value; preview(); } })];
      } });
    // Option groups this item uses: toggle chips; a new group can be made here.
    const linked = new Set(feed.links.filter((l) => l.product_id === p?.id).map((l) => l.group_id));
    let groupIds = activeGroups.filter((g) => linked.has(g.id)).map((g) => g.id);
    const groupChips = () => toggleChips(activeGroups.map((g) => [g.id, g.name]), groupIds, (sel) => { groupIds = sel; preview(); }, 'Option groups');
    const groupWrap = h('span', { class: 'grp-wrap' }, activeGroups.length ? groupChips() : h('span', { class: 'muted' }, 'None yet'));
    const newGroup = h('div');
    const openNewGroup = () => {
      const gname = h('input', { placeholder: 'e.g. Spice level', 'aria-label': 'Group name' });
      const rule = h('select', { 'aria-label': 'Rule' }, h('option', { value: '1-1' }, 'Pick exactly 1'), h('option', { value: '0-1' }, 'Optional, up to 1'), h('option', { value: '0-5' }, 'Optional, up to 5'));
      const opts = h('textarea', { rows: 3, placeholder: 'One per line, Thai after |, price at the end:\nMild | เผ็ดน้อย\nHot | เผ็ด\nExtra hot +0.50', 'aria-label': 'Options' });
      setKids(newGroup, h('div', { class: 'newgrp' },
        h('div', { class: 'newgrp-row' }, gname, rule), opts,
        h('div', { class: 'newgrp-row' },
          h('button', { onclick: () => setKids(newGroup) }, 'Cancel'),
          h('button', { class: 'primary', onclick: act(async () => {
            const [mn, mx] = rule.value.split('-').map(Number);
            const options = opts.value.split('\n').map((l) => l.trim()).filter(Boolean).map((l) => {
              const m = l.match(/^(.*?)\s*([+-]\s*[£$€]?\d+(?:[.,]\d{1,2})?)$/);
              const [name, alt = ''] = (m ? m[1] : l).split('|').map((x) => x.trim());
              return { id: null, name, name_alt: alt || null, price_cents: m ? parseSignedMoney(m[2].replace(/\s|[£$€]/g, '')) : 0 };
            });
            if (!gname.value.trim()) return toast('Type the group name', true);
            if (!options.length) return toast('Add at least one option, one per line', true);
            const g = await call('create_option_group', { name: gname.value, minSelect: mn, maxSelect: mx });
            const saved = await call('save_group_options', { groupId: g.id, options });
            activeGroups.push(saved);
            groups.push(saved);
            groupIds = [...groupIds, saved.id];
            setKids(groupWrap, groupChips());
            setKids(newGroup);
            toast(`${saved.name} added and ticked`);
          }) }, 'Create group'))));
      tapFocus(gname);
    };
    // Live preview of the order-screen tile.
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
          (named.length > 0 || groupIds.length > 0) && h('span', { class: 'more' }, '•••')));
    };
    for (const el of [f.name, f.alt, f.price]) el.addEventListener('input', preview);
    f.starter.addEventListener('change', preview);
    preview();

    // Check everything, showing problems next to the field.
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
      for (const c of choices) {
        if (!c.name.trim()) continue;
        const el = choiceList.querySelector(`[data-f="choice${c.key}"]`);
        try { variants.push({ id: c.id, name: c.name, name_alt: c.alt.trim() || null, price_cents: parseMoney(c.price) }); err(el, null); } catch (e) { err(el, e.message); }
      }
      if (!ok) {
        // A problem inside Advanced: open it.
        const adv = s.el.querySelector('.adv-body');
        if (adv?.hidden && adv.querySelector('.bad')) adv.previousElementSibling?.click();
        s.el.querySelector('.bad')?.scrollIntoView({ block: 'center' });
        return null;
      }
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
    const save = async (again = false) => {
      const v = validate();
      if (!v) return 'Check the fields in red';
      const saved = p
        ? await call('update_product', { id: p.id, product: { ...v.product, active: f.active.checked } }, { quiet: true })
        : await call('create_product', { product: { ...v.product, stock_qty: parseInt(f.stock.value || '0', 10) || 0 } }, { quiet: true });
      if (p || groupIds.length) await call('set_product_option_groups', { productId: saved.id, groupIds }, { quiet: true });
      if (p || v.variants.length) await call('set_variants', { productId: saved.id, variants: v.variants }, { quiet: true });
      if (saved.drink !== f.drink.checked) await call('set_drink', { id: saved.id, drink: f.drink.checked }, { quiet: true });
      if (saved.dessert !== f.dessert.checked) await call('set_dessert', { id: saved.id, dessert: f.dessert.checked }, { quiet: true });
      if (!!saved.set_only !== f.setOnly.checked) await call('set_set_only', { id: saved.id, setOnly: f.setOnly.checked }, { quiet: true });
      if (!!saved.sold_out !== f.sold.checked) await call('set_sold_out', { id: saved.id, soldOut: f.sold.checked }, { quiet: true });
      state.menu = null;
      if (again) {
        // Keep going in the same category.
        toast(`${saved.name} saved. Next item:`);
        setTimeout(async () => { await renderMenuAdmin(); state.openEditor?.(null, { category_id: v.product.category_id }); });
      } else {
        toast('Saved');
        setTimeout(changed);
      }
    };
    // The keyboard's Done (Enter) moves on to the price; Enter on the price saves.
    f.name.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); if (f.name.value.trim()) tapFocus(f.price); } });
    f.alt.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); tapFocus(f.price); } });
    f.price.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); if (f.name.value.trim() && f.price.value.trim()) s.save.click(); } });

    const delta = h('input', { placeholder: '+5 or -2', inputmode: 'numeric', class: 'short-in', 'aria-label': 'Stock change' });
    const reason = h('input', { placeholder: 'Reason', 'aria-label': 'Reason' });
    const stockRow = p && p.track_stock && srow({ label: `Stock: ${p.stock_qty}`, cls: 'text-row stock-row', ctl: [delta, reason, h('button', { onclick: act(async () => {
      await call('adjust_stock', { id: p.id, delta: parseInt(delta.value, 10) || 0, reason: reason.value });
      toast('Stock updated');
      p.stock_qty += parseInt(delta.value, 10) || 0;
      stockRow.querySelector('.srow-label span').textContent = `Stock: ${p.stock_qty}`;
      delta.value = '';
    }) }, 'Adjust')] });
    const advRows = [
      field('Tax %', f.tax), field('Cost', f.cost), rows.setOnly,
      srow({ label: 'Choices', hint: 'Each with its own price', stacked: true, ctl: choiceList }),
      srow({ label: 'Option groups', hint: 'Sizes, extras', stacked: true, ctl: h('div', { class: 'grp-edit' }, groupWrap,
        h('button', { class: 'ghost add-grp', onclick: openNewGroup }, icon('plus', 18), 'New group'), newGroup) }),
      field('Barcode', f.barcode), field('Code / PLU', f.sku), rows.track,
      !p && field('Opening stock', f.stock), stockRow, p && rows.active];
    const left = p && h('span', { class: 'sheet-left' },
      h('button', { class: 'ghost', onclick: act(async () => { s.close(); await duplicate(p); }) }, 'Duplicate'),
      isAdmin() && confirmBtn('Delete', 'Tap again to delete', async () => {
        await call('delete_product', { id: p.id });
        s.close();
        toast(`${p.name} deleted`);
        await changed();
      }, 'ghost danger'));
    const again = !p && h('button', { class: 'ghost', onclick: async () => {
      const r = await save(true).catch((e) => String(e?.message ?? e));
      if (typeof r === 'string') s.show(r, false); else s.close();
    } }, 'Save & next');
    const s = sheet({ title: p ? p.name : 'New item', wide: true, onSave: () => save(false), left: left || again,
      body: [
        h('div', { class: 'item-top' }, ssec(null, [field('Name', f.name), field('Thai name', f.alt, { hint: 'On kitchen tickets' }),
          field('Category', f.cat), field(taxIncluded ? 'Price' : 'Price before tax', f.price, { hint: taxIncluded ? 'Tax included' : '' }),
          teams.length > 0 && field('Kitchen team', f.team)]), tile),
        ssec('Kitchen', [rows.starter, rows.prep, rows.drink, rows.dessert]),
        p && ssec('Today', [rows.sold]),
        ssec(null, advanced('menuitem', advRows)),
      ] });
    tapFocus(f.name);
    if (!p) f.name.select();
  }

  // ---- set menus: price, sections (Starter, Main, Side...) and their dishes ----
  async function editSet(p) {
    // Any dish can go in a set; its choices and options are picked with it on the order.
    const allDishes = (await call('list_products', { includeInactive: false, limit: 500, offset: 0 })).filter((x) => !x.is_set);
    const pool = allDishes;
    const byId = Object.fromEntries(allDishes.map((x) => [x.id, x]));
    const withable = [...allDishes];
    const sections = p
      ? (await call('get_set', { setId: p.id })).map((s) => ({ name: s.name, alt: s.name_alt ?? '', pick: s.pick, starter: s.starter,
        items: s.items.map((c) => c.product_id), with: s.with_items.map((c) => c.product_id) }))
      : [['Starter', true], ['Main', false], ['Side', false]].map(([name, starter]) => ({ name, alt: '', pick: 1, starter, items: [], with: [] }));
    const name = h('input', { value: p?.name ?? '', placeholder: 'e.g. Dinner set for 1', 'data-f': 'sname', 'aria-label': 'Name' });
    const alt = h('input', { value: p?.name_alt ?? '', placeholder: 'Optional', lang: 'th', 'aria-label': 'Thai name' });
    const cat = h('select', { 'aria-label': 'Category' }, h('option', { value: '' }, 'No category'), cats.filter((c) => c.active || c.id === p?.category_id).map((c) => h('option', { value: c.id }, c.name)));
    cat.value = p?.category_id ?? (tabCat() ?? '');
    const price = h('input', { value: p ? money(p.price_cents) : '', placeholder: '0.00', inputmode: 'decimal', class: 'short-in', 'data-f': 'sprice', 'aria-label': 'Price' });
    const tax = h('input', { value: pct(p ? p.tax_rate_bp : defaultsFor(cat.value ? Number(cat.value) : null).tax), inputmode: 'decimal', class: 'short-in', 'aria-label': 'Tax %' });
    const active = switchRow({ label: 'On the menu', value: p ? p.active : true, onChange: () => {} });
    const secList = h('div', { class: 'stack' });
    // Searchable dish picker: tap a dish to add it, tap again to take it out.
    const dishPicker = (chosen, from, ui, onChange) => {
      const q = h('input', { type: 'search', placeholder: 'Search dishes', value: ui.q ?? '', 'aria-label': 'Search dishes' });
      const tabsEl = h('div', { class: 'cat-tabs' });
      const grid = h('div', { class: 'pick-grid' });
      const drawList = () => {
        const term = q.value.trim().toLowerCase();
        const here = [...new Set(from.map((x) => x.category_id ?? 0))];
        if (ui.cat != null && !here.includes(ui.cat)) ui.cat = null;
        setKids(tabsEl, h('button', { class: ui.cat == null ? 'on' : '', onclick: () => { ui.cat = null; drawList(); } }, 'All'),
          here.map((c) => h('button', { class: ui.cat === c ? 'on' : '', style: catStyle(catById[c]?.color),
            onclick: () => { ui.cat = c; drawList(); } }, catById[c]?.name ?? 'No category')));
        const shown = from.filter((x) => (ui.cat == null || (x.category_id ?? 0) === ui.cat) && (!term || matches(x, term)));
        setKids(grid, shown.slice(0, 120).map((x) => h('button', {
          class: chosen.includes(x.id) ? 'on' : '',
          onclick: () => {
            const at = chosen.indexOf(x.id);
            if (at >= 0) chosen.splice(at, 1); else chosen.push(x.id);
            onChange();
            drawList();
          },
        }, h('span', {}, x.name), x.name_alt && h('small', { lang: 'th' }, x.name_alt), x.set_only && h('small', {}, 'set only'))),
        !shown.length && h('span', { class: 'muted' }, term ? 'No dish matches.' : 'No dishes here.'));
      };
      q.addEventListener('input', () => { ui.q = q.value; drawList(); });
      drawList();
      return h('div', { class: 'stack dish-picker' }, h('div', { class: 'searchbox' }, icon('search', 18), q), tabsEl, grid);
    };
    const draw = () => setKids(secList, sections.map((sec, i) => {
      sec.ui ??= { open: !sec.items.length };
      sec.withUi ??= { open: false };
      const pick = stepper({ value: sec.pick, min: 1, max: 10, label: 'Choose', onChange: (v) => (sec.pick = v) });
      const chips = h('div', { class: 'opts' });
      const drawChips = () => setKids(chips, sec.items.length ? sec.items.map((id) => h('button', { class: 'chip-btn', title: 'Remove from this section',
        onclick: () => { sec.items.splice(sec.items.indexOf(id), 1); draw(); } }, byId[id]?.name ?? `#${id}`, ' ×'))
        : h('span', { class: 'muted' }, 'No dishes yet'));
      drawChips();
      const withChips = h('div', { class: 'opts' });
      const drawWith = () => setKids(withChips, sec.with.map((id) => h('button', { class: 'chip-btn', title: 'Remove',
        onclick: () => { sec.with.splice(sec.with.indexOf(id), 1); draw(); } }, byId[id]?.name ?? `#${id}`, ' ×')));
      drawWith();
      const newName = h('input', { placeholder: 'New dish just for sets', 'aria-label': 'New dish name' });
      const newAlt = h('input', { placeholder: 'Thai', lang: 'th', 'aria-label': 'New dish Thai name' });
      const addNew = h('div', { class: 'sec-add' }, newName, newAlt, h('button', { onclick: act(async () => {
        if (!newName.value.trim()) return toast('Type the new dish name', true);
        const made = await call('create_product', { product: {
          name: newName.value.trim(), name_alt: newAlt.value.trim() || null, category_id: null, price_cents: 0, cost_cents: 0,
          tax_rate_bp: Math.round(Number(tax.value || 0) * 100), stock_qty: 0, track_stock: false, starter: sec.starter, prep: false, team_id: null,
        } });
        await call('set_set_only', { id: made.id, setOnly: true });
        made.set_only = true;
        pool.push(made); byId[made.id] = made; withable.push(made); sec.items.push(made.id);
        toast(`${made.name} added (only sold in sets)`);
        draw();
      }) }, '+ New dish'));
      return h('div', { class: 'card stack inset set-edit' },
        h('div', { class: 'sec-head' },
          h('input', { value: sec.name, placeholder: 'Section, e.g. Main', 'aria-label': 'Section name', oninput: (e) => (sec.name = e.target.value) }),
          h('input', { value: sec.alt, placeholder: 'Thai', lang: 'th', 'aria-label': 'Section Thai name', oninput: (e) => (sec.alt = e.target.value) }),
          h('span', { class: 'sec-tools' },
            h('button', { title: 'Move up', 'aria-label': 'Move up', disabled: i === 0, onclick: () => { [sections[i - 1], sections[i]] = [sections[i], sections[i - 1]]; draw(); } }, '↑'),
            h('button', { title: 'Move down', 'aria-label': 'Move down', disabled: i === sections.length - 1, onclick: () => { [sections[i + 1], sections[i]] = [sections[i], sections[i + 1]]; draw(); } }, '↓'),
            h('button', { class: 'danger', title: 'Remove this section', 'aria-label': 'Remove this section', onclick: () => { sections.splice(i, 1); draw(); } }, '×'))),
        h('div', { class: 'sgroup' },
          srow({ label: 'Choose', ctl: pick }),
          switchRow({ label: 'Starter course', hint: 'Goes with the starters', value: sec.starter, onChange: (v) => { sec.starter = v; } })),
        h('div', { class: 'row' }, chips, h('span', { style: 'flex:1' }),
          h('button', { class: sec.ui.open ? 'on' : '', onclick: () => { sec.ui.open = !sec.ui.open; draw(); } }, sec.ui.open ? 'Done' : '+ Add dishes')),
        sec.ui.open && dishPicker(sec.items, pool, sec.ui, drawChips),
        sec.ui.open && addNew,
        // Optional: each dish here comes with one of these (e.g. rice or noodles).
        h('div', { class: 'with-edit' },
          h('span', { class: 'muted small' }, sec.with.length ? 'Comes with one of:' : 'Comes with: nothing'),
          withChips, h('span', { style: 'flex:1' }),
          h('button', { class: sec.withUi.open ? 'on' : '', onclick: () => { sec.withUi.open = !sec.withUi.open; draw(); } },
            sec.withUi.open ? 'Done' : 'Choose')),
        sec.withUi.open && dishPicker(sec.with, withable, sec.withUi, drawWith));
    }));
    draw();
    const save = async () => {
      fieldErr(name); fieldErr(price);
      if (!name.value.trim()) { fieldErr(name, 'Type a name'); return 'Type a name'; }
      let cents;
      try { cents = parseMoney(price.value); } catch (e) { fieldErr(price, e.message); return `Price: ${e.message}`; }
      const t = Number(String(tax.value).replace(',', '.').replace('%', '') || 0);
      if (!Number.isFinite(t) || t < 0 || t > 100) return 'Tax: from 0 to 100';
      if (!sections.length) return 'Add at least one section';
      const bad = sections.find((x) => !x.name.trim() || !x.items.length);
      if (bad) return `${bad.name || 'A section'}: give it a name and at least one dish`;
      const fields = { name: name.value, name_alt: alt.value.trim() || null, category_id: cat.value ? Number(cat.value) : null,
        price_cents: cents, cost_cents: 0, tax_rate_bp: Math.round(t * 100), track_stock: false, starter: false, prep: false, team_id: null };
      const saved = p
        ? await call('update_product', { id: p.id, product: { ...fields, sku: p.sku, barcode: p.barcode, active: active.input.checked } }, { quiet: true })
        : await call('create_product', { product: { ...fields, stock_qty: 0 } }, { quiet: true });
      try {
        await call('save_set', { setId: saved.id, sections: sections.map((x) => ({
          name: x.name.trim(), name_alt: x.alt.trim() || null, pick: x.pick, starter: x.starter, product_ids: x.items, with_ids: x.with })) }, { quiet: true });
      } catch (e) {
        // A new set that could not be saved must not stay behind as a plain item.
        if (!p) await call('delete_product', { id: saved.id }).catch(() => {});
        throw e;
      }
      toast(`${saved.name} saved`);
      setTimeout(changed);
    };
    const del = p && isAdmin() && confirmBtn('Delete', 'Tap again to delete', async () => {
      await call('delete_product', { id: p.id });
      s.close();
      toast(`${p.name} deleted`);
      await changed();
    }, 'ghost danger');
    const s = sheet({ title: p ? p.name : 'New set menu', wide: true, onSave: save, saveLabel: 'Save set', left: del,
      body: [ssec(null, [field('Name', name), field('Thai name', alt), field('Category', cat),
        field(taxIncluded ? 'Price' : 'Price before tax', price, { hint: taxIncluded ? 'Tax included' : '' }), field('Tax %', tax), p && active],
      'The bill shows the set and its price. Options with a price add to it.'),
      h('h3', { class: 'ssec-title set-secs' }, 'Sections'), secList,
      h('button', { class: 'ladd set-add', onclick: () => { sections.push({ name: '', alt: '', pick: 1, starter: false, items: [], with: [] }); draw(); } }, icon('plus', 18), 'Section')] });
    if (!p) tapFocus(name);
  }

  state.openEditor = editProduct;

  // ---- paste a whole menu from a spreadsheet ----
  function pasteImport() {
    const text = h('textarea', { rows: 8, 'aria-label': 'Menu rows', placeholder: 'Name\tPrice\tCategory\tThai name\nPad Thai\t9.50\tMains\tผัดไทย\nSpring rolls\t4.50\tStarters\tปอเปี๊ยะ\nCola\t2.50\tDrinks' });
    const out = h('div', { class: 'stack' });
    let rows = [];
    const parse = () => {
      rows = text.value.split(/\r?\n/).map((l) => l.trim()).filter(Boolean).map((line, i) => {
        // Spreadsheets paste tab-separated; typed lines use commas ("quoted, names" allowed).
        const cells = line.includes('\t') ? line.split('\t') : splitCsv(line);
        const [nm = '', priceText = '', category = '', alt = ''] = cells.map((c) => c.trim());
        let price = null, error = null;
        try { price = parseMoney(priceText); } catch { error = priceText ? `"${priceText}" is not a price` : 'no price'; }
        if (!nm) error = 'no name';
        return { line: i + 1, name: nm, alt, price, category, error };
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
      fresh.length > 0 && h('div', { class: 'muted' }, `New categories: ${fresh.join(', ')}`));
      if (s) { s.save.disabled = !rows.length || bad.length > 0; s.save.textContent = bad.length ? `Fix ${bad.length} line(s)` : `Import ${rows.length || ''} item(s)`.replace('  ', ' '); }
    };
    text.addEventListener('input', parse);
    let s = null;
    s = sheet({ title: 'Paste a menu', wide: true, saveLabel: 'Import',
      body: [ssec(null, [srow({ label: 'Rows', hint: 'Name, price, category, Thai name', tip: 'Copy rows from Excel or Google Sheets, or type one item per line like "Pad Thai, 9.50, Mains, ผัดไทย". Missing categories are created.', stacked: true, ctl: text })]), out],
      onSave: async () => {
        const r = await call('import_items', { rows: rows.map((x) => ({ name: x.name, name_alt: x.alt || null, price_cents: x.price, category: x.category || null })) }, { quiet: true });
        toast(`${r.items} items imported` + (r.new_categories.length ? `, ${r.new_categories.length} new categories` : ''));
        setTimeout(changed);
      } });
    parse();
    tapFocus(text);
  }

  let searchTimer;
  search.addEventListener('input', () => { clearTimeout(searchTimer); searchTimer = setTimeout(drawProducts, 150); });
  // ⋯ More: the rarer things.
  const moreBtn = moreMenu('the menu', () => [
    h('button', { onclick: act(() => editSet(null)) }, '+ Set menu'),
    h('button', { onclick: pasteImport }, 'Paste a menu'),
    h('button', { onclick: () => { state.menuRemoved = !state.menuRemoved; loadProducts(); } }, state.menuRemoved ? 'Hide removed' : 'Show removed'),
    isAdmin() && h('button', { onclick: () => bulkDelete('Delete menu items', (state.menuShown ?? []).map((p) => ({
      id: p.id, label: p.name, sub: [catById[p.category_id]?.name, money(p.price_cents)].filter(Boolean).join(' · '),
    })), (ids) => deleteMany('product', ids, 'item(s)'), () => changed()) }, 'Delete several…'),
    isAdmin() && h('button', { onclick: () => openManage('g:data', 'menu') }, 'Menu backups'),
  ]);
  moreBtn.classList.add('mmore');
  screen({ action: h('button', { class: 'primary', onclick: () => editProduct(null) }, icon('plus', 18), 'Item') },
    h('div', { class: 'menu-md' }, catPane,
      h('div', { class: 'mmain' }, catChips,
        h('div', { class: 'mtools' }, h('label', { class: 'searchbox grow' }, icon('search', 20), search), moreBtn),
        h('div', { class: 'md-list' }, listHead, list))));
  drawCats();
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
  // Changes on this floor must be saved or discarded before going elsewhere (the save bar).
  const busy = () => !leaveOk();
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
    tapFocus(name);
  };
  state.dirty = false;
  if (!floors.length) {
    return screen(h('div', { class: 'page' }, h('div', { class: 'sgroup' }, emptyState({ icon: 'plan', title: 'No floors yet', text: 'Add a floor, e.g. Ground floor, then its tables.',
      action: h('button', { class: 'primary', onclick: newFloor }, icon('plus', 18), 'Add a floor') }))));
  }
  const floor = floors.find((f) => f.id === state.floorId);
  const { w: PW, h: PH } = state.plan;
  const SNAP = 10;
  let tables = await call('list_tables', { floorId: floor.id });
  let sel = null;
  const plan = planBox(250);
  plan.classList.add('edit');
  const panel = h('div', { class: 'plan-side' });
  // Changes wait in the save bar under the plan: "Layout changed · Discard · Save layout".
  const bar = saveBar({ text: 'Layout changed', saveLabel: 'Save layout', hold: true, onSave: () => save(), onDiscard: () => setTimeout(renderPlan) });
  const floorTools = [];
  const setDirty = (d) => bar.update(d ? 1 : 0);
  // While the bar shows (it holds Discard and Save layout), This floor and Delete floors make room.
  const observeBar = new MutationObserver(() => { for (const b of floorTools) if (b) b.hidden = !bar.hidden; });
  observeBar.observe(bar, { attributes: true, attributeFilter: ['hidden'] });
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
      setKids(panel, h('div', { class: 'sgroup plan-empty' }, emptyState({ icon: 'plan', title: 'Tap a table to change it',
        text: `Drag to move. ${floor.name}: ${n} table${n === 1 ? '' : 's'}${tables.length > n ? `, ${tables.length - n} other` : ''}.` })));
      return;
    }
    const t = sel;
    const name = h('input', { value: t.name, maxlength: 20, 'aria-label': 'Name' });
    const label = () => (t.is_table === false ? t.name : `Table ${t.name}`);
    const title = h('h3', {}, label());
    // Applied as soon as something is typed or picked, so Save can be tapped straight away.
    const apply = () => {
      t.name = name.value.trim() || t.name;
      t.x = clamp(t.x, 0, PW - t.w);
      t.y = clamp(t.y, 0, PH - t.h);
      title.textContent = label();
      setDirty(true);
      drawTables();
    };
    name.addEventListener('input', apply);
    const size = (key, lbl) => srow({ label: lbl, ctl: stepper({ value: t[key], min: 30, max: 400, step: SNAP, label: lbl, onChange: (v) => { t[key] = clamp(snap(v), 30, 400); apply(); } }) });
    // Off: a shape that only shows where something is (the bar, a window, the door).
    const kind = switchRow({ label: 'This is a table', hint: (v) => (v ? 'Counted on Tables' : 'Only shows a place'), value: t.is_table !== false,
      onChange: (v) => { t.is_table = v; apply(); } });
    setKids(panel, h('div', { class: 'plan-card' },
      h('div', { class: 'plan-card-head' }, title,
        h('button', { class: 'icon-btn ghost', title: 'Done', 'aria-label': 'Done', onclick: () => { sel = null; draw(); } }, icon('x', 20))),
      h('div', { class: 'sgroup' },
        srow({ label: 'Name', cls: 'text-row', ctl: name }),
        srow({ label: 'Shape', ctl: sseg([['rect', 'Square'], ['round', 'Round']], t.shape, (v) => { t.shape = v; apply(); }, 'Shape') }),
        size('w', 'Width'), size('h', 'Depth'), kind),
      h('div', { class: 'plan-card-actions' },
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
    draw();
    toast('Floor plan saved');
  }

  // Floors across the top, the tools under them, then the plan with its card at the side.
  screen(h('div', { class: 'page plan-page' },
    h('div', { class: 'plan-bar' },
      segmented(floors.map((f) => [f.id, f.name]), floor.id, (id) => { if (busy()) return; state.floorId = id; renderPlan(); }),
      h('button', { onclick: newFloor }, icon('plus', 18), 'Floor'),
      h('span', { class: 'grow' }),
      bar,
      floorTools[0] = h('button', { onclick: floorSettings }, icon('settings', 18), 'This floor'),
      floorTools[1] = isAdmin() && floors.length > 1 && h('button', { class: 'danger-outline', onclick: () => {
        if (busy()) return;
        bulkDelete('Delete floors', floors.map((f) => ({ id: f.id, label: f.name })),
          (ids) => deleteMany('floor', ids, 'floor(s)'), () => { setDirty(false); state.floorId = null; return renderPlan(); });
      } }, 'Delete floors…')),
    h('div', { class: 'plan-tools' },
      h('button', { onclick: addShape('rect') }, h('span', { class: 'shape-ico sq' }), 'Square table'),
      h('button', { onclick: addShape('round') }, h('span', { class: 'shape-ico rd' }), 'Round table'),
      h('button', { onclick: addShape('rect', false) }, h('span', { class: 'shape-ico area' }), 'Area (not a table)')),
    h('div', { class: 'plan-editor' }, plan, panel)));
  draw();
}

// ---------- users (admin) ----------

async function renderUsers() {
  state.dirty = false;
  const ROLES = [['staff', 'Staff'], ['manager', 'Manager'], ['admin', 'Admin']];
  const ROLE_HINT = { staff: 'Takes orders and payments', manager: 'Also menu, reports, voids and kitchen settings', admin: 'Everything' };
  let users = await call('list_users');
  const list = h('div', { class: 'olist', role: 'list', 'aria-label': 'Staff' });
  const detail = h('div', { class: 'md-body' });
  const meta = (u) => [ROLE_LABEL[u.role] ?? 'Staff', u.active ? (u.last_login_at ? `in ${friendlyTime(u.last_login_at)}` : 'not in yet') : 'switched off'].join(' · ');
  const drawList = () => setKids(list, users.map((u) => {
    const row = h('div', { class: `urow${u.id === state.staffSel ? ' on' : ''}${u.active ? '' : ' off'}`, role: 'listitem', tabindex: 0, 'data-id': u.id,
      onclick: () => pick(u.id), onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); pick(u.id); } } },
    h('span', { class: 'avatar' }, initials(u.name)),
    h('span', { class: 'u-text' }, h('strong', {}, u.name), h('small', {}, meta(u))),
    !u.active && h('span', { class: 'pill' }, 'Off'),
    h('span', { class: 'chev' }, icon('chev', 18)));
    return row;
  }));
  const pick = (id) => { state.staffSel = id; drawList(); drawDetail(); md.show(); };
  // Saves go through update_user with the whole person; your own change logs in again (name, role).
  const saveUser = async (u, patch) => {
    const next = { ...u, ...patch };
    const saved = await call('update_user', { id: u.id, name: next.name, role: next.role, active: next.active }, { quiet: true });
    Object.assign(u, saved);
    drawList();
    // The server has your new name and role already (or logged you out): show them.
    if (u.id === state.session.user_id) setTimeout(() => { if (!saved.active) forgetSession(); start(); }, 300);
  };
  function drawDetail() {
    const u = users.find((x) => x.id === state.staffSel);
    if (!u) return setKids(detail, h('div', { class: 'sgroup md-empty' }, emptyState({ icon: 'users', title: 'Tap a person to see them' })));
    const me = u.id === state.session.user_id;
    const name = textRow({ id: 'name', label: 'Name', value: u.name, max: 40, validate: (v) => (v ? null : 'Type a name'),
      save: (v) => saveUser(u, { name: v }), after: () => { title.textContent = u.name; } });
    // Demoting yourself asks first, in a strip under the role.
    const strip = h('div', { class: 'srow-strip', hidden: true });
    const role = segRow({ id: 'role', label: 'Role', hint: ROLE_HINT[u.role], items: ROLES, value: u.role, stacked: true,
      save: async (v) => {
        if (me && u.role === 'admin' && v !== 'admin') {
          setKids(strip, h('span', { class: 'grow' }, 'You will lose admin.'),
            h('button', { class: 'ghost', onclick: () => { strip.hidden = true; rowState(role, 'idle'); } }, 'Keep'),
            h('button', { class: 'danger solid', onclick: () => { strip.hidden = true; role.seg.set(v); doRole(v); } }, 'Change'));
          strip.hidden = false;
          throw new Error('confirm below');
        }
        await saveUser(u, { role: v });
      },
      after: (v) => setHint(role, ROLE_HINT[v]) });
    const doRole = (v) => saveWith(role, () => saveUser(u, { role: v }), () => role.seg.set(u.role)).then((ok) => { if (ok) setHint(role, ROLE_HINT[v]); });
    const active = switchRow({ id: 'active', label: 'Can log in', hint: (v) => (v ? '' : 'Off: cannot log in'), value: u.active, save: (v) => saveUser(u, { active: v }) });
    const pinRow = navRow({ id: 'pin', label: 'PIN', value: 'Change', onOpen: (row) => pinSheet(u, row) });
    const title = h('h3', {}, u.name);
    setKids(detail, h('div', { class: 'ucard' },
      h('div', { class: 'ucard-head' }, h('span', { class: 'avatar lg' }, initials(u.name)), title, me && h('span', { class: 'pill plain' }, 'You')),
      ssec('Details', [name, role, strip, active, pinRow])));
  }
  // The PIN in a sheet, with the shared-PIN warning as one line.
  const pinField = (u) => {
    const pin = h('input', { type: 'password', inputmode: 'numeric', maxlength: 8, autocomplete: 'off', placeholder: '4 to 8 digits', 'aria-label': 'PIN' });
    const shared = h('div', { class: 'warn-box', hidden: true });
    let t;
    pin.addEventListener('input', () => {
      clearTimeout(t);
      const v = pin.value;
      if (!/^\d{4,8}$/.test(v)) { shared.hidden = true; return; }
      t = setTimeout(async () => {
        const names = await invoke('pin_shared', { pin: v, exceptUserId: u?.id ?? null }).catch(() => []);
        if (pin.value !== v) return;
        shared.hidden = !names.length;
        shared.textContent = names.length ? `Also used by ${names.join(', ')}. Better one PIN each.` : '';
      }, 200);
    });
    return { pin, shared, ok: () => /^\d{4,8}$/.test(pin.value) };
  };
  function pinSheet(u, row) {
    const f = pinField(u);
    const s = sheet({ title: `New PIN for ${u.name}`, saveLabel: 'Save PIN', body: [ssec(null, [srow({ label: 'PIN', cls: 'text-row', ctl: f.pin })]), f.shared],
      onSave: async () => {
        if (!f.ok()) return 'Use 4 to 8 digits';
        await call('reset_pin', { id: u.id, pin: f.pin.value }, { quiet: true });
        rowState(row, 'saved');
      } });
    f.pin.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); if (f.ok()) s.save.click(); else OSK?.hide?.(); } });
    tapFocus(f.pin);
  }
  // + Staff member: name, role and PIN in a sheet.
  function addSheet() {
    const name = h('input', { placeholder: 'Name', maxlength: 40, 'aria-label': 'Name', enterkeyhint: 'next' });
    const role = sseg(ROLES, 'staff', (v) => setHint(roleRow, ROLE_HINT[v]), 'Role');
    const roleRow = srow({ label: 'Role', hint: ROLE_HINT.staff, stacked: true, ctl: role });
    const f = pinField(null);
    const s = sheet({ title: 'New staff member', saveLabel: 'Add',
      body: [ssec(null, [srow({ label: 'Name', cls: 'text-row', ctl: name }), roleRow, srow({ label: 'PIN', cls: 'text-row', ctl: f.pin })]), f.shared],
      onSave: async () => {
        if (!name.value.trim()) { tapFocus(name); return 'Type a name'; }
        if (!f.ok()) return 'PIN: use 4 to 8 digits';
        const made = await call('create_user', { name: name.value, role: role.value(), pin: f.pin.value }, { quiet: true });
        users = await call('list_users');
        state.staffSel = made.id;
        drawList();
        drawDetail();
        md.show();
      } });
    // The keyboard's Done (Enter) moves on to the PIN; Enter on a complete form adds.
    name.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); if (name.value.trim()) tapFocus(f.pin); } });
    f.pin.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); if (name.value.trim() && f.ok()) s.save.click(); } });
    tapFocus(name);
  }
  if (!users.some((u) => u.id === state.staffSel)) state.staffSel = wideQ.matches ? users[0]?.id ?? null : null;
  const md = masterDetail({ cls: 'md-staff', back: 'Staff', list: h('div', { class: 'md-list' }, list), detail });
  drawList();
  drawDetail();
  screen({ action: h('button', { class: 'primary', onclick: addSheet }, icon('plus', 18), 'Staff member') }, md);
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
async function renderShift() {
  let [cur, past] = await Promise.all([call('current_shift'), call('list_shifts', { limit: 20 }).catch(() => [])]);
  const moneyIn = (label, placeholder = '0.00') => h('input', { inputmode: 'decimal', placeholder, 'aria-label': label, class: 'money-in' });
  const cents = (el) => { try { return el.value.trim() ? parseMoney(el.value) : null; } catch { return null; } };
  let main;
  let refreshNow = async () => true;
  if (!cur) {
    // No shift: one card to open it with the float.
    const float = moneyIn('Cash in the drawer', 'e.g. 100.00');
    const open = act(async () => {
      const c = cents(float);
      if (c == null) { nudge(float); return toast('Type the cash in the drawer (0 if empty)', true); }
      await call('open_shift', { floatCents: c });
      toast('Shift open');
      renderShift();
    });
    float.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); if (float.value.trim()) open(e); } });
    main = h('div', { class: 'scard shift-start' },
      h('div', { class: 'scard-head' }, h('span', { class: 'lead' }, icon('shift', 22)), h('div', {}, h('h3', {}, 'Start a shift'), h('span', { class: 'muted' }, 'Count the float first.'))),
      h('label', { class: 'money-field' }, h('span', {}, 'Cash in the drawer'), float),
      h('button', { class: 'primary lg', onclick: open }, 'Open shift'));
  } else {
    // Money in / out: amount and reason in a sheet.
    const move = (kind) => {
      const amt = moneyIn('Amount');
      const why = h('input', { placeholder: kind === 'in' ? 'e.g. more change' : 'e.g. milk, supplier', 'aria-label': 'Reason', maxlength: 80 });
      sheet({ title: kind === 'in' ? 'Money in' : 'Money out', saveLabel: kind === 'in' ? 'Put in' : 'Take out',
        body: ssec(null, [srow({ label: 'Amount', cls: 'text-row', ctl: amt }), srow({ label: 'Reason', hint: kind === 'in' ? 'Optional' : '', cls: 'text-row', ctl: why })]),
        onSave: async () => {
          const c = cents(amt);
          if (!c) return 'Type an amount';
          const r = await call('cash_move', { kind, amountCents: c, reason: why.value }, { quiet: true });
          toast(`${kind === 'in' ? 'Money in' : 'Money out'}: ${money(c)}` + (r.print.error ? ` (drawer did not open: ${r.print.error})` : ''));
          setTimeout(renderShift);
        } });
      tapFocus(amt);
    };
    const line = (label, c, cls = '') => h('div', { class: `sum-row ${cls}` }, h('span', {}, label), h('strong', {}, money(c)));
    const count = moneyIn('Cash counted');
    const note = h('input', { placeholder: 'Note (optional)', 'aria-label': 'Note', maxlength: 120 });
    const diff = h('span', { class: 'pill plain diff', hidden: true });
    const showDiff = () => {
      const c = cents(count);
      diff.hidden = c == null;
      if (c == null) return;
      const d = c - cur.expected_cents;
      diff.className = `pill diff ${d === 0 ? 'ok' : d < 0 ? 'bad' : 'plain'}`;
      diff.textContent = d === 0 ? 'Exactly right' : d < 0 ? `Short ${money(-d)}` : `Over ${money(d)}`;
    };
    count.addEventListener('input', showDiff);
    const expected = h('span', { class: 'muted small' }, `Expected ${money(cur.expected_cents)}`);
    const closeBtn = h('button', { class: 'primary lg' }, icon('printer', 18), 'Close shift & print');
    let sureTimer;
    const unsure = () => { clearTimeout(sureTimer); delete closeBtn.dataset.sure; closeBtn.classList.remove('danger', 'solid'); closeBtn.classList.add('primary'); setKids(closeBtn, icon('printer', 18), 'Close shift & print'); };
    count.addEventListener('input', () => { if (closeBtn.dataset.sure) unsure(); });
    const close = act(async () => {
      const c = cents(count);
      if (c == null) { nudge(count); return toast('Type the cash you counted', true); }
      // The drawer as it is now: a payment taken on another till since this page
      // was drawn changes what should be there. Changed: shown, and asked again.
      const was = cur.expected_cents;
      if (!await refreshNow()) return;
      if (cur.expected_cents !== was) {
        unsure();
        return toast(`Expected cash changed to ${money(cur.expected_cents)} (was ${money(was)}): a payment or money move since. Check the count, then close again.`, true, 7000);
      }
      // Short: a second tap closes it, so a typing slip is not printed as missing cash.
      if (c < cur.expected_cents && !closeBtn.dataset.sure) {
        closeBtn.dataset.sure = '1';
        closeBtn.classList.remove('primary'); closeBtn.classList.add('danger', 'solid');
        setKids(closeBtn, `Short ${money(cur.expected_cents - c)}: tap again to close`);
        sureTimer = setTimeout(unsure, 6000);
        return;
      }
      clearTimeout(sureTimer);
      const r = await call('close_shift', { countedCents: c, note: note.value });
      toast('Shift closed' + (r.print.error ? ` (report did not print: ${r.print.error})` : r.print.attempted ? ' · report printed' : ''), !!r.print.error);
      await renderShift();
      reportSheet('Shift closed', r.text, r.shift?.id ? () => call('print_shift', { id: r.shift.id }) : null);
    });
    const nowCard = () => h('section', { class: 'scard' },
        h('div', { class: 'scard-title' }, h('h3', {}, 'Drawer now'), h('span', { class: 'pill ok' }, 'Open')),
        h('div', { class: 'muted small' }, `Since ${friendlyTime(cur.opened_at)} · ${cur.opened_by_name} · ${cur.payment_count} payment${cur.payment_count === 1 ? '' : 's'}`),
        h('div', { class: 'kpi-big' }, h('small', {}, 'Should be in the drawer'), h('strong', {}, money(cur.expected_cents))),
        h('div', { class: 'scard-acts' }, h('button', { onclick: () => move('in') }, icon('plus', 18), 'Money in'), h('button', { onclick: () => move('out') }, icon('minus', 18), 'Money out')),
        h('div', { class: 'sum-rows' },
          line('Float', cur.float_cents), line(`${payName('cash')} sales`, cur.cash_cents),
          cur.cash_tip_cents ? line(`${payName('cash')} tips`, cur.cash_tip_cents) : null,
          cur.paid_in_cents ? line('Money in', cur.paid_in_cents) : null, cur.paid_out_cents ? line('Money out', -cur.paid_out_cents) : null,
          cur.refund_cents ? line(`${payName('cash')} refunds`, -cur.refund_cents) : null,
          h('div', { class: 'sum-sep' }),
          line(payName('card'), cur.card_cents), cur.card_tip_cents ? line(`${payName('card')} tips`, cur.card_tip_cents) : null,
          cur.other_cents ? line('Delivery apps / other', cur.other_cents) : null),
        cur.moves.length > 0 && h('div', { class: 'moves' }, cur.moves.map((m) => h('div', { class: 'move' },
          h('span', {}, [m.kind === 'in' ? 'Money in' : 'Money out', m.reason, m.user_name].filter(Boolean).join(' · ')),
          h('strong', {}, `${m.kind === 'in' ? '' : '−'}${money(m.amount_cents)}`)))));
    let nowSec = nowCard();
    // The figures again from the server (every 10 s while this page shows, and
    // right before closing). False when the shift was closed elsewhere (redrawn).
    refreshNow = async () => {
      const fresh = await call('current_shift');
      if (!fresh || fresh.id !== cur.id) { toast('This shift was closed on another till', true); renderShift(); return false; }
      if (JSON.stringify(fresh) !== JSON.stringify(cur)) {
        cur = fresh;
        const next = nowCard();
        nowSec.replaceWith(next);
        nowSec = next;
        expected.textContent = `Expected ${money(cur.expected_cents)}`;
        showDiff();
      }
      return true;
    };
    main = h('div', { class: 'shift-grid' },
      nowSec,
      h('section', { class: 'scard' },
        h('div', { class: 'scard-title' }, h('h3', {}, 'Close shift')),
        h('label', { class: 'money-field' }, h('span', {}, 'Cash counted'), count),
        h('div', { class: 'diff-row' }, expected, diff),
        note,
        closeBtn));
    closeBtn.onclick = close;
  }
  const closed = past.filter((p) => p.shift.closed_at);
  const t = (x) => new Date(x * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  screen(h('div', { class: 'spage' }, main,
    closed.length > 0 && ssec('Past shifts', closed.map((p) => {
      const d = p.shift.difference_cents;
      return srow({ id: `shift${p.shift.id}`, label: `${friendlyTime(p.shift.opened_at).replace(/^./, (c) => c.toUpperCase())} – ${t(p.shift.closed_at)}`, cls: 'brow',
        hint: `${p.shift.closed_by_name} · Expected ${money(p.shift.expected_cents)}`,
        ctl: [h('span', { class: `pill ${d === 0 ? 'ok' : d < 0 ? 'bad' : 'plain'}` }, d === 0 ? 'Exactly right' : `${d < 0 ? 'Short' : 'Over'} ${money(Math.abs(d))}`),
          moreMenu('this shift', () => [h('button', { onclick: () => reportSheet(friendlyTime(p.shift.opened_at), p.text, () => call('print_shift', { id: p.shift.id })) }, 'View'),
            h('button', { onclick: act(async () => printResult(await call('print_shift', { id: p.shift.id }), 'Shift report printed')) }, 'Print')])] });
    }))));
  if (!cur && wideQ.matches) main.querySelector('input')?.focus();
  if (cur) live(async () => { if (document.getElementById('modal').hidden && !document.hidden && main.isConnected) await refreshNow(); }, 10000);
}

async function renderReports() {
  // The report text (a day close, a shift) in a sheet, with Print where it can be printed again.
  const eod = h('div', { class: 'eod' });
  const kpis = h('div', { class: 'kpis kpis5' });
  const period = h('h3', { class: 'ssec-title rperiod' });
  const cards = h('div', { class: 'rcards' });
  let mode = 'today', picked = today();
  const dayOf = () => (mode === 'today' ? today() : mode === 'yesterday' ? isoDate(new Date(Date.now() - 86400000)) : picked);
  const dayBtn = h('button', { class: 'daybtn', 'aria-label': 'Pick a day', onclick: () => pickDay() });
  const seg = sseg([['today', 'Today'], ['yesterday', 'Yesterday']], mode, (v) => { mode = v; load(); }, 'Day');
  const drawDayBtn = () => {
    dayBtn.classList.toggle('on', mode === 'day');
    setKids(dayBtn, icon('calendar', 20), mode === 'day' && h('span', {}, dayLabel(picked)));
    seg.set(mode === 'day' ? null : mode);
  };
  function pickDay() {
    const date = h('input', { type: 'date', value: picked, max: today(), 'aria-label': 'Day' });
    sheet({ title: 'Sales on one day', saveLabel: 'Show', body: ssec(null, [srow({ label: 'Day', ctl: date, cls: 'text-row' })]), onSave: () => {
      if (!date.value) return 'Pick a day';
      picked = date.value; mode = 'day'; load();
    } });
  }
  const bars = (title, rows, empty = 'No sales yet') => {
    const max = Math.max(1, ...rows.map((r) => r.v));
    return h('section', { class: 'rcard' }, h('h3', { class: 'ssec-title' }, title),
      rows.length ? h('div', { class: 'bars' }, rows.map((r) => h('div', { class: 'bar-row' },
        h('span', { class: 'bar-l' }, r.label), h('strong', {}, r.text),
        h('span', { class: 'meter', role: 'img', 'aria-label': `${Math.round(r.v * 100 / (r.sum || max))}%` }, h('span', { class: 'fill', style: `width:${Math.max(2, Math.round(r.v * 100 / max))}%` })))))
        : h('div', { class: 'rnone' }, empty));
  };
  const share = (list, label) => {
    const sum = list.reduce((n, g) => n + g.total_cents, 0);
    return list.map((g) => ({ label: label(g.key), v: g.total_cents, sum, text: `${g.count} · ${money(g.total_cents)}` }));
  };

  // End of day: the open business day so far, the close button, past closes.
  let prevOpen = false;
  const drawEod = async () => {
    const day = await call('business_day');
    const now = Math.floor(Date.now() / 1000) + 1;
    const [s, closes] = await Promise.all([call('sales_summary', { from: day.since, to: now }), call('day_closes', { limit: 30 })]);
    // "Other" payments are the delivery apps': by its name when there was only one.
    const platforms = s.by_channel.filter((c) => c.key !== 'Direct');
    const apps = platforms.length === 1 ? platforms[0].key : 'Delivery apps';
    const closeBtn = h('button', { class: 'primary', disabled: day.open_orders > 0, onclick: () => sheet({ title: `Close ${dayLabel(day.date)}?`, saveLabel: 'Close day',
      body: ssec(null, [srow({ label: `${s.order_count} paid · ${money(s.total_cents)}`, hint: 'Prints the summary and starts the next day.' })]),
      onSave: async () => {
        const r = await call('close_day', {}, { quiet: true });
        await drawBusinessDay();
        const c = r.view.close;
        const msg = `${dayLabel(c.business_date)} closed: ${c.summary.order_count} orders, ${money(c.summary.total_cents)}`;
        if (r.empty) toast(`${dayLabel(c.business_date)} closed. No sales, nothing printed.`);
        else if (r.print.error) toast(`${msg}. The summary did not print (${r.print.error})`, true);
        else toast(r.print.attempted ? `${msg} · summary printed` : `${msg} · no receipt printer`);
        setTimeout(async () => { await drawEod(); if (!r.empty) reportSheet(`${dayLabel(c.business_date)}`, r.view.text, () => call('reprint_day_close', { id: c.id })); });
      } }) }, 'Close day');
    setKids(eod,
      h('div', { class: 'eod-main' },
        h('div', { class: 'eod-text' }, h('strong', {}, `Business day · ${dayLabel(day.date)}`),
          h('span', {}, `Since ${friendlyTime(day.since)} · ${s.order_count} paid · ${money(s.total_cents)}`),
          s.by_payment.length > 0 && h('span', { class: 'eod-chips' }, s.by_payment.map((g) => h('span', { class: 'chip' }, `${payName(g.key, apps)} ${money(g.total_cents)}`)))),
        day.open_orders > 0 && h('button', { class: 'pill bad pill-btn', onclick: () => go('orders') }, `${day.open_orders} open order${day.open_orders === 1 ? '' : 's'}`),
        closeBtn),
      h('details', { class: 'eod-prev', open: prevOpen }, h('summary', { onclick: (e) => { prevOpen = !e.currentTarget.parentNode.open; } }, icon('chev', 16), 'Previous days', h('small', {}, `${closes.length}`)),
        h('div', { class: 'eod-list' }, !closes.length && h('div', { class: 'rnone eod-none' }, 'No day closed yet'), closes.map((v) => srow({ label: dayLabel(v.close.business_date), cls: 'brow',
          hint: `${v.close.closed_by_name} · ${ordersText(v.close.summary.order_count)} · ${friendlyTime(v.close.to)}`,
          ctl: [h('strong', { class: 'eod-total' }, money(v.close.summary.total_cents)),
            moreMenu(dayLabel(v.close.business_date), () => [h('button', { onclick: () => reportSheet(dayLabel(v.close.business_date), v.text, () => call('reprint_day_close', { id: v.close.id })) }, 'View'),
              h('button', { onclick: act(async () => printResult(await call('reprint_day_close', { id: v.close.id }), 'Summary printed')) }, 'Print')])] })))));
  };

  let loads = 0;
  const load = act(async () => {
    drawDayBtn();
    const mine = ++loads;
    const [from, to] = dayRange(dayOf());
    const [s, top] = await Promise.all([call('sales_summary', { from, to }), call('top_products', { from, to, limit: 20 })]);
    if (mine !== loads) return;
    const pay = (k) => s.by_payment.find((g) => g.key === k)?.total_cents ?? 0;
    period.textContent = `${{ today: 'Today · ', yesterday: 'Yesterday · ' }[mode] ?? ''}${dayLabel(dayOf())}`;
    const kpi = (label, value, sub = null) => h('div', { class: 'kpi' }, h('small', {}, label), h('strong', {}, value), sub && h('span', { class: 'kpi-sub' }, sub));
    setKids(kpis,
      kpi('Sales', money(s.total_cents)),
      kpi('Orders', String(s.order_count), s.voided_count ? `${s.voided_count} voided` : null),
      kpi('Average bill', s.order_count ? money(Math.round(s.total_cents / s.order_count)) : '—'),
      kpi('Tips', money(s.tip_cents), 'Not in sales'),
      kpi(`${payName('cash')} · ${payName('card')}`, `${money(pay('cash'))} · ${money(pay('card'))}`));
    const line = (k, v) => h('div', { class: 'sum-row' }, h('span', {}, k), h('strong', {}, v));
    setKids(cards,
      h('section', { class: 'rcard' }, h('h3', { class: 'ssec-title' }, 'Summary'), h('div', { class: 'sum-rows' },
        line('Paid orders', s.order_count), line(s.tax_included_cents ? 'Sales (menu prices)' : 'Gross', money(s.subtotal_cents)), line('Discounts', money(s.discount_cents)),
        s.tax_cents - s.tax_included_cents ? line('Tax added on top', money(s.tax_cents - s.tax_included_cents)) : null,
        line('Service charge', money(s.service_cents)), h('div', { class: 'sum-row grand' }, h('span', {}, 'Total'), h('strong', {}, money(s.total_cents))),
        s.tip_cents ? line('Tips (not in total)', money(s.tip_cents)) : null,
        s.tax_included_cents ? line('Tax included in prices', money(s.tax_included_cents)) : null,
        line('Voided / refunded', `${s.voided_count} · ${money(s.voided_total_cents)}`), line('Cancelled', s.cancelled_count))),
      bars('By payment', share(s.by_payment, (k) => payName(k, 'Other / platform'))),
      bars('By order type', share(s.by_type, (k) => TYPE[k] ?? k)),
      s.by_channel.length > 0 && bars('By delivery app', share(s.by_channel, (k) => k)),
      bars('By category (before tax)', share(s.by_category, (k) => k)),
      bars('Top items', top.map((t) => ({ label: t.name, v: t.revenue_cents, text: `${t.qty} · ${money(t.revenue_cents)}` }))));
  });
  screen({ action: h('div', { class: 'dhead-tools' }, seg, dayBtn) }, h('div', { class: 'rpage' }, eod, h('div', { class: 'rperiod-wrap' }, period, kpis), cards));
  await Promise.all([drawEod(), load()]);
}
/// A printed report (day close, shift) as text in a sheet; `print()` prints it again.
function reportSheet(title, text, print = null) {
  sheet({ title, body: h('pre', { class: 'report' }, text),
    left: print && h('button', { onclick: act(async () => printResult(await print(), 'Printed')) }, icon('printer', 18), 'Print') });
}

// ---------- settings (admin) ----------

// ---------- customers ----------

const ordersText = (n) => `${n} order${n === 1 ? '' : 's'}`;
async function renderCustomers() {
  state.dirty = false;
  const q = h('input', { type: 'search', placeholder: 'Phone, name or postcode', 'aria-label': 'Search customers', enterkeyhint: 'search' });
  const list = h('div', { class: 'olist', role: 'list', 'aria-label': 'Customers' });
  const panel = h('div', { class: 'md-body' });
  const blank = () => setKids(panel, h('div', { class: 'sgroup md-empty' }, emptyState({ icon: 'customers', title: 'Tap a customer to see them' })));
  let timer;
  let found = [];
  let openId = null;
  const load = async () => {
    found = await call('search_customers', { query: q.value, limit: 100 });
    setKids(list, found.length ? found.map((c) => h('div', { class: `urow crow${c.id === openId ? ' on' : ''}`, role: 'listitem', tabindex: 0, 'data-id': c.id,
      onclick: () => edit(c), onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); edit(c); } } },
    h('span', { class: 'avatar' }, initials(c.name || '?')),
    h('span', { class: 'u-text' }, h('strong', {}, c.name || c.phone || '—'), h('small', {}, [c.phone, c.postcode].filter(Boolean).join(' · ') || addressText(c))),
    c.order_count > 0 ? h('span', { class: 'pill plain' }, ordersText(c.order_count)) : h('span', {}),
    h('span', { class: 'chev' }, icon('chev', 18))))
      : emptyState({ icon: 'customers', title: q.value ? 'Nothing found' : 'No saved customers', text: q.value ? 'Try a phone number or postcode.' : 'They are added with delivery and collection orders.',
        action: !q.value && h('button', { class: 'primary', onclick: () => edit(null) }, icon('plus', 18), 'Customer') }));
  };
  q.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(() => load().catch(() => {}), 150); });
  const mark = (id) => { openId = id; for (const r of list.querySelectorAll('.crow')) r.classList.toggle('on', Number(r.dataset.id) === id); };

  async function edit(c) {
    if (!leaveOk()) return;
    mark(c?.id ?? null);
    const ce = customerEditor({ customer: c, search: false, saveToggle: false, rows: true });
    const history = h('div');
    const orderFor = (type) => act(async () => {
      await ce.commit(true);
      bar.update(0);
      await startCustomerOrder(type, ce, null);
    });
    // Typed changes: the save bar (and Manage keeps you here until saved or let go).
    const bar = saveBar({ text: 'Not saved yet', onSave: async () => { const saved = await ce.commit(true); toast('Saved'); bar.update(0); await load(); await edit(saved); },
      onDiscard: () => setTimeout(() => (c ? edit(c) : (blank(), md.close(true)))) });
    ce.el.addEventListener('input', () => bar.update(1));
    bar.classList.add('askable');
    bar.closeTo = () => { mark(null); blank(); md.close(true); };
    setKids(panel, h('div', { class: 'ccard' },
      h('div', { class: 'ucard-head' }, h('span', { class: 'avatar lg' }, initials(c?.name || '+')),
        h('div', { class: 'ucard-title' }, h('h3', {}, c ? c.name || c.phone : 'New customer'), c && h('small', {}, ordersText(c.order_count))),
        c && isAdmin() && moreMenu(c.name || 'customer', () => [confirmBtn('Delete', 'Tap again to delete', async () => {
          closeInfoPop();
          await call('delete_customer', { id: c.id }); bar.update(0); toast('Customer removed'); mark(null); blank(); md.close(true); await load();
        }, 'ghost danger')])),
      ce.el,
      bar,
      c && (typeShown('delivery') || typeShown('collection')) && h('div', { class: 'ccard-acts' },
        typeShown('delivery') && h('button', { onclick: orderFor('delivery') }, icon('bike', 18), 'New delivery'),
        typeShown('collection') && h('button', { onclick: orderFor('collection') }, icon('walk', 18), 'New collection')),
      history));
    md.show();
    if (c) {
      const orders = await call('customer_orders', { id: c.id });
      setKids(history, orders.length ? ssec('Recent orders', orders.map((o) => h('div', { class: 'orow', role: 'button', tabindex: 0, onclick: () => { if (leaveOk()) openOrder(o.id); } },
        h('span', { class: 'o-main' }, h('span', { class: 'o-l1' }, h('strong', {}, `#${o.id}`), h('span', { class: 'o-time' }, friendlyTime(o.opened_at)), typeChip(o.order_type)),
          h('span', { class: 'o-l2' }, statusPill(o.status))),
        h('strong', { class: 'o-total' }, money(o.total_cents)), h('span', { class: 'chev' }, icon('chev', 18))))) : '');
    }
    if (!c) { ce.focus(); tapFocus(document.activeElement); }
  }

  const md = masterDetail({ cls: 'md-customers', back: 'Customers',
    list: [h('div', { class: 'md-tools' }, h('label', { class: 'searchbox grow' }, icon('search', 20), q),
      isAdmin() && h('button', { onclick: () => bulkDelete('Delete customers', found.map((c) => ({ id: c.id, label: c.name, sub: c.phone ?? '' })),
        (ids) => deleteMany('customer', ids, 'customer(s)'), async () => { if (!state.dirty) { mark(null); blank(); } await load(); }) }, 'Select…')),
    h('div', { class: 'md-list' }, list)],
    detail: panel });
  blank();
  screen({ action: h('button', { class: 'primary', onclick: () => edit(null) }, icon('plus', 18), 'Customer') }, md);
  await load();
  if (wideQ.matches) q.focus();
}

/// Printer "Image mode" choices (saved value, label), numbered as on the
/// Thai test page so "the one marked 2" is the one to pick. Standard is the default.
const IMAGE_MODES = [['column', '1. Compatible (older printers)'], ['raster_small', '2. Small pieces'], ['raster', '3. Standard (default)']];

// ---------- Settings: Devices & display ----------

async function devicesGroup() {
  const raw = await invoke('get_settings', { keys: ['start_fullscreen', 'button_size'] });
  const srv = state.remote ? null : await call('server_status', {}, { quiet: true }).catch(() => null);
  const autostart = state.remote ? null : await call('get_autostart', {}, { quiet: true }).catch(() => null);
  const theme = segRow({ id: 'theme', label: 'Theme', hint: 'This device only', items: [['light', 'Light'], ['dark', 'Dark'], ['auto', 'Auto']], value: themePref(),
    stacked: false, cls: 'seg-fold', save: (v) => setTheme(v) });
  // The moon / sun button at the top changes it too: follow it while the row is on screen.
  const follow = () => { if (theme.isConnected) theme.seg.set(themePref()); else document.removeEventListener('themechange', follow); };
  document.addEventListener('themechange', follow);
  return [
    ssec('This device', [
      theme,
      textRow({ id: 'device_name', label: "This device's name", hint: 'Shown on tickets', value: pref(DEVICE_NAME_KEY, ''), placeholder: deviceLabel(), max: 40,
        save: async (v) => { setPref(DEVICE_NAME_KEY, v); } }),
      !state.remote && autostart != null && switchRow({ id: 'autostart', label: 'Start with Windows', hint: 'Opens POS when the PC starts', value: autostart,
        save: (v) => call('set_autostart', { enabled: v }, { quiet: true }) }),
      !state.remote && keySwitch(raw, 'start_fullscreen', 'Full screen at start', { def: false }),
      !state.remote && srow({ id: 'fullscreen', label: 'Full screen', hint: 'Or press F11', ctl: h('button', { class: 'test', onclick: act(toggleFullscreen) }, icon('expand', 18), 'On / off') }),
    ]),
    ssec('All devices', [segRow({ id: 'buttons', label: 'Menu button size', hint: 'Large: fewer tiles fit', items: [['normal', 'Normal'], ['large', 'Large']],
      value: raw.button_size === 'large' ? 'large' : 'normal', save: (v) => saveKey('button_size', v),
      after: (v) => { if (state.ui) state.ui.bigButtons = v === 'large'; document.body.classList.toggle('big-buttons', v === 'large'); } })]),
    srv && ssec('Other devices', serverRows(srv)),
  ];
}
/// Local server: status, [Start] / [Stop], the sheet for port and start-up; the addresses while it runs.
function serverRows(st) {
  // Only Start is one tap: stopping cuts off every tablet and the kitchen screen, so it
  // lives in the sheet behind a second tap.
  const start = async (row) => {
    rowState(row, 'saving');
    try { await call('set_server', { settings: st.settings, run: true }, { quiet: true }); await redrawGroup('server'); } catch (e) { rowState(row, 'error', String(e?.message ?? e), true); }
  };
  const pill = st.running ? { text: 'Running', cls: 'ok' } : st.error ? { text: 'Error', cls: 'bad' } : { text: 'Stopped' };
  const row = statusRow({ id: 'server', icon: 'wifi', title: 'Local server', pill, onOpen: () => serverSheet(st),
    meta: st.error && !st.running ? st.error : `Port ${st.settings.port} · ${st.settings.auto_start ? 'Starts with the POS' : 'Start by hand'}`,
    action: !st.running && h('button', { class: 'test', onclick: () => start(row) }, 'Start') });
  return [row, st.running && st.urls.length > 0 && srow({ id: 'urls', label: 'Open on a tablet', stacked: true, cls: 'urls-row',
    ctl: h('div', { class: 'server-urls' }, st.urls.map((u) => h('code', {}, u))) })];
}
function serverSheet(st) {
  const f = { port: String(st.settings.port), auto: !!st.settings.auto_start };
  const port = h('input', { value: f.port, inputmode: 'numeric', class: 'short-in', 'aria-label': 'Port' });
  port.addEventListener('input', () => { f.port = port.value; });
  const apply = async (on) => {
    const n = Number(f.port);
    if (!Number.isInteger(n) || n < 1024 || n > 65535) return 'Use a port from 1024 to 65535';
    await call('set_server', { settings: { port: n, auto_start: f.auto }, run: on }, { quiet: true });
    redrawGroup('server');
  };
  let sure = false, sureTimer;
  const stop = st.running && h('button', { class: 'ghost danger', onclick: async () => {
    if (!sure) {
      sure = true;
      stop.textContent = 'Tap again: stop · tablets disconnect';
      sureTimer = setTimeout(() => { sure = false; stop.textContent = 'Stop server'; }, 5000);
      return;
    }
    clearTimeout(sureTimer);
    const m = await apply(false).catch((e) => String(e?.message ?? e));
    if (m) { s.show(m, false); sure = false; stop.textContent = 'Stop server'; } else s.close();
  } }, 'Stop server');
  const left = st.running
    ? stop
    : h('button', { onclick: async () => { const m = await apply(true).catch((e) => String(e?.message ?? e)); if (m) s.show(m, false); else s.close(); } }, 'Start');
  const s = sheet({ title: 'Local server', left, saveLabel: st.running ? 'Save & restart' : 'Save', onSave: () => apply(st.running), body: [
    ssec(null, [srow({ label: 'Port', cls: 'text-row', ctl: port }),
      switchRow({ label: 'Start with the POS', value: f.auto, onChange: (v) => { f.auto = v; } })],
    'The first time, Windows may ask: choose Allow on private networks.'),
  ] });
}

const printResult = (r, ok) => toast(r.error ? `Did not print: ${r.error}` : ok, !!r.error);

// ---------- Manage: one shell (side list + detail pane) ----------
// Wide screens (≥ 1000 px): the side list and the open page or settings group side
// by side. Phones: the list, then the page on its own with ‹ Manage to go back.

/// Daily work pages, in the order they are used.
const MANAGE_DAILY = ['tickets', 'history', 'reports', 'shift', 'menuAdmin', 'customers', 'kitchen', 'plan', 'users', 'log'];
/// Settings groups. `need`: who sees the group (its loosest row): 'all',
/// 'kitchen' (managers and admins: the server's kitchen settings) or 'admin'.
const MANAGE_GROUPS = [
  { id: 'printers', icon: 'printer', title: 'Printers', need: 'admin', build: printersGroup },
  { id: 'payments', icon: 'receipt', title: 'Payments & receipts', need: 'admin', build: paymentsGroup },
  { id: 'orders', icon: 'orders', title: 'Orders', need: 'kitchen', build: ordersGroup },
  { id: 'kitchen', icon: 'chef', title: 'Kitchen', need: 'kitchen', build: kitchenGroup, subs: { teams: { title: 'Teams', build: teamsPage } } },
  { id: 'login', icon: 'lock', title: 'Login & staff', need: 'admin', build: loginGroup },
  { id: 'restaurant', icon: 'store', title: 'Restaurant', need: 'admin', build: restaurantGroup },
  { id: 'delivery', icon: 'bike', title: 'Delivery apps', need: 'admin', build: deliveryGroup },
  // Start with Windows, full screen and the local server are this PC's own (not on a phone or tablet).
  { id: 'devices', icon: 'monitor', title: 'Devices & display', need: 'admin', build: devicesGroup },
  { id: 'data', icon: 'database', title: 'Backup & data', need: 'admin', build: dataGroup,
    subs: { backups: { title: 'All backups', build: () => backupsPage('full') }, menus: { title: 'All menu backups', build: () => backupsPage('menu') } } },
  // Checking for and installing updates is for admins only.
  { id: 'about', icon: 'info', title: 'About', need: 'all', build: aboutGroup },
];
const needOk = (need) => !need || need === 'all' || (need === 'kitchen' || need === 'manager' ? isManager() : isAdmin());
const visibleGroups = () => MANAGE_GROUPS.filter((g) => needOk(g.need));
const visiblePages = () => MANAGE_DAILY.filter((id) => canSee(VIEWS.find(([v]) => v === id)?.[2]));
/// Staff have two things in Manage (Kitchen screen, About): two cards, no side list.
const useShell = () => isManager();
const wideQ = matchMedia('(min-width: 1000px)');

/// What this device opened last in Manage ("g:printers", "p:history").
const MANAGE_LAST = 'pos.manage.last';
const lastManage = () => { try { return localStorage.getItem(MANAGE_LAST); } catch { return null; } };
const rememberManage = (item) => { try { localStorage.setItem(MANAGE_LAST, item); } catch {} };
const pref = (k, def = null) => { try { return localStorage.getItem(k) ?? def; } catch { return def; } };
const setPref = (k, v) => { try { localStorage.setItem(k, v); } catch {} };
const manageItem = () => (state.view === 'settings' ? (state.settingsTab ? `g:${state.settingsTab}` : null) : `p:${state.view}`);

/// Unsaved changes in a list with a save bar (or the floor plan) keep you on the page.
function leaveOk() {
  if (state.dirty) {
    toast('Save or discard first', true);
    const bar = document.querySelector('.savebar:not([hidden])');
    // Scrolled away (a phone, down at the table editor): bring it back into view.
    if (bar) { bar.scrollIntoView({ block: 'nearest' }); nudge(bar); }
    return false;
  }
  return true;
}

// Phones: list → group → sheet are history steps, so the phone's back button works.
const backStack = [];
let ignorePops = 0;
const phoneQ = matchMedia('(max-width: 999px)');
function pushBack(kind) {
  if (!phoneQ.matches) return;
  backStack.push(kind);
  history.pushState({ posBack: backStack.length }, '');
}
/// The step was closed on screen: drop its history entry without acting on it.
function popBack(kind) {
  if (backStack.at(-1) !== kind) return;
  backStack.pop();
  ignorePops++;
  history.back();
}
function clearBack() {
  const n = backStack.length;
  if (!n) return;
  backStack.length = 0;
  ignorePops++;
  history.go(-n);
}
window.addEventListener('popstate', () => {
  if (ignorePops) { ignorePops--; return; }
  const kind = backStack.pop();
  if (kind === 'sheet') return state.sheetBack?.();
  if (kind === 'item') return state.itemBack?.();
  if (kind === 'sub') {
    if (!leaveOk()) { pushBack('sub'); return; }
    state.groupSub = null;
    return renderShell();
  }
  if (kind === 'detail' && state.session && !state.orderId && manageItem()) {
    if (!leaveOk()) { pushBack('detail'); return; }
    manageBack();
  }
});
/// ‹ Manage on a phone: back to the list.
function manageBack() {
  state.view = 'settings';
  state.settingsTab = null;
  state.groupSub = null;
  renderShell();
}
const tapBack = () => { if (!leaveOk()) return; if (['detail', 'item'].includes(backStack.at(-1))) history.back(); else manageBack(); };

/// Open a page ("p:history") or a settings group ("g:printers"), optionally
/// showing one of its rows (a search result).
function openManage(item, row = null) {
  if (!leaveOk()) return;
  const fromList = !wideQ.matches && !manageItem();
  const id = item.slice(2);
  if (item.startsWith('p:')) { state.view = id; state.settingsTab = null; } else { state.view = 'settings'; state.settingsTab = id; }
  state.groupSub = null;
  state.findRow = row;
  if (fromList) pushBack('detail');
  // A page folds the list to the rail (below 1366 px): its search results have no room.
  if (mshell && (mshell.el.classList.contains('peek') || item.startsWith('p:')) && mshell.find.value) {
    mshell.find.value = '';
    mshell.find.dispatchEvent(new Event('input'));
  }
  mshell?.el.classList.remove('peek');
  renderShell();
}

let mshell = null;
/// Nothing of Manage outlives a login: unsaved list changes (the save bar's
/// state.dirty), the open group, a search result to show, the side list, the
/// phone's back steps, the Menu page's tab, order mode, removed items and
/// open editor, the person picked in Staff. Called on every logout and login.
function resetManage() {
  Object.assign(state, { dirty: false, settingsTab: null, groupSub: null, findRow: null, sheetBack: null, itemBack: null, groupGen: (state.groupGen ?? 0) + 1,
    menuTab: null, menuShown: null, menuReorder: false, menuRemoved: false, newSubOf: null, openEditor: null, staffSel: null,
    ticketSel: null, ticketTab: null, logFilter: null, histOpen: null });
  ticketsLoggedOut();
  mshell = null;
  if (backStack.length) clearBack();
}
/// The shell is built once per login: the side list keeps its scroll and search text.
function manageScreen(kids) {
  if (!mshell || mshell.user !== state.session.user_id || mshell.role !== state.session.role) mshell = buildShell();
  if (mshell.el.parentNode !== app || app.childElementCount !== 1) setKids(app, mshell.el);
  const item = manageItem();
  if (item) rememberManage(item);
  if (item?.startsWith('g:')) setPref('pos.manage.lastGroup', item);
  for (const el of mshell.el.querySelectorAll('[data-item]')) {
    const on = el.dataset.item === item || (el.dataset.item === 'rail-settings' && item?.startsWith('g:'));
    el.classList.toggle('on', on);
    if (on && el.dataset.item !== 'rail-settings') el.setAttribute('aria-current', 'page'); else el.removeAttribute('aria-current');
  }
  mshell.el.classList.toggle('rail', state.view !== 'settings');
  mshell.el.classList.toggle('at-list', !item);
  mshell.el.classList.toggle('at-detail', !!item);
  if (mshell.item !== item) { mshell.detail.scrollTop = 0; if (!wideQ.matches) app.scrollTop = 0; mshell.item = item; }
  setKids(mshell.detail, kids);
  drawTicketBadges();
  // The selected item in the middle of the side list when it is near an edge (a short
  // till shows a few rows either side of it). Only the list scrolls, never the page.
  const sel = item && wideQ.matches && mshell.el.querySelector(`.mnav-wide [data-item="${item}"]`);
  const side = mshell.el.querySelector('.mside');
  if (sel && sel.offsetParent) {
    const top = sel.getBoundingClientRect().top - side.getBoundingClientRect().top;
    const head = side.querySelector('.mside-search')?.offsetHeight ?? 0;
    if (top < head + 88 || top + sel.offsetHeight > side.clientHeight - 88) side.scrollTop += top - (side.clientHeight + head - sel.offsetHeight) / 2;
  }
}

/// Detail header: [‹ Manage on phones] [icon] Title … [action]. A level-2 page
/// (`up`: { label, onclick }) shows its way back on every screen, without the icon.
function dhead(ic, title, action = null, up = null) {
  return h('div', { class: `dhead${up ? ' sub' : ''}` },
    up ? h('button', { class: 'ghost back show', onclick: up.onclick, 'aria-label': `Back to ${up.label}` }, icon('back', 22), h('span', {}, up.label))
      : h('button', { class: 'ghost back', onclick: tapBack, 'aria-label': 'Back to Manage' }, icon('back', 22), h('span', {}, 'Manage')),
    ic && !up && h('span', { class: 'dicon' }, icon(ic, 20)), h('h2', {}, title), action);
}
/// A group's level-2 page (Kitchen › Teams, Backup › All backups): a history step on phones.
function openSub(id) {
  if (!leaveOk()) return;
  state.groupSub = id;
  pushBack('sub');
  renderShell();
}
function closeSub() {
  if (!leaveOk()) return;
  if (backStack.at(-1) === 'sub') popBack('sub');
  state.groupSub = null;
  renderShell();
}

function buildShell() {
  const el = h('div', { class: 'manage' });
  const detail = h('div', { class: 'mdetail' });
  const find = h('input', { type: 'search', placeholder: 'e.g. printer, tax, PIN', 'aria-label': 'Search Manage', enterkeyhint: 'search' });
  const hits = h('div', { class: 'mnav-hits', hidden: true, role: 'list' });
  const pages = visiblePages();
  const groups = visibleGroups();
  const name = (id) => VIEWS.find(([v]) => v === id)?.[1];
  // On the rail (a page open, 1000-1365 px) each icon has a short name under it.
  const RAIL_NAME = { shift: 'Shift', kitchen: 'Kitchen', plan: 'Floor', menuAdmin: 'Menu', log: 'Activity' };
  const item = (data, ic, label, cls = '') => h('button', { class: `mnav-item ${cls}`, 'data-item': data, onclick: () => openManage(data), title: label },
    icon(ic, 20), h('span', {}, label), h('small', { class: 'rail-label', 'aria-hidden': 'true' }, RAIL_NAME[data.slice(2)] ?? label));
  let folded = pref('pos.manage.fold') === '1';
  const dailyItems = h('div', { class: 'mnav-daily', hidden: folded }, pages.map((id) => item(`p:${id}`, id, name(id))));
  const foldBtn = h('button', { class: 'mnav-title fold', 'aria-expanded': String(!folded), onclick: () => {
    folded = !folded;
    dailyItems.hidden = folded;
    foldBtn.setAttribute('aria-expanded', String(!folded));
    setPref('pos.manage.fold', folded ? '1' : '0');
  } }, h('span', {}, 'Daily work'), h('span', { class: 'chev' }, icon('chev', 16)));
  // The rail (a page open below 1366 px): search, the page icons and one Settings icon.
  const railSettings = h('button', { class: 'mnav-item rail-only', 'data-item': 'rail-settings', title: 'Settings', 'aria-label': 'Settings', onclick: () => {
    const last = pref('pos.manage.lastGroup');
    const g = groups.find((x) => `g:${x.id}` === last) ?? groups[0];
    openManage(`g:${g.id}`);
  } }, icon('settings', 20), h('small', { class: 'rail-label', 'aria-hidden': 'true' }, 'Settings'));
  const railFind = h('button', { class: 'mnav-item rail-only', title: 'Search', 'aria-label': 'Search', onclick: () => { el.classList.add('peek'); tapFocus(find); } },
    icon('search', 20), h('small', { class: 'rail-label', 'aria-hidden': 'true' }, 'Search'));
  const wide = h('nav', { class: 'mnav-wide', 'aria-label': 'Manage' },
    railFind,
    pages.length > 0 && [foldBtn, dailyItems],
    h('div', { class: 'mnav-title' }, 'Settings'),
    groups.map((g) => item(`g:${g.id}`, g.icon, g.title, 'grp')),
    h('hr', { class: 'rail-only' }), railSettings);
  // Phones: the pages as tiles, the settings groups as a list.
  const phone = h('div', { class: 'mnav-phone' },
    pages.length > 0 && [h('div', { class: 'mnav-title' }, 'Daily work'),
      h('div', { class: 'dgrid' }, pages.map((id) => h('button', { class: 'dtile', 'data-item': `p:${id}`, onclick: () => openManage(`p:${id}`) }, icon(id, 20), h('span', {}, name(id)))))],
    h('div', { class: 'mnav-title' }, 'Settings'),
    h('div', { class: 'sgroup' }, groups.map((g) => {
      const r = h('div', { class: 'srow nav grp-row', role: 'button', tabindex: 0, 'data-item': `g:${g.id}`, onclick: () => openManage(`g:${g.id}`),
        onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openManage(`g:${g.id}`); } } },
      h('span', { class: 'srow-text' }, h('span', { class: 'srow-label' }, h('span', { class: 'grp-ic' }, icon(g.icon, 20)), g.title)),
      h('span', { class: 'sstate' }), h('span', { class: 'srow-ctl' }, h('span', { class: 'chev' }, icon('chev', 20))));
      return r;
    })));
  const lists = h('div', { class: 'mnav-lists' }, wide, phone);
  const search = () => {
    const typed = find.value.trim();
    lists.hidden = !!typed;
    hits.hidden = !typed;
    if (!typed) return;
    const { shown, locked } = searchManage(typed);
    setKids(hits, shown.length ? shown.map((e) => h('button', { class: 'mnav-item mnav-hit', role: 'listitem', onclick: () => openManage(e.item, e.row) },
      icon(e.icon, 20), h('span', {}, e.label, h('small', {}, e.where))))
      : h('div', { class: 'sempty' }, h('span', { class: 'eic' }, icon(locked ? 'lock' : 'search', 26)),
        h('strong', {}, locked ? 'Admin only' : 'Nothing found'),
        h('p', {}, locked ? `${typed.charAt(0).toUpperCase()}${typed.slice(1)} is changed by an admin. Ask an admin to log in on this till.` : 'Try printer, tax or PIN.')));
  };
  find.addEventListener('input', search);
  find.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { const one = hits.querySelectorAll('.mnav-hit'); if (one.length === 1) one[0].click(); }
    if (e.key === 'Escape' && find.value) { e.stopPropagation(); find.value = ''; search(); }
  });
  const side = h('aside', { class: 'mside' }, h('div', { class: 'mside-search' }, h('label', { class: 'searchbox' }, icon('search', 20), find)), lists, hits);
  const scrim = h('div', { class: 'mscrim', onclick: () => el.classList.remove('peek') });
  setKids(el, side, scrim, detail);
  setKids(app, el);
  return { el, detail, find, user: state.session.user_id, role: state.session.role, item: undefined };
}
// Across 1000 px (a tablet turned): the list on its own needs a page next to it.
wideQ.addEventListener?.('change', () => {
  if (wideQ.matches && state.session && !state.orderId && state.view === 'settings' && !state.settingsTab && useShell() && !document.body.classList.contains('locked')) renderShell();
});

/// The Manage tab: the last group or page this device opened (wide screens), the list (phones).
async function renderSettings() {
  if (!useShell()) return renderStaffManage();
  if (!state.settingsTab) {
    if (!wideQ.matches) { drawTitle('Manage'); return manageScreen([]); }
    const last = lastManage();
    const page = last?.startsWith('p:') && visiblePages().includes(last.slice(2)) && last.slice(2);
    if (page) { state.view = page; return renderShell(); }
    const groups = visibleGroups();
    state.settingsTab = (groups.find((g) => `g:${g.id}` === last) ?? groups[0]).id;
  }
  const g = visibleGroups().find((x) => x.id === state.settingsTab);
  if (!g) { state.settingsTab = null; return renderSettings(); }
  drawTitle('Manage');
  const gen = (state.groupGen = (state.groupGen ?? 0) + 1);
  const sub = state.groupSub && g.subs?.[state.groupSub];
  if (!sub) state.groupSub = null;
  const body = h('div', { class: 'sbody', 'data-group': g.id, 'data-sub': sub ? state.groupSub : null });
  manageScreen([h('div', { class: 'sbody-wrap' }, sub ? dhead(null, sub.title, null, { label: g.title, onclick: closeSub }) : dhead(g.icon, g.title), body)]);
  const kids = await (sub ? sub.build(g) : g.build(g));
  if (gen !== state.groupGen) return;
  setKids(body, kids);
  const row = state.findRow;
  state.findRow = null;
  if (row) showRow(body, row);
}
/// Draw the open settings group again (after a sheet saved), keeping the scroll.
async function redrawGroup(savedRow = null) {
  const body = document.querySelector('.mdetail .sbody');
  const g = MANAGE_GROUPS.find((x) => x.id === body?.dataset.group);
  if (!g) return;
  const sub = body.dataset.sub && g.subs?.[body.dataset.sub];
  const kids = await (sub ? sub.build(g) : g.build(g));
  setKids(body, kids);
  if (savedRow) rowState(body.querySelector(`[data-row="${savedRow}"]`), 'saved');
}
/// A search result: open Advanced if the row is in it, bring it into view and flash it.
function showRow(body, id) {
  let row = body.querySelector(`[data-row="${id}"]`);
  // A row hidden while its master switch is off (the team rows): show the switch.
  if (row?.hidden && row.dataset.dep) row = body.querySelector(`[data-row="${row.dataset.dep}"]`);
  if (!row) return;
  const adv = row.closest('.adv-body');
  if (adv?.hidden) adv.previousElementSibling?.click();
  row.scrollIntoView({ block: 'center' });
  row.classList.remove('found');
  void row.offsetWidth;
  row.classList.add('found');
  setTimeout(() => row.classList.remove('found'), 2100);
}

/// Staff: Kitchen screen and About as two cards.
function renderStaffManage() {
  if (state.settingsTab === 'about') {
    drawTitle('About', 'Manage');
    const back = () => { state.settingsTab = null; renderShell(); };
    return setKids(app, h('div', { class: 'staff-manage' },
      h('div', { class: 'dhead' }, h('button', { class: 'ghost back show', onclick: back }, icon('back', 22), h('span', {}, 'Manage')), h('h2', {}, 'About')),
      creditsCard()));
  }
  drawTitle('Manage');
  setKids(app, h('div', { class: 'staff-manage' },
    h('button', { class: 'dtile dtile-big', onclick: () => go('kitchen') }, icon('kitchen', 26), h('span', {}, 'Kitchen screen')),
    h('button', { class: 'dtile dtile-big', 'data-tickets': '', onclick: () => myTicketsSheet() }, icon('chat', 26), h('span', {}, 'My tickets'), countEl(tk.info?.unread_mine ?? tk.info?.unread)),
    h('button', { class: 'dtile dtile-big', onclick: () => { state.settingsTab = 'about'; renderShell(); } }, icon('info', 26), h('span', {}, 'About'))));
}

// ---------- Manage search ----------

/// Every page, group and setting row, with the words people use for them.
/// `need` as for the groups ('manager' for pages managers open); `local`:
/// not on a browser terminal; `row`: the row to show in its group.
const MANAGE_INDEX = [
  ['p:history', 'History', 'paid orders reprint receipt void refund past old bills delete', 'manager'],
  ['p:reports', 'Reports', 'sales money totals end of day close day summary takings', 'manager'],
  ['p:shift', 'Shift & cash', 'shift float cash count drawer money in out till', 'manager'],
  ['p:menuAdmin', 'Menu', 'menu items dishes food drinks categories prices sets options sizes extras sold out thai names paste starter dessert', 'manager'],
  ['p:customers', 'Customers', 'customer phone address postcode delivery saved', 'manager'],
  ['p:kitchen', 'Kitchen screen', 'kitchen screen tablet kds cook chef done display', false],
  ['p:plan', 'Floor plan', 'floor plan tables layout rooms terrace seats', 'manager'],
  ['p:users', 'Staff', 'staff users people pin password roles manager admin waiter', 'admin'],
  ['p:tickets', 'Tickets', 'tickets contact admin help problem messages reply urgent staff support', 'manager'],
  ['p:log', 'Activity log', 'activity log audit history who cancelled voided deleted refund pin logins changes needs a look', 'manager'],
  ['g:printers', 'Printers', 'printer printers receipt kitchen bar ticket copies paper thai image test cash drawer windows usb network', 'admin'],
  ['g:printers#receipt', 'Receipt printer', 'receipt bill printer copies paper 58 80 mm item names thai english bold image mode raster slow send test', 'admin'],
  ['g:printers#kitchen', 'Kitchen printer', 'kitchen printer tickets copies starters mains all courses together call mains again prepare added later additional large text font table image mode raster slow send test', 'admin'],
  ['g:printers#addroute', 'Add kitchen printer', 'bar dessert drinks extra other second printer route team', 'admin'],
  ['g:printers#drawer', 'Cash drawer', 'cash drawer open test', 'admin'],
  ['g:payments', 'Payments & receipts', 'pay payment payments cash card tips split exact receipt bill service charge', 'admin'],
  ['g:payments#pay_default_method', 'Opens on', 'finish pay default payment method cash card', 'admin'],
  ['g:payments#exact_cash_pays', '“Exact” pays at once', 'exact cash amount', 'admin'],
  ['g:payments#split_max', 'Split evenly up to', 'split evenly people bill', 'admin'],
  ['g:payments#cash_quick', 'Cash buttons', 'cash quick amounts notes money', 'admin'],
  ['g:payments#tips_on', 'Ask for tips', 'tip tips gratuity', 'admin'],
  ['g:payments#tip_presets', 'Tip buttons', 'tip tips percent', 'admin'],
  ['g:payments#pay_name_cash', 'Cash is called', 'payment names rename cash thai', 'admin'],
  ['g:payments#pay_name_card', 'Card is called', 'payment names rename card qr', 'admin'],
  ['g:payments#auto_print', 'Print after payment', 'receipt automatic print', 'admin'],
  ['g:payments#drawer_on_cash', 'Open drawer for cash', 'cash drawer open', 'admin'],
  ['g:payments#staff_copy', 'Staff copy after payment', 'staff copy tax class receipt', 'admin'],
  ['g:payments#tax_breakdown', 'VAT breakdown on bills', 'vat tax breakdown receipt bill', 'admin'],
  ['g:payments#show_staff', 'Show “Served by”', 'served by staff name receipt bill', 'admin'],
  ['g:payments#footer', 'Bottom message', 'footer thank you message receipt bill', 'admin'],
  ['g:payments#service', 'Service charge', 'service charge percent rate', 'admin'],
  ['g:orders', 'Orders', 'order types discounts starter main course quick notes cancel cancelled fix sent', 'kitchen'],
  ['g:orders#show_takeaway', 'Takeaway', 'order types takeaway button', 'admin'],
  ['g:orders#show_collection', 'Collection', 'order types collection button', 'admin'],
  ['g:orders#show_delivery', 'Delivery', 'order types delivery own drivers button', 'admin'],
  ['g:orders#labels', 'Names on tickets', 'words labels dine-in takeaway collection delivery table call mains tickets bills', 'admin'],
  ['g:orders#discount_presets', 'Discount buttons', 'discount discounts percent', 'admin'],
  ['g:orders#discount_staff_max_pct', 'Staff limit', 'discount discounts limit manager pin percent', 'manager'],
  ['g:orders#starter_choice', 'Ask “Starter or main?”', 'starter main course courses', 'kitchen'],
  ['g:orders#quick_notes', 'Quick notes', 'quick notes no onions extra spicy', 'kitchen'],
  ['g:orders#print_cancelled_order', 'Print CANCELLED ticket', 'cancel cancelled cancelling kitchen ticket', 'kitchen'],
  ['g:orders#staff_cancel_sent', 'Staff can fix sent dishes', 'fix sent remove reduce dish', 'admin'],
  ['g:kitchen', 'Kitchen', 'kitchen screen kds teams team drinks desserts tickets orange red refresh beep sound curry rice station sample', 'kitchen'],
  ['g:kitchen#drinks_split', 'Drinks on their own ticket', 'drinks own ticket bar split desserts', 'kitchen'],
  ['g:kitchen#desserts_split', 'Desserts on their own ticket', 'desserts own ticket split drinks', 'kitchen'],
  ['g:kitchen#teams_on', 'Use kitchen teams', 'kitchen teams team station on off', 'kitchen'],
  ['g:kitchen#teams', 'Teams', 'kitchen teams team names thai station curry rice grill order list add', 'kitchen'],
  ['g:kitchen#teams_split', 'Own ticket per team', 'teams team own separate ticket headers split', 'kitchen'],
  ['g:kitchen#team_names', 'Team names', 'team names english thai language tickets', 'kitchen'],
  ['g:kitchen#team_choice', 'Ask “Which team?”', 'which team choose teams', 'kitchen'],
  ['g:kitchen#sample', 'Sample tickets', 'sample test print teams tickets', 'kitchen'],
  ['g:kitchen#kds_slow_mins', 'Orange after', 'kitchen screen kds orange slow minutes colour color late', 'kitchen'],
  ['g:kitchen#kds_late_mins', 'Red after', 'kitchen screen kds red late minutes colour color', 'kitchen'],
  ['g:kitchen#kds_refresh_secs', 'Refresh every', 'kitchen screen kds refresh seconds', 'kitchen'],
  ['g:kitchen#kds_paid_hours', 'Keep paid orders', 'kitchen screen kds paid orders hours remove', 'kitchen'],
  ['g:kitchen#kds_beep', 'Beep for new tickets', 'kitchen screen kds beep sound new ticket', 'kitchen'],
  ['g:login', 'Login & staff', 'login log in logout auto lock pin start screen staff timeout', 'admin'],
  ['g:login#start_screen', 'Start screen', 'after login start screen tables orders takeaway', 'admin'],
  ['g:login#auto_logout_minutes', 'Log out after', 'auto logout idle timeout lock', 'admin'],
  ['g:login#auto_logout_warn_secs', 'Warn before', 'warning logout seconds', 'admin'],
  ['g:login#pin_auto_login', 'Log in without OK', 'pin auto login ok', 'admin'],
  ['g:login#staff', 'Staff & PINs', 'staff users people roles pin', 'admin'],
  ['g:login#tickets_call_text', 'Who staff call when urgent', 'tickets contact admin phone call urgent number', 'admin'],
  ['g:restaurant', 'Restaurant', 'restaurant name address phone vat tax rate prices included currency', 'admin'],
  ['g:restaurant#store_name', 'Restaurant name', 'restaurant shop name', 'admin'],
  ['g:restaurant#shop_name', 'Name on bills', 'shop name bill receipt top', 'admin'],
  ['g:restaurant#header', 'Address & phone', 'address phone vat number header bill receipt', 'admin'],
  ['g:restaurant#currency', 'Currency symbol', 'currency pound baht euro dollar money symbol', 'admin'],
  ['g:restaurant#prices_include_tax', 'Prices include tax', 'tax vat prices include included added on top', 'admin'],
  ['g:restaurant#default_tax_bp', 'Default tax rate', 'tax vat rate percent default apply items 0', 'admin'],
  ['g:delivery', 'Delivery apps', 'delivery apps uber eats just eat deliveroo grab platform', 'admin'],
  ['g:devices', 'Devices & display', 'theme dark light button size large full screen display local server network tablet phone wifi', 'admin'],
  ['g:devices#theme', 'Theme', 'theme light or dark mode night auto colours', 'admin'],
  ['g:devices#device_name', "This device's name", 'device name tablet till label tickets', 'admin'],
  ['g:devices#buttons', 'Menu button size', 'button size large big tiles', 'admin'],
  ['g:devices#autostart', 'Start with Windows', 'start with windows startup boot automatically open pc switched on login', 'admin', true],
  ['g:devices#start_fullscreen', 'Full screen at start', 'full screen start open window display', 'admin', true],
  ['g:devices#fullscreen', 'Full screen', 'full screen display window f11', 'admin', true],
  ['g:devices#server', 'Local server', 'local server network tablet phone browser wifi port start stop', 'admin', true],
  ['g:devices#urls', 'Open on a tablet', 'address url tablet phone browser local server', 'admin', true],
  ['g:data', 'Backup & data', 'backup restore reset delete data menu copy export import move new pc usb', 'admin'],
  ['g:data#backup', 'Back up now', 'backup last backup restore copy', 'admin'],
  ['g:data#backups_kept', 'Keep backups', 'keep how many backups daily old delete', 'admin'],
  ['g:data#fromfile', 'Restore from a file', 'restore backup file upload new pc usb', 'admin'],
  ['g:data#folder', 'Backup folder', 'backup folder path usb new pc move open', 'admin'],
  ['g:data#menu', 'Back up the menu', 'menu backup copy restore', 'admin'],
  ['g:data#menufile', 'Load a menu file', 'menu file load import other till', 'admin'],
  ['g:data#reset', 'Reset data', 'reset delete clear data danger', 'admin'],
  ['g:data#log_keep_days', 'Keep for (Activity log)', 'log keep days retention activity space tickets clear old', 'admin'],
  ['g:data#backup_hourly_on', 'Back up while open', 'auto automatic backup hourly every hours schedule', 'admin'],
  ['g:data#backup_dayclose_on', 'Back up at day close', 'backup day close end of day automatic', 'admin'],
  ['g:data#backup_exit_on', 'Back up when POS closes', 'backup close exit shut down automatic', 'admin'],
  ['g:data#backup_emergency_on', 'Back up after a crash', 'emergency crash power cut backup unclean', 'admin'],
  ['g:data#backup_copy_on', 'Also copy backups to', 'second copy usb onedrive google drive folder backup d drive', 'admin'],
  ['g:about', 'About', 'about version credits', 'all'],
  ['g:about#updates', 'Updates', 'update updates install version', 'admin', true],
].map(([id, label, words, need, local]) => {
  const [item, row] = id.split('#');
  return { item, row: row ?? null, label, words: `${label} ${words}`.toLowerCase(), need, local: !!local };
});
function searchManage(typed) {
  const ws = typed.toLowerCase().split(/\s+/).filter(Boolean);
  const found = MANAGE_INDEX.filter((e) => ws.every((w) => e.words.includes(w)));
  const groups = visibleGroups().map((g) => g.id), pages = visiblePages();
  const can = (e) => needOk(e.need) && !(e.local && state.remote)
    && (e.item.startsWith('p:') ? pages.includes(e.item.slice(2)) : groups.includes(e.item.slice(2)));
  const shown = found.filter(can).map((e) => {
    const g = MANAGE_GROUPS.find((x) => `g:${x.id}` === e.item);
    return { ...e, icon: g ? g.icon : e.item.slice(2), where: e.row ? g.title : g ? 'Settings' : 'Daily work' };
  });
  // Names that start with what was typed first, then the groups and pages themselves.
  const rank = (e) => (e.label.toLowerCase().startsWith(ws[0]) ? 0 : 2) + (e.row ? 1 : 0);
  shown.sort((a, b) => rank(a) - rank(b));
  // "Admin only" is for what a better role would show; the main till's own rows
  // (Start with Windows, Full screen…) are not in a browser for anyone.
  return { shown: shown.slice(0, 30), locked: !shown.length && found.some((e) => !(e.local && state.remote) && !can(e)) };
}

// ---------- Manage building blocks ----------

/// Focus a field the user's tap has just made (a chip turned into a box, + Add, a
/// stepper's number) and open the on-screen keyboard for it, which on its own opens
/// only for a tap on a field.
function tapFocus(el) {
  if (!el) return;
  el.focus();
  window.OSK?.open?.(el);
}
/// Shake a control that was put back (a save that did not go through).
function nudge(el) {
  if (!el) return;
  el.classList.remove('nudge');
  void el.offsetWidth;
  el.classList.add('nudge');
  el.addEventListener('animationend', () => el.classList.remove('nudge'), { once: true });
}
/// A setting row: label, one-line hint (longer text behind (i)), save state, control.
function srow({ id, label, hint = '', tip, ctl, cls = '', tag = 'div', stacked = false } = {}) {
  const row = h(tag, { class: `srow ${cls}${stacked ? ' stacked' : ''}`, 'data-row': id },
    h('span', { class: 'srow-text' }, h('span', { class: 'srow-label' }, h('span', {}, label), tip && infoTip(tip, label)),
      h('span', { class: 'srow-hint', hidden: !hint }, hint)),
    h('span', { class: 'sstate', role: 'status' }),
    h('span', { class: 'srow-ctl' }, ctl));
  row.hintText = hint;
  return row;
}
function setHint(row, text) {
  row.hintText = text;
  if (row.classList.contains('err')) return;
  const el = row.querySelector('.srow-hint');
  el.textContent = text;
  el.hidden = !text;
}
/// The 20 px cell between a row's text and its control: saving (after 150 ms), ✓, or "!".
/// `plain`: show `msg` as it is (not "Not saved: …"), e.g. for an action's result.
function rowState(row, st, msg = '', plain = false) {
  if (!row) return;
  const cell = row.querySelector('.sstate');
  if (!cell) return;
  clearTimeout(row.stateTimer);
  cell.removeAttribute('aria-label');
  if (st === 'saving') { row.stateTimer = setTimeout(() => { cell.className = 'sstate saving'; }, 150); return; }
  const hint = row.querySelector('.srow-hint');
  row.classList.remove('err');
  if (hint) { hint.textContent = row.hintText ?? ''; hint.hidden = !row.hintText; }
  if (st === 'error') {
    cell.className = 'sstate error';
    cell.setAttribute('aria-label', 'Not saved');
    row.classList.add('err');
    if (hint) { hint.hidden = false; hint.textContent = plain ? msg : `Not saved: ${msg}`; }
    nudge(row.querySelector('.srow-ctl'));
  } else if (st === 'saved') {
    cell.className = 'sstate';
    void cell.offsetWidth;
    cell.className = 'sstate saved';
    cell.setAttribute('aria-label', 'Saved');
    row.stateTimer = setTimeout(() => { cell.className = 'sstate'; }, 1700);
  } else cell.className = 'sstate';
}
/// Run a save for a row: the row shows saving / ✓ / the reason it was not saved
/// (no toast). `revert` puts the control back. True when it was saved.
async function saveWith(row, fn, revert = null) {
  rowState(row, 'saving');
  try {
    await fn();
    rowState(row, 'saved');
    return true;
  } catch (e) {
    revert?.();
    rowState(row, 'error', String(e?.message ?? e));
    return false;
  }
}
const saveKey = (key, value) => call('set_setting', { key, value: String(value) }, { quiet: true });
/// Receipt fields (and the receipt printer) live in one object: read it, change
/// only these fields, write it back. One at a time, so two quick taps both count.
let receiptQueue = Promise.resolve();
function saveReceipt(patch) {
  const run = receiptQueue.then(async () => {
    const now = await call('get_receipt_settings', {}, { quiet: true });
    await call('set_receipt_settings', { settings: { ...now, ...patch } }, { quiet: true });
  });
  receiptQueue = run.catch(() => {});
  return run;
}

/// (i): a short explanation in a pop-over, instead of a paragraph on the page.
let openPop = null;
function infoTip(text, label) {
  return popButton(h('button', { type: 'button', class: 'itip', 'aria-label': `About ${label}`, 'aria-expanded': 'false' }), () => text, label);
}
/// ⋯: a short menu of actions (Download, Delete) in the same pop-over; `items()` gives its buttons.
const moreMenu = (label, items) => popButton(h('button', { type: 'button', class: 'ghost more-btn', 'aria-label': `More for ${label}`, 'aria-expanded': 'false' }, icon('more', 20)),
  () => h('div', { class: 'pop-menu' }, items()), label, 'menu');
/// A button that opens `content()` in the pop-over under it (one open at a time).
function popButton(b, content, label, role = 'dialog') {
  b.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const mine = openPop?.btn === b;
    closeInfoPop();
    if (mine) return;
    const pop = h('div', { class: `pop${role === 'menu' ? ' pop-menu-host' : ''}`, role, 'aria-label': label }, content());
    if (role === 'menu') pop.addEventListener('click', (ev) => { if (ev.target.closest('button') && !ev.target.closest('button').dataset.keep) setTimeout(closeInfoPop); });
    document.body.append(pop);
    const place = () => {
      const r = b.getBoundingClientRect();
      const left = Math.max(16, Math.min(r.left - 12, innerWidth - pop.offsetWidth - 16));
      let top = r.bottom + 2;
      if (top + pop.offsetHeight > innerHeight - 12) top = r.top - pop.offsetHeight - 2;
      pop.style.left = `${left}px`;
      pop.style.top = `${Math.max(8, top)}px`;
    };
    place();
    b.setAttribute('aria-expanded', 'true');
    openPop = { btn: b, pop, place, at: Date.now() };
  });
  return b;
}
function closeInfoPop() {
  if (!openPop) return;
  openPop.pop.remove();
  openPop.btn.setAttribute('aria-expanded', 'false');
  openPop = null;
}
document.addEventListener('pointerdown', (e) => { if (openPop && !openPop.pop.contains(e.target) && !openPop.btn.contains(e.target)) closeInfoPop(); }, true);
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && openPop) { e.stopImmediatePropagation(); closeInfoPop(); } }, true);
// Scrolling closes it; the last scroll events of the swipe before the tap (or the
// page bringing the row into view) only move it with its button.
document.addEventListener('scroll', () => { if (openPop && Date.now() - openPop.at < 400) openPop.place(); else closeInfoPop(); }, true);

/// On/off switch row: the whole row toggles. `save(v)` saves (row state), or
/// `onChange(v)` just reports (in a sheet). `hint` may depend on the value.
let switchSeq = 0;
function switchRow({ id, label, hint = '', tip, value, save, onChange, after }) {
  const hintFor = (v) => (typeof hint === 'function' ? hint(v) : hint);
  // `for`: an (i) button inside the label would otherwise be the control the label works.
  const uid = `sw${++switchSeq}`;
  const input = h('input', { type: 'checkbox', role: 'switch', checked: !!value, id: uid });
  const row = srow({ id, label, hint: hintFor(!!value), tip, tag: 'label', ctl: h('span', { class: 'sw' }, input, h('span', {})) });
  row.htmlFor = uid;
  input.addEventListener('change', async () => {
    const v = input.checked;
    setHint(row, hintFor(v));
    if (!save) return onChange?.(v);
    if (await saveWith(row, () => save(v), () => { input.checked = !v; row.hintText = hintFor(!v); })) after?.(v);
  });
  row.input = input;
  return row;
}
/// A "1" / "0" setting as a switch (not set = `def`).
const keySwitch = (raw, key, label, { def = true, ...o } = {}) =>
  switchRow({ id: key, label, value: raw[key] == null ? def : raw[key] !== '0', save: (v) => saveKey(key, v ? '1' : '0'), ...o });

/// − value + : tap the value to type it; hold a button to repeat.
/// `allowEmpty`: below `min` shows `emptyLabel` and gives null.
function stepper({ value, min, max, step = 1, unit = '', allowEmpty = false, emptyLabel = 'Same', label = '', onChange }) {
  let v = value;
  const minus = h('button', { type: 'button', 'aria-label': `Less ${label}`.trim() }, '−');
  const plus = h('button', { type: 'button', 'aria-label': `More ${label}`.trim() }, '+');
  const val = h('span', { class: 'val', role: 'spinbutton', tabindex: 0, 'aria-label': label, 'aria-valuemin': min, 'aria-valuemax': max });
  const el = h('span', { class: 'sstep' }, minus, val, plus);
  const round = (n) => Math.round(n * 100) / 100;
  const draw = () => {
    if (v == null) { val.className = 'val is-empty'; setKids(val, emptyLabel); val.removeAttribute('aria-valuenow'); val.setAttribute('aria-valuetext', emptyLabel); }
    else { val.className = 'val'; setKids(val, String(v), unit && h('small', {}, unit)); val.setAttribute('aria-valuenow', v); val.removeAttribute('aria-valuetext'); }
    minus.disabled = v == null || (!allowEmpty && v <= min);
    plus.disabled = v != null && v >= max;
  };
  const set = (n) => {
    if (n != null && n < min) n = allowEmpty ? null : min;
    if (n != null) n = Math.min(max, round(n));
    if (n === v) return;
    v = n;
    draw();
    onChange?.(v);
  };
  const bump = (dir) => set(v == null ? (dir > 0 ? min : null) : round(v + dir * step));
  // Hold to repeat: once at once, then every 120 ms after 400 ms.
  for (const [b, dir] of [[minus, -1], [plus, 1]]) {
    let t1, t2;
    const stop = () => { clearTimeout(t1); clearInterval(t2); };
    b.addEventListener('pointerdown', (e) => {
      if (b.disabled || e.button > 0) return;
      e.preventDefault();
      bump(dir);
      t1 = setTimeout(() => { t2 = setInterval(() => { if (b.disabled) stop(); else bump(dir); }, 120); }, 400);
    });
    for (const ev of ['pointerup', 'pointerleave', 'pointercancel']) b.addEventListener(ev, stop);
    b.addEventListener('click', (e) => { if (e.detail === 0) bump(dir); }); // keyboard
  }
  val.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowUp' || e.key === 'ArrowRight') { e.preventDefault(); bump(1); }
    else if (e.key === 'ArrowDown' || e.key === 'ArrowLeft') { e.preventDefault(); bump(-1); }
    else if (e.key === 'Enter') { e.preventDefault(); val.click(); }
  });
  // Tap the number to type it.
  val.addEventListener('click', () => {
    const input = h('input', { class: 'val', inputmode: step % 1 ? 'decimal' : 'numeric', value: v ?? '', 'aria-label': label });
    val.replaceWith(input);
    tapFocus(input);
    input.select();
    let done = false;
    const finish = (keep) => {
      if (done) return;
      done = true;
      input.replaceWith(val);
      if (!keep) return;
      const raw = input.value.trim().replace(',', '.');
      const n = raw === '' ? (allowEmpty ? null : NaN) : Number(raw);
      if (n === null) set(null);
      else if (Number.isFinite(n)) set(n);
    };
    input.addEventListener('blur', () => finish(true));
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); finish(true); val.focus(); }
      else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); finish(false); val.focus(); }
    });
  });
  draw();
  el.set = (n) => { v = n; draw(); };
  el.value = () => v;
  return el;
}
/// A stepper row: saves 600 ms after the last tap (one write).
function stepperRow({ id, label, hint, tip, save, validate, ...o }) {
  let saved = o.value, timer, row;
  const st = stepper({ ...o, label, onChange: (v) => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const bad = validate?.(v);
      if (bad) { st.set(saved); return rowState(row, 'error', bad); }
      if (await saveWith(row, () => save(v), () => st.set(saved))) saved = v;
    }, 600);
  } });
  row = srow({ id, label, hint, tip, ctl: st });
  row.stepper = st;
  return row;
}

/// Segmented control: up to 4 short choices. `onPick(v, previous)`.
function sseg(items, current, onPick, label = '') {
  let cur = current;
  const el = h('span', { class: 'sseg', role: 'radiogroup', 'aria-label': label });
  const draw = () => setKids(el, items.map(([v, l]) => h('button', { type: 'button', role: 'radio', 'aria-checked': String(v === cur), class: v === cur ? 'on' : '',
    onclick: () => { if (v === cur) return; const prev = cur; cur = v; draw(); onPick(v, prev); } }, l)));
  draw();
  el.set = (v) => { cur = v; draw(); };
  el.setItems = (list) => { items = list; draw(); };
  el.value = () => cur;
  return el;
}
function segRow({ id, label, hint, tip, items, value, save, after, stacked = items.length >= 3, cls = '' }) {
  let row;
  const seg = sseg(items, value, async (v, prev) => {
    if (await saveWith(row, () => save(v), () => seg.set(prev))) after?.(v);
  }, label);
  row = srow({ id, label, hint, tip, ctl: seg, stacked, cls });
  row.seg = seg;
  return row;
}
/// Native select (long lists): saves at once.
function selectRow({ id, label, hint, tip, options, value, save, after }) {
  const sel = h('select', { 'aria-label': label }, options.map(([v, l]) => h('option', { value: v }, l)));
  sel.value = String(value);
  let prev = sel.value, row;
  sel.addEventListener('change', async () => {
    const v = sel.value;
    if (await saveWith(row, () => save(v), () => { sel.value = prev; })) { prev = v; after?.(v); }
  });
  row = srow({ id, label, hint, tip, ctl: sel });
  row.select = sel;
  return row;
}
/// Text row: saves on blur or Enter; Esc puts the saved text back.
function textRow({ id, label, hint, tip, value, placeholder, max, area = false, cls = '', save, validate, after, attrs = {}, unit = '' }) {
  const input = area ? h('textarea', { rows: 3, placeholder, 'aria-label': label, ...attrs }) : h('input', { placeholder, maxlength: max, 'aria-label': label, ...attrs });
  input.value = value ?? '';
  let saved = input.value, row;
  const commit = async () => {
    const v = input.value.trim();
    if (v === saved) { input.value = saved; if (row.classList.contains('err')) rowState(row, 'idle'); return; }
    const bad = validate?.(v);
    if (bad) return rowState(row, 'error', bad);
    if (await saveWith(row, () => save(v), () => { input.value = saved; })) { saved = v; input.value = v; after?.(v); }
  };
  input.addEventListener('blur', commit);
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !area) { e.preventDefault(); input.blur(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); input.value = saved; input.blur(); }
  });
  row = srow({ id, label, hint, tip, ctl: [input, unit && h('span', { class: 'sunit' }, unit)], cls: `${area ? '' : 'text-row'} ${cls}`, stacked: area });
  row.input = input;
  return row;
}
/// A row that opens a sheet or another page: label … value ›.
function navRow({ id, label, hint, tip, value = null, onOpen, cls = '' }) {
  const row = srow({ id, label, hint, tip, cls: `nav ${cls}`, ctl: [value != null && h('span', { class: 'srow-value' }, value), h('span', { class: 'chev' }, icon('chev', 20))] });
  row.tabIndex = 0;
  row.setAttribute('role', 'button');
  row.addEventListener('click', (e) => { if (!e.target.closest('.itip')) onOpen(row); });
  row.addEventListener('keydown', (e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === row) { e.preventDefault(); onOpen(row); } });
  return row;
}
/// Status row (printers, service charge): icon tile, title, one meta line, a pill, [button] ›.
function statusRow({ id, icon: ic, title, meta, pill, action, onOpen }) {
  const row = h('div', { class: `srow sstat${onOpen ? ' nav' : ''}`, 'data-row': id, role: onOpen && 'button', tabindex: onOpen && 0 },
    h('span', { class: 'lead' }, icon(ic, 20)),
    h('span', { class: 'srow-text' }, h('span', { class: 'srow-label' }, title), meta && h('span', { class: 'srow-hint' }, meta)),
    h('span', { class: 'sstate', role: 'status' }),
    pill ? h('span', { class: `pill ${pill.cls ?? ''}` }, pill.text) : h('span', {}),
    h('span', { class: 'srow-ctl' }, action, onOpen && h('span', { class: 'chev' }, icon('chev', 20))));
  row.hintText = meta;
  if (onOpen) {
    row.addEventListener('click', (e) => { if (!e.target.closest('button')) onOpen(row); });
    row.addEventListener('keydown', (e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === row) { e.preventDefault(); onOpen(row); } });
  }
  return row;
}
/// A section: title, one card of rows, an optional one-line foot. Nothing if no row shows.
function ssec(title, rows, foot = null) {
  rows = rows.flat().filter(Boolean);
  if (!rows.length) return null;
  return h('section', { class: 'ssec' }, title && h('h3', { class: 'ssec-title' }, title), h('div', { class: 'sgroup' }, rows), foot && h('div', { class: 'ssec-foot' }, foot));
}
/// "Advanced": the last thing in a card; open or closed is kept on this device.
function advanced(key, rows) {
  rows = rows.filter(Boolean);
  const k = `pos.adv.${key}`;
  let open = pref(k) === '1';
  const body = h('div', { class: 'adv-body', hidden: !open }, rows);
  const btn = h('button', { type: 'button', class: 'adv', 'aria-expanded': String(open), onclick: () => {
    open = !open;
    body.hidden = !open;
    btn.setAttribute('aria-expanded', String(open));
    setPref(k, open ? '1' : '0');
  } }, h('span', { class: 'chev' }, icon('chev', 18)), 'Advanced', h('span', { class: 'n' }, rows.length));
  return [btn, body];
}
/// Chips for a short list of numbers (cash amounts, discount buttons): tap one to
/// change it, × removes it, + Add. `fixed`: always these many (tips: three).
/// `onChange(list)`: false or { bad: i } = refused, so the list goes back to the saved one.
function chipList({ values, min = 0, max = 8, fixed = false, unit = '', label = '', onChange }) {
  let list = [...values];
  const el = h('span', { class: 'chiplist', role: 'group', 'aria-label': label });
  const fmt = (n) => (Number.isFinite(n) ? `${n}${unit ? ` ${unit}` : ''}` : String(n));
  const commit = async (next) => {
    const prev = list;
    list = fixed ? next : [...new Set(next)].sort((a, b) => a - b);
    draw();
    // Refused (a wrong value, or the server said no): back to the last saved list, so
    // the next change saves; the row keeps saying why.
    const r = await onChange(list);
    if (r === false || r?.bad != null) { list = prev; draw(); }
  };
  const edit = (i, chip) => {
    const input = h('input', { inputmode: 'decimal', value: i < list.length ? String(list[i]) : '', 'aria-label': label });
    setKids(chip, input);
    chip.classList.add('editing');
    tapFocus(input);
    input.select();
    let done = false;
    const finish = (keep) => {
      if (done) return;
      done = true;
      const raw = input.value.trim().replace('%', '').replace(',', '.').trim();
      if (!keep || (raw === '' && i >= list.length)) return draw();
      const next = [...list];
      if (raw === '') { if (fixed || list.length <= min) return draw(); next.splice(i, 1); }
      else if (i >= list.length) next.push(Number(raw)); else next[i] = Number(raw);
      if (next.some((n) => !Number.isFinite(n))) { draw(); return onChange(null); }
      if (next.join() === list.join()) return draw();
      commit(next);
    };
    input.addEventListener('blur', () => finish(true));
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); input.blur(); }
      else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); finish(false); }
    });
  };
  const draw = (bad = -1) => setKids(el, list.map((n, i) => {
    const chip = h('span', { class: `chipx${fixed ? ' fixed' : ''}${i === bad ? ' bad' : ''}` });
    const t = h('span', { class: 't', role: 'button', tabindex: 0, 'aria-label': `Change ${fmt(n)}`, onclick: () => edit(i, chip),
      onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); edit(i, chip); } } }, fmt(n));
    setKids(chip, t, !fixed && list.length > min && h('button', { type: 'button', class: 'x', 'aria-label': `Remove ${fmt(n)}`,
      onclick: () => commit(list.filter((_, k) => k !== i)) }, '×'));
    return chip;
  }), !fixed && list.length < max && h('button', { type: 'button', class: 'chip-add', onclick: (e) => {
    const chip = h('span', { class: 'chipx' });
    e.currentTarget.replaceWith(chip);
    edit(list.length, chip);
  } }, '+ Add'));
  draw();
  return el;
}
function chipRow({ id, label, hint, tip, validate, save, ...o }) {
  let row;
  const chips = chipList({ ...o, label, onChange: async (list) => {
    if (list == null) { rowState(row, 'error', 'Use numbers only'); return false; }
    const bad = validate(list);
    if (bad) { rowState(row, 'error', bad.msg); return { bad: bad.index ?? -1 }; }
    return saveWith(row, () => save(list));
  } });
  row = srow({ id, label, hint, tip, ctl: chips, stacked: true });
  return row;
}
/// Toggle chips (pick any): `onChange(list, previous)`.
function toggleChips(items, selected, onChange, label = '') {
  let sel = new Set(selected);
  const el = h('span', { class: 'chiplist', role: 'group', 'aria-label': label });
  const draw = () => setKids(el, items.map(([v, l]) => h('button', { type: 'button', class: `tchip${sel.has(v) ? ' on' : ''}`, 'aria-pressed': String(sel.has(v)),
    onclick: () => { const prev = [...sel]; if (sel.has(v)) sel.delete(v); else sel.add(v); draw(); onChange([...sel], prev); } }, l)));
  draw();
  el.set = (list) => { sel = new Set(list); draw(); };
  el.value = () => items.map(([v]) => v).filter((v) => sel.has(v));
  return el;
}
const emptyState = ({ icon: ic, title, text, action = null }) =>
  h('div', { class: 'sempty' }, h('span', { class: 'eic' }, icon(ic, 28)), h('strong', {}, title), text && h('p', {}, text), action);

/// A sheet: a pop-up on wide screens, a bottom sheet on phones, for settings made of
/// several fields. `onSave()` saves (throw or return a message to keep it open);
/// `onTest()` returns { ok, text }. Closing with changes asks first, in the foot.
function sheet({ title, body, onSave, onTest, saveLabel = 'Save', wide = false, left = null, danger = false }) {
  const m = document.getElementById('modal');
  const msg = h('div', { class: 'sheet-msg', hidden: true, role: 'status' });
  const show = (text, ok) => { msg.hidden = !text; msg.textContent = text ?? ''; msg.className = `sheet-msg ${ok ? 'ok' : 'bad'}`; };
  let dirty = false;
  const bodyEl = h('div', { class: 'sheet-body' }, body);
  bodyEl.addEventListener('input', () => { dirty = true; });
  bodyEl.addEventListener('change', () => { dirty = true; });
  bodyEl.addEventListener('click', (e) => { if (e.target.closest('.sseg button, .sstep button, .chiplist button, .lrow .del, .ladd')) dirty = true; });
  const foot = h('div', { class: 'sheet-foot' });
  // `onSave()` may return false: stay open, nothing to say (e.g. the first of two taps).
  const save = onSave && h('button', { class: danger ? 'danger solid' : 'primary', onclick: async () => {
    const label = save.textContent;
    save.disabled = true;
    setKids(save, h('span', { class: 'spin' }), danger ? 'Working…' : 'Saving…');
    show(null);
    try {
      const r = await onSave();
      if (typeof r === 'string') throw r;
      if (r === false) { save.disabled = false; if (save.querySelector('.spin')) setKids(save, label); return; }
      close();
    } catch (e) {
      show(String(e?.message ?? e), false);
      save.disabled = false;
      setKids(save, label);
    }
  } }, saveLabel);
  const test = onTest && h('button', { onclick: async () => {
    test.disabled = true;
    show(null);
    try { const r = await onTest(); show(r.text, r.ok); } catch (e) { show(String(e?.message ?? e), false); } finally { test.disabled = false; }
  } }, 'Test');
  const normal = () => setKids(foot, left, test, h('span', { class: 'grow' }), h('button', { class: 'ghost', onclick: () => tryClose() }, onSave ? 'Cancel' : 'Close'), save);
  const ask = () => setKids(foot, h('strong', { class: 'grow' }, 'Discard changes?'), h('button', { onclick: normal }, 'Keep editing'),
    h('button', { class: 'danger solid', onclick: close }, 'Discard'));
  const tryClose = () => { if (dirty && onSave) ask(); else close(); };
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    state.sheetBack = null;
    if (document.getElementById('modal').contains(el)) closeModal();
    popBack('sheet');
  };
  const el = h('div', { class: `sheet${wide ? ' wide' : ''}`, role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
    h('div', { class: 'grab' }),
    h('div', { class: 'sheet-head' }, h('h3', {}, title), h('button', { class: 'ghost icon-btn', 'aria-label': 'Close', onclick: () => tryClose() }, icon('x'))),
    bodyEl, msg, foot);
  normal();
  closeModal();
  m.className = 'sheet-host';
  setKids(m, el);
  m.hidden = false;
  m.onclick = (e) => { if (e.target === m) tryClose(); };
  state.modalKeys = (e) => { if (e.key === 'Escape') tryClose(); };
  document.addEventListener('keydown', state.modalKeys);
  // The phone's back button closes the sheet (or asks first).
  pushBack('sheet');
  state.sheetBack = () => { if (dirty && onSave) { ask(); pushBack('sheet'); } else { closed = true; state.sheetBack = null; closeModal(); } };
  requestAnimationFrame(placeToast);
  return { el, show, close, foot, normal, test, save, body: bodyEl, clean: () => { dirty = false; } };
}

/// The bar under a list or form with changes: "N changes · Discard · Save" (or `text`).
/// While it shows, state.dirty keeps you on the page; `hold`: auto logout waits too
/// (the floor plan, which takes long to redo).
function saveBar({ onSave, onDiscard, text: fixed = null, saveLabel = 'Save', hold = false }) {
  const text = h('strong', {});
  const save = h('button', { class: 'primary' }, saveLabel);
  const discard = h('button', { class: 'ghost', onclick: () => { onDiscard(); el.update(0); } }, 'Discard');
  const el = h('div', { class: 'savebar', hidden: true, role: 'region', 'aria-label': 'Unsaved changes' }, text, discard, save);
  const normal = () => setKids(el, text, discard, save);
  /// × or Esc with changes: ask in the bar (`then` runs after Discard; default onDiscard).
  el.ask = (then = onDiscard) => {
    setKids(el, h('strong', {}, 'Discard changes?'), h('button', { class: 'ghost', onclick: normal }, 'Keep editing'),
      h('button', { class: 'danger solid', onclick: () => { then(); el.update(0); } }, 'Discard'));
    el.querySelector('button')?.focus();
  };
  save.onclick = async () => {
    save.disabled = true;
    setKids(save, h('span', { class: 'spin' }), 'Saving…');
    try { await onSave(); el.update(0); } catch (e) { text.textContent = `Not saved: ${String(e?.message ?? e)}`; text.classList.add('bad'); } finally { save.disabled = false; setKids(save, saveLabel); }
  };
  el.update = (n) => {
    normal();
    el.hidden = !n;
    state.dirty = n > 0 ? (hold ? 'hold' : true) : false;
    text.classList.remove('bad');
    text.textContent = fixed ?? `${n} change${n === 1 ? '' : 's'}`;
  };
  return el;
}
/// A list to edit and put in order: drag the grip (or ↑ ↓ on it), × removes, + Add.
/// `render(row)` gives the cells between the grip and ×; `onChange()` after each change.
function listEditor({ rows, render, make, addLabel, empty, max = 99, cls = '', onChange, name = (r) => r.name }) {
  let seq = 0;
  const key = new WeakMap();
  const k = (r) => { if (!key.has(r)) key.set(r, ++seq); return key.get(r); };
  const list = h('div', { class: 'llist' });
  const emptyEl = empty ? h('div', { class: 'lempty' }, empty) : null;
  const addBtn = h('button', { type: 'button', class: 'ladd', onclick: () => {
    rows.push(make());
    draw();
    onChange();
    tapFocus(list.lastElementChild?.querySelector('input:not([type=color])'));
  } }, icon('plus', 18), addLabel);
  const move = (i, j) => { const [r] = rows.splice(i, 1); rows.splice(j, 0, r); draw(); onChange(); list.children[j]?.querySelector('.lgrip')?.focus(); };
  const draw = () => {
    setKids(list, rows.map((r, i) => h('div', { class: `lrow ${cls}`, 'data-id': k(r) },
      h('span', { class: 'lgrip', role: 'button', tabindex: 0, 'aria-label': `Drag ${name(r) || 'row'}`, title: 'Drag to change the order',
        onkeydown: (e) => {
          if (e.key === 'ArrowUp' && i > 0) { e.preventDefault(); move(i, i - 1); }
          else if (e.key === 'ArrowDown' && i < rows.length - 1) { e.preventDefault(); move(i, i + 1); }
        } }, icon('grip', 20)),
      render(r),
      h('button', { type: 'button', class: 'del', 'aria-label': `Remove ${name(r) || 'row'}`, onclick: () => { rows.splice(i, 1); draw(); onChange(); } }, '×'))));
    if (emptyEl) emptyEl.hidden = rows.length > 0;
    addBtn.hidden = rows.length >= max;
  };
  sortable(list, { item: '.lrow', handle: '.lgrip', onDrop: (ids) => {
    const byKey = new Map(rows.map((r) => [k(r), r]));
    rows.splice(0, rows.length, ...ids.map((id) => byKey.get(id)));
    draw();
    onChange();
  } });
  draw();
  const el = h('div', { class: 'leditor' }, emptyEl, list, addBtn);
  el.redraw = draw;
  return el;
}

// ---------- Settings: Printers ----------

const PAPER = [[42, '80 mm'], [32, '58 mm'], [48, '80 mm small']];
const PRINTER_KINDS = [['network', 'Network'], ['windows', 'Windows'], ['file', 'Port'], ['none', 'None']];
const paperText = (w) => (PAPER.find(([v]) => v === Number(w ?? 42)) ?? PAPER[0])[1];
const connText = (c) => ({ network: `Network · ${c.host || '?'}`, windows: `Windows · ${c.name || '?'}`, file: `Port · ${c.path || '?'}` })[c.kind] ?? 'Not set up';
const PRINTER_TIP = 'Windows: a printer installed in Windows (USB). Port: a COM port or a shared printer path.';
const IMAGE_TIP = 'If Thai text prints badly, tap Test Thai and pick the number that prints clearly.';
/// Last test result per printer this session (there is no live printer status).
state.printerStatus = {};
const hhmm = (d = new Date()) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }).replace(/\s*[ap]\.?\s?m\.?$/i, '');
/// What a test print said, as the printer's status: { ok, at, error }.
const testResult = (r) => (r?.error ? { ok: false, error: r.error } : { ok: true, at: hhmm() });

async function printersGroup() {
  const [rc, kc, routes, teams] = await Promise.all([call('get_receipt_settings'), call('get_kitchen_printer'), call('get_kitchen_routes'), call('list_teams').catch(() => [])]);
  const teamName = (id) => teams.find((t) => t.id === id)?.name;
  const prints = (r) => [r.drinks && 'Drinks', r.desserts && 'Desserts', ...(r.team_ids ?? []).map(teamName)].filter(Boolean);
  const printerRow = (key, ic, title, cfg, meta, open, test) => {
    let row;
    const make = () => {
      const st = state.printerStatus[key];
      const on = cfg.kind && cfg.kind !== 'none';
      const pill = st?.busy ? { text: 'Printing…', cls: 'busy' } : st?.ok ? { text: `OK · ${st.at}`, cls: 'ok' } : st ? { text: 'Didn’t print', cls: 'bad' }
        : on ? { text: 'Set up', cls: 'plain' } : { text: 'Not set up' };
      return statusRow({ id: key, icon: ic, title, meta: st && !st.ok && !st.busy ? st.error : meta, pill, onOpen: open,
        action: on && h('button', { class: 'test', onclick: async () => {
          state.printerStatus[key] = { busy: true };
          redraw();
          try { state.printerStatus[key] = testResult(await test()); } catch (e) { state.printerStatus[key] = { ok: false, error: String(e?.message ?? e) }; }
          redraw();
        } }, 'Test') });
    };
    const redraw = () => { const n = make(); row.replaceWith(n); row = n; };
    row = make();
    return row;
  };
  const ctx = { rc, kc, routes, teams };
  return [
    ssec('Printers', [
      printerRow('receipt', 'receipt', 'Receipt printer', rc, `${connText(rc)} · ${paperText(rc.width)}`,
        () => printerSheet('receipt', rc, ctx), () => call('test_receipt_printer', { settings: rc, drawerOnly: false }, { quiet: true })),
      printerRow('kitchen', 'printer', 'Kitchen printer', kc, `${connText(kc)} · ${paperText(kc.width)}`,
        () => printerSheet('kitchen', kc, ctx), () => call('test_kitchen_printer', { config: kc }, { quiet: true })),
      routes.map((r, i) => printerRow(`route${i}`, 'cup', r.label || `Kitchen printer ${i + 2}`, r,
        `${connText(r)} · ${prints(r).length ? `Prints ${prints(r).join(', ')}` : 'Prints nothing yet'}`,
        () => printerSheet('route', r, { ...ctx, index: i }), () => call('test_route_printer', { route: r }, { quiet: true }))),
      h('button', { class: 'ladd', 'data-row': 'addroute', onclick: () => printerSheet('route', { kind: 'network', port: 9100, width: 42, copies: 1, team_ids: [] }, { ...ctx, index: null }) },
        icon('plus', 18), 'Add kitchen printer'),
    ], 'Extra printers take drinks, desserts or a team. The rest prints in the kitchen.'),
    ssec('Cash drawer', [statusRow({ id: 'drawer', icon: 'drawer', title: 'Cash drawer', meta: 'Plugged into the receipt printer',
      onOpen: () => openManage('g:payments', 'drawer_on_cash'),
      action: h('button', { class: 'test', onclick: async (e) => {
        const row = e.currentTarget.closest('.srow');
        if (!rc.kind || rc.kind === 'none') return rowState(row, 'error', 'Set up the receipt printer first', true);
        rowState(row, 'saving');
        try {
          const r = await call('test_receipt_printer', { settings: rc, drawerOnly: true }, { quiet: true });
          if (r.error) rowState(row, 'error', `Didn’t open: ${r.error}`, true); else rowState(row, 'saved');
        } catch (err) { rowState(row, 'error', String(err?.message ?? err), true); }
      } }, 'Open') })]),
  ];
}

/// Receipt, kitchen or an extra kitchen printer ("route"): connection, paper,
/// copies… in a sheet. Test prints with the form as it is (not saved yet).
async function printerSheet(kind, cfg, { rc, routes, teams, index = null }) {
  const sys = await call('list_system_printers').catch(() => []);
  const route = kind === 'route', kitchen = kind === 'kitchen';
  const NAMES = kitchen ? [['both', 'EN + TH'], ['english', 'English'], ['thai', 'Thai']] : [['english', 'English'], ['both', 'EN + TH']];
  const f = {
    kind: cfg.kind ?? 'none', host: cfg.host ?? '', port: String(cfg.port ?? 9100), name: cfg.name ?? '', path: cfg.path ?? '',
    width: Number(cfg.width ?? 42), copies: cfg.copies ?? 1, copy_delay_secs: cfg.copy_delay_secs ?? 2,
    names: NAMES.some(([v]) => v === cfg.names) ? cfg.names : NAMES[0][0], bold: cfg.bold ?? true, large_text: cfg.large_text ?? true,
    app_font: cfg.app_font ?? true, call_table_only: cfg.call_table_only ?? true, slow_send: cfg.slow_send ?? true,
    image_mode: IMAGE_MODES.some(([v]) => v === cfg.image_mode) ? cfg.image_mode : 'raster',
    starter_copies: cfg.starter_copies ?? null, mains_copies: cfg.mains_copies ?? null, prep_copies: cfg.prep_copies ?? null, additional_copies: cfg.additional_copies ?? null,
    all_courses_copies: cfg.all_courses_copies ?? null, call_again_copies: cfg.call_again_copies ?? null,
    label: cfg.label ?? '', drinks: !!cfg.drinks, desserts: !!cfg.desserts, team_ids: [...(cfg.team_ids ?? [])],
  };
  if (route && f.kind === 'none') f.kind = 'network';
  const field = (key, attrs = {}) => { const el = h('input', { value: f[key], ...attrs }); el.addEventListener('input', () => { f[key] = el.value; }); return el; };
  const host = srow({ label: 'IP address', cls: 'text-row', ctl: field('host', { placeholder: '192.168.1.50', inputmode: 'decimal', 'aria-label': 'IP address' }) });
  const port = srow({ label: 'Port number', ctl: field('port', { inputmode: 'numeric', class: 'short-in', 'aria-label': 'Port number' }) });
  let winCtl;
  if (sys.length) {
    winCtl = h('select', { 'aria-label': 'Printer' }, sys.map((n) => h('option', { value: n }, n)));
    if (f.name && !sys.includes(f.name)) winCtl.append(h('option', { value: f.name }, f.name));
    winCtl.value = f.name || sys[0];
    f.name = winCtl.value;
    winCtl.addEventListener('change', () => { f.name = winCtl.value; });
  } else winCtl = field('name', { placeholder: 'Name in Windows', 'aria-label': 'Printer' });
  const win = srow({ label: 'Printer', cls: 'text-row', ctl: winCtl });
  const path = srow({ label: 'Port or path', cls: 'text-row', ctl: field('path', { placeholder: 'COM3 or \\\\PC\\Printer', 'aria-label': 'Port or path' }) });
  const slow = switchRow({ id: 'slow', label: 'Send slowly', hint: 'For printers that skip lines', value: f.slow_send, onChange: (v) => { f.slow_send = v; } });
  const showConn = () => {
    host.hidden = port.hidden = f.kind !== 'network';
    win.hidden = f.kind !== 'windows';
    path.hidden = f.kind !== 'file';
    slow.hidden = !['network', 'file'].includes(f.kind);
  };
  const type = srow({ label: 'Type', tip: PRINTER_TIP, stacked: true, ctl: sseg(route ? PRINTER_KINDS.slice(0, 3) : PRINTER_KINDS, f.kind, (v) => { f.kind = v; showConn(); noneHides(); }, 'Type') });
  showConn();
  const delay = !route && srow({ label: 'Wait between copies', ctl: stepper({ value: f.copy_delay_secs, min: 0, max: 30, unit: 's', label: 'Wait between copies', onChange: (v) => { f.copy_delay_secs = v; } }) });
  if (delay) delay.hidden = f.copies < 2;
  const copies = srow({ label: 'Copies', ctl: stepper({ value: f.copies, min: 1, max: 9, label: 'Copies', onChange: (v) => { f.copies = v; if (delay) delay.hidden = v < 2; } }) });
  const paper = srow({ label: 'Paper', stacked: true, ctl: sseg(PAPER, f.width, (v) => { f.width = v; }, 'Paper') });
  // The form as the server takes it.
  const out = () => {
    const c = { ...cfg };
    for (const k of ['host', 'port', 'name', 'path']) delete c[k];
    Object.assign(c, { kind: f.kind, width: f.width, copies: f.copies, image_mode: f.image_mode, slow_send: f.slow_send });
    if (!route) Object.assign(c, { copy_delay_secs: f.copy_delay_secs, bold: f.bold });
    // Not set = the printer's usual names: left unset unless changed.
    if (!route && (cfg.names != null || f.names !== NAMES[0][0])) c.names = f.names;
    if (kitchen) Object.assign(c, { large_text: f.large_text, app_font: f.app_font, call_table_only: f.call_table_only,
      starter_copies: f.starter_copies, mains_copies: f.mains_copies, prep_copies: f.prep_copies, additional_copies: f.additional_copies,
      all_courses_copies: f.all_courses_copies, call_again_copies: f.call_again_copies });
    if (route) Object.assign(c, { label: f.label.trim(), drinks: f.drinks, desserts: f.desserts, team_ids: f.team_ids });
    if (f.kind === 'network') Object.assign(c, { host: f.host.trim(), port: Number(f.port) || 9100 });
    if (f.kind === 'windows') c.name = f.name;
    if (f.kind === 'file') c.path = f.path.trim();
    return c;
  };
  const problem = () => (f.kind === 'network' && !f.host.trim() ? 'Type the IP address'
    : f.kind === 'network' && !(Number(f.port) >= 1 && Number(f.port) <= 65535) ? 'Use a port from 1 to 65535'
      : f.kind === 'windows' && !f.name ? 'Choose the printer' : f.kind === 'file' && !f.path.trim() ? 'Type the port or path' : null);
  const thai = h('button', { type: 'button', onclick: async () => {
    if (f.kind === 'none' || problem()) return s.show(problem() ?? 'Choose a printer type first', false);
    try {
      const r = await call('test_thai_printer', { config: out() }, { quiet: true });
      s.show(r.error ? `Didn’t print: ${r.error}` : 'Thai test sent: pick the number that prints clearly', !r.error);
    } catch (e) { s.show(String(e?.message ?? e), false); }
  } }, 'Test Thai');
  const imageMode = h('select', { 'aria-label': 'Image mode' }, IMAGE_MODES.map(([v, l]) => h('option', { value: v }, l)));
  imageMode.value = f.image_mode;
  imageMode.addEventListener('change', () => { f.image_mode = imageMode.value; });
  const imageRow = srow({ id: 'image', label: 'Image mode', tip: IMAGE_TIP, cls: 'image-row', stacked: true, ctl: [imageMode, thai] });
  const COPY_TIPS = {
    starter_copies: 'Dine-in. Starters: sent first. Mains: Call mains, or the first mains after starters. Same = Copies.',
    all_courses_copies: 'Dine-in sent with “All courses together”: starters and mains on one ticket. Same = Copies.',
    call_again_copies: 'The table called again once the mains are in the kitchen. Same = Mains copies.',
  };
  const copySteps = (key, label) => srow({ label, tip: COPY_TIPS[key] ?? null,
    ctl: stepper({ value: f[key], min: 1, max: 9, allowEmpty: true, label, onChange: (v) => { f[key] = v; } }) });
  const sw = (key, label, hint) => switchRow({ label, hint, value: f[key], onChange: (v) => { f[key] = v; } });
  const nameRow = route && srow({ label: 'Name', cls: 'text-row', ctl: field('label', { placeholder: 'e.g. Bar', 'aria-label': 'Name' }) });
  const printsRow = route && srow({ label: 'Prints', stacked: true, ctl: toggleChips([['drinks', 'Drinks'], ['desserts', 'Desserts'], ...teams.map((t) => [`t${t.id}`, t.name])],
    [f.drinks && 'drinks', f.desserts && 'desserts', ...f.team_ids.map((id) => `t${id}`)].filter(Boolean), (list) => {
      f.drinks = list.includes('drinks');
      f.desserts = list.includes('desserts');
      f.team_ids = teams.filter((t) => list.includes(`t${t.id}`)).map((t) => t.id);
    }, 'Prints') });
  let paperSec, ticketSec;
  const body = [
    route && ssec(null, [nameRow]),
    ssec('Connection', [type, host, port, win, path]),
    paperSec = ssec('Paper & copies', [paper, copies, delay]),
    ticketSec = route ? h('section', { class: 'ssec' }, h('div', { class: 'sgroup' }, printsRow, advanced('route', [imageRow, slow])),
      h('div', { class: 'ssec-foot' }, 'Names and font follow the kitchen printer.'))
      : h('section', { class: 'ssec' }, h('h3', { class: 'ssec-title' }, 'Ticket'), h('div', { class: 'sgroup' },
        srow({ label: 'Item names', ctl: sseg(NAMES, f.names, (v) => { f.names = v; }, 'Item names'), stacked: NAMES.length > 2 }),
        sw('bold', 'Bold text'),
        kitchen && sw('large_text', 'Large text', 'Fewer letters per line'),
        advanced(kind, kitchen ? [copySteps('starter_copies', 'Starters copies'), copySteps('mains_copies', 'Mains copies'),
          copySteps('all_courses_copies', 'All courses together copies'), copySteps('call_again_copies', 'Call mains again copies'), copySteps('prep_copies', 'Prepare copies'),
          srow({ label: 'Added-later copies', ctl: stepper({ value: f.additional_copies ?? 1, min: 1, max: 9, label: 'Added-later copies', onChange: (v) => { f.additional_copies = v === 1 ? null : v; } }) }),
          sw('app_font', 'Print in the app’s font'), sw('call_table_only', '“Call mains again” shows table only'), imageRow, slow] : [imageRow, slow]))),
  ];
  const testIt = async () => {
    if (f.kind === 'none') return { ok: false, text: 'Choose a printer type first' };
    if (problem()) return { ok: false, text: problem() };
    const r = kind === 'receipt' ? await call('test_receipt_printer', { settings: { ...rc, ...out() }, drawerOnly: false }, { quiet: true })
      : kitchen ? await call('test_kitchen_printer', { config: out() }, { quiet: true }) : await call('test_route_printer', { route: out() }, { quiet: true });
    return r.error ? { ok: false, text: `Didn’t print: ${r.error}` } : { ok: true, text: 'Test ticket printed.' };
  };
  const key = kind === 'route' ? (index == null ? null : `route${index}`) : kind;
  const save = async () => {
    if (f.kind !== 'none' && problem()) return problem();
    if (kind === 'receipt') {
      const now = await call('get_receipt_settings', {}, { quiet: true });
      for (const k of ['host', 'port', 'name', 'path']) delete now[k];
      const c = out();
      const part = Object.fromEntries(['kind', 'host', 'port', 'name', 'path', 'width', 'copies', 'copy_delay_secs', 'names', 'bold', 'image_mode', 'slow_send'].filter((k) => k in c).map((k) => [k, c[k]]));
      await call('set_receipt_settings', { settings: { ...now, ...part } }, { quiet: true });
    } else if (kitchen) await call('set_kitchen_printer', { config: out() }, { quiet: true });
    else {
      const list = routes.map((r) => ({ ...r }));
      if (index == null) list.push(out()); else list[index] = out();
      await call('set_kitchen_routes', { routes: list }, { quiet: true });
    }
    if (key) delete state.printerStatus[key];
    redrawGroup(key ?? `route${routes.length}`);
  };
  // Remove (extra printers): asks once, in the foot.
  const remove = route && index != null && h('button', { class: 'ghost danger', onclick: () => setKids(s.foot,
    h('strong', { class: 'grow' }, `Remove ${f.label.trim() || 'this printer'}?`), h('button', { onclick: s.normal }, 'Keep'),
    h('button', { class: 'danger solid', onclick: async () => {
      try {
        await call('set_kitchen_routes', { routes: routes.filter((_, i) => i !== index) }, { quiet: true });
        s.close();
        state.printerStatus = {};
        redrawGroup();
      } catch (e) { s.show(String(e?.message ?? e), false); s.normal(); }
    } }, 'Remove')) }, 'Remove');
  const title = kind === 'receipt' ? 'Receipt printer' : kitchen ? 'Kitchen printer' : index == null ? 'New kitchen printer' : (cfg.label || 'Kitchen printer');
  const s = sheet({ title, body, onSave: save, onTest: testIt, left: remove });
  // No printer: nothing about paper, copies or tickets, and nothing to test.
  function noneHides() {
    const none = f.kind === 'none';
    for (const el of [paperSec, ticketSec, s?.test]) if (el) el.hidden = none;
  }
  noneHides();
}

// ---------- Settings: Payments & receipts ----------

async function paymentsGroup() {
  const [raw, rc, svc] = await Promise.all([invoke('get_settings', { keys: PAY_KEYS }), call('get_receipt_settings'), call('get_service_settings')]);
  const c = paySettings(raw);
  const methods = () => [['cash', payName('cash')], ['card', payName('card')]];
  const opensOn = segRow({ id: 'pay_default_method', label: 'Opens on', items: methods(), value: c.method, stacked: false, save: (v) => saveKey('pay_default_method', v) });
  const tipChips = chipRow({ id: 'tip_presets', label: 'Tip buttons', values: c.tips, fixed: true, unit: '%',
    validate: (l) => (goodTips(l) ? null : { msg: 'Use more than 0 and up to 100 %', index: l.findIndex((p) => !(p > 0 && p <= 100 && twoDecimals(p))) }),
    save: (l) => saveKey('tip_presets', l.join(',')) });
  tipChips.hidden = !c.tipsOn;
  tipChips.dataset.dep = 'tips_on';
  // Cash and Card must keep different names (empty = the standard name).
  const other = { cash: raw.pay_name_card ?? '', card: raw.pay_name_cash ?? '' };
  const nameRow = (m, label, tip) => textRow({ id: `pay_name_${m}`, label, tip, value: raw[`pay_name_${m}`] ?? '', placeholder: m === 'cash' ? 'Cash' : 'Card', max: PAY_NAME_MAX,
    validate: (v) => ((v || (m === 'cash' ? 'Cash' : 'Card')).toLowerCase() === (other[m] || (m === 'cash' ? 'Card' : 'Cash')).toLowerCase() ? 'Cash and Card need different names' : null),
    save: (v) => saveKey(`pay_name_${m}`, v),
    after: async (v) => { other[m === 'cash' ? 'card' : 'cash'] = v; await loadUiSettings(); opensOn.seg.setItems(methods()); } });
  const receipt = (key, label, value, o = {}) => switchRow({ id: key, label, value, save: (v) => saveReceipt({ [key]: v }), ...o });
  const svcOn = svc.rate_bp > 0;
  return [
    ssec('Finish & Pay', [
      opensOn,
      keySwitch(raw, 'exact_cash_pays', '“Exact” pays at once', { def: false, hint: 'Off: Exact fills in the amount' }),
      stepperRow({ id: 'split_max', label: 'Split evenly up to', value: c.splitMax, min: 2, max: 12, unit: 'people', save: (v) => saveKey('split_max', v) }),
      chipRow({ id: 'cash_quick', label: 'Cash buttons', tip: 'The total rounded up is offered too. The 4 smallest amounts above the total show.', values: c.notes.map((x) => x / 100), min: 1, max: 8,
        validate: (l) => (goodNotes(l) ? null : { msg: 'Use 1 to 8 amounts above 0', index: l.findIndex((x) => !(x > 0 && x <= 1e6 && twoDecimals(x))) }),
        save: (l) => saveKey('cash_quick', l.join(',')) }),
    ]),
    ssec('Tips', [keySwitch(raw, 'tips_on', 'Ask for tips', { after: (v) => { tipChips.hidden = !v; } }), tipChips]),
    ssec('Payment names', [nameRow('cash', 'Cash is called'), nameRow('card', 'Card is called', 'Used on buttons, receipts and reports, e.g. เงินสด or QR / Card.')]),
    ssec('Receipts', [
      receipt('auto_print', 'Print after payment', rc.auto_print !== false),
      receipt('drawer_on_cash', 'Open drawer for cash', rc.drawer_on_cash !== false),
      receipt('staff_copy', 'Staff copy after payment', rc.staff_copy !== false, { hint: 'With tax by class' }),
      receipt('tax_breakdown', 'VAT breakdown on bills', rc.tax_breakdown === true),
      receipt('show_staff', 'Show “Served by”', rc.show_staff !== false),
      textRow({ id: 'footer', label: 'Bottom message', value: rc.footer ?? '', placeholder: 'e.g. Thank you!', save: (v) => saveReceipt({ footer: v || null }) }),
    ]),
    ssec('Service charge', [statusRow({ id: 'service', icon: 'tag', title: 'Service charge',
      meta: svcOn ? `${pct(svc.rate_bp)} % · ${svc.order_types.map((t) => TYPE[t]).join(', ') || 'no orders'}` : 'Not added to bills',
      pill: svcOn ? { text: 'On', cls: 'ok' } : { text: 'Off' }, onOpen: () => serviceSheet(svc) })]),
  ];
}
function serviceSheet(svc) {
  const f = { rate: svc.rate_bp / 100, label: svc.label ?? '', types: [...svc.order_types] };
  const label = h('input', { value: f.label, placeholder: 'Service charge', 'aria-label': 'Name on bill' });
  label.addEventListener('input', () => { f.label = label.value; });
  sheet({ title: 'Service charge', body: [
    ssec('Charge', [srow({ label: 'Rate', ctl: stepper({ value: f.rate, min: 0, max: 30, step: 0.5, unit: '%', label: 'Rate', onChange: (v) => { f.rate = v; } }) }),
      srow({ label: 'Name on bill', cls: 'text-row', ctl: label })]),
    ssec('Add it to new', [srow({ label: 'Order types', stacked: true, ctl: toggleChips(Object.entries(TYPE), f.types, (l) => { f.types = l; }, 'Order types') })],
      '% of the food after discounts, no tax. Staff can remove it.'),
  ], onSave: async () => {
    if (!(f.rate >= 0 && f.rate <= 30) || !twoDecimals(f.rate)) return 'Use 0 to 30 %';
    await call('set_service_settings', { settings: { rate_bp: Math.round(f.rate * 100), label: f.label.trim() || 'Service charge', order_types: f.types } }, { quiet: true });
    state.menu = null;
    redrawGroup('service');
  } });
}

// ---------- Settings: Orders ----------

const TICKET_LABELS = [['label_dine_in', 'Dine-in', 'DINE-IN', 'Before the table, e.g. DINE-IN T3. Empty: the table alone.'],
  ['label_takeaway', 'Takeaway', 'TAKEAWAY'], ['label_collection', 'Collection', 'COLLECTION'], ['label_delivery', 'Delivery', 'DELIVERY'],
  ['label_table', 'Call mains', 'TABLE', 'Before the number on Call mains again. Empty: the number alone.']];
async function ordersGroup() {
  const keys = ['show_takeaway', 'show_collection', 'show_delivery', 'discount_presets', 'discount_staff_max_pct', 'starter_choice', QUICK_NOTES_KEY,
    'print_cancelled_order', 'staff_cancel_sent', ...TICKET_LABELS.map(([k]) => k)];
  const raw = await invoke('get_settings', { keys });
  const admin = isAdmin();
  const ui = uiSettings(raw);
  // Takeaway, collection and delivery print the standard word when blank; Dine-in and
  // Call mains print nothing (pos-core printing.rs label()).
  const labelsCustom = TICKET_LABELS.some(([k, , def]) => raw[k] != null && raw[k].trim() !== def && (BLANK_PRINTS_NOTHING.has(k) || raw[k].trim() !== ''));
  const notes = quickNotes(raw[QUICK_NOTES_KEY]);
  return [
    admin && ssec('Order types', [
      keySwitch(raw, 'show_takeaway', 'Takeaway'),
      keySwitch(raw, 'show_collection', 'Collection'),
      keySwitch(raw, 'show_delivery', 'Delivery', { hint: 'Your own drivers' }),
      navRow({ id: 'labels', label: 'Names on tickets', value: labelsCustom ? 'Custom' : 'Standard', onOpen: () => labelsSheet(raw) }),
    ], 'Dine-in always shows. Open orders keep working.'),
    ssec('Discounts', [
      admin && chipRow({ id: 'discount_presets', label: 'Discount buttons', values: ui.discountPresets, max: 8, unit: '%',
        validate: (l) => { const i = l.findIndex((n) => !Number.isInteger(n) || n < 1 || n > 100); return i >= 0 ? { msg: 'Use whole numbers from 1 to 100 %', index: i } : null; },
        save: (l) => saveKey('discount_presets', l.join(',')) }),
      staffLimitRow(ui.discountLimit),
    ]),
    ssec('Taking orders', [
      keySwitch(raw, 'starter_choice', 'Ask “Starter or main?”', { hint: 'Dine-in, when a starter is tapped',
        tip: 'Main course: the dish waits for the mains. “Which team?” replaces this when it is set.' }),
      navRow({ id: QUICK_NOTES_KEY, label: 'Quick notes', value: `${notes.length} note${notes.length === 1 ? '' : 's'}`, onOpen: () => quickNotesSheet(notes) }),
    ]),
    ssec('Cancel & fix', [
      keySwitch(raw, 'print_cancelled_order', 'Print CANCELLED ticket', { hint: 'When a whole order is cancelled',
        tip: 'Lists dishes still cooking. Off: nothing prints, so tell the kitchen. Removing one dish always prints.' }),
      admin && keySwitch(raw, 'staff_cancel_sent', 'Staff can fix sent dishes', { hint: 'With a manager PIN. Off: managers and admins only' }),
    ]),
  ];
}
/// Staff limit on discounts: No limit, or Up to N % (more needs a manager's PIN).
function staffLimitRow(limit) {
  let saved = limit, timer, row;
  const step = stepper({ value: limit ?? DISCOUNT_LIMIT_DEFAULT, min: 0, max: 100, step: 5, unit: '%', label: 'Staff limit', onChange: (v) => {
    clearTimeout(timer);
    timer = setTimeout(async () => { if (await saveWith(row, () => saveKey('discount_staff_max_pct', v), () => step.set(saved ?? DISCOUNT_LIMIT_DEFAULT))) saved = v; }, 600);
  } });
  step.hidden = limit == null;
  const seg = sseg([['none', 'No limit'], ['upto', 'Up to']], limit == null ? 'none' : 'upto', async (v, prev) => {
    step.hidden = v === 'none';
    const value = v === 'none' ? '' : step.value();
    if (await saveWith(row, () => saveKey('discount_staff_max_pct', value), () => { seg.set(prev); step.hidden = prev === 'none'; })) saved = v === 'none' ? null : step.value();
  }, 'Staff limit');
  row = srow({ id: 'discount_staff_max_pct', label: 'Staff limit', hint: 'More needs a manager PIN', stacked: true, cls: 'limit-row', ctl: [seg, step] });
  return row;
}
const BLANK_PRINTS_NOTHING = new Set(['label_dine_in', 'label_table']);
function labelsSheet(raw) {
  // Dine-in and Call mains: a saved blank prints nothing ("None"); emptied now, the same.
  const blankPh = (key, def, cleared) => (BLANK_PRINTS_NOTHING.has(key) && (cleared || raw[key] != null) ? 'None' : def);
  const touched = new Set();
  const inputs = TICKET_LABELS.map(([key, , def]) => {
    const el = h('input', { value: raw[key] ?? '', placeholder: blankPh(key, def, false), 'aria-label': key });
    el.addEventListener('input', () => { touched.add(key); el.placeholder = blankPh(key, def, true); });
    return el;
  });
  sheet({ title: 'Names on tickets', body: [
    ssec(null, TICKET_LABELS.map(([, label, , tip], i) => srow({ label, tip, cls: 'text-row', ctl: inputs[i] })),
      'Empty: the standard word. Dine-in and Call mains: nothing.'),
  ], onSave: async () => {
    // Only boxes that were changed are written: an untouched empty box stays "not set".
    for (const [i, [key]] of TICKET_LABELS.entries()) {
      const v = inputs[i].value.trim();
      if (v !== (raw[key] ?? '').trim() || (touched.has(key) && raw[key] == null && BLANK_PRINTS_NOTHING.has(key))) await call('set_setting', { key, value: v }, { quiet: true });
    }
    redrawGroup('labels');
  } });
}
function quickNotesSheet(notes) {
  const rows = notes.map((text) => ({ text }));
  const editor = listEditor({ rows, cls: 'notes', max: 30, addLabel: 'Add note', name: (r) => r.text, make: () => ({ text: '' }), onChange: () => {},
    empty: emptyState({ icon: 'orders', title: 'No quick notes', text: 'Add the requests staff type most.' }),
    render: (r) => h('input', { value: r.text, maxlength: 40, placeholder: 'e.g. No onions', 'aria-label': 'Quick note', oninput: (e) => { r.text = e.target.value; } }) });
  sheet({ title: 'Quick notes', body: [h('div', { class: 'sgroup' }, editor)], onSave: async () => {
    const list = rows.map((r) => r.text.trim()).filter(Boolean);
    if (list.some((n) => n.length > 40)) return 'Keep each note to 40 characters';
    await call('set_setting', { key: QUICK_NOTES_KEY, value: list.join('\n') }, { quiet: true });
    state.menu = null;
    redrawGroup(QUICK_NOTES_KEY);
  } });
}

// ---------- Settings: Kitchen ----------

const KITCHEN_KEYS = [...KDS_KEYS, 'drinks_split', 'desserts_split', 'teams_on', 'teams_split', 'team_names', 'team_choice'];
const TEAM_NAMES = [['both', 'EN + TH'], ['english', 'English'], ['thai', 'Thai']];
const KDS_DOT = (cls) => h('span', { class: `kds-dot ${cls}` });
async function kitchenGroup() {
  const [raw, teams] = await Promise.all([invoke('get_settings', { keys: KITCHEN_KEYS }), call('list_teams').catch(() => [])]);
  let n = null;
  if (isAdmin()) {
    const [rc, kc, routes] = await Promise.all([call('get_receipt_settings'), call('get_kitchen_printer'), call('get_kitchen_routes')]).catch(() => [null, null, []]);
    n = [rc, kc, ...routes].filter((p) => p?.kind && p.kind !== 'none').length;
  }
  const k = kdsSettings(raw);
  const teamsOn = raw.teams_on == null ? true : raw.teams_on !== '0';
  // "Which team?": two or more teams, or none (off). One alone is not a choice.
  const chosen = (raw.team_choice ?? '').split(',').map((x) => parseInt(x, 10)).filter((x) => teams.some((t) => t.id === x));
  let choiceRow;
  const choice = teams.length >= 2 && toggleChips(teams.map((t) => [t.id, t.name]), chosen, async (list) => {
    if (list.length === 1) return rowState(choiceRow, 'error', 'Pick 2 or more, or none');
    await saveWith(choiceRow, () => saveKey('team_choice', list.join(',')));
  }, 'Which team?');
  choiceRow = srow({ id: 'team_choice', label: 'Ask “Which team?”', hint: teams.length >= 2 ? 'Dine-in. Pick 2 or more, or none.' : 'Needs 2 or more teams',
    tip: 'The dish prints under the chosen team for this order. Replaces Starter or main.', stacked: !!choice, ctl: choice || null });
  const sample = srow({ id: 'sample', label: 'Sample tickets', ctl: h('button', { class: 'test', onclick: async () => {
    rowState(sample, 'saving');
    try {
      const r = await call('test_teams_print', {}, { quiet: true });
      if (r.error) rowState(sample, 'error', `Didn’t print: ${r.error}`, true); else rowState(sample, 'saved');
    } catch (e) { rowState(sample, 'error', String(e?.message ?? e), true); }
  } }, 'Print') });
  const deps = [
    navRow({ id: 'teams', label: 'Teams', value: teams.length ? teams.map((t) => t.name).join(', ') : 'None yet', onOpen: () => openSub('teams') }),
    keySwitch(raw, 'teams_split', 'Own ticket per team', { hint: 'Off: one ticket with team headers' }),
    segRow({ id: 'team_names', label: 'Team names', items: TEAM_NAMES, value: TEAM_NAMES.some(([v]) => v === raw.team_names) ? raw.team_names : 'both',
      stacked: false, cls: 'seg-fold', save: (v) => saveKey('team_names', v) }),
    choiceRow, sample,
  ];
  const showDeps = (on) => { for (const r of deps) r.hidden = !on; };
  for (const r of deps) r.dataset.dep = 'teams_on';
  showDeps(teamsOn);
  // Red comes after orange: each checks the other's saved value.
  let slow = k.slow, late = k.late;
  const orange = stepperRow({ id: 'kds_slow_mins', label: 'Orange after', value: k.slow, min: 1, max: 239, unit: 'min',
    validate: (v) => (v >= late ? 'Orange must come before red' : null), save: async (v) => { await saveKey('kds_slow_mins', v); slow = v; } });
  const red = stepperRow({ id: 'kds_late_mins', label: 'Red after', value: k.late, min: 2, max: 240, unit: 'min',
    validate: (v) => (v <= slow ? 'Red must come after orange' : null), save: async (v) => { await saveKey('kds_late_mins', v); late = v; } });
  orange.querySelector('.srow-label').prepend(KDS_DOT('slow'));
  red.querySelector('.srow-label').prepend(KDS_DOT('late'));
  return [
    ssec('Kitchen tickets', [
      keySwitch(raw, 'drinks_split', 'Drinks on their own ticket', { hint: 'Drinks still go at once' }),
      keySwitch(raw, 'desserts_split', 'Desserts on their own ticket'),
      isAdmin() && navRow({ id: 'printers', label: 'Printers', value: n == null ? null : `${n} printer${n === 1 ? '' : 's'}`, onOpen: () => openManage('g:printers') }),
    ], 'Mark drinks and desserts in Menu.'),
    ssec('Teams', [
      // Switched on, the team rows come back with what is saved (Which team? reads empty while off).
      keySwitch(raw, 'teams_on', 'Use kitchen teams', { hint: (v) => (v ? '' : 'Off: one ticket, no team headers'), after: (v) => (v ? redrawGroup('teams_on') : showDeps(false)) }),
      deps,
    ]),
    ssec('Kitchen screen', [
      orange, red,
      stepperRow({ id: 'kds_refresh_secs', label: 'Refresh every', value: k.refresh, min: 2, max: 30, unit: 's', save: (v) => saveKey('kds_refresh_secs', v) }),
      stepperRow({ id: 'kds_paid_hours', label: 'Keep paid orders', hint: '0 = remove at once', value: k.paid, min: 0, max: 24, unit: 'h', save: (v) => saveKey('kds_paid_hours', v) }),
      keySwitch(raw, 'kds_beep', 'Beep for new tickets', { def: false,
        tip: 'Browsers play sound only after one tap. If the screen says “Tap to turn on the sound”, tap it once.' }),
    ]),
  ];
}

/// Kitchen › Teams: names (English, Thai) in ticket order; the save bar saves the list.
async function teamsPage() {
  const load = async () => (await call('list_teams')).map((t) => ({ id: t.id, name: t.name, name_alt: t.name_alt ?? '', items: t.items }));
  let saved = await load();
  const rows = saved.map((r) => ({ ...r }));
  const changes = () => {
    const ids = new Set(rows.map((r) => r.id));
    return rows.filter((r, i) => { const s = saved[i]; return !s || s.id !== r.id || s.name !== r.name || (s.name_alt ?? '') !== (r.name_alt ?? ''); }).length
      + saved.filter((s) => !ids.has(s.id)).length;
  };
  const bar = saveBar({
    onSave: async () => {
      const bad = rows.findIndex((r) => !r.name.trim());
      if (bad >= 0) {
        const input = editor.querySelectorAll('.lrow')[bad]?.querySelector('input');
        input?.classList.add('bad');
        tapFocus(input);
        throw new Error('Give the team a name');
      }
      await call('save_teams', { teams: rows.map((r) => ({ id: r.id, name: r.name.trim(), name_alt: (r.name_alt ?? '').trim() || null })) }, { quiet: true });
      state.menu = null;
      saved = await load();
      rows.splice(0, rows.length, ...saved.map((r) => ({ ...r })));
      editor.redraw();
    },
    onDiscard: () => { rows.splice(0, rows.length, ...saved.map((r) => ({ ...r }))); editor.redraw(); },
  });
  const typed = (r, key) => (e) => { r[key] = e.target.value; e.target.classList.remove('bad'); bar.update(changes()); };
  const editor = listEditor({ rows, cls: 'teams', addLabel: 'Add team', make: () => ({ id: null, name: '', name_alt: '', items: null }), onChange: () => bar.update(changes()),
    empty: emptyState({ icon: 'chef', title: 'No teams yet', text: 'Each team gets its own ticket header.' }),
    render: (r) => [h('input', { value: r.name, maxlength: 40, placeholder: 'e.g. Curry', 'aria-label': 'Team name', oninput: typed(r, 'name') }),
      h('input', { value: r.name_alt ?? '', maxlength: 40, placeholder: 'แกง', lang: 'th', class: 'alt-input', 'aria-label': 'Thai name', oninput: typed(r, 'name_alt') }),
      h('span', { class: 'lmeta' }, r.items == null ? 'New' : `${r.items} item${r.items === 1 ? '' : 's'}`)] });
  state.dirty = false;
  return [h('p', { class: 'snote' }, 'Set a team per category in Menu. Drag to change the ticket order.'), h('div', { class: 'sgroup' }, editor), bar];
}

// ---------- Settings: Login & staff ----------

async function loginGroup() {
  const raw = await invoke('get_settings', { keys: ['start_screen', 'auto_logout_minutes', 'auto_logout_warn_secs', 'pin_auto_login', 'tickets_call_text'] });
  const callText = raw.tickets_call_text ?? '';
  const ui = uiSettings(raw);
  const people = await call('list_users').then((u) => u.length).catch(() => null);
  const mins = [[0, 'Never'], [1, '1 minute'], [2, '2 minutes'], [5, '5 minutes'], [10, '10 minutes'], [15, '15 minutes'], [30, '30 minutes'], [60, '1 hour']];
  if (!mins.some(([v]) => v === ui.autoLogout)) mins.push([ui.autoLogout, `${ui.autoLogout} minutes`]);
  const secs = [[0, 'No warning'], [10, '10 seconds'], [20, '20 seconds'], [30, '30 seconds'], [60, '1 minute']];
  if (!secs.some(([v]) => v === ui.warnSecs)) secs.push([ui.warnSecs, `${ui.warnSecs} seconds`]);
  const warn = selectRow({ id: 'auto_logout_warn_secs', label: 'Warn before', options: secs, value: ui.warnSecs, save: (v) => saveKey('auto_logout_warn_secs', v) });
  warn.hidden = !ui.autoLogout;
  warn.dataset.dep = 'auto_logout_minutes';
  return [
    ssec('After login', [segRow({ id: 'start_screen', label: 'Start screen', items: START_SCREENS, value: ui.startScreen,
      tip: 'New takeaway opens an order at once. An empty one is dropped.', save: (v) => saveKey('start_screen', v) })]),
    ssec('Auto logout', [
      selectRow({ id: 'auto_logout_minutes', label: 'Log out after', hint: 'No touch for this long', options: mins, value: ui.autoLogout,
        save: (v) => saveKey(AUTO_LOGOUT_KEY, v), after: (v) => { state.autoLogout = Number(v); warn.hidden = !Number(v); } }),
      warn,
    ], 'Open orders are kept. Applies to every device.'),
    ssec('PIN', [keySwitch(raw, 'pin_auto_login', 'Log in without OK', { hint: 'As soon as the PIN is complete',
      tip: 'Works after each person logs in once. Shorter PINs still press OK.' })]),
    ssec('Tickets', [textRow({ id: 'tickets_call_text', label: 'Who staff call when urgent', hint: 'Shown when no manager is logged in', value: callText,
      placeholder: 'e.g. Quinn: 07700 900123', max: 80, save: (v) => saveKey('tickets_call_text', v) })]),
    ssec('Staff', [navRow({ id: 'staff', label: 'Staff & PINs', value: people == null ? null : `${people} ${people === 1 ? 'person' : 'people'}`, onOpen: () => openManage('p:users') })]),
  ];
}

// ---------- Settings: Restaurant ----------

async function restaurantGroup() {
  const [rc, raw, untaxed] = await Promise.all([call('get_receipt_settings'), invoke('get_settings', { keys: ['prices_include_tax', 'default_tax_bp'] }),
    call('untaxed_items', {}, { quiet: true }).catch(() => 0)]);
  return [
    ssec('Name & address', [
      textRow({ id: 'store_name', label: 'Restaurant name', value: state.storeName ?? '', placeholder: 'Restaurant name', save: (v) => saveKey('store_name', v),
        after: (v) => {
          state.storeName = v;
          document.title = v || 'PuppxPOS';
          document.getElementById('brand').title = v || 'PuppxPOS';
          const bills = document.querySelector('[data-row="shop_name"] input');
          if (bills) bills.placeholder = v || 'Restaurant name';
        } }),
      textRow({ id: 'shop_name', label: 'Name on bills', hint: 'Empty: the restaurant name', value: rc.shop_name ?? '', placeholder: state.storeName || 'Restaurant name',
        save: (v) => saveReceipt({ shop_name: v || null }) }),
      textRow({ id: 'header', label: 'Address & phone', hint: 'One per line, e.g. VAT number', area: true, value: rc.header ?? '', placeholder: '12 High Street\n020 7946 0000',
        save: (v) => saveReceipt({ header: v || null }) }),
      textRow({ id: 'currency', label: 'Currency symbol', value: rc.currency ?? '£', max: 4, cls: 'short', attrs: { class: 'short-in' }, save: (v) => saveReceipt({ currency: v }) }),
    ]),
    taxSection(raw, untaxed),
  ];
}
/// Tax: prices include it or not, the default rate, and items still at 0 %.
function taxSection(raw, untaxed) {
  let rate = Number(raw.default_tax_bp) || 0;
  const example = (on) => {
    if (!on) return 'Tax is added on top';
    if (!rate) return 'The menu price is what the customer pays';
    const net = 1000 / (1 + rate / 10000);
    return `10.00 at ${pct(rate)} % = ${(net / 100).toFixed(2)} + ${((1000 - net) / 100).toFixed(2)} tax`;
  };
  const incl = switchRow({ id: 'prices_include_tax', label: 'Prices include tax', hint: example, value: raw.prices_include_tax !== '0',
    save: (v) => saveKey('prices_include_tax', v ? '1' : '0'), after: () => { state.menu = null; } });
  // Items still at 0 % that a rate would now apply to: one tap gives them the rate.
  const strip = h('div', { class: 'srow-strip', role: 'status' });
  const drawStrip = (n) => {
    strip.hidden = !n || !rate;
    if (strip.hidden) return;
    setKids(strip, h('span', { class: 'grow' }, `${n} item${n === 1 ? ' is' : 's are'} at 0 %`),
      h('button', { onclick: async (e) => {
        const b = e.currentTarget;
        b.disabled = true;
        try {
          const done = await call('apply_default_tax', {}, { quiet: true });
          state.menu = null;
          setKids(strip, h('span', { class: 'grow ok' }, `✓ ${done} item${done === 1 ? '' : 's'} updated`));
          setTimeout(() => call('untaxed_items', {}, { quiet: true }).then(drawStrip).catch(() => {}), 2500);
        } catch (err) { b.disabled = false; setKids(strip, h('span', { class: 'grow bad' }, `Not saved: ${String(err?.message ?? err)}`), b); }
      } }, `Apply ${pct(rate)} %`));
  };
  drawStrip(untaxed);
  const taxRow = textRow({ id: 'default_tax_bp', label: 'Default tax rate', tip: 'A category can have its own rate (Menu › category).',
    value: rate ? pct(rate) : '', placeholder: '0', unit: '%', cls: 'short', attrs: { class: 'short-in', inputmode: 'decimal' },
    validate: (v) => { const n = Number((v || '0').replace(',', '.').replace('%', '')); return Number.isFinite(n) && n >= 0 && n <= 100 && twoDecimals(n) ? null : 'Use 0 to 100 %.'; },
    save: (v) => saveKey('default_tax_bp', Math.round(Number((v || '0').replace(',', '.').replace('%', '')) * 100)),
    after: (v) => {
      rate = Math.round(Number((v || '0').replace(',', '.').replace('%', '')) * 100);
      state.menu = null;
      setHint(incl, example(incl.input.checked));
      call('untaxed_items', {}, { quiet: true }).then(drawStrip).catch(() => {});
    } });
  return h('section', { class: 'ssec', 'data-row': 'tax' }, h('h3', { class: 'ssec-title' }, 'Tax'), h('div', { class: 'sgroup' }, incl, taxRow, strip));
}

// ---------- Settings: Delivery apps ----------

async function deliveryGroup() {
  const load = async () => (await call('list_channels')).map((c) => ({ id: c.id, name: c.name, color: c.color || '#374151' }));
  let saved = await load();
  const rows = saved.map((r) => ({ ...r }));
  const changes = () => {
    const ids = new Set(rows.map((r) => r.id));
    return rows.filter((r, i) => { const s = saved[i]; return !s || s.id !== r.id || s.name !== r.name || s.color !== r.color; }).length
      + saved.filter((s) => !ids.has(s.id)).length;
  };
  const bar = saveBar({
    onSave: async () => {
      await call('save_channels', { channels: rows.filter((r) => r.name.trim()).map((r) => ({ id: r.id, name: r.name.trim(), color: r.color })) }, { quiet: true });
      saved = await load();
      rows.splice(0, rows.length, ...saved.map((r) => ({ ...r })));
      editor.redraw();
    },
    onDiscard: () => { rows.splice(0, rows.length, ...saved.map((r) => ({ ...r }))); editor.redraw(); },
  });
  const editor = listEditor({ rows, cls: 'apps', addLabel: 'Add app', make: () => ({ id: null, name: '', color: '#374151' }), onChange: () => bar.update(changes()),
    empty: emptyState({ icon: 'bike', title: 'No delivery apps', text: 'Add Uber Eats, Just Eat or others.' }),
    render: (r) => [h('input', { type: 'color', class: 'swatch', value: r.color, 'aria-label': `Colour of ${r.name || 'the app'}`, oninput: (e) => { r.color = e.target.value; bar.update(changes()); } }),
      h('input', { value: r.name, placeholder: 'App name', 'aria-label': 'App name', oninput: (e) => { r.name = e.target.value; bar.update(changes()); } })] });
  state.dirty = false;
  return [h('p', { class: 'snote' }, 'Each app gets an Orders button, a ticket heading and a report line.'), h('div', { class: 'sgroup' }, editor), bar];
}

/// Pick several things and delete them in one go (admins only), in a sheet:
/// a switch per thing, then Delete N and a second tap. `items`: [{ id, label, sub }];
/// `del(ids)` deletes and returns a message.
function bulkDelete(title, items, del, done) {
  if (!items.length) return toast('Nothing to delete here', true);
  const picked = new Set();
  let sure = false;
  const drawBtn = () => {
    s.save.textContent = !picked.size ? 'Pick what to delete' : sure ? `Tap again: delete ${picked.size}` : `Delete ${picked.size}`;
    s.save.disabled = !picked.size;
  };
  const rows = items.map((it) => switchRow({ label: it.label, hint: it.sub ?? '', value: false, onChange: (v) => {
    if (v) picked.add(it.id); else picked.delete(it.id);
    sure = false;
    all.input.checked = picked.size === items.length;
    drawBtn();
  } }));
  const all = switchRow({ label: 'All', hint: `${items.length}`, value: false, onChange: (v) => {
    picked.clear();
    if (v) items.forEach((it) => picked.add(it.id));
    for (const r of rows) r.input.checked = v;
    sure = false;
    drawBtn();
  } });
  const s = sheet({ title, danger: true, saveLabel: 'Delete', body: [ssec(null, [all]), ssec(null, rows)], onSave: async () => {
    if (!sure) { sure = true; drawBtn(); return false; }
    const msg = await del([...picked]);
    toast(msg);
    setTimeout(() => done?.());
  } });
  drawBtn();
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

// ---------- Settings: Backup & data ----------

const KEEP_LIMITS = [3, 90, 14]; // backups_kept: min, max, not set
const listBackups = () => call('list_full_backups', {}, { quiet: true }).catch(() => []);
const listMenuBackups = () => call('list_menu_backups', {}, { quiet: true }).catch(() => []).then((l) => [...l].sort((a, b) => b.saved_at - a.saved_at));
/// One full backup: when, kind · size, [Restore] and ⋯ (Download in a browser, Delete).
function backupRow(f) {
  const row = srow({ id: `b:${f.name}`, label: friendlyTime(f.modified), hint: `${backupKind(f.name)} · ${fileSize(f.size)}`, cls: 'brow', ctl: [
    h('button', { class: 'test', onclick: async () => {
      rowState(row, 'saving');
      try { const sum = await call('inspect_backup', { name: f.name }, { quiet: true }); rowState(row, 'idle'); restoreSheet(f, sum); } catch (e) { rowState(row, 'error', String(e?.message ?? e), true); }
    } }, 'Restore'),
    moreMenu(friendlyTime(f.modified), () => [
      state.remote && h('button', { onclick: act(() => downloadBackup(f.name)) }, 'Download'),
      deleteItem(async () => { await call('delete_backups', { names: [f.name] }, { quiet: true }); toast('Backup deleted'); }),
    ]),
  ] });
  return row;
}
/// One menu backup: when, N items, [Restore] (a confirm sheet) and ⋯ Delete.
function menuBackupRow(f) {
  return srow({ id: `m:${f.name}`, label: friendlyTime(f.saved_at), hint: `${f.items} items`, cls: 'brow', ctl: [
    h('button', { class: 'test', onclick: () => menuReplaceSheet(`Restore the menu from ${friendlyTime(f.saved_at)}?`, `${f.items} items`, () => call('restore_menu_backup', { name: f.name }, { quiet: true })) }, 'Restore'),
    moreMenu(friendlyTime(f.saved_at), () => [deleteItem(async () => { await call('delete_menu_backups', { names: [f.name] }, { quiet: true }); toast('Menu backup deleted'); })]),
  ] });
}
/// Delete in a ⋯ menu: the first tap asks, the second deletes, then the page is drawn again.
const deleteItem = (fn) => {
  const b = confirmBtn('Delete', 'Tap again to delete', async () => { closeInfoPop(); await fn(); redrawGroup(); }, 'ghost danger');
  b.dataset.keep = '1';
  return b;
};
/// Replace the menu (from a menu backup or a file): one confirm sheet.
function menuReplaceSheet(title, facts, run) {
  sheet({ title, danger: true, saveLabel: 'Replace menu', body: [h('p', { class: 'snote' }, facts),
    h('div', { class: 'warn-box' }, 'The menu here is replaced. Past orders do not change.')], onSave: async () => {
    const n = await run();
    state.menu = null;
    toast(`Menu restored: ${n.items} items, ${n.categories} categories, ${n.option_groups} option groups`);
    redrawGroup();
  } });
}
/// A hidden file picker: `pick(file)` when one is chosen.
const filePicker = (accept, pick) => {
  const input = h('input', { type: 'file', accept, hidden: true });
  input.addEventListener('change', () => { const f = input.files[0]; input.value = ''; if (f) pick(f); });
  return input;
};
async function dataGroup() {
  const [files, menus, raw, dir] = await Promise.all([listBackups(), listMenuBackups(), invoke('get_setting', { key: 'backups_kept' }).catch(() => null),
    invoke('backup_folder').catch(() => null)]);
  const [KMIN, KMAX, KDEF] = KEEP_LIMITS;
  const kept = Number(raw);
  // The newest regular backup (daily, or Back up now): not a copy made before a restore, reset or update.
  const last = files.find((f) => backupKind(f.name) === 'Backup');
  const backupBtn = h('button', { class: 'primary', onclick: async () => {
    const row = backupBtn.closest('.srow');
    backupBtn.disabled = true;
    rowState(row, 'saving');
    try { toast(`Backup saved: ${await call('backup_now', {}, { quiet: true })}`); await redrawGroup('backup'); } catch (e) { backupBtn.disabled = false; rowState(row, 'error', String(e?.message ?? e)); }
  } }, 'Back up now');
  // Restore from a file: the upload's progress in the row's hint, then the restore sheet.
  const fromFile = navRow({ id: 'fromfile', label: 'Restore from a file…', onOpen: () => fullPick.click() });
  const fullPick = filePicker('.db', async (f) => {
    try {
      const r = await uploadBackup(f, (n) => setHint(fromFile, `Loading ${f.name}… ${n} %`));
      setHint(fromFile, '');
      restoreSheet(r.file, r.summary);
    } catch (e) { setHint(fromFile, ''); rowState(fromFile, 'error', String(e?.message ?? e), true); }
  });
  const menuPick = filePicker('.json,application/json', async (f) => {
    let menu;
    try { menu = JSON.parse(await f.text()); } catch { menu = null; }
    if (menu?.format !== 'pos-menu') return rowState(menuFile, 'error', 'This file is not a POS menu backup', true);
    menuReplaceSheet(`Replace the menu with ${f.name}?`, `${menu.items.length} items, ${menu.categories.length} categories`, () => call('import_menu', { menu }, { quiet: true }));
  });
  const menuFile = navRow({ id: 'menufile', label: 'Load a menu file…', onOpen: () => menuPick.click() });
  const menuBtn = h('button', { class: 'test', onclick: async () => {
    const row = menuBtn.closest('.srow');
    rowState(row, 'saving');
    try {
      const r = await call('backup_menu', {}, { quiet: true });
      toast(`Menu saved (${r.file.items} items)`);
      if (state.remote) {
        // In a browser, also give the user the file.
        const a = h('a', { href: URL.createObjectURL(new Blob([JSON.stringify(r.menu, null, 2)], { type: 'application/json' })), download: r.file.name });
        a.click();
        setTimeout(() => URL.revokeObjectURL(a.href), 1000);
      }
      await redrawGroup('menu');
    } catch (e) { rowState(row, 'error', String(e?.message ?? e)); }
  } }, 'Back up');
  return [
    ssec('Full backup', [
      statusRow({ id: 'backup', icon: 'database', title: 'Last backup', meta: `${last ? friendlyTime(last.modified) : 'None yet'} · daily at start`, action: backupBtn }),
      stepperRow({ id: 'backups_kept', label: 'Keep', hint: 'Daily copies only', value: Number.isInteger(kept) && kept >= KMIN && kept <= KMAX ? kept : KDEF,
        min: KMIN, max: KMAX, unit: 'backups', save: (v) => saveKey('backups_kept', v) }),
      files.slice(0, 3).map(backupRow),
      files.length > 3 && navRow({ id: 'allbackups', label: 'All backups', value: String(files.length), onOpen: () => openSub('backups') }),
      fromFile, fullPick,
      dir && srow({ id: 'folder', label: 'Backup folder', hint: dir, cls: 'path-row',
        tip: 'Moving to a new PC? Copy the newest backup to a USB stick, then use Restore from a file there.',
        ctl: !state.remote && h('button', { class: 'test', onclick: act(() => invoke('open_backup_folder')) }, 'Open') }),
    ]),
    ssec('Menu only', [
      srow({ id: 'menu', label: 'Back up the menu', ctl: menuBtn }),
      menus.slice(0, 3).map(menuBackupRow),
      menus.length > 3 && navRow({ id: 'allmenus', label: 'All menu backups', value: String(menus.length), onOpen: () => openSub('menus') }),
      menuFile, menuPick,
    ]),
    ...await backupAutoSection(),
    await retentionSection(),
    ssec('Danger zone', [navRow({ id: 'reset', label: 'Reset data…', cls: 'danger', onOpen: resetSheet })]),
  ];
}
/// Backup & data › All backups (or All menu backups): every one, and Select… to delete several.
async function backupsPage(kind) {
  const full = kind === 'full';
  const list = full ? await listBackups() : await listMenuBackups();
  const select = () => (full
    ? bulkDelete('Delete backups', list.map((f) => ({ id: f.name, label: friendlyTime(f.modified), sub: `${backupKind(f.name)} · ${fileSize(f.size)}` })),
      async (names) => `${await call('delete_backups', { names })} backup(s) deleted`, () => redrawGroup())
    : bulkDelete('Delete menu backups', list.map((f) => ({ id: f.name, label: friendlyTime(f.saved_at), sub: `${f.items} items` })),
      async (names) => `${await call('delete_menu_backups', { names })} menu backup(s) deleted`, () => redrawGroup()));
  if (!list.length) return [h('div', { class: 'sgroup' }, emptyState({ icon: 'database', title: 'No backups yet', text: full ? 'One is made every day at start.' : 'Back up the menu first.' }))];
  return [h('div', { class: 'sbar' }, h('span', { class: 'grow muted' }, `${list.length} backup${list.length === 1 ? '' : 's'}`), h('button', { onclick: select }, 'Select…')),
    h('div', { class: 'sgroup' }, list.map(full ? backupRow : menuBackupRow))];
}
/// Danger zone: delete chosen data (switches), with the PIN and RESET typed. A full backup is made first.
function resetSheet() {
  const f = { sales: false, when: 'all', date: today(), customers: false, menu: false, floor: false, staff: false, settings: false };
  const date = h('input', { type: 'date', value: f.date, hidden: true, 'aria-label': 'Before this date' });
  date.addEventListener('change', () => { f.date = date.value; });
  const whenRow = srow({ label: 'Which sales', stacked: true, cls: 'when-row', ctl: [sseg([['all', 'All'], ['before', 'Before a date']], 'all', (v) => { f.when = v; date.hidden = v !== 'before'; }, 'Which sales'), date] });
  whenRow.hidden = true;
  const sw = (key, label, hint) => switchRow({ label, hint, value: false, onChange: (v) => { f[key] = v; if (key === 'sales') whenRow.hidden = !v; check(); } });
  const pin = h('input', { type: 'password', inputmode: 'numeric', autocomplete: 'off', 'aria-label': 'Your PIN' });
  const word = h('input', { placeholder: 'RESET', autocomplete: 'off', autocapitalize: 'characters', spellcheck: 'false', 'aria-label': 'Type RESET' });
  const any = () => f.sales || f.customers || f.menu || f.floor || f.staff || f.settings;
  const s = sheet({ title: 'Reset data', danger: true, saveLabel: 'Delete selected', body: [
    ssec('Delete', [sw('sales', 'Sales history', 'Orders, payments, day closes'), whenRow, sw('customers', 'Customers'), sw('menu', 'Menu'),
      sw('floor', 'Floor plan'), sw('staff', 'Staff', 'Everyone except you'), sw('settings', 'Settings')]),
    h('div', { class: 'warn-box' }, 'A full backup is saved first.'),
    ssec('Confirm', [srow({ label: 'Your PIN', cls: 'text-row', ctl: pin }), srow({ label: 'Type RESET', cls: 'text-row', ctl: word })]),
  ], onSave: async () => {
    const options = {
      sales: f.sales ? (f.when === 'all' ? { when: 'all' } : { when: 'before', date: f.date }) : null,
      customers: f.customers, menu: f.menu, floor_plan: f.floor, staff: f.staff, settings: f.settings,
    };
    const r = await call('reset_data', { options, pin: pin.value, confirm: word.value.trim() }, { quiet: true });
    const n = r.report;
    toast(`Deleted: ${n.orders} orders, ${n.customers} customers, ${n.items} items, ${n.tables} tables, ${n.staff} staff. A full backup was saved first.`);
    Object.assign(state, { menu: null, cats: null });
    setTimeout(() => start().catch((e) => toast(String(e?.message ?? e), true)));
  } });
  const NAMES = [['sales', 'sales history'], ['customers', 'customers'], ['menu', 'menu'], ['floor', 'floor plan'], ['staff', 'staff'], ['settings', 'settings']];
  function check() {
    const picked = NAMES.filter(([k]) => f[k]).map(([, n]) => n);
    s.save.textContent = !picked.length ? 'Delete selected' : picked.length <= 2 ? `Delete ${picked.join(' & ')}` : `Delete ${picked.length} kinds`;
    s.save.disabled = !(any() && word.value.trim() === 'RESET' && /^\d{4,8}$/.test(pin.value));
  }
  pin.addEventListener('input', check);
  word.addEventListener('input', check);
  check();
}

// ---------- Settings: About ----------

/// Product name, version and credits (word for word), then Updates (admins on the main till).
function creditsCard() {
  return h('div', { class: 'about-credits sgroup' },
    h('img', { class: 'about-logo', src: 'icon.png', alt: '' }),
    h('div', { class: 'about-text' },
      h('div', { class: 'about-head' }, h('h3', { class: 'about-name' }, 'PuppxPOS'), state.version && h('span', { class: 'chip info' }, `Version ${state.version}`)),
      h('div', { class: 'about-by' }, 'Developed by ', h('strong', {}, 'Pheeraphat Bunliang')),
      h('div', { class: 'muted small' }, 'Designed and developed with AI-assisted programming.'),
      h('div', { class: 'about-copy muted small' }, '© 2026 Pheeraphat Bunliang. All rights reserved.')));
}
function aboutGroup() {
  if (!isAdmin()) return [creditsCard()];
  if (state.remote) return [creditsCard(), ssec('Updates', [srow({ id: 'updates', label: 'Updates', hint: 'Installed on the main till' })])];
  const btn = h('button', { class: 'test' }, 'Check now');
  const row = srow({ id: 'updates', label: 'Updates', hint: 'Checked every few hours', ctl: btn,
    tip: 'Installing takes about a minute. A backup is made first. Orders, menu and settings are kept.' });
  const drawReady = (v) => {
    setHint(row, `Version ${v} is ready`);
    setKids(row.querySelector('.srow-ctl'), h('button', { class: 'primary', onclick: installUpdate }, 'Install'));
  };
  btn.onclick = async () => {
    btn.disabled = true;
    rowState(row, 'saving');
    const info = await checkForUpdate(true);
    btn.disabled = false;
    if (!info) return rowState(row, 'error', 'Could not check. Try again later.', true);
    rowState(row, 'saved');
    if (info.available) drawReady(info.available); else setHint(row, `Up to date · ${info.current}`);
  };
  if (state.update?.available) drawReady(state.update.available);
  return [creditsCard(), ssec('Updates', [row])];
}

// ---------- full backups: restore, load from a file, download ----------

const fileSize = (b) => (b >= 1048576 ? `${(b / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(b / 1024))} KB`);
/// What a full backup file is, from its name (see prune_backups in commands.rs).
function backupKind(name) {
  const kinds = [['pos-auto-', 'While open'], ['pos-exit-', 'When POS closed'], ['pos-emergency-', 'After a crash'],
    ['pos-before-update-', 'Before an update'], ['pos-before-reset-', 'Before a reset'],
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
/// Show what a backup holds, then RESTORE typed and the admin's PIN (the keypad,
/// after the sheet) replace all data with it. `f`: the file (name, modified); `s`: inspect_backup.
function restoreSheet(f, s) {
  const when = s.last_used_at ?? f.modified;
  const word = h('input', { placeholder: 'RESTORE', autocomplete: 'off', autocapitalize: 'characters', spellcheck: 'false', 'aria-label': 'Type RESTORE' });
  const pin = h('input', { type: 'password', inputmode: 'numeric', autocomplete: 'off', 'aria-label': 'Your PIN' });
  const fact = (k, v) => h('div', {}, h('span', { class: 'muted' }, k), h('strong', {}, String(v)));
  const sh = sheet({ title: 'Restore this backup?', danger: true, saveLabel: 'Restore now', body: [
    h('div', { class: 'restore-facts' },
      fact('Restaurant', s.store_name ?? '(no name)'), fact('Last used', friendlyTime(when)),
      fact('Orders', s.open_orders ? `${s.orders} (${s.open_orders} open)` : s.orders), fact('Menu items', s.menu_items),
      fact('Customers', s.customers), fact('Staff', s.staff)),
    h('p', { class: 'snote' }, `${backupKind(f.name)} · ${fileSize(f.size ?? 0)}`, s.schema_version < s.app_schema_version ? ' · older version: brought up to date' : ''),
    h('div', { class: 'warn-box' }, 'Everything here is replaced. A safety copy is saved first. All devices log out.'),
    ssec('Confirm', [srow({ label: 'Type RESTORE', cls: 'text-row', ctl: word }), srow({ label: 'Your PIN', cls: 'text-row', ctl: pin })]),
  ], onSave: async () => {
    const r = await invoke('restore_backup', { name: f.name, pin: pin.value, confirm: word.value.trim() });
    const n = r.summary;
    const msg = `Restored backup from ${friendlyTime(when)}: ${n.orders} orders, ${n.menu_items} menu items, ${n.customers} customers, ${n.staff} staff. ` +
      'Log in with a PIN from the backup.';
    // Reload like every other till and tablet will, and say what happened after.
    try { sessionStorage.setItem(FLASH_KEY, msg); } catch {}
    forgetSession();
    location.reload();
    return false;
  } });
  const check = () => { sh.save.disabled = !(word.value.trim() === 'RESTORE' && /^\d{4,8}$/.test(pin.value)); };
  word.addEventListener('input', check);
  pin.addEventListener('input', check);
  check();
}

// ---------- tickets ("Contact admin") ----------

/// Category: [icon, name]. Quick text: one tap adds it to the message.
const TCAT = { printer: ['printer', 'Printer'], payment: ['card', 'Payment / card'], order: ['receipt', 'Order / bill'],
  menu: ['tag', 'Menu / price'], till: ['monitor', 'Till / tablet'], other: ['help', 'Other'] };
const TQUICK = {
  printer: ['Not printing', 'Paper out', 'Prints blank', 'Prints twice'],
  payment: ['Card machine offline', 'Payment declined', 'Charged twice', 'Need a refund approved'],
  order: ['Wrong item on bill', "Can't close bill", 'Need a refund approved', 'Split went wrong'],
  menu: ['Wrong price', 'Item missing', 'Should be sold out', 'Wrong option'],
  till: ['Screen frozen', 'Very slow', 'Wi-Fi drops', 'Logged out by itself'],
  other: [],
};
const TSTATUS = { open: ['New', 'bad'], in_progress: ['In progress', 'prog'], waiting: ['Waiting', 'wait'], done: ['Done', 'ok'] };
const tcatName = (c) => TCAT[c]?.[1] ?? 'Other';
const tcatIcon = (c) => TCAT[c]?.[0] ?? 'help';
const tstatusPill = (st) => { const [l, c] = TSTATUS[st] ?? [st, '']; return h('span', { class: `pill ${c}` }, l); };
const hm = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
// 12-hour clocks ("10:25 AM") need a wider time column in the Activity log.
if (/[AP]M/i.test(new Date(2020, 0, 1, 15).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }))) document.body.classList.add('h12');
/// "now", "3 min", "2 h", "3 d".
function shortAge(t) {
  const m = Math.max(0, Math.floor((Date.now() / 1000 - t) / 60));
  return m < 1 ? 'now' : m < 60 ? `${m} min` : m < 1440 ? `${Math.floor(m / 60)} h` : `${Math.floor(m / 1440)} d`;
}
/// A table name as on the order screen: "2" → "Table 2", "Window" stays.
const tableTitle = (n) => (n == null ? null : /^\d+[a-z]?$/i.test(n) ? `Table ${n}` : n);
/// May answer tickets (see every ticket, the badge, the urgent alert): managers and admins.
const isAnswerer = () => isManager();

/// This device's name in tickets: set by an admin (Devices & display), else "Main till"
/// on the desktop app or a short browser name ("iPad · Safari").
const DEVICE_NAME_KEY = 'pos.deviceName';
function deviceLabel() {
  const named = pref(DEVICE_NAME_KEY, '').trim();
  if (named) return named;
  if (!state.remote) return 'Main till';
  const ua = navigator.userAgent;
  const dev = /iPad/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1) ? 'iPad' : /iPhone/.test(ua) ? 'iPhone'
    : /Android/.test(ua) ? 'Android' : /Windows/.test(ua) ? 'Windows' : /Mac/.test(ua) ? 'Mac' : /Linux/.test(ua) ? 'Linux' : 'Browser';
  const br = /Edg\//.test(ua) ? 'Edge' : /Chrome\//.test(ua) ? 'Chrome' : /Firefox\//.test(ua) ? 'Firefox' : /Safari\//.test(ua) ? 'Safari' : '';
  return br ? `${dev} · ${br}` : dev;
}

/// What the poll knows (app_status.tickets), print failures of the last 30 minutes,
/// the ticket on screen (no toast or banner for it) and what a view wants redrawn.
const tk = { info: null, fails: [], viewing: null, watch: null, seq: null, first: true, polling: false };
const ssGet = (k) => { try { return JSON.parse(sessionStorage.getItem(k) || '[]'); } catch { return []; } };
const ssSet = (k, v) => { try { sessionStorage.setItem(k, JSON.stringify(v.slice(-50))); } catch {} };
const lsList = (k) => { try { return JSON.parse(localStorage.getItem(k) || '[]'); } catch { return []; } };
/// The urgent tone: on unless turned off on this device (never on the Kitchen screen).
const TONE_KEY = 'pos.ticketSound';
const toneOn = () => pref(TONE_KEY, '1') !== '0';

/// A count in a red circle (nothing for 0).
const countEl = (n, cls = 'mnav-n') => (n > 0 ? h('span', { class: cls }, n > 99 ? '99+' : String(n)) : null);
function setCount(host, n, cls) {
  if (!host) return;
  host.querySelector(`:scope > .${cls}`)?.remove();
  const c = countEl(n, cls);
  if (c) host.append(c);
}

/// Logged out: no badges, no banner; the next login starts afresh.
function ticketsLoggedOut() {
  tk.info = null;
  tk.first = true;
  tk.started = false;
  tk.viewing = null;
  tk.watch = null;
  tk.seq = null;
  drawTicketBadges();
}

/// One look at app_status (the 10 s loop): badges, a toast for a new reply, the
/// urgent alert, and a reload if a backup was restored on another till.
async function pollTickets() {
  if (!state.session || tk.polling || document.visibilityState === 'hidden') return;
  tk.polling = true;
  try {
    const st = await invoke('app_status');
    if (dataChanged(st) || !state.session) return;
    if (st.session && st.session.user_id !== state.session.user_id) return;
    backupAlert(st.backup_alert);
    const t = st.tickets ?? null;
    tk.info = t;
    drawTicketBadges();
    if (!t) return;
    replyToast(t);
    drawUrgent();
    if (tk.seq != null && t.seq !== tk.seq) tk.watch?.();
    tk.seq = t.seq;
  } catch { /* offline: try again next time */ } finally { tk.polling = false; }
}

/// A new reply on one of my tickets: "Mia replied to #14: …" with Open (once per message on this device).
function replyToast(t) {
  const key = `pos.tkToast.${state.session.user_id}`;
  const last = Number(pref(key, '0'));
  const first = tk.first;
  tk.first = false;
  const l = t.latest;
  if (!l) return;
  if (!first && l.msg_id <= last) return;
  setPref(key, String(Math.max(last, l.msg_id)));
  if (tk.viewing === l.ticket_id) return;
  const open = { label: 'Open', onclick: () => openTicket(l.ticket_id) };
  const quote = l.preview.length > 40 ? `${l.preview.slice(0, 40)}…` : l.preview;
  // My own tickets only: a manager's `unread` counts every ticket.
  const mineN = t.unread_mine ?? t.unread;
  if (first && mineN > 1) toast(`${mineN} replies to your tickets`, false, 8000, { label: 'Open', onclick: () => myTicketsSheet() });
  else toast(`${l.by} replied to #${l.ticket_id}: “${quote}”`, false, 8000, open);
}

/// Badges: ? (staff, their unread replies); Tickets in the bar, on the Manage list
/// and on the phone's Manage tab (managers and admins).
function drawTicketBadges() {
  const t = state.session ? tk.info : null;
  const n = t?.unread ?? 0;
  const ans = !!state.session && isAnswerer();
  const tb = document.getElementById('tickets');
  tb.hidden = !ans;
  tb.setAttribute('aria-label', n ? `Tickets, ${n} new` : 'Tickets');
  setCount(tb, ans ? n : 0, 'tb-badge');
  tb.querySelector('.tb-badge')?.classList.toggle('urgent', !!t?.urgent?.length);
  setCount(document.getElementById('help'), ans ? 0 : n, 'tb-badge');
  for (const b of document.querySelectorAll('#nav button')) setCount(b, b.getAttribute('aria-label') === 'Manage' && ans ? n : 0, 'nav-n');
  for (const el of document.querySelectorAll('.manage [data-item="p:tickets"]')) {
    setCount(el, ans ? n : 0, 'mnav-n');
    // The count goes before the rail's short name.
    const c = el.querySelector(':scope > .mnav-n');
    if (c) el.querySelector('.rail-label')?.before(c);
  }
  for (const el of document.querySelectorAll('[data-tickets]')) setCount(el, t?.unread_mine ?? n, 'mnav-n');
  if (!t || !ans) document.getElementById('ubar').replaceChildren();
  else drawUrgent();
}

/// Urgent tickets (managers and admins): a red bar under the top bar on every screen
/// but that ticket's own, and one short tone per ticket on this device.
const UB_HIDDEN = 'pos.urgentHidden', TONE_DONE = 'pos.ticketToned';
function drawUrgent() {
  const bar = document.getElementById('ubar');
  const t = tk.info;
  if (!state.session || !isAnswerer() || !t?.urgent?.length || document.body.classList.contains('locked')) return bar.replaceChildren();
  const hidden = new Set(ssGet(UB_HIDDEN));
  const list = t.urgent.filter((u) => !hidden.has(u.id) && tk.viewing !== u.id);
  // urgent holds at most 3; urgent_count is all of them (less those hidden here).
  const count = Math.max(list.length, (t.urgent_count ?? t.urgent.length) - (t.urgent.length - list.length));
  // The tone: once per urgent ticket per device, never on the Kitchen screen.
  const toned = lsList(TONE_DONE);
  const fresh = t.urgent.filter((u) => !toned.includes(u.id));
  if (fresh.length) {
    try { localStorage.setItem(TONE_DONE, JSON.stringify([...toned, ...fresh.map((u) => u.id)].slice(-100))); } catch {}
    if (toneOn() && state.view !== 'kitchen') urgentTone();
  }
  // The Tickets page lists them already (urgent first): no banner over it.
  if (!list.length || (state.view === 'tickets' && !state.orderId)) return bar.replaceChildren();
  const u = list[0];
  const hide = () => { ssSet(UB_HIDDEN, [...hidden, ...list.map((x) => x.id)]); bar.replaceChildren(); };
  bar.replaceChildren(h('div', { class: 'urgent-bar', role: 'alert' }, icon('alert', 22),
    count > 1
      ? h('span', { class: 'ub-text' }, h('strong', {}, 'Urgent'), h('span', {}, `${count} tickets`))
      : h('span', { class: 'ub-text' }, h('strong', {}, 'Urgent'), h('span', {}, `${tcatName(u.category)}: ${u.preview}`),
        h('small', {}, [u.by, u.device, shortAge(u.created_at)].filter(Boolean).join(' · '))),
    h('button', { class: 'open', onclick: () => (count > 1 ? (state.ticketSel = null, state.ticketTab = 'open', go('tickets')) : openTicket(u.id)) }, 'Open'),
    h('button', { class: 'x', 'aria-label': 'Hide on this device', onclick: hide }, icon('x', 20))));
}
/// Three falling notes (~0.6 s), unlike the kitchen screen's two rising beeps.
function urgentTone() {
  const ctx = kdsAudioCtx();
  if (!ctx) return;
  if (ctx.state !== 'running') ctx.resume?.().catch(() => {});
  const t = ctx.currentTime + 0.02;
  [[0, 1318], [0.2, 1047], [0.4, 784]].forEach(([d, f]) => {
    const o = ctx.createOscillator(), g = ctx.createGain();
    o.type = 'triangle'; o.frequency.value = f;
    g.gain.setValueAtTime(0.0001, t + d);
    g.gain.exponentialRampToValueAtTime(0.45, t + d + 0.015);
    g.gain.exponentialRampToValueAtTime(0.0001, t + d + 0.19);
    o.connect(g); g.connect(ctx.destination);
    o.start(t + d); o.stop(t + d + 0.2);
  });
}
// Browsers allow sound only after a tap: the first tap of a manager or admin readies it.
document.addEventListener('pointerdown', () => {
  if (state.session && isAnswerer() && toneOn() && kdsAudio.ctx?.state !== 'running') kdsAudioCtx()?.resume?.().catch(() => {});
}, true);

/// Open one ticket: managers and admins on the Tickets page, staff in My tickets.
function openTicket(id) {
  if (!isAnswerer()) return myTicketsSheet(id);
  state.ticketSel = id;
  if (state.view === 'tickets' && !state.orderId) return renderShell();
  go('tickets');
}

/// ? Help: Contact admin and My tickets, above "Got it".
function helpTickets() {
  if (!state.session) return null;
  const t = tk.info;
  const row = (ic, label, hint, n, onclick) => {
    const r = srow({ label, hint, cls: 'nav', ctl: [countEl(n), h('span', { class: 'chev' }, icon('chev', 20))] });
    r.querySelector('.srow-label').prepend(icon(ic, 20));
    r.tabIndex = 0;
    r.setAttribute('role', 'button');
    r.onclick = () => { closeModal(); onclick(); };
    r.onkeydown = (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); r.click(); } };
    return r;
  };
  const mine = row('receipt', 'My tickets', 'None yet', t?.unread_mine ?? t?.unread ?? 0, () => myTicketsSheet());
  // The latest ticket under My tickets ("#14 Printer · In progress").
  invoke('list_tickets', { tab: 'mine', limit: 1 }).then((rows) => {
    const r = rows?.[0];
    if (r) setHint(mine, `#${r.id} ${tcatName(r.category)} · ${TSTATUS[r.status]?.[0] ?? r.status}`);
  }).catch(() => {});
  return h('div', { class: 'help-tickets sgroup' },
    row('chat', 'Contact admin', 'Printer, card machine, prices, anything', 0, () => newTicketSheet()), mine);
}

/// Contact admin: what about, what happened, how soon. Stays open after Send to say it went.
async function newTicketSheet(preset = {}) {
  let cat = preset.category ?? null, urgency = 'normal';
  const msgIn = h('textarea', { class: 'tmsg-in', rows: 3, maxlength: 500, placeholder: 'A few words', 'aria-label': 'Message' });
  msgIn.value = preset.body ?? '';
  msgIn.addEventListener('pointerup', () => tapFocus(msgIn));
  const quick = h('div', { class: 'qtext' });
  const addText = (q) => {
    const v = msgIn.value.trim().replace(/[.\s]+$/, '');
    msgIn.value = (v ? `${v}. ${q}` : q).slice(0, 500);
    msgIn.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const drawQuick = () => setKids(quick, (TQUICK[cat] ?? []).map((q) => h('button', { type: 'button', onclick: () => addText(q) }, q)));
  const cats = h('div', { class: 'tcats', role: 'radiogroup', 'aria-label': 'What is it about?' });
  const drawCats = () => setKids(cats, Object.entries(TCAT).map(([k, [ic, l]]) => h('button', { type: 'button', class: `tchip${k === cat ? ' on' : ''}`,
    role: 'radio', 'aria-checked': String(k === cat), 'aria-pressed': String(k === cat), onclick: () => { cat = k; drawCats(); drawQuick(); drawDup(); } }, icon(ic, 20), l)));
  // The same person, the same kind of problem, a few minutes ago, still open: add to that ticket instead.
  let recent = [], addTo = null;
  const dupBox = h('div', { class: 'tdup', hidden: true });
  const drawDup = () => {
    const now = Date.now() / 1000;
    const d = recent.find((t) => t.category === cat && t.status !== 'done' && now - t.created_at < 5 * 60);
    if (!d) { addTo = null; dupBox.hidden = true; return paintSend(); }
    if (addTo && addTo !== d.id) addTo = null;
    dupBox.hidden = false;
    setKids(dupBox, h('span', { class: 'grow' }, `You sent #${d.id} ${tcatName(d.category)} ${shortAge(d.created_at) === 'now' ? 'just now' : `${shortAge(d.created_at)} ago`}.`),
      sseg([['new', 'New ticket'], ['add', `Add to #${d.id}`]], addTo ? 'add' : 'new', (v) => { addTo = v === 'add' ? d.id : null; paintSend(); }, 'Same problem?'));
    paintSend();
  };
  invoke('list_tickets', { tab: 'mine', limit: 20 }).then((rows) => { recent = (rows ?? []).filter((t) => t.mine ?? t.created_by === state.session.user_id); drawDup(); }).catch(() => {});
  drawCats();
  drawQuick();
  const seg = sseg([['normal', 'Normal'], ['urgent', 'Urgent (now)']], urgency, (v) => { urgency = v; paintUrgent(); }, 'How soon?');
  const paintUrgent = () => seg.querySelectorAll('button').forEach((b, i) => b.classList.toggle('urgent', i === 1 && urgency === 'urgent'));
  const origSet = seg.set;
  seg.set = (v) => { origSet(v); paintUrgent(); };
  // What goes with it: who, this device, the order on screen, print failures, version.
  const onOrder = state.orderId;
  let orderText = onOrder ? `Order #${onOrder}` : null;
  const screenLabel = onOrder ? `Order #${onOrder}` : VIEWS.find(([v]) => v === state.view)?.[1] ?? state.view;
  const fails = tk.fails.filter((f) => Date.now() - f.at < 30 * 60000).length;
  const parts = () => [state.session.name, deviceLabel(), orderText, fails && `${fails} print failure${fails > 1 ? 's' : ''}`].filter(Boolean);
  const attachText = h('span', {});
  const drawAttach = () => setKids(attachText, 'Sent with: ', h('b', {}, parts().join(' · ')), state.version ? ` · v${state.version}` : '');
  drawAttach();
  const attach = popButton(h('button', { type: 'button', class: 'tattach', 'aria-expanded': 'false', 'aria-label': 'What is sent with it' }, icon('paperclip', 16), attachText),
    () => h('ul', { class: 'help-steps' }, [...parts(), screenLabel && `Screen: ${screenLabel}`, state.version && `Version ${state.version}`].filter(Boolean).map((x) => h('li', {}, x))),
    'Sent with it');
  if (onOrder) invoke('get_order', { id: onOrder }).then((o) => { orderText = `Order #${o.id} · ${orderTitle(o)}`; drawAttach(); }).catch(() => {});
  const s = sheet({ title: 'Contact admin', saveLabel: 'Send', left: attach, body: [
    h('section', { class: 'ssec' }, h('h3', { class: 'ssec-title' }, 'What is it about?'), cats, dupBox),
    h('section', { class: 'ssec' }, h('h3', { class: 'ssec-title' }, 'What happened?'), msgIn, quick),
    h('section', { class: 'ssec' }, h('h3', { class: 'ssec-title' }, 'How soon?'), h('div', { class: 'tsend-row' }, seg)),
  ], onSave: async () => {
    if (!cat) return "Pick what it's about";
    const body = msgIn.value.trim();
    if (!body) return 'Add a few words';
    if (addTo) {
      await call('reply_ticket', { id: addTo, body }, { quiet: true });
      s.clean();
      setKids(s.body, h('div', { class: 'tsent' }, h('span', { class: 'eic' }, icon('check', 30)), h('strong', {}, `Added to #${addTo}`),
        h('p', {}, 'You will see the answer under ? Help → My tickets.')));
      setKids(s.foot, h('span', { class: 'grow' }), h('button', { class: 'primary', onclick: () => { s.close(); toast('Sent'); } }, 'Done'));
      pollTickets();
      return false;
    }
    const r = await call('create_ticket', { category: cat, urgency, body, orderId: onOrder ?? null,
      screen: onOrder ? 'order' : state.view, screenLabel, device: deviceLabel() }, { quiet: true });
    const callText = r.answerers_online ? null : r.call_text ?? null;
    s.clean();
    setKids(s.body, h('div', { class: 'tsent' }, h('span', { class: 'eic' }, icon('check', 30)), h('strong', {}, `Sent · #${r.ticket?.id ?? r.id ?? ''}`),
      h('p', {}, 'You will see the answer under ? Help → My tickets.'),
      !r.answerers_online && h('div', { class: 'warn-box' }, "No manager or admin is logged in right now. If it can't wait, phone them.", callText?.trim() && h('b', {}, callText.trim()))));
    setKids(s.foot, h('span', { class: 'grow' }), h('button', { class: 'primary', onclick: () => { s.close(); toast('Sent'); } }, 'Done'));
    s.foot.querySelector('.primary').focus();
    pollTickets();
    return false;
  } });
  const paintSend = () => { if (!s?.save || s.save.disabled) return; setKids(s.save, icon('send', 20), addTo ? `Add to #${addTo}` : 'Send'); };
  s.save.prepend(icon('send', 20));
  // sheet() puts the label back after a refusal; keep the icon with it.
  new MutationObserver(() => { if (!s.save.querySelector('.icon, .spin')) s.save.prepend(icon('send', 20)); }).observe(s.save, { childList: true });
}

/// One ticket in a list (Tickets page, My tickets).
function ticketRow(t, { on = false, tab = null, mine = false, onclick } = {}) {
  // Staff see New / In progress / Done; Waiting (an answer is waiting for them) is the answerers' word.
  const st = mine || t.status === 'done' ? t.status : t.waiting || tab === 'waiting' ? 'waiting' : t.status;
  const late = tab === 'open' && t.status !== 'done' && Date.now() / 1000 - (t.last_msg_at ?? t.created_at) > 15 * 60;
  const who = mine
    ? (t.last_msg_by_name && t.last_msg_by_name !== state.session.name ? `${t.status === 'done' ? 'Done' : 'Replied'} by ${t.last_msg_by_name} · ${hm(t.last_msg_at)}` : `Sent ${hm(t.created_at)}`)
    : [t.created_by_name, t.device].filter(Boolean).join(' · ');
  const row = h('div', { class: `orow trow${t.unread ? ' unread' : ''}${t.urgency === 'urgent' ? ' is-urgent' : ''}${on ? ' on' : ''}`, role: 'listitem', tabindex: 0, 'data-id': t.id,
    onclick, onkeydown: (e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === row) { e.preventDefault(); onclick(); } } },
  h('span', { class: 'tcat-ic' }, icon(tcatIcon(t.category), 20)),
  h('span', { class: 'o-main' },
    h('span', { class: 'o-l1' }, h('strong', {}, `#${t.id} ${tcatName(t.category)}`), t.urgency === 'urgent' && h('span', { class: 'pill urgent' }, 'Urgent')),
    h('span', { class: 'tprev' }, t.preview ?? ''),
    h('span', { class: 'o-l2' }, tstatusPill(st), h('span', { class: 'o-where' }, who))),
  h('span', { class: 'tright' }, h('span', { class: `tage${late ? ' late' : ''}` }, shortAge(t.last_msg_at ?? t.created_at)),
    t.unread ? h('span', { class: 'udot', role: 'img', 'aria-label': 'Unread' }) : null));
  return row;
}

/// The conversation: messages (mine on the right) and status changes as centred lines.
function ticketThread(t) {
  const me = state.session.user_id;
  const msgs = t.messages ?? [];
  const out = msgs.map((m) => (m.kind === 'status'
    ? h('div', { class: 'tsys' }, `${m.user_name}: ${TSTATUS[m.body]?.[0] ?? m.body} · ${hm(m.created_at)}`)
    : h('div', { class: `tmsg${(m.mine ?? m.user_id === me) ? ' me' : ''}` }, h('div', { class: 'tby' }, (m.mine ?? m.user_id === me) ? null : m.user_name, h('time', {}, hm(m.created_at))), h('p', {}, m.body))));
  // Nobody has answered for 10 minutes (no internet push: the admin may be out).
  if (t.status === 'open' && msgs.filter((m) => m.kind !== 'status').length === 1 && Date.now() / 1000 - t.created_at > 600 && (t.mine ?? t.created_by === me))
    out.splice(1, 0, h('div', { class: 'tsys' }, 'No answer yet'));
  return h('div', { class: 'thread' }, out);
}

/// Reply box: quick replies (each may set a status), a one-line box (OSK on the till) and Send.
function ticketComposer(t, quick, onSent) {
  const box = h('textarea', { rows: 1, maxlength: 500, placeholder: 'Reply…', 'aria-label': 'Reply' });
  box.addEventListener('pointerup', () => tapFocus(box));
  // The keyboard (on-screen or the phone's) takes the bottom: keep the last message in view above the box.
  const showLast = () => {
    const last = (box.closest('.tdetail') ?? box.closest('.sheet'))?.querySelector('.thread')?.lastElementChild;
    if (!last) return;
    last.style.scrollMarginBottom = `${(box.closest('.sheet-foot') ? 0 : box.closest('.composer')?.offsetHeight ?? 0) + 12}px`;
    last.scrollIntoView({ block: 'end' });
  };
  box.addEventListener('focus', () => { setTimeout(showLast, 120); setTimeout(showLast, 450); });
  const send = h('button', { class: 'primary', 'aria-label': 'Send' }, icon('send', 20), 'Send');
  const go = async (body, setStatus = null) => {
    body = body.trim();
    if (!body) { tapFocus(box); return; }
    send.disabled = true;
    try {
      await call('reply_ticket', { id: t.id, body, setStatus }, { quiet: true });
      box.value = '';
      await onSent();
    } catch (e) { toast(String(e?.message ?? e), true); } finally { send.disabled = false; }
  };
  send.onclick = () => go(box.value);
  box.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); go(box.value); } });
  return h('div', { class: 'composer' },
    h('div', { class: 'qtext' }, quick.map(([label, st]) => h('button', { type: 'button', onclick: () => go(label, typeof st === 'function' ? st() : st) }, label))),
    h('div', { class: 'composer-row' }, box, send));
}

/// Staff (and anyone, for their own tickets): My tickets in a sheet; a row opens its thread there.
async function myTicketsSheet(openId = null) {
  const s = sheet({ title: 'My tickets', wide: true, body: [] });
  const head = s.el.querySelector('.sheet-head');
  const title = head.querySelector('h3');
  const back = h('button', { class: 'ghost icon-btn', 'aria-label': 'Back to My tickets', onclick: () => list() }, icon('back'));
  const stillOpen = () => document.getElementById('modal').contains(s.el);
  const closed = () => { tk.viewing = null; tk.watch = null; };
  const done = state.onModalClose;
  state.onModalClose = () => { closed(); done?.(); };
  async function list() {
    tk.viewing = null;
    back.remove();
    title.textContent = 'My tickets';
    setKids(s.foot, h('span', { class: 'grow' }), h('button', { class: 'ghost', onclick: () => s.close() }, 'Close'),
      h('button', { class: 'primary', onclick: () => newTicketSheet() }, icon('plus', 20), 'New ticket'));
    let rows;
    try { rows = await call('list_tickets', { tab: 'mine', limit: 100 }, { quiet: true }); } catch (e) {
      return setKids(s.body, emptyState({ icon: 'alert', title: 'Could not load', text: String(e?.message ?? e) }));
    }
    if (!stillOpen() || title.textContent !== 'My tickets') return;
    tk.watch = list;
    setKids(s.body, rows.length
      ? h('div', { class: 'md-list', role: 'list', style: 'margin-top:10px' }, rows.map((t) => ticketRow(t, { mine: true, onclick: () => thread(t.id) })))
      : emptyState({ icon: 'chat', title: 'No tickets yet', text: 'Tap New ticket when something needs an admin.' }),
    h('div', { class: 'ssec-foot' }, 'Done tickets stay here for 30 days.'));
  }
  async function thread(id) {
    let t;
    try { t = await call('get_ticket', { id }, { quiet: true }); } catch (e) { toast(String(e?.message ?? e), true); return list(); }
    if (!stillOpen()) return;
    tk.viewing = id;
    tk.watch = () => thread(id);
    if (!back.isConnected) head.prepend(back);
    title.textContent = `#${t.id} ${tcatName(t.category)}`;
    const scroller = s.body;
    setKids(s.body, h('div', { class: 'tdetail', style: 'padding-top:10px' },
      h('div', { class: 'row' }, tstatusPill(t.status), h('span', { class: 'md-span' }, [`Sent ${hm(t.created_at)}`, t.urgency === 'urgent' && 'Urgent'].filter(Boolean).join(' · '))),
      ticketThread(t)));
    const reopenable = t.status !== 'done' || (t.can_reopen ?? (t.closed_at && Date.now() / 1000 - t.closed_at < 7 * 86400));
    const comp = ticketComposer(t, [['Fixed now, thanks', 'done'], ['Still not working', () => (t.status === 'done' ? 'open' : null)], ['Done it', null]],
      () => thread(id));
    comp.style.cssText = 'width:100%;margin:0;padding:0;background:transparent;position:static';
    setKids(s.foot, reopenable ? comp : h('div', { class: 'muted grow' }, 'Done over 7 days ago. Open a new ticket if it happens again.'));
    scroller.scrollTop = scroller.scrollHeight;
    pollTickets();
  }
  if (openId) await thread(openId); else await list();
}

/// Manage → Tickets (managers and admins): Open / Waiting / Done beside the open ticket.
async function renderTickets() {
  const tab = state.ticketTab ?? 'open';
  const listEl = h('div', { class: 'md-list', role: 'list', 'aria-label': 'Tickets' });
  const detail = h('div', { class: 'md-body' });
  const counts = { open: tk.info?.open ?? null, waiting: tk.info?.waiting ?? null };
  const label = (k, l) => (counts[k] ? `${l} · ${counts[k]}` : l);
  const seg = sseg([['open', 'Open'], ['waiting', 'Waiting'], ['done', 'Done']], tab, (v) => { state.ticketTab = v; load(); }, 'Show');
  const drawSeg = () => seg.setItems([['open', label('open', 'Open')], ['waiting', label('waiting', 'Waiting')], ['done', 'Done']]);
  const empty = () => setKids(detail, h('div', { class: 'sgroup md-empty' }, emptyState({ icon: 'chat', title: 'Tap a ticket to read it' })));
  const md = masterDetail({ cls: 'md-tickets', back: 'Tickets', list: [h('div', { class: 'md-tools' }, seg), listEl], detail });
  const soundItem = () => h('button', { 'data-keep': '1', onclick: (e) => {
    setPref(TONE_KEY, toneOn() ? '0' : '1');
    e.currentTarget.textContent = soundText();
  } }, soundText());
  const soundText = () => `Sound for urgent tickets: ${toneOn() ? 'On' : 'Off'} (this device)`;
  screen({ action: moreMenu('Tickets', () => [soundItem()]) }, md);
  empty();
  let loads = 0;
  async function load() {
    const mine = ++loads;
    const cur = state.ticketTab ?? 'open';
    const rows = await call('list_tickets', { tab: cur, limit: 100 });
    if (mine !== loads || !listEl.isConnected) return;
    // Tab counts: app_status (the poll); the shown tab's own list is the freshest.
    counts.open = cur === 'open' ? rows.length : tk.info?.open ?? counts.open;
    counts.waiting = cur === 'waiting' ? rows.length : tk.info?.waiting ?? counts.waiting;
    drawSeg();
    setKids(listEl, rows.length ? rows.map((t) => ticketRow(t, { tab: cur, on: t.id === state.ticketSel, onclick: () => show(t.id) }))
      : emptyState({ icon: 'chat', ...{ open: { title: 'Nothing to answer', text: 'New tickets from staff show up here.' },
        waiting: { title: 'Nothing waiting' }, done: { title: 'No done tickets in the last 30 days' } }[cur] }));
  }
  async function show(id, quietly = false) {
    let t;
    try { t = await call('get_ticket', { id }, { quiet: quietly }); } catch {
      if (state.ticketSel === id) { state.ticketSel = null; tk.viewing = null; empty(); md.close(true); }
      return;
    }
    if (!detail.isConnected) return;
    state.ticketSel = id;
    tk.viewing = id;
    for (const r of listEl.querySelectorAll('.orow')) {
      r.classList.toggle('on', Number(r.dataset.id) === id);
      if (Number(r.dataset.id) === id) { r.classList.remove('unread'); r.querySelector('.udot')?.remove(); }
    }
    const keepScroll = quietly ? detail.closest('.md-main')?.scrollTop : null;
    const typed = detail.querySelector('.composer textarea')?.value;
    setKids(detail, ticketDetail(t, { refresh: async () => { await show(id, true); await load(); pollTickets(); }, deleted: async () => {
      state.ticketSel = null; tk.viewing = null; empty(); md.close(true); await load(); pollTickets();
    } }));
    if (typed) detail.querySelector('.composer textarea').value = typed;
    const main = detail.closest('.md-main');
    if (main) main.scrollTop = keepScroll ?? 0;
    md.show();
    drawUrgent();
  }
  tk.watch = async () => { await load(); if (state.ticketSel && !document.activeElement?.closest?.('.composer')) await show(state.ticketSel, true); };
  // Leaving the page: the ticket is no longer on screen.
  const stop = setInterval(() => { if (!listEl.isConnected) { clearInterval(stop); tk.watch = null; tk.viewing = null; drawUrgent(); } }, 1000);
  await load();
  if (state.ticketSel) await show(state.ticketSel);
}

/// One ticket for managers and admins: status, the thread, what was sent with it, the reply box.
function ticketDetail(t, { refresh, deleted }) {
  const ctx = t.context ?? {};
  const head = h('div', { class: 'thead' });
  const statusSeg = h('span', { class: 'sseg', role: 'radiogroup', 'aria-label': 'Status' }, [['open', 'New'], ['in_progress', 'In progress'], ['done', 'Done']].map(([v, l]) =>
    h('button', { type: 'button', role: 'radio', 'data-v': v, 'aria-checked': String(t.status === v), class: t.status === v ? 'on' : '', onclick: async (e) => {
      if (t.status === v) return;
      for (const b of statusSeg.children) { b.classList.toggle('on', b === e.currentTarget); b.setAttribute('aria-checked', String(b === e.currentTarget)); }
      rowState(head, 'saving');
      try { await call('set_ticket_status', { id: t.id, status: v }, { quiet: true }); rowState(head, 'saved'); await refresh(); } catch (err) {
        rowState(head, 'idle');
        for (const b of statusSeg.children) b.classList.toggle('on', b.dataset.v === t.status);
        nudge(statusSeg);
        toast(String(err?.message ?? err), true);
      }
    } }, l)));
  const contextText = () => [`Ticket #${t.id} ${tcatName(t.category)}${t.urgency === 'urgent' ? ' (urgent)' : ''}`, `${t.created_by_name} · ${t.device} · ${friendlyTime(t.created_at)}`,
    t.order_id && `Order #${t.order_id}${t.table_name ? ` · ${tableTitle(t.table_name)}` : ''}`, ctx.screen_label && `Screen: ${ctx.screen_label}`, ctx.version && `Version ${ctx.version}`,
    ...(ctx.print_failures ?? []).map((f) => `Print failure${f.at ? ` ${hm(f.at)}` : ''}: ${f.message ?? f}`)].filter(Boolean).join('\n');
  const delSheet = () => sheet({ title: `Delete ticket #${t.id} for good?`, danger: true, saveLabel: 'Delete', body: [h('p', { class: 'snote' }, 'Its messages go too.')],
    onSave: async () => { await call('delete_ticket', { id: t.id }, { quiet: true }); toast(`Ticket #${t.id} deleted`); await deleted(); } });
  setKids(head, h('h3', {}, `#${t.id} ${tcatName(t.category)} `, t.urgency === 'urgent' && h('span', { class: 'pill urgent', style: 'vertical-align:3px' }, 'Urgent')),
    h('span', { class: 'sstate', role: 'status' }), statusSeg,
    moreMenu(`ticket #${t.id}`, () => [
      h('button', { onclick: () => navigator.clipboard?.writeText(contextText()).then(() => toast('Copied'), () => toast('Could not copy', true)) }, t.order_id ? 'Copy order # and context' : 'Copy context'),
      h('button', { onclick: () => openLog(t.order_id ? { q: `#${t.order_id}`, at: t.created_at } : { at: t.created_at }) }, 'Related activity'),
      isAdmin() && h('button', { class: 'danger', onclick: delSheet }, 'Delete ticket…'),
    ]));
  const fails = ctx.print_failures ?? [];
  const terminal = ctx.terminal_label ?? (t.terminal === 'local' ? 'Main till' : 'Browser');
  const crow = (label, value, { hint = null, btn = null, cls = '' } = {}) => srow({ label, hint, cls, ctl: btn ?? h('span', { class: 'srow-value' }, value) });
  const ost = t.order_status !== undefined ? t.order_status : ctx.order_status;
  const orderRow = t.order_id && crow('Order', null, { hint: [`#${t.order_id}`, tableTitle(t.table_name), t.order_id && (ost ? STATUS_LABEL[ost] ?? ost : ost === null ? 'Deleted' : null)].filter(Boolean).join(' · '),
    btn: h('button', { onclick: act(() => openAnyOrder(t.order_id)) }, 'Open order') });
  const actRow = t.order_id && crow(`Activity on #${t.order_id}`, '…', { cls: 'nav' });
  if (actRow) {
    actRow.hidden = true;
    actRow.onclick = () => openLog({ q: `#${t.order_id}`, at: t.created_at });
    invoke('order_activity', { orderId: t.order_id }).then((rows) => {
      const n = rows.filter((r) => !r.action.startsWith('ticket.')).length;
      if (!n) return;
      actRow.hidden = false;
      setKids(actRow.querySelector('.srow-ctl'), h('span', { class: 'srow-value' }, `${n} ${n === 1 ? 'entry' : 'entries'}`), h('span', { class: 'chev' }, icon('chev', 20)));
    }).catch(() => {});
  }
  const failN = ctx.print_failure_count ?? fails.length;
  const failText = fails.length ? `${fails[0].message ?? fails[0]}${failN > 1 ? ` · ${failN}× since ${hm(fails.at(-1).at ?? t.created_at)}` : ''}` : null;
  return h('div', { class: 'tdetail' }, head,
    h('div', { class: 'md-span' }, `${t.created_by_name} · ${t.device || terminal} · ${hm(t.created_at)} (${shortAge(t.created_at)}${shortAge(t.created_at) === 'now' ? '' : ' ago'})`),
    ticketThread(t),
    h('section', { class: 'ssec', style: 'margin-top:0' }, h('h3', { class: 'ssec-title' }, icon('paperclip', 16), ' Sent with it'),
      h('div', { class: 'sgroup tctx' }, h('div', { class: 'tctx-grid' },
        crow('Who', t.created_by_name),
        crow('Device', `${t.device || '—'} (${terminal})`),
        orderRow,
        ctx.screen_label && crow('Screen', ctx.screen_label),
        fails.length > 0 && crow('Print failures', null, { cls: 'bad', hint: failText,
          btn: isAdmin() ? h('button', { onclick: () => openManage('g:printers') }, 'Printers') : h('span', { class: 'srow-value' }, String(failN)) }),
        ctx.version && crow('Version', ctx.version),
        actRow))),
    ticketComposer(t, [['On my way', null], ['Try again now', null], ['Fixed, check it', 'done'], ['After service', null]], refresh));
}

/// An order from a ticket or the log: the order screen while open, else History.
async function openAnyOrder(id) {
  let o;
  try { o = await invoke('get_order', { id }); } catch { return toast('This order was deleted', true); }
  if (o.status === 'open') return openOrder(id);
  state.histOpen = id;
  if (state.view === 'history' && !state.orderId) return renderShell();
  go('history');
}

// ---------- Activity log ----------

/// Type filter (server-side groups, see list_activity `kind`).
const ACT_KINDS = [['void', 'Cancels & voids'], ['money', 'Money'], ['login', 'Logins & PINs'], ['setup', 'Settings & menu'], ['shift', 'Shifts & day close'], ['other', 'Other']];
const LOG_KEEP = [[30, '30 days'], [90, '90 days'], [180, '180 days'], [365, '1 year'], [0, 'Forever']];
const LOG_KEEP_DEFAULT = 180;
const PIN_RE = /\s*\((?:PIN of (.+?) #\d+|no PIN), \w+\)\s*$/;
const ROLE_RE = /\((?:PIN of .+? #\d+|no PIN), (\w+)\)\s*$/;

/// Open the Activity log with a search and/or the day of `at`.
function openLog({ q = '', at = null } = {}) {
  state.logFilter = { mode: at ? 'day' : 'today', day: at ? isoDate(new Date(at * 1000)) : today(), userId: null, kind: null, look: false, q };
  if (state.view === 'log' && !state.orderId) return renderShell();
  go('log');
}

/// One log entry as plain words: { ic, g (icon colour), text, sub, opens, look }.
function actText(r) {
  const d = r.detail ?? '';
  const a = r.action;
  const plain = d.replace(PIN_RE, '');
  const who = r.user_name ?? (r.user_id != null ? 'Former staff' : /^(login\.failed|pin\.failed)$/.test(a) ? 'Someone' : 'System');
  const o = r.order_id;
  const title = r.order ? orderTitle(r.order) : r.order_title;
  const at = title ? ` · ${title}` : '';
  const term = r.device || termLabel(r.terminal);
  const c = (s) => money(Number(s) || 0);
  const ref = r.ref_name;
  const order = o ? { order: o } : null;
  let m;
  const x = (ic, g, text, sub = who, opens = order) => ({ ic, g, text, sub, opens });
  switch (a) {
    case 'order.cancel': {
      if (/^\d+ empty$/.test(d)) return x('orders', 'other', r.user_id == null ? `Closed automatically · #${o} (empty)` : `Closed empty order #${o}`);
      if (/empty order\(s\) at day close/.test(d)) return x('orders', 'other', `Closed ${d}`, who, null);
      // The automatic sweep: "5 empty order(s) untouched for over 30 minutes" (or "left open over…").
      if ((m = /^(\d+) empty order\(s\) (?:untouched|left open) for over (\d+) minutes?/.exec(d))) {
        const n = Number(m[1]);
        return x('orders', 'other', `Closed ${n} empty order${n === 1 ? '' : 's'} (left open over ${m[2]} min)`, r.user_id == null ? 'automatic' : who, null);
      }
      const reason = plain.replace(/^\d+\s*/, '');
      return x('ban', 'void', `Cancelled order #${o}${at}`, `${who}${reason ? ` · reason: ${reason}` : ''}`);
    }
    case 'order.void': return x('ban', 'void', `Voided paid order #${o}${r.order?.total_cents != null ? ` · ${money(r.order.total_cents)}` : ''}${at}`, `${who}${plain.replace(/^\d+\s*/, '') ? ` · ${plain.replace(/^\d+\s*/, '')}` : ''}`);
    case 'order.delete': m = /^#(\d+) (\w+) total (-?\d+)\s*(.*)$/.exec(d);
      return x('trash', 'void', m ? `Deleted order #${m[1]} (${m[2]}, ${c(m[3])})` : `Deleted order ${d}`, `${who}${m?.[4] ? ` · ${m[4]}` : ''}`, null);
    case 'order.item_void': m = /^order \d+ (.+) x(\d+)$/.exec(plain);
      return x('ban', 'void', m ? `Removed ${m[2]}× ${m[1]} (sent) · #${o}` : `Removed a sent dish · #${o}`, `${who}${at}`);
    case 'order.payment_removed': m = /payment \d+ (\w+) (-?\d+) tip (-?\d+) taken by (.+)$/.exec(plain);
      return x('coin', 'money', m ? `Took back a ${m[1]} payment of ${c(m[2])} · #${o}` : `Took back a payment · #${o}`, `${who}${m ? ` · taken by ${m[4]}` : ''}`);
    case 'order.payment_other': m = /^order \d+ (-?\d+)/.exec(plain); return x('coin', 'money', `Other payment ${m ? c(m[1]) : ''} · #${o}`);
    case 'order.part_payment': m = /^order \d+ (\w+) (-?\d+)/.exec(d); return x('coin', 'money', m ? `Part payment ${c(m[2])} (${m[1]}) · #${o}` : `Part payment · #${o}`);
    case 'order.closed_nothing_to_pay': return x('check', 'money', `Closed with nothing to pay · #${o}${at}`);
    case 'order.discount': m = /^order \d+ (.+) (-?\d+)$/.exec(d); return x('tag', 'money', m ? `Discount ${c(m[2])} on ${m[1]} · #${o}` : `Discount · #${o}`, `${who}${at}`);
    case 'order.discount_approved': m = /^order \d+ (.+) (-?\d+) for (.+)$/.exec(d);
      return x('tag', 'money', m ? `Discount ${c(m[2])} on ${m[1]} · #${o}` : `Discount · #${o}`);
    case 'order.price': m = /^order \d+ (.+) (-?\d+) -> (-?\d+) by (.+)$/.exec(d);
      return x('tag', 'money', m ? `Price of ${m[1]} ${c(m[2])} → ${c(m[3])} · #${o}` : `Changed a price · #${o}`);
    case 'order.price_qty_approved': m = /^order \d+ (.+) x(\d+) -> x(\d+) at (-?\d+) for (.+)$/.exec(d);
      return x('tag', 'money', m ? `${m[1]} ×${m[2]} → ×${m[3]} at a changed price ${c(m[4])} · #${o}` : `More at a changed price · #${o}`);
    case 'order.open_item': m = /^order \d+ (.+) (-?\d+)$/.exec(plain); return x('plus', 'money', m ? `Open item ${m[1]} ${c(m[2])} · #${o}` : `Open item · #${o}`);
    case 'order.service': m = /^\d+ (-?\d+) \(was (-?\d+)\)/.exec(plain);
      return x('receipt', 'money', m ? `Service charge ${pct(+m[1])}% (was ${pct(+m[2])}%) · #${o}` : `Service charge · #${o}`);
    case 'order.tax': {
      m = /^\d+ (Some\((\d+)\)|None) \(was (Some\((\d+)\)|None)\)/.exec(plain);
      const rate = (s, n) => (s === 'None' ? 'menu rates' : `${pct(+n)}%`);
      return x('receipt', 'money', m ? `Tax for the whole order ${rate(m[1], m[2])} (was ${rate(m[3], m[4])}) · #${o}` : `Tax for the whole order · #${o}`);
    }
    case 'drawer.open': return d === 'no sale' ? { ...x('drawer', 'money', 'Opened the drawer · no sale', [who, term].filter(Boolean).join(' · '), null), look: true }
      : x('drawer', 'money', `Drawer opened for cash · #${o}`);
    case 'order.move': return x('move', 'other', `Moved #${o} to ${ref ? `table ${ref}` : 'another table'}`);
    case 'order.merge': m = /-> order (\d+)/.exec(d); return x('move', 'other', `Joined #${o} onto #${m?.[1] ?? '?'}`, who, m ? { order: +m[1] } : order);
    case 'order.move_items': m = /-> order (\d+): (\d+) dish/.exec(d); return x('move', 'other', m ? `Moved ${m[2]} ${m[2] === '1' ? 'dish' : 'dishes'} from #${o} to #${m[1]}` : `Moved dishes from #${o}`, who, m ? { order: +m[1] } : order);
    case 'kitchen.main_course': return x('chef', 'other', `Called mains · #${o}`);
    case 'login': case 'logout': return x('user', 'login', `${a === 'login' ? 'Logged in' : 'Logged out'}${r.fold > 1 ? ` ×${r.fold}` : ''}`,
      [who, term, r.fold > 1 && `first ${hm(r.foldEnd)}`].filter(Boolean).join(' · '), null);
    case 'login.failed': return x('key', 'login', r.fold > 1 ? `Wrong PIN ×${r.fold}` : 'Wrong PIN at login', ['Someone', term].filter(Boolean).join(' · '), null);
    case 'pin.failed': return x('key', 'login', r.fold > 1 ? `Wrong manager PIN ×${r.fold}` : 'Wrong manager PIN', ['Someone', term].filter(Boolean).join(' · '), null);
    case 'reset.bad_pin': return x('key', 'login', 'Wrong PIN on Reset data', who, null);
    case 'log.bad_pin': return x('key', 'login', 'Wrong PIN on Clear log', who, null);
    case 'user.reset_pin': return x('key', 'login', `Reset the PIN of ${ref ?? `#${d}`}`, who, { item: 'p:users' });
    case 'setup.admin_created': return x('user', 'login', 'Created the first admin', who, null);
    case 'shift.open': m = /float (-?\d+)/.exec(d); return x('shift', 'shift', `Started the shift${m ? ` · float ${c(m[1])}` : ''}`, who, { item: 'p:shift' });
    case 'shift.cash': m = /^shift \d+ (in|out) (-?\d+)\s*(.*)$/.exec(d);
      return x('cash', 'shift', m ? `Cash ${m[1]} ${c(m[2])}${m[3] ? ` · ${m[3]}` : ''}` : 'Cash in / out', who, { item: 'p:shift' });
    case 'shift.close': {
      m = /counted (-?\d+) expected (-?\d+)/.exec(d);
      const diff = m ? +m[1] - +m[2] : 0;
      return { ...x('shift', 'shift', m ? `Closed the shift · counted ${c(m[1])} (expected ${c(m[2])})` : 'Closed the shift', who, { item: 'p:shift' }),
        bad: diff ? ` · ${money(Math.abs(diff))} ${diff < 0 ? 'short' : 'over'}` : null };
    }
    case 'day.close': m = /^(.+?): (\d+) orders, total (-?\d+)/.exec(d);
      return x('reports', 'shift', m ? `Closed the day ${m[1]} · ${m[2]} orders · ${c(m[3])}` : `Closed the day ${d}`, who, { item: 'p:reports' });
    case 'setting.update': {
      const e = MANAGE_INDEX.find((i) => i.row === d);
      return x('settings', 'setup', `Changed ${e ? e.label : d}`, who, e && needOk(e.need) ? { item: e.item, row: e.row } : null);
    }
    case 'printer.routes': case 'printer.kitchen': case 'printer.receipt':
      return x('printer', 'setup', `Changed printer settings (${a.slice(8)})`, who, isAdmin() ? { item: 'g:printers' } : null);
    case 'service.settings': return x('settings', 'setup', 'Changed the service charge', who, isAdmin() ? { item: 'g:payments', row: 'service' } : null);
    case 'server.settings': return x('settings', 'setup', 'Changed the network server', who, isAdmin() ? { item: 'g:devices', row: 'server' } : null);
    case 'teams.save': return x('settings', 'setup', 'Changed kitchen teams', who, { item: 'g:kitchen', row: 'teams' });
    case 'channels.save': return x('settings', 'setup', 'Changed delivery apps', who, isAdmin() ? { item: 'g:delivery' } : null);
    case 'device.autostart': return x('monitor', 'setup', `Start with Windows ${d}`, who, isAdmin() ? { item: 'g:devices', row: 'autostart' } : null);
    case 'user.create': case 'user.update': {
      m = /^\d+ (.+?) (admin|manager|staff)(.*)$/.exec(d);
      const name = m?.[1] ?? ref ?? '';
      return x('users', 'setup', a === 'user.create' ? `Added staff member ${name}${m ? ` (${ROLE_LABEL[m[2]] ?? m[2]})` : ''}` : `Changed staff member ${name}${/active=false/.test(d) ? ' · turned off' : ''}`,
        who, isAdmin() ? { item: 'p:users' } : null);
    }
    case 'customer.create': case 'customer.update': case 'customer.delete':
      return x('customers', 'setup', `${{ create: 'Added', update: 'Changed', delete: 'Deleted' }[a.slice(9)]} customer ${ref ?? `#${d}`}`, who, { item: 'p:customers' });
    case 'floor.create': case 'floor.update': case 'floor.delete': case 'floor.layout': {
      const name = ref ?? d.replace(/^\d+\s*/, '');
      return x('plan', 'setup', a === 'floor.layout' ? 'Changed the floor plan' : `${{ create: 'Added', update: 'Renamed', delete: 'Deleted' }[a.slice(6)]} floor ${name}`, who, { item: 'p:plan' });
    }
    case 'backup': return x('database', 'other', 'Made a backup', who, isAdmin() ? { item: 'g:data' } : null);
    case 'backup.restore': return x('database', 'other', `Restored a backup${d ? ` ${d}` : ''}`, 'System', null);
    case 'data.reset': {
      const names = [['sales', 'sales'], ['customers', 'customers'], ['menu', 'menu'], ['floor_plan', 'floor plan'], ['staff', 'staff'], ['settings', 'settings']];
      const on = names.filter(([k]) => new RegExp(`${k}: (true|Some)`).test(d)).map(([, n]) => n);
      return x('trash', 'other', `Reset data${on.length ? `: ${on.join(', ')}` : ''}`, who, null);
    }
    case 'log.cleared': case 'log.auto_cleared': {
      let j = {};
      try { j = JSON.parse(d); } catch {}
      const day = (t) => (t ? new Date((typeof t === 'number' ? t : Date.parse(t) / 1000) * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short' }) : '?');
      const n = Number(j.count ?? 0).toLocaleString();
      if (a === 'log.auto_cleared') return { ic: 'lock', g: 'cleared', cleared: true, text: `Cleared ${n} entries older than ${day(j.cutoff)} automatically${j.tickets ? ` · ${j.tickets} done ticket${j.tickets === 1 ? '' : 's'}` : ''}`,
        sub: `System${j.days ? ` · keep for ${j.days} days` : ''} · cannot be deleted`, opens: null };
      return { ic: 'lock', g: 'cleared', cleared: true, text: j.kind === 'selected' ? `Deleted ${n} selected ${j.count === 1 ? 'entry' : 'entries'}` : `Cleared ${n} log entries · ${day(j.from)} to ${day(j.to)}`,
        sub: `${who}${j.csv ? ' · CSV saved first' : ''} · cannot be deleted`, opens: null };
    }
    default: break;
  }
  if (a.startsWith('ticket.')) {
    m = /^#(\d+)\s*(.*)$/.exec(d);
    const id = m ? +m[1] : null;
    const rest = m?.[2] ?? d;
    const text = a === 'ticket.create' ? `Ticket #${id} · ${tcatName(rest.split(' ')[0])}${/urgent/.test(rest) ? ' · Urgent' : ''}`
      : a === 'ticket.status' ? `Ticket #${id} → ${TSTATUS[rest.split('->').pop().trim()]?.[0] ?? rest}` : a === 'ticket.delete' ? `Deleted ticket #${id}` : `Ticket #${id}`;
    return x('chat', 'system', text, who, id && a !== 'ticket.delete' ? { ticket: id } : null);
  }
  if (a.startsWith('product.')) {
    m = /^(\d+) (.*)$/.exec(d);
    const name = ref ?? m?.[2] ?? d;
    const flag = { starter: 'starter', prep: 'prep ahead', dessert: 'dessert', drink: 'drink', set_only: 'set only' }[a.slice(8)];
    const text = a === 'product.sold_out' ? `Marked ${ref ?? `#${m?.[1]}`} ${/true/.test(d) ? 'sold out' : 'back on sale'}`
      : flag ? `${ref ?? `#${m?.[1]}`}: ${flag} ${/true/.test(d) ? 'on' : 'off'}`
        : `${{ create: 'Added', update: 'Changed', delete: 'Deleted', duplicate: 'Copied' }[a.slice(8)] ?? 'Changed'} menu item ${name}`;
    return x(a === 'product.sold_out' ? 'tag' : 'menuAdmin', 'setup', text, who, { item: 'p:menuAdmin' });
  }
  if (a.startsWith('category.')) {
    m = /^(\d+) (.*?)( active=.*)?$/.exec(d);
    const name = ref ?? m?.[2] ?? '';
    return x('menuAdmin', 'setup', `${{ create: 'Added', update: 'Changed', delete: 'Deleted' }[a.slice(9)] ?? 'Changed'} category ${a.slice(9) === 'create' || a.slice(9) === 'update' ? name : ref ?? ''}`.trim(), who, { item: 'p:menuAdmin' });
  }
  if (a.startsWith('options.') || a.startsWith('menu.')) {
    const text = { 'menu.variants': 'Changed sizes', 'menu.set': 'Changed set menus', 'menu.import': 'Pasted a menu', 'menu.restore': 'Restored a menu backup',
      'menu.apply_default_tax': `Applied the default tax to ${/^(\d+)/.exec(d)?.[1] ?? 'the'} items` }[a] ?? 'Changed option groups';
    return x('menuAdmin', 'setup', text, who, { item: 'p:menuAdmin' });
  }
  return { ic: 'info', g: 'other', text: a, sub: d, mono: true, opens: null };
}

/// Where a row was done: the main till or a browser (`terminal`: 'local' / 'web-xxxxxx').
const termLabel = (t) => (!t ? null : t === 'local' ? 'Main till' : t.startsWith('web') ? 'Browser' : t);

/// Fold failed PINs in a row (same device, within 2 minutes) into one "Wrong PIN ×3".
function foldPins(rows) {
  const out = [];
  // Logged in / out: one row per person, device and day ("Logged in ×5", the newest time).
  const seen = new Map();
  for (const r of rows) {
    if (/^log(in|out)$/.test(r.action) && r.user_id != null) {
      const key = `${r.action}|${r.user_id}|${r.device ?? r.terminal ?? ''}|${r.day ?? isoDate(new Date(r.at * 1000))}`;
      const first = seen.get(key);
      if (first) { first.fold = (first.fold ?? 1) + 1; first.foldEnd = r.at; first.foldIds.push(r.id); continue; }
      const row = { ...r, foldEnd: r.at, foldIds: [r.id] };
      seen.set(key, row);
      out.push(row);
      continue;
    }
    const p = out.at(-1);
    if (p && /^(login|pin)\.failed$/.test(r.action) && p.action === r.action && p.terminal === r.terminal && p.foldEnd - r.at <= 120) {
      p.fold = (p.fold ?? 1) + 1;
      p.foldEnd = r.at;
      p.foldIds.push(r.id);
      continue;
    }
    out.push({ ...r, foldEnd: r.at, foldIds: [r.id] });
  }
  return out;
}

/// One row: time, icon, the words, "PIN: Mia", › when it opens something.
function actRow(r, { compact = false, selecting = false, checked = false, onToggle = null, here = null } = {}) {
  const t = actText(r);
  if (here != null) {
    if (t.opens?.order === here) t.opens = null;
    t.text = t.text.replace(new RegExp(` · #${here}$`), '');
  }
  const cleared = t.cleared || r.cleared;
  // The server names whose PIN approved it (old rows: from the detail's "(PIN of …)").
  const approver = r.approver_name !== undefined ? (r.approver_name && r.approver_name !== r.user_name ? r.approver_name : null) : PIN_RE.exec(r.detail ?? '')?.[1] ?? null;
  const look = !cleared && (r.look || t.look);
  const opens = !selecting && t.opens;
  const open = () => {
    if (!opens) return;
    if (opens.order) return openAnyOrder(opens.order);
    if (opens.ticket) return openTicket(opens.ticket);
    if (opens.item) openManage(opens.item, opens.row ?? null);
  };
  const box = selecting && h('input', { type: 'checkbox', checked, disabled: !!cleared, 'aria-label': 'Select', tabindex: -1 });
  const sub = t.mono ? h('small', { class: 'mono' }, t.sub) : h('small', {}, h('b', {}, t.sub.split(' · ')[0]), t.sub.includes(' · ') ? ` · ${t.sub.split(' · ').slice(1).join(' · ')}` : '');
  const row = h('div', { class: `arow${opens ? ' link' : ''}${look ? ' look' : ''}${cleared ? ' cleared' : ''}${selecting ? ' sel' : ''}${selecting && cleared ? ' locked' : ''}${checked ? ' checked' : ''}`,
    role: 'listitem', tabindex: opens || (selecting && !cleared) ? 0 : null,
    onclick: () => { if (selecting) { if (!cleared) onToggle?.(); } else open(); },
    onkeydown: (e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === row) { e.preventDefault(); row.click(); } } },
  box, h('span', { class: 'at' }, hm(r.at)),
  h('span', { class: `aic g-${cleared ? 'system' : t.g}` }, icon(cleared ? 'lock' : t.ic, compact ? 16 : 20)),
  h('span', { class: 'atext' }, h('strong', {}, t.text, t.bad && h('span', { class: 'bad-text' }, t.bad)), sub),
  h('span', { class: 'aend' }, approver && h('span', { class: 'pill pin' }, `PIN: ${approver}`), opens && h('span', { class: 'chev' }, icon('chev', 18))));
  return row;
}

/// The dates of the log (and History): Today = the open business day, Yesterday =
/// the day closed last, 7 days, or one picked calendar day. { from, to, span }.
const OPEN_END = 4102444800;
async function logRange(mode, picked) {
  const daysAgo = (n) => { const d = new Date(); d.setDate(d.getDate() - n); return d; };
  const day = await call('business_day');
  logRange.today = day.date;
  logRange.yesterday = day.prev_date;
  if (mode === 'day') { const [from, to] = dayRange(picked); return { from, to, span: `${dayLabel(picked)} · midnight to midnight` }; }
  const stale = Date.now() / 1000 - day.since > 36 * 3600;
  if (stale) {
    const [from, to] = mode === 'week' ? [dayRange(isoDate(daysAgo(6)))[0], OPEN_END] : mode === 'today' ? [dayRange(today())[0], OPEN_END] : dayRange(isoDate(daysAgo(1)));
    return { from, to, span: `Day not closed since ${friendlyTime(day.since)}: calendar days` };
  }
  if (mode === 'today') return { from: day.since, to: OPEN_END, span: `Since ${hm(day.since)} (business day)` };
  if (mode === 'yesterday') return { from: day.prev_since, to: day.since, span: `${day.prev_date ? dayLabel(day.prev_date) : 'Previous day'} · ${friendlyTime(day.prev_since)} to ${friendlyTime(day.since)}` };
  return { from: day.week_since, to: OPEN_END, span: `Since ${friendlyTime(day.week_since)}` };
}

/// Manage → Activity log (managers and admins). Admins can also delete and clear.
async function renderActivity() {
  const f = state.logFilter ??= { mode: 'today', day: today(), userId: null, kind: null, look: false, q: '' };
  let rows = [], next = null, total = null, lookCount = 0, selecting = false;
  const sel = new Set();
  let staff = null;
  const listEl = h('div', { class: 'alog', role: 'list', 'aria-label': 'Activity' });
  const span = h('div', { class: 'alog-span' });
  const seg = sseg([['today', 'Today'], ['yesterday', 'Yesterday'], ['week', '7 days']], f.mode === 'day' ? null : f.mode, (v) => { f.mode = v; load(); }, 'Dates');
  const dayBtn = h('button', { class: 'daybtn', 'aria-label': 'Pick a day', onclick: () => {
    const date = h('input', { type: 'date', value: f.day, max: today(), 'aria-label': 'Day' });
    const s = sheet({ title: 'Show one day', saveLabel: 'Show', body: ssec(null, [srow({ label: 'Day', ctl: date, cls: 'text-row' })]), onSave: () => {
      if (!date.value) return 'Pick a day';
      f.day = date.value; f.mode = 'day'; load();
    } });
    date.addEventListener('keydown', (e) => { if (e.key === 'Enter') s.save.click(); });
  } });
  const fbtn = (ic, text) => h('button', { class: 'fbtn' }, ic && icon(ic, 18), h('span', {}, text), h('span', { class: 'chev' }, icon('chev', 14)));
  const whoBtn = fbtn('user', 'Everyone');
  const typeBtn = fbtn(null, 'All types');
  const lookBtn = h('button', { class: 'fbtn look', 'aria-pressed': 'false', onclick: () => { f.look = !f.look; load(); } }, icon('alert', 18), 'Needs a look', h('span', { class: 'n', hidden: true }));
  const find = h('input', { type: 'search', placeholder: 'Order #, table or name', 'aria-label': 'Search the log', enterkeyhint: 'search', value: f.q });
  find.addEventListener('pointerup', () => tapFocus(find));
  let ft;
  find.addEventListener('input', () => { clearTimeout(ft); ft = setTimeout(() => { f.q = find.value.trim(); load(); }, 250); });
  const whoItems = () => {
    const on = (staff ?? []).filter((u) => u.active), off = (staff ?? []).filter((u) => !u.active);
    return [{ v: null, label: 'Everyone', on: f.userId == null }, ...on.map((u) => ({ v: u.id, label: u.name, on: f.userId === u.id })),
      off.length && '-', ...off.map((u) => ({ v: u.id, label: u.name, on: f.userId === u.id }))].filter(Boolean);
  };
  const tools = h('div', { class: 'alog-tools' }, seg, dayBtn, whoBtn, typeBtn, lookBtn, h('label', { class: 'searchbox' }, icon('search', 20), find));
  // Who / type menus: built on each open, so the ✓ follows the filter.
  const menuFor = (btn, title, items, pick) => {
    btn.onclick = async (e) => {
      e.preventDefault(); e.stopPropagation();
      if (btn === whoBtn && !staff) staff = await call('activity_staff').catch(() => []);
      const list = items();
      const choose = (v) => { closeInfoPop(); if (document.querySelector('#modal .sheet')) closeModal(); pick(v); };
      const make = () => h('div', { class: 'pop-menu' }, list.map((it) => (it === '-' ? h('hr') : h('button', { class: it.on ? 'on' : '', onclick: () => choose(it.v) }, it.on ? icon('check', 16) : null, it.label))));
      if (innerWidth <= 700) return sheet({ title, body: make() });
      const mine = openPop?.btn === btn;
      closeInfoPop();
      if (mine) return;
      const pop = h('div', { class: 'pop pop-menu-host', role: 'menu', 'aria-label': title }, make());
      document.body.append(pop);
      const place = () => { const r = btn.getBoundingClientRect(); pop.style.left = `${Math.max(16, Math.min(r.left, innerWidth - pop.offsetWidth - 16))}px`; pop.style.top = `${Math.min(r.bottom + 2, innerHeight - pop.offsetHeight - 8)}px`; };
      place();
      btn.setAttribute('aria-expanded', 'true');
      openPop = { btn, pop, place, at: Date.now() };
    };
  };
  menuFor(whoBtn, 'Who', whoItems, (v) => { f.userId = v; load(); });
  menuFor(typeBtn, 'Type', () => [{ v: null, label: 'All types', on: !f.kind }, ...ACT_KINDS.map(([v, l]) => ({ v, label: l, on: f.kind === v }))], (v) => { f.kind = v; load(); });
  const selCount = h('strong', {});
  const selBar = h('div', { class: 'savebar', hidden: true, role: 'region', 'aria-label': 'Selected entries' }, selCount,
    h('button', { class: 'ghost', onclick: () => setSelecting(false) }, 'Cancel'),
    h('button', { class: 'danger solid', onclick: () => deleteSelected() }, 'Delete…'));
  const setSelecting = (on) => { selecting = on; sel.clear(); draw(); };
  const selBtn = isAdmin() && h('button', { class: 'sel-btn', onclick: () => setSelecting(!selecting) }, 'Select…');
  const menu = isAdmin() && moreMenu('Activity log', () => [
    h('button', { class: 'phone-only-item', onclick: () => setSelecting(!selecting) }, selecting ? 'Stop selecting' : 'Select…'),
    h('button', { onclick: () => clearSheet() }, 'Clear older entries…'),
    h('button', { onclick: act(() => exportCsv()) }, 'Export CSV…'),
    h('button', { onclick: () => openManage('g:data', 'log_keep_days') }, `Keep for ${keepLabel(state.logKeep)} ›`),
  ]);
  screen({ action: isAdmin() ? [selBtn, menu] : null }, h('div', { class: 'stack' }, tools, span, listEl, selBar));
  if (isAdmin()) call('get_setting', { key: 'log_keep_days' }, { quiet: true }).then((v) => { state.logKeep = v == null || v === '' ? LOG_KEEP_DEFAULT : Number(v); }).catch(() => {});

  const query = (extra = {}) => ({ from: range.from, to: range.to, userId: f.userId, kind: f.kind, look: f.look || null, q: f.q || null, ...extra });
  let range = { from: 0, to: OPEN_END }, days = [];
  let loads = 0;
  async function load() {
    const mine = ++loads;
    seg.set(f.mode === 'day' ? null : f.mode);
    dayBtn.classList.toggle('on', f.mode === 'day');
    setKids(dayBtn, icon('calendar', 20), f.mode === 'day' && h('span', {}, dayLabel(f.day)));
    const wl = staff?.find((u) => u.id === f.userId)?.name;
    whoBtn.classList.toggle('on', f.userId != null);
    setKids(whoBtn, icon('user', 18), h('span', {}, f.userId == null ? 'Everyone' : wl ?? 'One person'), h('span', { class: 'chev' }, icon('chev', 14)));
    typeBtn.classList.toggle('on', !!f.kind);
    setKids(typeBtn, h('span', {}, f.kind ? ACT_KINDS.find(([v]) => v === f.kind)[1] : 'All types'), h('span', { class: 'chev' }, icon('chev', 14)));
    lookBtn.classList.toggle('on', f.look);
    lookBtn.setAttribute('aria-pressed', String(f.look));
    range = await logRange(f.mode, f.day);
    const r = await call('list_activity', { ...query(), limit: 200 });
    if (mine !== loads || !listEl.isConnected) return;
    rows = r.rows ?? [];
    next = r.next ?? null;
    days = r.days ?? [];
    total = r.total ?? (days.length ? days.reduce((n, d) => n + d.count, 0) : null);
    lookCount = r.look_count ?? 0;
    const n = lookBtn.querySelector('.n');
    n.hidden = !lookCount;
    n.textContent = lookCount;
    span.textContent = [f.mode === 'week' && f.look ? '7 days' : range.span, f.look && 'Needs a look', `${(total ?? rows.length).toLocaleString()} ${(total ?? rows.length) === 1 ? 'entry' : 'entries'}`].filter(Boolean).join(' · ');
    sel.clear();
    draw();
  }
  async function more(btn) {
    btn.disabled = true;
    try {
      const r = await call('list_activity', { ...query(), before: next, limit: 200 });
      rows.push(...(r.rows ?? []));
      next = r.next ?? null;
      draw();
    } finally { btn.disabled = false; }
  }
  function draw() {
    selBar.hidden = !selecting;
    selCount.textContent = `${sel.size} selected`;
    selBar.querySelector('.danger').disabled = !sel.size;
    if (selBtn) selBtn.textContent = selecting ? 'Done' : 'Select…';
    if (!rows.length) {
      const filtered = f.userId != null || f.kind || f.look || f.q;
      return setKids(listEl, filtered
        ? emptyState({ icon: 'search', title: 'Nothing matches', action: h('button', { onclick: () => { Object.assign(f, { userId: null, kind: null, look: false, q: '' }); find.value = ''; load(); } }, 'Clear filters') })
        : emptyState({ icon: 'activity', title: 'No activity', text: 'Nothing logged on this day.' }));
    }
    const out = [];
    let lastDay = null;
    const tday = logRange.today ?? today(), yest = logRange.yesterday ?? isoDate(new Date(Date.now() - 86400000));
    for (const r of foldPins(rows)) {
      // The business day (as History), else the calendar day.
      const d = r.day ?? isoDate(new Date(r.at * 1000));
      if (d !== lastDay) {
        lastDay = d;
        const n = days.find((x) => x.date === d)?.count;
        const name = d === tday ? `Today · ${dayLabel(d)}` : d === yest ? `Yesterday · ${dayLabel(d)}` : dayLabel(d);
        out.push(h('div', { class: 'aday' }, h('span', {}, name), n != null && h('span', {}, `${n.toLocaleString()} ${n === 1 ? 'entry' : 'entries'}`)));
      }
      const ids = r.foldIds;
      out.push(actRow(r, { selecting, checked: ids.every((i) => sel.has(i)), onToggle: () => {
        const on = !ids.every((i) => sel.has(i));
        for (const i of ids) { if (on) sel.add(i); else sel.delete(i); }
        draw();
      } }));
    }
    const left = total != null ? total - rows.length : null;
    if (next) out.push(h('button', { class: 'ghost olist-more alog-more', onclick: (e) => more(e.currentTarget) }, left > 0 ? `Show more (${left.toLocaleString()})` : 'Show more'));
    const keep = listEl.closest('.mdetail')?.scrollTop;
    setKids(listEl, out);
    if (keep != null) listEl.closest('.mdetail').scrollTop = keep;
  }
  function deleteSelected() {
    const ids = [...sel];
    if (!ids.length) return;
    const pin = h('input', { type: 'password', inputmode: 'numeric', autocomplete: 'off', 'aria-label': 'Your PIN' });
    pin.addEventListener('pointerup', () => tapFocus(pin));
    const n = ids.length;
    const s = sheet({ title: `Delete ${n} ${n === 1 ? 'entry' : 'entries'}?`, danger: true, saveLabel: `Delete ${n}`, body: [
      h('p', { class: 'snote' }, `A line saying you deleted ${n} ${n === 1 ? 'entry' : 'entries'} stays in the log.`),
      ssec(null, [srow({ label: 'Your PIN', cls: 'text-row', ctl: pin })]),
    ], onSave: async () => {
      const r = await call('delete_activity', { ids, pin: pin.value }, { quiet: true }).catch((e) => { throw pinError(String(e?.message ?? e)); });
      toast(`${(r?.deleted ?? n).toLocaleString()} ${(r?.deleted ?? n) === 1 ? 'entry' : 'entries'} deleted`);
      selecting = false;
      await load();
    } });
    s.save.prepend(icon('trash', 20));
    const check = () => { s.save.disabled = !/^\d{4,8}$/.test(pin.value); };
    pin.addEventListener('input', check);
    check();
  }
  async function exportCsv(extra = null) {
    const r = await call('export_activity', { ...(extra ?? query()), save: !state.remote });
    return saveCsv(r);
  }
  function clearSheet() {
    const keep = state.logKeep ?? LOG_KEEP_DEFAULT;
    const def = new Date(); def.setDate(def.getDate() - (keep || 90));
    const date = h('input', { type: 'date', value: isoDate(def), max: today(), 'aria-label': 'Older than' });
    const sumN = h('dd', {}, '…'), sumKept = h('dd', {}, '…');
    const csv = switchRow({ label: 'Save a CSV copy first', hint: state.remote ? 'Downloads to this device' : 'To Downloads on this PC', value: true, onChange: () => check() });
    const word = h('input', { class: 'typed', placeholder: 'DELETE', autocomplete: 'off', autocapitalize: 'characters', spellcheck: 'false', 'aria-label': 'Type DELETE' });
    const pin = h('input', { type: 'password', inputmode: 'numeric', autocomplete: 'off', 'aria-label': 'Your PIN' });
    for (const i of [word, pin]) i.addEventListener('pointerup', () => tapFocus(i));
    let count = 0;
    const before = () => dayRange(date.value || today())[0];
    const s = sheet({ title: 'Clear older entries', danger: true, saveLabel: 'Delete', body: [
      h('section', { class: 'ssec' }, h('div', { class: 'sgroup' }, srow({ label: 'Older than', cls: 'text-row', ctl: date }),
        h('dl', { class: 'dsum' }, h('dt', {}, 'Entries'), sumN, h('dt', {}, 'Kept'), sumKept))),
      ssec(null, [csv]),
      ssec('To confirm', [srow({ label: 'Type DELETE', cls: 'text-row', ctl: word }), srow({ label: 'Your PIN', cls: 'text-row', ctl: pin })],
        'A line saying who cleared what stays in the log for good.'),
    ], onSave: async () => {
      let saved = null;
      if (csv.input.checked) {
        try { saved = await exportCsvRaw({ before: before() }); } catch (e) { throw `CSV not saved, nothing deleted: ${String(e?.message ?? e)}`; }
      }
      const r = await call('clear_activity', { before: before(), pin: pin.value, confirm: word.value.trim().toUpperCase(), csv: !!saved }, { quiet: true })
        .catch((e) => { throw pinError(String(e?.message ?? e)); });
      const n = r?.deleted ?? count;
      toast(`${n.toLocaleString()} ${n === 1 ? 'entry' : 'entries'} deleted${saved ? ` · CSV saved${state.remote ? '' : ' to Downloads'}` : ''}`, false, 4000);
      await load();
    } });
    s.save.prepend(icon('trash', 20));
    const dd = (t) => new Date(t * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
    let seq = 0;
    const summary = async () => {
      const mine = ++seq;
      sumN.textContent = '…';
      const r = await call('activity_count', { before: before() }, { quiet: true }).catch(() => null);
      if (mine !== seq) return;
      count = r?.count ?? 0;
      sumN.textContent = r ? (count ? `${count.toLocaleString()} · ${dd(r.from)} to ${dd(r.to)}` : 'None') : '?';
      sumKept.textContent = `Clearings, and everything from ${dd(before())}`;
      check();
    };
    function check() {
      setKids(s.save, icon('trash', 20), `Delete ${count.toLocaleString()}`);
      s.save.disabled = !(count > 0 && word.value.trim().toUpperCase() === 'DELETE' && /^\d{4,8}$/.test(pin.value));
    }
    date.addEventListener('change', summary);
    word.addEventListener('input', check);
    pin.addEventListener('input', check);
    summary();
  }
  async function exportCsvRaw(args) {
    const r = await call('export_activity', { ...args, save: !state.remote }, { quiet: true });
    return saveCsv(r, true);
  }
  await load();
}
const keepLabel = (d) => LOG_KEEP.find(([v]) => v === (d ?? LOG_KEEP_DEFAULT))?.[1] ?? `${d} days`;
/// export_activity: the desktop app has saved the file (its path back); a browser gets the CSV text to download.
function saveCsv(r, quiet = false) {
  if (r && typeof r === 'object' && r.path) { if (!quiet) toast(`CSV saved: ${r.path}`, false, 5000); return r.path; }
  const text = typeof r === 'string' ? r : r?.csv ?? '';
  const name = r?.file_name ?? `activity-${today()}.csv`;
  const a = h('a', { href: URL.createObjectURL(new Blob([text.startsWith('﻿') ? text : `﻿${text}`], { type: 'text/csv;charset=utf-8' })), download: name });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 60000);
  if (!quiet) toast('CSV downloaded');
  return name;
}

/// History order detail (managers and admins): who paid / cancelled / voided it, and its Activity.
function orderWho(o) {
  if (!isManager()) return null;
  const by = o.closed_by_name ?? o.paid_by_name;
  if (!by || o.status === 'open') return null;
  const t = o.closed_at ?? o.paid_at;
  const apn = o.close_approver_name ?? o.approver_name;
  const ap = apn && apn !== by ? apn : null;
  const verb = { paid: 'Paid by', cancelled: 'Cancelled by', voided: 'Voided by' }[o.status] ?? 'Closed by';
  return h('div', { class: 'rcpt-who' }, h('span', {}, `${verb} `, h('b', {}, o.status === 'paid' ? (o.paid_by_name ?? by) : by),
    ap && [' · PIN of ', h('b', {}, ap), (o.close_approver_role ?? o.approver_role) ? ` (${(ROLE_LABEL[o.close_approver_role ?? o.approver_role] ?? o.close_approver_role).toLowerCase()})` : ''], t ? ` · ${hm(t)}` : ''));
}
function orderActivity(o) {
  if (!isManager()) return null;
  const box = h('div', { class: 'ract', hidden: true });
  invoke('order_activity', { orderId: o.id }).then((rows) => {
    const shown = rows.filter((r) => r.action !== 'order.open');
    if (!shown.length || !box.isConnected) return;
    const opened = { id: 0, at: o.opened_at, action: 'order.opened', detail: '', user_name: o.opened_by_name, order_id: o.id };
    setKids(box, h('h4', {}, h('span', {}, 'Activity'), h('button', { class: 'ghost', onclick: () => openLog({ q: `#${o.id}`, at: o.opened_at }) }, 'All in log ', icon('chev', 14))),
      actRowOpened(opened, o), shown.map((r) => actRow({ ...r, order: null, order_title: null }, { compact: true, here: o.id })));
    box.hidden = false;
  }).catch(() => {});
  return box;
}
const actRowOpened = (r, o) => h('div', { class: 'arow', role: 'listitem' }, h('span', { class: 'at' }, hm(r.at)), h('span', { class: 'aic g-shift' }, icon('orders', 16)),
  h('span', { class: 'atext' }, h('strong', {}, `Opened${where(o) ? ` · ${orderTitle(o)}` : ''}`), h('small', {}, h('b', {}, r.user_name ?? ''))), h('span', { class: 'aend' }));

// ---------- start-up notices (admins): unclean shutdown, damaged database ----------

/// Admins: what app_status.backup_alert says (damaged data → Restore; did not close properly;
/// second copy failed). Each kind once per login on this device.
const alertShown = new Set();
function backupAlert(al) {
  if (!al || !isAdmin()) return;
  const key = `${state.session.user_id}:${al.kind}`;
  if (alertShown.has(key)) return;
  alertShown.add(key);
  const toData = (row = null) => openManage('g:data', row);
  if (al.kind === 'damaged') {
    return showModal(h('h3', {}, 'The POS data is damaged'),
      h('div', { class: 'stack confirm-hint' }, 'Restore the newest good backup. Your backups were not changed.'),
      h('button', { class: 'danger solid paybig', onclick: () => { closeModal(); state.view = 'settings'; state.settingsTab = 'data'; state.groupSub = 'backups'; state.findRow = null; renderShell(); } }, 'Restore a backup'),
      h('div', { class: 'row' }, h('span', { class: 'grow' }), h('button', { onclick: closeModal }, 'Later')));
  }
  if (al.kind === 'unclean') {
    // At the top of the screen: never over Send or Finish & Pay.
    return toast(al.message || 'POS did not close properly last time.', false, 12000, { label: 'OK', onclick: () => invoke('dismiss_backup_alert').catch(() => {}) }, { top: true });
  }
  toast(al.message || 'Second copy failed', true, 10000, { label: 'Fix', onclick: () => toData('backup_copy') }, { top: true });
}

// ---------- Backup & data: automatic backups, a second copy, Keep for (admins) ----------

const BK = { auto: 'backup_hourly_on', hours: 'backup_hourly_every', keep: 'backup_hourly_keep', dayClose: 'backup_dayclose_on', exit: 'backup_exit_on',
  emergency: 'backup_emergency_on', copyOn: 'backup_copy_on' };
async function backupAutoSection() {
  const st = await call('backup_status', {}, { quiet: true }).catch(() => null);
  if (!st) return [];
  const cfg = st.settings ?? {}, now = st.state ?? {};
  const every = segRow({ id: BK.hours, label: 'Every', items: [[1, '1 h'], [2, '2 h'], [4, '4 h']], value: [1, 2, 4].includes(Number(cfg.hourly_every)) ? Number(cfg.hourly_every) : 2,
    stacked: false, save: (v) => saveKey(BK.hours, v) });
  const keep = stepperRow({ id: BK.keep, label: 'Keep', hint: 'These backups only', value: Number(cfg.hourly_keep) || 12, min: 3, max: 48, unit: 'backups', save: (v) => saveKey(BK.keep, v) });
  for (const r of [every, keep]) { r.hidden = !cfg.hourly_on; r.dataset.dep = BK.auto; }
  const sw = (k, on, label, hint, o = {}) => switchRow({ id: BK[k], label, hint, value: !!on, save: (v) => saveKey(BK[k], v ? '1' : '0'), ...o });
  const rows = [
    sw('auto', cfg.hourly_on, 'Back up while open', 'Only when something changed', { after: (v) => { every.hidden = !v; keep.hidden = !v; } }),
    every, keep,
    sw('dayClose', cfg.dayclose_on, 'Back up at day close'),
    sw('exit', cfg.exit_on, 'Back up when POS closes'),
    sw('emergency', cfg.emergency_on, 'Back up after a crash', 'Or a power cut', { tip: 'At the next start POS checks the data first, then makes a backup before anyone sells.' }),
  ];
  // A second copy (USB stick, D:, a OneDrive folder): only the main till picks the folder.
  const dir = cfg.copy_dir || '';
  const pick = async (btn) => {
    const row = btn.closest('.srow');
    rowState(row, 'saving');
    try {
      const r = await invoke('pick_backup_copy_folder');
      if (!r) return rowState(row, 'idle');
      redrawGroup(BK.copyOn);
    } catch (e) { rowState(row, 'error', String(e?.message ?? e)); }
  };
  const choose = st.can_pick_folder && h('button', { class: 'test', onclick: (e) => pick(e.currentTarget) }, dir ? 'Change…' : 'Choose folder…');
  const copy = srow({ id: BK.copyOn, label: 'Also copy backups to', hint: dir || (st.can_pick_folder ? 'Pick a folder: USB stick, D: or OneDrive' : 'Chosen on the main till'), cls: dir ? 'path-row' : '',
    tip: 'Every new backup is copied there too, keeping the same number. If the folder is missing, the backup still works here.',
    ctl: [choose, dir && cfg.copy_on && h('button', { class: 'ghost', onclick: async (e) => {
      const row = e.currentTarget.closest('.srow');
      await saveWith(row, () => call('clear_backup_copy_folder', {}, { quiet: true })) && redrawGroup(BK.copyOn);
    } }, 'Turn off')] });
  const err = now.copy_error || (st.copy_dir_ok === false ? 'folder not found' : null);
  const status = dir && cfg.copy_on && statusRow({ id: 'copy_status', icon: err ? 'alert' : 'check', title: err ? 'Second copy failed' : 'Second copy',
    meta: err ? `${err[0].toUpperCase()}${err.slice(1)}${now.copy_error_at ? ` · ${friendlyTime(now.copy_error_at)}` : ''}` : now.copy_ok_at ? `Last copied ${friendlyTime(now.copy_ok_at)}` : 'Not copied yet',
    pill: err ? { cls: 'bad', text: 'Failed' } : now.copy_ok_at ? { cls: 'ok', text: 'OK' } : null });
  return [ssec('Automatic backups', rows), ssec('Second copy', [copy, status])];
}
async function retentionSection() {
  const [raw, info] = await Promise.all([invoke('get_setting', { key: 'log_keep_days' }).catch(() => null), invoke('log_retention').catch(() => null)]);
  const keep = raw == null || raw === '' ? LOG_KEEP_DEFAULT : Number(raw);
  const dd = (t) => new Date(t * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
  const statHint = info && [info.oldest && `Oldest: ${dd(info.oldest)}`,
    info.last_cleanup_at && `last cleanup ${friendlyTime(info.last_cleanup_at)}, ${(() => { const n = Number(info.last_cleanup_count ?? 0), k = Number(info.last_cleanup_tickets ?? 0); return `${n.toLocaleString()} ${n === 1 ? 'entry' : 'entries'}${k ? ` and ${k} done ticket${k === 1 ? '' : 's'}` : ''} removed`; })()}`].filter(Boolean).join(' · ');
  const keepRow = segRow({ id: 'log_keep_days', label: 'Keep for', hint: 'Activity log entries, and tickets that are done', items: LOG_KEEP, value: LOG_KEEP.some(([v]) => v === keep) ? keep : LOG_KEEP_DEFAULT,
    tip: 'Older entries are deleted once a day. A line saying how many stays in the log. Older backups still hold them.',
    save: (v) => saveKey('log_keep_days', v), after: (v) => { state.logKeep = v; setHint(keepRow, 'Older entries go at the next cleanup (start or day close)'); } });
  return ssec('Activity log and tickets', [
    keepRow,
    info && srow({ id: 'log_size', label: 'In the log now', hint: [`${Number(info.count).toLocaleString()} entries · ${fileSize(info.approx_bytes ?? 0)}`, statHint].filter(Boolean).join(' · ') }),
    navRow({ id: 'log_open', label: 'Open the Activity log', onOpen: () => openManage('p:log') }),
  ], 'Older backups still hold entries cleared here until they are deleted.');
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
document.getElementById('tickets').append(icon('chat', 22));
document.getElementById('tickets').onclick = () => { state.ticketTab ??= 'open'; go('tickets'); };
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
    const now = Date.now();
    tk.fails = [...tk.fails.filter((f) => now - f.at < 30 * 60000), ...list.map((f) => ({ at: now, message: f.message }))];
    const text = list.length > 1 ? `${list.at(-1).message} (and ${list.length - 1} more)` : list[0].message;
    toast(text, true, 12000, { label: 'Tell admin', onclick: () => newTicketSheet({ category: 'printer', body: text }) });
  }).catch(() => {});
  // Tickets: badges, reply toasts, the urgent alert (and a restore on another till).
  pollTickets();
}, 10000);
// Back on screen: look at once, not up to 10 s later.
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && state.session) pollTickets(); });
