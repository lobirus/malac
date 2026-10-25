// Builds the text outline returned by browser_snapshot: title, URL,
// headings, visible text blocks and interactive elements. Each interactive
// element gets a ref such as [12], stored in its data-malac-ref attribute so
// it stays the same across snapshots of one document. element.js resolves
// refs through window.__malacRefs, which also reaches into shadow roots.
(() => {
  const TEXT_LIMIT = 300;
  const NAME_LIMIT = 80;
  const VALUE_LIMIT = 100;
  const OPTION_LIMIT = 10;
  const OUTPUT_LIMIT = 500000;

  const SKIPPED = new Set([
    'SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'HEAD', 'META', 'LINK', 'svg', 'CANVAS',
  ]);
  const ROLES = new Set([
    'button', 'link', 'checkbox', 'radio', 'switch', 'tab', 'menuitem', 'menuitemcheckbox',
    'menuitemradio', 'option', 'combobox', 'textbox', 'searchbox', 'slider', 'spinbutton',
    'treeitem',
  ]);
  const INLINE = new Set(['inline', 'inline-block', 'inline-flex', 'inline-grid', 'contents']);

  const refs = new Map();
  window.__malacRefs = refs;
  const lines = [];
  let length = 0;
  let text = '';

  const squash = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const clip = (s, n) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);
  const quote = (s) => JSON.stringify(s);

  const push = (line) => {
    if (length > OUTPUT_LIMIT || line === lines[lines.length - 1]) return;
    lines.push(line);
    length += line.length + 1;
  };

  const flush = () => {
    const block = squash(text);
    text = '';
    if (block) push(clip(block, TEXT_LIMIT));
  };

  // Removes and returns the lines pushed since `start`.
  const take = (start) => {
    const taken = lines.splice(start);
    for (const line of taken) length -= line.length + 1;
    return taken;
  };

  const refOf = (el) => {
    let ref = Number(el.getAttribute('data-malac-ref'));
    if (!ref || (refs.has(ref) && refs.get(ref) !== el)) {
      ref = window.__malacNextRef = (window.__malacNextRef || 0) + 1;
      el.setAttribute('data-malac-ref', ref);
    }
    refs.set(ref, el);
    return ref;
  };

  const roleOf = (el, style) => {
    const explicit = (el.getAttribute('role') || '').split(' ')[0];
    if (ROLES.has(explicit)) return explicit;
    switch (el.tagName) {
      case 'A':
        return el.hasAttribute('href') ? 'link' : null;
      case 'BUTTON':
      case 'SUMMARY':
        return 'button';
      case 'SELECT':
        return 'combobox';
      case 'TEXTAREA':
        return 'textbox';
      case 'INPUT': {
        const type = (el.getAttribute('type') || 'text').toLowerCase();
        if (type === 'hidden') return null;
        if (['button', 'submit', 'reset', 'image'].includes(type)) return 'button';
        if (type === 'checkbox' || type === 'radio') return type;
        if (type === 'range') return 'slider';
        if (type === 'file') return 'file';
        return 'textbox';
      }
    }
    if (el.isContentEditable && !el.parentElement?.isContentEditable) return 'editable';
    if (typeof el.onclick === 'function') return 'clickable';
    if (style.cursor === 'pointer' && el.parentElement &&
        getComputedStyle(el.parentElement).cursor !== 'pointer') {
      return 'clickable';
    }
    return null;
  };

  const nameOf = (el) => {
    const label = el.getAttribute('aria-label');
    if (squash(label)) return squash(label);
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {
      const joined = squash(labelledBy.split(/\s+/)
        .map((id) => document.getElementById(id)?.innerText || '').join(' '));
      if (joined) return joined;
    }
    if (el.labels?.length) {
      const joined = squash([...el.labels].map((l) => l.innerText).join(' '));
      if (joined) return joined;
    }
    if (el.tagName === 'INPUT') {
      if (['button', 'submit', 'reset'].includes(el.type)) return squash(el.value);
      if (el.type === 'image') return squash(el.alt);
    }
    if (!['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName)) {
      const own = squash(el.innerText);
      if (own) return own;
      const img = el.querySelector('img[alt]');
      if (squash(img?.alt)) return squash(img.alt);
    }
    return squash(el.getAttribute('title'));
  };

  const href = (el) => {
    const raw = el.getAttribute('href') || '';
    if (raw.startsWith('javascript:')) return '';
    try {
      const url = new URL(el.href);
      const short = url.origin === location.origin ? url.pathname + url.search + url.hash : url.href;
      return clip(short, VALUE_LIMIT);
    } catch {
      return clip(raw, VALUE_LIMIT);
    }
  };

  const describe = (el, role, ref) => {
    const parts = [`[${ref}] ${role}`];
    const name = clip(nameOf(el), NAME_LIMIT);
    if (name) parts.push(quote(name));
    const tag = el.tagName;
    const field = (tag === 'INPUT' || tag === 'TEXTAREA') &&
      ['textbox', 'searchbox', 'combobox', 'spinbutton', 'slider'].includes(role);
    if (field) {
      if (tag === 'INPUT' && el.type !== 'text') parts.push(`type=${el.type}`);
      if (el.placeholder && el.placeholder !== name) parts.push(`placeholder=${quote(clip(el.placeholder, NAME_LIMIT))}`);
      if (el.value) parts.push(`value=${el.type === 'password' ? '"••••"' : quote(clip(el.value, VALUE_LIMIT))}`);
    }
    if (tag === 'SELECT') {
      const options = [...el.options];
      const selected = options.filter((o) => o.selected).map((o) => squash(o.label));
      if (selected.length) parts.push(`value=${quote(selected.join(', '))}`);
      const labels = options.slice(0, OPTION_LIMIT).map((o) => squash(o.label));
      const more = options.length > OPTION_LIMIT ? ` (+${options.length - OPTION_LIMIT} more)` : '';
      parts.push(`options: ${labels.join(' | ')}${more}`);
    }
    if (role === 'link') {
      const target = href(el);
      if (target) parts.push(`-> ${target}`);
    }
    const checked = el.checked ?? el.getAttribute('aria-checked') === 'true';
    if (['checkbox', 'radio', 'switch', 'menuitemcheckbox', 'menuitemradio'].includes(role)) {
      parts.push(checked ? 'checked' : 'unchecked');
    }
    if (el.getAttribute('aria-selected') === 'true') parts.push('selected');
    if (el.getAttribute('aria-pressed') === 'true') parts.push('pressed');
    const expanded = el.getAttribute('aria-expanded');
    if (expanded) parts.push(expanded === 'true' ? 'expanded' : 'collapsed');
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') parts.push('disabled');
    return parts.join(' ');
  };

  const hasBox = (el) => el.getClientRects().length > 0;

  const children = (node) => {
    if (node.shadowRoot) return node.shadowRoot.childNodes;
    if (node.tagName === 'SLOT') {
      const assigned = node.assignedNodes({ flatten: true });
      if (assigned.length) return assigned;
    }
    return node.childNodes;
  };

  // With `refsOnly`, only interactive elements are listed because the caller
  // has already used the text.
  const walk = (node, refsOnly = false) => {
    for (const child of children(node)) {
      if (length > OUTPUT_LIMIT) return;
      if (child.nodeType === Node.TEXT_NODE) {
        if (!refsOnly) text += child.textContent;
        continue;
      }
      if (child.nodeType !== Node.ELEMENT_NODE || SKIPPED.has(child.tagName)) continue;
      const el = child;
      const style = getComputedStyle(el);
      if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse') {
        continue;
      }
      if (el.tagName === 'BR') {
        flush();
        continue;
      }
      if (el.tagName === 'IFRAME') {
        flush();
        if (hasBox(el)) push(`iframe ${quote(el.title || '')} -> ${clip(el.src, VALUE_LIMIT)}`);
        continue;
      }
      const role = roleOf(el, style);
      if (role && hasBox(el)) {
        flush();
        const ref = refOf(el);
        const container = (role === 'clickable' || role === 'editable') &&
          squash(el.innerText).length > NAME_LIMIT;
        if (container) {
          push(`[${ref}] ${role}:`);
          walk(el, refsOnly);
          flush();
        } else {
          push(describe(el, role, ref));
        }
        continue;
      }
      const heading = /^H([1-6])$/.exec(el.tagName);
      if (heading && !refsOnly) {
        flush();
        const hashes = '#'.repeat(Number(heading[1]));
        const title = clip(squash(el.innerText), TEXT_LIMIT);
        const start = lines.length;
        walk(el, true);
        const refs = take(start);
        // A heading that is only a link reads best as one line.
        if (refs.length === 1 && refs[0].includes(quote(clip(title, NAME_LIMIT)))) {
          push(`${hashes} ${refs[0]}`);
        } else {
          if (title) push(`${hashes} ${title}`);
          for (const line of refs) push(line);
        }
        continue;
      }
      // A label's text is the name of its control, which is listed itself.
      if (el.tagName === 'LABEL' && el.control && hasBox(el.control)) {
        walk(el, true);
        continue;
      }
      if (style.display === 'table-cell') {
        if (!refsOnly && squash(text)) text += ' | ';
        walk(el, refsOnly);
        continue;
      }
      const block = !INLINE.has(style.display);
      if (block) flush();
      walk(el, refsOnly);
      if (block) flush();
    }
  };

  walk(document.body || document.documentElement);
  flush();

  const header = [`Title: ${document.title}`, `URL: ${location.href}`];
  const scrollable = document.documentElement.scrollHeight - innerHeight;
  if (scrollable > 0) {
    header.push(`Scroll: ${Math.round(scrollY)} of ${scrollable}px`);
  }
  const body = lines.length ? lines.join('\n') : '(no visible content)';
  return `${header.join('\n')}\n\n${body}`;
})()
