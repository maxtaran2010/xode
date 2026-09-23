(() => {
  const MAXL = 1500;
  const refs = [];
  window.__xodeRefs = refs;
  const out = [];
  const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'HEAD', 'META', 'LINK', 'SVG', 'svg', 'CANVAS', 'PATH', 'OPTION']);
  const BLOCK = new Set(['P', 'DIV', 'SECTION', 'ARTICLE', 'LI', 'UL', 'OL', 'TR', 'TABLE', 'TBODY', 'THEAD', 'TFOOT', 'DL', 'DT', 'DD',
    'BLOCKQUOTE', 'PRE', 'FIGURE', 'FIGCAPTION', 'BR', 'HR', 'LABEL', 'FIELDSET', 'LEGEND', 'DETAILS', 'BODY', 'CENTER', 'ADDRESS']);
  const LAND = { NAV: 'nav', MAIN: 'main', FORM: 'form', DIALOG: 'dialog', HEADER: 'header', FOOTER: 'footer', ASIDE: 'aside' };
  const LROLE = { navigation: 'nav', main: 'main', dialog: 'dialog', alertdialog: 'dialog', banner: 'header', contentinfo: 'footer', search: 'search', form: 'form', menu: 'menu', listbox: 'listbox', tablist: 'tabs' };
  const IROLES = new Set(['button', 'link', 'textbox', 'searchbox', 'combobox', 'checkbox', 'radio', 'switch', 'tab', 'menuitem',
    'menuitemcheckbox', 'menuitemradio', 'option', 'slider', 'spinbutton', 'treeitem']);
  const INTERACTIVE = 'a[href],button,input,select,textarea,[role=button],[role=link],[role=checkbox],[role=tab],[role=menuitem],[contenteditable=true]';
  const clip = (s, n) => { s = (s || '').replace(/\s+/g, ' ').trim(); return s.length > n ? s.slice(0, n - 1) + '…' : s; };
  let buf = '';
  let depth = 0;
  let stop = false;
  const ind = () => ' '.repeat(Math.min(depth, 10));
  const emit = (l) => { if (out.length >= MAXL) { stop = true; return; } out.push(ind() + l); };
  const flush = () => { const t = clip(buf, 300); buf = ''; if (t) emit(t); };
  const hidden = (el) => {
    if (el.hidden || el.getAttribute('aria-hidden') === 'true') return true;
    if (el.checkVisibility) return !el.checkVisibility({ visibilityProperty: true });
    const s = getComputedStyle(el);
    return s.display === 'none' || s.visibility === 'hidden';
  };
  const role = (el) => {
    const r = el.getAttribute('role');
    if (r && IROLES.has(r)) return r;
    const t = el.tagName;
    if (t === 'A' && el.hasAttribute('href')) return 'link';
    if (t === 'BUTTON' || t === 'SUMMARY') return 'button';
    if (t === 'SELECT') return 'combobox';
    if (t === 'TEXTAREA') return 'textbox';
    if (t === 'INPUT') {
      const ty = (el.type || 'text').toLowerCase();
      if (ty === 'hidden') return null;
      if (['button', 'submit', 'reset', 'image'].includes(ty)) return 'button';
      if (ty === 'checkbox' || ty === 'radio') return ty;
      if (ty === 'range') return 'slider';
      if (ty === 'search') return 'searchbox';
      return 'textbox';
    }
    if (el.isContentEditable && !(el.parentElement && el.parentElement.isContentEditable)) return 'textbox';
    if (el.hasAttribute('onclick') || (el.getAttribute('tabindex') === '0' && getComputedStyle(el).cursor === 'pointer')) return 'clickable';
    return null;
  };
  const label = (el) => {
    let s = el.getAttribute('aria-label');
    if (s) return s;
    const lb = el.getAttribute('aria-labelledby');
    if (lb) {
      s = lb.split(/\s+/).map((id) => { const e = document.getElementById(id); return e ? e.innerText : ''; }).join(' ');
      if (s.trim()) return s;
    }
    const t = el.tagName;
    if (t === 'INPUT' || t === 'TEXTAREA' || t === 'SELECT') {
      if (el.labels && el.labels.length) return el.labels[0].innerText;
      if (t === 'INPUT' && ['button', 'submit', 'reset'].includes(el.type)) return el.value;
      return el.placeholder || el.title || el.name || el.alt || '';
    }
    const txt = el.innerText;
    if (txt && txt.trim()) return txt;
    const img = el.querySelector && el.querySelector('img[alt]');
    return el.title || (img ? img.alt : '') || '';
  };
  const href = (a) => {
    try {
      const raw = typeof a.href === 'string' ? a.href : a.getAttribute('href');
      if (!raw) return '';
      const u = new URL(raw, location.href);
      if (u.protocol === 'javascript:') return '';
      return u.origin === location.origin ? u.pathname + u.search + u.hash : u.href;
    } catch (e) { return ''; }
  };
  const line = (el, r) => {
    const i = refs.push(el) - 1;
    let s = `[e${i}] ${r}`;
    const t = el.tagName;
    if (t === 'INPUT' && !['text', 'search', 'checkbox', 'radio', 'button', 'submit', 'range'].includes(el.type)) s += ':' + el.type;
    const nm = clip(label(el), 80);
    if (nm) s += ` "${nm}"`;
    if (r === 'link') { const h = href(el); if (h) s += ' -> ' + clip(h, 80); }
    if (t === 'SELECT') { const o = el.selectedOptions && el.selectedOptions[0]; s += ` = "${clip(o ? o.text : '', 60)}"`; if (el.options.length) s += ` (${el.options.length} options)`; }
    else if ((t === 'INPUT' || t === 'TEXTAREA') && r !== 'button' && el.value && r !== 'checkbox' && r !== 'radio') {
      s += el.type === 'password' ? ' = "***"' : ` = "${clip(el.value, 80)}"`;
    } else if (el.isContentEditable && r === 'textbox') s += ` = "${clip(el.innerText, 80)}"`;
    if (el.checked || el.getAttribute('aria-checked') === 'true' || el.getAttribute('aria-selected') === 'true') s += ' [x]';
    if (el.getAttribute('aria-expanded')) s += el.getAttribute('aria-expanded') === 'true' ? ' (expanded)' : ' (collapsed)';
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') s += ' (disabled)';
    if (el === document.activeElement) s += ' (focused)';
    emit(s);
  };
  const walkKids = (n) => {
    const kids = n.shadowRoot ? n.shadowRoot.childNodes : n.childNodes;
    for (const c of kids) { if (stop) return; walk(c); }
  };
  const walk = (n) => {
    if (stop) return;
    if (n.nodeType === 3) { buf += ' ' + n.nodeValue; return; }
    if (n.nodeType === 11) { walkKids(n); return; }
    if (n.nodeType !== 1) return;
    const el = n;
    const t = el.tagName;
    if (SKIP.has(t)) return;
    if (t === 'SLOT') { for (const c of el.assignedNodes()) walk(c); return; }
    if (hidden(el)) return;
    if (t === 'IFRAME') {
      let d = null;
      try { d = el.contentDocument; } catch (e) {}
      flush();
      if (d && d.body) { emit('iframe:'); depth++; walk(d.body); flush(); depth--; }
      else emit(`iframe ${clip(el.src, 80)}`);
      return;
    }
    const r = role(el);
    if (r) {
      flush();
      line(el, r);
      if (['clickable', 'tab', 'menuitem', 'option', 'treeitem'].includes(r) && el.querySelector(INTERACTIVE)) { depth++; walkKids(el); flush(); depth--; }
      return;
    }
    const ar = el.getAttribute('role');
    if (/^H[1-6]$/.test(t) || ar === 'heading') {
      flush();
      const lvl = /^H[1-6]$/.test(t) ? +t[1] : +(el.getAttribute('aria-level') || 2);
      const tx = clip(el.innerText, 150);
      if (tx) emit('#'.repeat(lvl) + ' ' + tx);
      if (el.querySelector(INTERACTIVE)) { depth++; walkKids(el); buf = ''; depth--; }
      return;
    }
    const land = LAND[t] || LROLE[ar];
    if (land) {
      flush();
      const at = out.length;
      emit(land + ':');
      depth++;
      walkKids(el);
      flush();
      depth--;
      if (out.length === at + 1) out.pop();
      return;
    }
    const block = BLOCK.has(t) || el.shadowRoot;
    if (block) flush();
    if (t === 'IMG') { const a = el.getAttribute('alt'); if (a && a.trim()) buf += ` [img: ${clip(a, 60)}]`; return; }
    walkKids(el);
    if (block) flush();
  };
  if (document.body) walk(document.body);
  flush();
  if (stop) out.push('… (truncated)');
  const vp = `scroll ${Math.round(scrollY)}/${Math.max(0, document.documentElement.scrollHeight - innerHeight)}`;
  return `# ${document.title}\n${location.href} (${vp})\n` + out.join('\n');
})()
