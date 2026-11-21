// Dragging a device's box on the Map (dcontroller #122): onto another
// place, onto "new place", onto "no place set", or up and down inside its
// place. Works on the drawn SVG and reports the drop as a `dc-place` event
// on the map's wrapper ({device, place, before}); the page stores it.
// A place's title is dragged the same way to move the place among the
// places (`dc-place-order`, {place, before}).
// Mouse only: a finger on the schema scrolls the page.
(function () {
  "use strict";
  let drag = null;
  let suppress = false;

  const el = (cls, text) => {
    const d = document.createElement("div");
    d.className = cls;
    if (text) d.textContent = text;
    document.body.appendChild(d);
    return d;
  };
  const inside = (r, x, y) => x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;

  // Where the pointer would drop the box: the "new place" target, a place's
  // area (and the box it would stand before there), or nowhere.
  function target(d, x, y) {
    if (inside(d.zone.getBoundingClientRect(), x, y)) return { kind: "new" };
    for (const area of d.wrap.querySelectorAll("svg.map rect.area")) {
      const r = area.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top - 20 || y > r.bottom + 20) continue;
      const place = area.getAttribute("data-place") || "";
      const boxes = Array.from(d.wrap.querySelectorAll("svg.map g.node:not(.asio)"))
        .filter((n) => (n.getAttribute("data-place") || "") === place && n.getAttribute("data-node") !== d.name)
        .map((n) => ({ name: n.getAttribute("data-node"), r: n.querySelector("rect").getBoundingClientRect() }))
        .sort((a, b) => a.r.top - b.r.top);
      const next = boxes.find((b) => (b.r.top + b.r.bottom) / 2 > y);
      const last = boxes[boxes.length - 1];
      // The line that shows where the box will stand.
      const at = next ? next.r.top - 8 : last ? last.r.bottom + 8 : r.top + 40;
      return { kind: "area", place, before: next ? next.name : "", area, line: { left: r.left + 8, width: r.width - 16, top: at } };
    }
    return null;
  }

  // A place being moved: the place it would stand before ("" for last),
  // and the line that shows it.
  function slot(d, x) {
    const areas = Array.from(d.wrap.querySelectorAll("svg.map rect.area"))
      .map((a) => ({ place: a.getAttribute("data-place") || "", r: a.getBoundingClientRect() }))
      .filter((a) => a.place !== d.place)
      .sort((a, b) => a.r.left - b.r.left);
    if (!areas.length) return null;
    // "no place set" stays last: a place goes before it at the most.
    const named = areas.filter((a) => a.place !== "");
    const next = named.find((a) => (a.r.left + a.r.right) / 2 > x);
    const last = named[named.length - 1];
    const ref = next || last || areas[0];
    const left = next ? next.r.left - 14 : last ? last.r.right + 11 : ref.r.left - 14;
    return { kind: "slot", before: next ? next.place : "", line: { left, top: ref.r.top, height: ref.r.height } };
  }

  function show(d, t) {
    if (d.kind === "place") {
      d.line.style.cssText = t
        ? `display:block;left:${t.line.left}px;top:${t.line.top}px;width:3px;height:${t.line.height}px`
        : "display:none";
      return;
    }
    d.wrap.querySelectorAll("svg.map rect.area.drop").forEach((a) => a.classList.remove("drop"));
    d.zone.classList.toggle("drop", !!t && t.kind === "new");
    // In "no place set" the order is the program's: no line there.
    if (t && t.kind === "area" && t.place !== "") {
      d.line.style.cssText = `display:block;left:${t.line.left}px;top:${t.line.top}px;width:${t.line.width}px`;
    } else {
      d.line.style.display = "none";
    }
    if (t && t.kind === "area") t.area.classList.add("drop");
  }

  function start(d) {
    d.on = true;
    d.ghost = el("map-drag-ghost", d.name || d.place);
    d.line = el("map-drag-line");
    document.body.classList.add("map-dragging");
    if (d.kind === "place") return;
    d.zone = el("map-drag-new", "+ new place");
    d.zone.setAttribute("data-testid", "map-new-place");
    const w = d.wrap.getBoundingClientRect();
    const top = Math.max(w.top, 8);
    const height = Math.max(Math.min(w.bottom, window.innerHeight - 8) - top, 60);
    d.zone.style.cssText = `left:${w.right - 150}px;top:${top}px;height:${height}px`;
  }

  function end(d) {
    for (const x of [d.ghost, d.line, d.zone]) if (x) x.remove();
    d.wrap.querySelectorAll("svg.map rect.area.drop").forEach((a) => a.classList.remove("drop"));
    document.body.classList.remove("map-dragging");
  }

  document.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || e.pointerType !== "mouse" || !(e.target instanceof Element)) return;
    const title = e.target.closest("svg.map text.col-title[data-place]");
    const node = title ? null : e.target.closest("svg.map g.node:not(.asio)");
    const wrap = (title || node) && (title || node).closest("[data-drag]");
    if (!wrap) return;
    drag = title
      ? { kind: "place", place: title.getAttribute("data-place"), wrap, x: e.clientX, y: e.clientY, on: false }
      : { kind: "device", name: node.getAttribute("data-node"), wrap, x: e.clientX, y: e.clientY, on: false };
  });

  document.addEventListener("pointermove", (e) => {
    const d = drag;
    if (!d) return;
    if (!d.on) {
      if (Math.hypot(e.clientX - d.x, e.clientY - d.y) < 8) return;
      start(d);
    }
    e.preventDefault();
    d.ghost.style.left = `${e.clientX + 12}px`;
    d.ghost.style.top = `${e.clientY + 12}px`;
    show(d, d.kind === "place" ? slot(d, e.clientX) : target(d, e.clientX, e.clientY));
  });

  const drop = (e) => {
    const d = drag;
    drag = null;
    if (!d || !d.on) return;
    const up = e.type === "pointerup";
    const t = !up ? null : d.kind === "place" ? slot(d, e.clientX) : target(d, e.clientX, e.clientY);
    end(d);
    // The click that ends a drag is not a choice of that device.
    suppress = true;
    setTimeout(() => (suppress = false), 0);
    if (!t) return;
    if (t.kind === "slot") {
      d.wrap.dispatchEvent(new CustomEvent("dc-place-order", { detail: { place: d.place, before: t.before } }));
      return;
    }
    let detail = null;
    if (t.kind === "new") {
      const name = (window.prompt(`A new place for ${d.name}: its name`) || "").trim().slice(0, 40);
      if (name) detail = { device: d.name, place: name, before: "" };
    } else if (t.place === "") {
      detail = { device: d.name, place: "", before: null };
    } else {
      detail = { device: d.name, place: t.place, before: t.before };
    }
    if (detail) d.wrap.dispatchEvent(new CustomEvent("dc-place", { detail }));
  };
  document.addEventListener("pointerup", drop);
  document.addEventListener("pointercancel", drop);
  document.addEventListener(
    "click",
    (e) => {
      if (!suppress) return;
      suppress = false;
      e.stopPropagation();
      e.preventDefault();
    },
    true,
  );
})();
