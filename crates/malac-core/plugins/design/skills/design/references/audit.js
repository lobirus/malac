// Pass this whole file as the `expression` of `browser_eval`. It returns a
// JSON report of problems a screenshot hides: horizontal overflow, controls
// without names, positive tabindex, images without alt, low text contrast,
// and the fonts and font sizes in use. Run it at each width and theme.
(() => {
  const LIMIT = 8;
  const width = document.documentElement.clientWidth;
  const shown = (el) => {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== "hidden" && s.display !== "none";
  };
  const describe = (el) => {
    let d = el.tagName.toLowerCase();
    if (el.id) d += "#" + el.id;
    else if (el.classList.length) d += "." + [...el.classList].slice(0, 2).join(".");
    const text = (el.innerText || el.value || "").trim().replace(/\s+/g, " ").slice(0, 40);
    return text ? `${d} "${text}"` : d;
  };
  const named = (el) => {
    const by = el.getAttribute("aria-labelledby");
    const labelled = by && by.split(/\s+/).some((id) => document.getElementById(id)?.textContent.trim());
    const label = el.id ? document.querySelector(`label[for="${CSS.escape(el.id)}"]`) : null;
    return Boolean(
      el.getAttribute("aria-label")?.trim() || labelled || el.title?.trim() ||
      label?.textContent.trim() || el.closest("label")?.textContent.trim() ||
      (el.tagName !== "INPUT" && el.tagName !== "SELECT" && el.tagName !== "TEXTAREA" &&
        (el.innerText?.trim() || el.querySelector("img[alt]:not([alt=''])"))) ||
      (el.type === "submit" || el.type === "button") && el.value
    );
  };
  const take = (els) => ({ count: els.length, examples: els.slice(0, LIMIT).map(describe) });

  // Horizontal overflow, and the elements that stick out of the viewport
  // without a scrolling container to hold them.
  const clipped = (el) => {
    for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
      if (/(auto|scroll|hidden|clip)/.test(getComputedStyle(p).overflowX)) return true;
    }
    return false;
  };
  const wide = [...document.body.querySelectorAll("*")].filter(
    (el) => shown(el) && el.getBoundingClientRect().right > width + 1 && !clipped(el)
  );
  const overflow = {
    page: Math.max(0, document.documentElement.scrollWidth - width),
    examples: wide.filter((el) => !wide.includes(el.parentElement)).slice(0, LIMIT).map(describe),
  };

  const controls = [...document.querySelectorAll(
    "button, a[href], [role=button], [role=link], [role=tab], [role=menuitem], [role=checkbox], [role=switch]"
  )].filter(shown);
  const fields = [...document.querySelectorAll(
    "input:not([type=hidden]):not([type=submit]):not([type=button]):not([type=reset]), select, textarea"
  )].filter(shown);

  // Contrast of text against the first opaque background behind it.
  const rgba = (value) => {
    const m = value.match(/^rgba?\(([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)(?:[ ,/]+([\d.]+%?))?\)$/) ||
      value.match(/^color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)(?: \/ ([\d.]+%?))?\)$/);
    if (!m) return null;
    const scale = value.startsWith("color(") ? 255 : 1;
    const alpha = m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : parseFloat(m[4]);
    return [m[1] * scale, m[2] * scale, m[3] * scale, alpha];
  };
  const luminance = ([r, g, b]) => {
    const c = [r, g, b].map((v) => {
      v /= 255;
      return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  };
  const background = (el) => {
    for (let e = el; e; e = e.parentElement) {
      const c = rgba(getComputedStyle(e).backgroundColor);
      if (c && c[3] >= 0.95) return c;
      if (c === null && getComputedStyle(e).backgroundColor !== "transparent") return null;
    }
    return rgba(getComputedStyle(document.documentElement).backgroundColor) || [255, 255, 255, 1];
  };
  const lowContrast = [];
  let unchecked = 0;
  const fonts = new Map();
  const sizes = new Map();
  for (const el of document.body.querySelectorAll("*")) {
    const own = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!own || !shown(el)) continue;
    const s = getComputedStyle(el);
    const family = s.fontFamily.split(",")[0].replace(/["']/g, "").trim();
    fonts.set(family, (fonts.get(family) || 0) + 1);
    sizes.set(s.fontSize, (sizes.get(s.fontSize) || 0) + 1);
    const fg = rgba(s.color);
    const bg = background(el);
    if (!fg || !bg) { unchecked++; continue; }
    const [l1, l2] = [luminance(fg), luminance(bg)].sort((a, b) => b - a);
    const ratio = (l1 + 0.05) / (l2 + 0.05);
    const px = parseFloat(s.fontSize);
    const large = px >= 24 || (px >= 18.66 && parseInt(s.fontWeight, 10) >= 700);
    if (ratio < (large ? 3 : 4.5) && fg[3] > 0.5) {
      lowContrast.push(`${describe(el)} ${ratio.toFixed(2)}:1 (${s.color} on rgb(${bg.slice(0, 3).join(", ")}))`);
    }
  }

  return {
    viewport: `${width}x${innerHeight}`,
    colorScheme: matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light",
    document: {
      title: document.title || null,
      lang: document.documentElement.lang || null,
      viewportMeta: Boolean(document.querySelector("meta[name=viewport]")),
      themeColor: document.querySelectorAll("meta[name=theme-color]").length,
      favicon: Boolean(document.querySelector("link[rel~=icon]")),
    },
    overflow,
    unnamedControls: take(controls.filter((el) => !named(el))),
    unlabelledFields: take(fields.filter((el) => !named(el))),
    positiveTabindex: take([...document.querySelectorAll("[tabindex]")].filter((el) => el.tabIndex > 0)),
    imagesWithoutAlt: take([...document.querySelectorAll("img:not([alt])")].filter(shown)),
    lowContrast: { count: lowContrast.length, examples: lowContrast.slice(0, LIMIT), unchecked },
    declaredFonts: Object.fromEntries([...fonts].sort((a, b) => b[1] - a[1])),
    loadedFonts: [...document.fonts].filter((f) => f.status === "loaded").map((f) => f.family.replace(/["']/g, ""))
      .filter((f, i, all) => all.indexOf(f) === i),
    fontSizes: Object.fromEntries([...sizes].sort((a, b) => parseFloat(a[0]) - parseFloat(b[0]))),
  };
})()
