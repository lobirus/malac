// Finds the target of browser_click, browser_type and browser_scroll by
// snapshot ref, CSS selector or focus, scrolls it into view and returns its
// center in viewport coordinates. With `focus` it focuses the element,
// selecting its contents when `clear` is set; with `option` it picks a
// <select> option.
(({ ref, selector, focused, focus, clear, option }) => {
  const label = (el) => {
    const button = ['button', 'submit', 'reset'].includes(el.type);
    const formField = ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName);
    const candidates = [
      el.getAttribute('aria-label'),
      el.labels ? [...el.labels].map((l) => l.innerText).join(' ') : '',
      formField ? (button ? el.value : '') : el.innerText,
      el.placeholder,
      el.getAttribute('title'),
    ];
    const name = (candidates.find((c) => c && c.trim()) || '').replace(/\s+/g, ' ').trim();
    const short = name.length > 60 ? `${name.slice(0, 59)}…` : name;
    const id = el.getAttribute('data-malac-ref');
    const tag = el.tagName.toLowerCase();
    return `${id ? `[${id}] ` : ''}${tag}${short ? ` ${JSON.stringify(short)}` : ''}`;
  };

  let el;
  if (ref != null) {
    el = window.__malacRefs?.get(ref) ?? document.querySelector(`[data-malac-ref="${ref}"]`);
    if (!el || !el.isConnected) {
      return { error: `ref [${ref}] is not on the page; take a new browser_snapshot` };
    }
  } else if (focused) {
    el = document.activeElement;
    while (el?.shadowRoot?.activeElement) el = el.shadowRoot.activeElement;
    if (!el || el === document.body) return { error: 'no element has focus' };
  } else {
    try {
      el = document.querySelector(selector);
    } catch (e) {
      return { error: `invalid selector ${JSON.stringify(selector)}: ${e.message}` };
    }
    if (!el) return { error: `no element matches ${JSON.stringify(selector)}` };
  }

  el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });

  if (option != null) {
    if (el.tagName !== 'SELECT') return { error: `${label(el)} is not a <select>` };
    const wanted = option.trim().toLowerCase();
    const options = [...el.options];
    const match = options.find((o) => o.value === option || o.label.trim().toLowerCase() === wanted) ??
      options.find((o) => o.label.toLowerCase().includes(wanted));
    if (!match) {
      const labels = options.map((o) => o.label.trim()).join(' | ');
      return { error: `${label(el)} has no option ${JSON.stringify(option)}; options: ${labels}` };
    }
    match.selected = true;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return { label: label(el), selected: match.label.trim() };
  }

  if (focus) {
    el.focus();
    if (clear) {
      if (typeof el.select === 'function') {
        el.select();
      } else if (el.isContentEditable) {
        const range = document.createRange();
        range.selectNodeContents(el);
        const selection = getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
      }
    }
  }

  const rect = el.getBoundingClientRect();
  if (!rect.width && !rect.height) return { error: `${label(el)} has no size; it may be hidden` };
  const x = rect.left + rect.width / 2;
  const y = rect.top + rect.height / 2;
  let hit = document.elementFromPoint(x, y);
  while (hit?.shadowRoot) {
    const inner = hit.shadowRoot.elementFromPoint(x, y);
    if (!inner || inner === hit) break;
    hit = inner;
  }
  const covered = hit && !el.contains(hit) && !hit.contains(el) ? label(hit) : null;
  const editable = el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName);
  return { x, y, label: label(el), covered, editable, select: el.tagName === 'SELECT' };
})
