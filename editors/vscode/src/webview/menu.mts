// A small accessible popup menu anchored to a button: arrow keys, Home/End,
// Enter/Space to pick, Escape or a click outside to close.

import { h, icon, setChildren } from "./dom.mjs";

export interface MenuItem {
  label: string;
  detail?: string;
  icon?: string;
  checked?: boolean;
  indent?: number;
  disabled?: boolean;
  /** Draws a separator above the item. */
  separator?: boolean;
  /** A non-interactive heading above the item. */
  heading?: string;
  /** The item's full name, for its tooltip and screen readers (a model under its provider's heading). */
  title?: string;
  onPick: () => void;
}

let open: { close: () => void } | null = null;

export function closeMenu(): void {
  open?.close();
}

export function showMenu(anchor: HTMLElement, items: MenuItem[], { label, placement = "above" }: { label: string; placement?: "above" | "below" }): void {
  closeMenu();
  const list = h("div", { class: "menu", role: "menu", "aria-label": label, tabindex: "-1" });
  const buttons: HTMLButtonElement[] = [];
  for (const item of items) {
    if (item.separator) list.append(h("div", { class: "menu-separator", role: "separator" }));
    if (item.heading) list.append(h("div", { class: "menu-heading", role: "presentation" }, item.heading));
    const button = h(
      "button",
      {
        class: `menu-item${item.checked ? " checked" : ""}`,
        type: "button",
        role: item.checked === undefined ? "menuitem" : "menuitemradio",
        "aria-checked": item.checked === undefined ? null : String(item.checked),
        disabled: item.disabled,
        tabindex: "-1",
        title: item.title ?? null,
        "aria-label": item.title ?? null,
      },
      h("span", { class: "menu-check", "aria-hidden": "true" }, item.checked ? icon("check", 14) : null),
      item.icon ? icon(item.icon, 14) : null,
      h("span", { class: "menu-label" }, item.label),
      item.detail ? h("span", { class: "menu-detail" }, item.detail) : null,
    );
    if (item.indent) button.style.paddingLeft = `${8 + item.indent * 14}px`;
    button.addEventListener("click", () => {
      close();
      item.onPick();
    });
    buttons.push(button);
    list.append(button);
  }
  document.body.append(list);
  const rect = anchor.getBoundingClientRect();
  const width = Math.min(Math.max(220, rect.width), window.innerWidth - 16);
  list.style.minWidth = `${width}px`;
  list.style.maxWidth = `${window.innerWidth - 16}px`;
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - list.offsetWidth - 8));
  list.style.left = `${left}px`;
  if (placement === "above" && rect.top > list.offsetHeight + 8) list.style.bottom = `${window.innerHeight - rect.top + 4}px`;
  else list.style.top = `${Math.min(rect.bottom + 4, window.innerHeight - list.offsetHeight - 8)}px`;
  anchor.setAttribute("aria-expanded", "true");

  const enabled = () => buttons.filter((b) => !b.disabled);
  const focusAt = (index: number) => {
    const list = enabled();
    if (!list.length) return;
    list[(index + list.length) % list.length].focus();
  };
  const current = () => enabled().indexOf(document.activeElement as HTMLButtonElement);
  const onKey = (event: KeyboardEvent) => {
    if (event.key === "ArrowDown") focusAt(current() + 1);
    else if (event.key === "ArrowUp") focusAt(current() - 1);
    else if (event.key === "Home") focusAt(0);
    else if (event.key === "End") focusAt(-1);
    else if (event.key === "Escape" || event.key === "Tab") {
      close();
      anchor.focus();
    } else return;
    event.preventDefault();
    event.stopPropagation();
  };
  const onDown = (event: MouseEvent) => {
    if (!list.contains(event.target as Node) && event.target !== anchor && !anchor.contains(event.target as Node)) close();
  };
  function close() {
    list.remove();
    anchor.setAttribute("aria-expanded", "false");
    document.removeEventListener("mousedown", onDown, true);
    window.removeEventListener("blur", close);
    if (open?.close === close) open = null;
  }
  list.addEventListener("keydown", onKey);
  document.addEventListener("mousedown", onDown, true);
  window.addEventListener("blur", close);
  open = { close };
  const checked = enabled().findIndex((b) => b.classList.contains("checked"));
  focusAt(checked >= 0 ? checked : 0);
}

/** A button that opens a menu built when it opens. */
export function menuButton(props: { class: string; label: string; title?: string; build: () => MenuItem[]; placement?: "above" | "below" }): HTMLButtonElement {
  const button = h("button", { class: props.class, type: "button", "aria-haspopup": "menu", "aria-expanded": "false", title: props.title ?? null, "aria-label": props.title ?? null });
  button.addEventListener("click", () => {
    if (button.getAttribute("aria-expanded") === "true") closeMenu();
    else showMenu(button, props.build(), { label: props.label, placement: props.placement });
  });
  button.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      showMenu(button, props.build(), { label: props.label, placement: props.placement });
    }
  });
  return button;
}

export function setButtonContent(button: HTMLElement, ...children: (Node | string | null)[]): void {
  setChildren(button, ...children);
}
