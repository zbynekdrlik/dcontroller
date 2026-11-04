// dcontroller's charts: a thin layer over uPlot (vendor/uplot, MIT). The
// Leptos app calls window.dcChart.latency(el, json) with a chart's data; this
// file keeps the charts in that element, updates them in place, follows the
// element's width, and reports a drag-zoom as a "dc-zoom" event on the
// element ({from, to} in unix seconds) so the app can fetch finer data.
// Double-click asks for the app's range again ("dc-unzoom").
//
// One series per source device. Two charts share the time axis and the
// cursor: late packets (the damage: packets that came after their flow's
// latency and were not played), shown only when there are any, and the
// margin (the worst peak against the flow's latency).
"use strict";
(() => {
  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const SYNC = "dc-latency";

  function axisStyle() {
    const grid = { stroke: css("--grid") || "#243049", width: 1 };
    return { stroke: css("--muted") || "#8b97ad", grid, ticks: grid, font: "11px system-ui, sans-serif" };
  }

  function zoomHooks(el) {
    return {
      setSelect: [
        (u) => {
          if (u.select.width < 4) return;
          const from = Math.floor(u.posToVal(u.select.left, "x"));
          const to = Math.ceil(u.posToVal(u.select.left + u.select.width, "x"));
          u.setSelect({ left: 0, width: 0 }, false);
          el.dispatchEvent(new CustomEvent("dc-zoom", { detail: { from, to } }));
        },
      ],
    };
  }

  function lateData(d) {
    return [d.t, ...d.sources.map((s) => s.late.map((v) => (v ? v : null)))];
  }

  function marginData(d) {
    return [d.t, ...d.sources.map((s) => s.values), ...d.thresholds.map((th) => d.t.map(() => th.value))];
  }

  function buildLate(box, el, d, width) {
    const axis = axisStyle();
    const series = [{}];
    for (const s of d.sources) {
      series.push({
        label: `${s.label} · late`,
        stroke: s.color,
        fill: s.color + "55",
        width: 1,
        paths: uPlot.paths.bars({ size: [0.9, 14] }),
        points: { show: false },
        value: (_, v) => (v == null ? "0" : String(v)),
      });
    }
    const opts = {
      width,
      height: 150,
      cursor: { sync: { key: SYNC }, drag: { x: true, y: false, setScale: false }, points: { show: false } },
      select: { show: true },
      legend: { live: true },
      scales: { x: { time: true }, y: { auto: true, range: (u, lo, hi) => [0, Math.max(hi, 1) * 1.1] } },
      axes: [
        axis,
        { ...axis, label: "late packets", labelSize: 14, size: 52, values: (u, v) => v.map((x) => (Number.isInteger(x) ? x : "")) },
      ],
      series,
      hooks: zoomHooks(el),
    };
    return new uPlot(opts, lateData(d), box);
  }

  function buildMargin(box, el, d, width) {
    const fmt = (v) => (v == null ? "–" : d.unit === "%" ? `${v.toFixed(0)} %` : `${v.toFixed(2)} ms`);
    const axis = axisStyle();
    const series = [{}];
    for (const s of d.sources) {
      series.push({
        label: s.label,
        stroke: s.color,
        width: 1.5,
        points: { show: d.t.length < 120 },
        spanGaps: false,
        value: (_, v) => fmt(v),
      });
    }
    for (const th of d.thresholds) {
      series.push({
        label: th.label,
        stroke: th.color,
        width: 1,
        dash: [6, 4],
        points: { show: false },
        value: (_, v) => fmt(v),
      });
    }
    const opts = {
      width,
      height: d.height || 280,
      cursor: { sync: { key: SYNC }, drag: { x: true, y: false, setScale: false }, points: { size: 6 } },
      select: { show: true },
      legend: { live: true },
      scales: { x: { time: true }, y: { auto: true, range: (u, lo, hi) => [0, Math.max(hi * 1.1, d.floor || 0.1)] } },
      axes: [
        axis,
        { ...axis, label: d.unit === "%" ? "typical peak, % of latency" : "typical peak, ms", labelSize: 14, size: 52,
          values: (u, v) => v.map((x) => (d.unit === "%" ? x.toFixed(0) : x.toFixed(2))) },
      ],
      series,
      hooks: zoomHooks(el),
    };
    return new uPlot(opts, marginData(d), box);
  }

  function destroy(el) {
    for (const u of el._dc || []) u.destroy();
    el._dc = null;
  }

  window.dcChart = {
    latency(el, json) {
      const d = JSON.parse(json);
      // Labels, latency lines, the floor and whether late packets show live
      // in the charts' options: a change to any of them builds them anew.
      const shape = [d.unit, d.sources.map((s) => s.label).join("|"), d.thresholds.map((th) => th.label + "=" + th.value).join("|"), d.late, d.floor].join("#");
      if (el._dc && el._dcShape === shape) {
        const [late, margin] = el._dcLate ? el._dc : [null, el._dc[0]];
        if (late) late.setData(lateData(d));
        margin.setData(marginData(d));
        return;
      }
      destroy(el);
      el.textContent = "";
      const width = el.clientWidth || 600;
      const charts = [];
      if (d.late) {
        const box = document.createElement("div");
        box.className = "chart-late";
        box.dataset.testid = "lat-chart-late";
        el.appendChild(box);
        charts.push(buildLate(box, el, d, width));
      }
      const box = document.createElement("div");
      box.className = "chart-margin";
      box.dataset.testid = "lat-chart-margin";
      el.appendChild(box);
      charts.push(buildMargin(box, el, d, width));
      for (const u of charts) u.over.addEventListener("dblclick", () => el.dispatchEvent(new CustomEvent("dc-unzoom")));
      el._dc = charts;
      el._dcLate = !!d.late;
      el._dcShape = shape;
      if (!el._dcResize) {
        el._dcResize = new ResizeObserver(() => {
          if (el._dc && el.clientWidth > 0) for (const u of el._dc) u.setSize({ width: el.clientWidth, height: u.height });
        });
        el._dcResize.observe(el);
      }
    },
  };
})();
