'use strict';
// On-screen keyboard for touch tills: English, Thai (Kedmanee) and a number pad.
//
// It opens when the user taps a text field (never when the app focuses a field
// by itself, e.g. the search box on the order screen), types with real input
// events so the app's own handlers run as if a physical keyboard were used, and
// keeps the field visible above it. On/off and the language are remembered per
// device in localStorage; app.js puts the on/off button in the top bar.
(() => {
  const LS_ON = 'osk_on', LS_LANG = 'osk_lang';
  const mem = {};
  const store = {
    get(k) { try { return localStorage.getItem(k); } catch { return mem[k] ?? null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch { mem[k] = v; } },
  };

  // Each layer is a list of rows; a row is a string of keys, or [unshifted, shifted] strings.
  const EN = [['qwertyuiop'], ['asdfghjkl'], ['zxcvbnm']];
  const TH = [
    ['ๅ/-ภถุึคตจขช', '+๑๒๓๔ู฿๕๖๗๘๙'],
    ['ๆไำพะัีรนยบล', '๐"ฎฑธํ๊ณฯญฐ,'],
    ['ฟหกดเ้่าสวง', 'ฤฆฏโฌ็๋ษศซ.'],
    ['ผปแอิืทมใฝ', '()ฉฮฺ์?ฒฬฦ'],
  ];
  const SYM = [['-/:;()£฿&@'], ['#%+=*_~<>"']];
  const DIGITS = '1234567890';
  const MARK = /[\u0E31\u0E34-\u0E3A\u0E47-\u0E4E]/;
  const ICON_BACK = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 5H9l-6 7 6 7h12z"/><path d="M13 9l4 6M17 9l-4 6"/></svg>';
  const ICON_SHIFT = '<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l9 10h-5v7H8v-7H3z"/></svg>';
  const ICON_DOWN = '<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 9l7 7 7-7"/></svg>';

  const S = {
    enabled: false, el: null, layer: 'letters', lang: store.get(LS_LANG) === 'th' ? 'th' : 'en',
    shift: 0, lastShift: 0, root: null, keys: null, visible: false,
  };

  // ------------------------------------------------------------ fields

  const TEXT_TYPES = new Set(['', 'text', 'search', 'email', 'url', 'tel', 'number', 'password']);
  function eligible(t) {
    const el = t instanceof Element ? t.closest('input, textarea') : null;
    if (!el || el.disabled || el.readOnly || el.closest('#osk')) return null;
    if (el.tagName === 'INPUT' && !TEXT_TYPES.has((el.getAttribute('type') || 'text').toLowerCase())) return null;
    return el;
  }
  // Number pad for number / numeric / decimal fields (and PINs).
  function numeric(el) {
    const im = el.dataset.oskIm ?? el.getAttribute('inputmode');
    return el.type === 'number' || im === 'numeric' || im === 'decimal' || im === 'tel';
  }
  const allowsDot = (el) => el.type !== 'number' && (el.dataset.oskIm ?? el.getAttribute('inputmode')) !== 'numeric';

  function target(el) {
    if (S.el && S.el !== el) release(S.el);
    S.el = el;
    // Stop the OS touch keyboard from also appearing over ours.
    if (el.dataset.oskIm === undefined) el.dataset.oskIm = el.getAttribute('inputmode') ?? '';
    el.setAttribute('inputmode', 'none');
    S.layer = numeric(el) ? 'num' : (S.layer === 'num' ? 'letters' : S.layer);
    const plainText = el.tagName === 'TEXTAREA' || ['', 'text'].includes(el.getAttribute('type') || '');
    S.shift = plainText && !el.value && S.lang === 'en' && S.layer === 'letters' ? 1 : 0;
  }
  function release(el) {
    if (el.dataset.oskIm !== undefined) {
      if (el.dataset.oskIm) el.setAttribute('inputmode', el.dataset.oskIm); else el.removeAttribute('inputmode');
      delete el.dataset.oskIm;
    }
  }

  // ------------------------------------------------------------ typing

  function fire(el, type, init = {}) {
    el.dispatchEvent(new Event(type, { bubbles: true, ...init }));
  }
  function insert(text) {
    const el = S.el;
    if (!el) return;
    if (typeof el.selectionStart === 'number' && el.type !== 'number') {
      el.setRangeText(text, el.selectionStart, el.selectionEnd, 'end');
    } else {
      el.value += text; // number fields have no caret
    }
    fire(el, 'input');
  }
  function backspace() {
    const el = S.el;
    if (!el) return;
    if (typeof el.selectionStart === 'number' && el.type !== 'number') {
      const [a, b] = [el.selectionStart, el.selectionEnd];
      if (a !== b) el.setRangeText('', a, b, 'end');
      else if (a > 0) el.setRangeText('', a - 1, a, 'end');
      else return;
    } else if (el.value) {
      el.value = el.value.slice(0, -1);
    } else return;
    fire(el, 'input');
  }
  function clear() {
    if (S.el) { S.el.value = ''; fire(S.el, 'input'); }
  }
  function enter() {
    const el = S.el;
    if (!el) return;
    if (el.tagName === 'TEXTAREA') return insert('\n');
    const init = { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true };
    el.dispatchEvent(new KeyboardEvent('keydown', init));
    el.dispatchEvent(new KeyboardEvent('keyup', init));
    hide();
  }
  function typeKey(ch) {
    insert(ch);
    if (S.shift === 1) { S.shift = 0; draw(); }
  }

  // ------------------------------------------------------------ drawing

  function key(label, cls, act, opts = {}) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = `osk-key ${cls || ''}`;
    if (opts.html) b.innerHTML = label; else b.textContent = label;
    if (opts.title) b.title = opts.title;
    if (opts.on) b.classList.add('on');
    if (opts.style) b.style.cssText = opts.style;
    let timer, repeat;
    const stop = () => { clearTimeout(timer); clearInterval(repeat); };
    b.addEventListener('pointerdown', (e) => {
      e.preventDefault(); // keep the focus in the field
      act();
      if (opts.repeat) { timer = setTimeout(() => { repeat = setInterval(act, 70); }, 420); }
    });
    for (const ev of ['pointerup', 'pointerleave', 'pointercancel']) b.addEventListener(ev, stop);
    b.addEventListener('click', (e) => e.preventDefault());
    return b;
  }
  const row = (cls) => { const r = document.createElement('div'); r.className = `osk-row ${cls || ''}`; return r; };

  function letterRows() {
    const rows = [];
    const upper = S.shift > 0;
    if (S.layer === 'sym') {
      const r0 = row(); for (const c of DIGITS) r0.append(key(c, '', () => typeKey(c))); rows.push(r0);
      for (const [s] of SYM) { const r = row(); for (const c of s) r.append(key(c, '', () => typeKey(c))); rows.push(r); }
      const r3 = row();
      r3.append(key('ABC', 'fn wide', () => { S.layer = 'letters'; draw(); }));
      for (const c of ".,?!'") r3.append(key(c, '', () => typeKey(c)));
      r3.append(key(ICON_BACK, 'fn wide', backspace, { html: true, repeat: true, title: 'Backspace' }));
      rows.push(r3);
      return rows;
    }
    const base = S.lang === 'th' ? TH : EN;
    if (S.lang === 'en') {
      const r0 = row(); for (const c of DIGITS) r0.append(key(c, '', () => typeKey(c))); rows.push(r0);
    }
    base.forEach((spec, i) => {
      const chars = spec.length === 2 ? (upper ? spec[1] : spec[0]) : spec[0];
      const r = row();
      const isLast = i === base.length - 1;
      if (isLast) r.append(key(ICON_SHIFT, `fn wide${S.shift ? ' on' : ''}${S.shift === 2 ? ' caps' : ''}`, shift, { html: true, title: 'Shift' }));
      for (const c of chars) {
        const out = S.lang === 'en' && upper ? c.toUpperCase() : c;
        // Vowel and tone marks sit on a letter: show them on a dotted circle so they can be seen.
        r.append(key(MARK.test(out) ? `\u25CC${out}` : out, '', () => typeKey(out)));
      }
      if (isLast) r.append(key(ICON_BACK, 'fn wide', backspace, { html: true, repeat: true, title: 'Backspace' }));
      rows.push(r);
    });
    return rows;
  }
  function shift() {
    const now = Date.now();
    S.shift = S.shift === 0 ? 1 : S.shift === 1 && now - S.lastShift < 450 ? 2 : 0;
    S.lastShift = now;
    draw();
  }

  function numberPad() {
    const el = S.el;
    const g = document.createElement('div');
    g.className = 'osk-num';
    const add = (label, cls, act, opts) => g.append(key(label, cls, act, opts));
    const digit = (c) => add(c, '', () => insert(c));
    for (const c of '789') digit(c);
    add(ICON_BACK, 'fn', backspace, { html: true, repeat: true, title: 'Backspace' });
    for (const c of '456') digit(c);
    add('OK', 'done', enter, { style: 'grid-row: 2 / span 2' });
    for (const c of '123') digit(c);
    if (allowsDot(el)) add('.', '', () => insert('.')); else g.append(document.createElement('span'));
    digit('0');
    add('C', 'fn', clear, { title: 'Clear' });
    add(ICON_DOWN, 'fn', hide, { html: true, title: 'Hide keyboard' });
    return g;
  }

  function draw() {
    if (!S.root) return;
    const box = S.root.querySelector('.osk-keys');
    box.replaceChildren();
    if (!S.el) return;
    if (S.layer === 'num') {
      box.append(numberPad());
    } else {
      box.append(...letterRows());
      const r = row('osk-bottom');
      r.append(key(ICON_DOWN, 'fn', hide, { html: true, title: 'Hide keyboard' }));
      r.append(key(S.layer === 'sym' ? 'ABC' : '123', 'fn', () => { S.layer = S.layer === 'sym' ? 'letters' : 'sym'; draw(); }));
      r.append(key(S.lang === 'en' ? 'ไทย' : 'EN', 'fn', () => {
        S.lang = S.lang === 'en' ? 'th' : 'en'; S.layer = 'letters'; S.shift = 0; store.set(LS_LANG, S.lang); draw();
      }, { title: 'Language' }));
      r.append(key('', 'space', () => insert(' '), { title: 'Space' }));
      r.append(key(',', '', () => typeKey(',')));
      r.append(key('.', '', () => typeKey('.')));
      r.append(key(S.el.tagName === 'TEXTAREA' ? '⏎' : 'Done', 'done', enter));
      box.append(r);
    }
    measure();
  }

  // ------------------------------------------------------------ show / hide

  function build() {
    if (S.root) return;
    S.root = document.createElement('div');
    S.root.id = 'osk';
    S.root.hidden = true;
    S.root.innerHTML = '<div class="osk-keys"></div>';
    // Taps on the keyboard must never take the focus away from the field.
    for (const ev of ['pointerdown', 'mousedown', 'touchstart']) S.root.addEventListener(ev, (e) => { if (e.target === S.root || e.target.classList.contains('osk-keys') || e.target.classList.contains('osk-row')) e.preventDefault(); });
    document.body.append(S.root);
  }
  function measure() {
    const h = S.visible ? S.root.offsetHeight : 0;
    document.documentElement.style.setProperty('--kb-h', `${h}px`);
  }
  function show(el) {
    build();
    target(el);
    S.visible = true;
    S.root.hidden = false;
    document.body.classList.add('kb-open');
    draw();
    // Once the layout has shrunk, bring the field into view.
    requestAnimationFrame(() => { measure(); setTimeout(() => el.scrollIntoView({ block: 'center', behavior: 'smooth' }), 60); });
  }
  function hide() {
    if (!S.visible) return;
    S.visible = false;
    S.root.hidden = true;
    document.body.classList.remove('kb-open');
    document.documentElement.style.setProperty('--kb-h', '0px');
    if (S.el) { release(S.el); S.el = null; }
    OSK.onchange?.();
  }

  // Open on a tap on a field; close on a tap anywhere else. Fields the app
  // focuses by itself do not open it.
  document.addEventListener('click', (e) => {
    if (!S.enabled) return;
    const el = eligible(e.target);
    if (el) { if (S.el !== el || !S.visible) show(el); }
  }, true);
  document.addEventListener('pointerdown', (e) => {
    if (!S.visible) return;
    if (e.target instanceof Element && (e.target.closest('#osk') || eligible(e.target) || e.target.closest('label'))) return;
    hide();
  }, true);
  // Moving to another field with the keyboard still open (Tab, or a script).
  document.addEventListener('focusin', (e) => {
    if (!S.enabled || !S.visible) return;
    const el = eligible(e.target);
    if (el && el !== S.el) { target(el); draw(); }
  });
  window.addEventListener('resize', () => { if (S.visible) measure(); });

  const OSK = {
    onchange: null,
    get enabled() { return S.enabled; },
    set enabled(v) {
      S.enabled = !!v;
      store.set(LS_ON, S.enabled ? '1' : '0');
      if (!S.enabled) hide();
      OSK.onchange?.();
    },
    get visible() { return S.visible; },
    /// First run on this device: on or off until the user chooses.
    init(defaultOn) {
      const v = store.get(LS_ON);
      S.enabled = v === null ? !!defaultOn : v === '1';
      OSK.onchange?.();
    },
    hide,
  };
  window.OSK = OSK;
})();
