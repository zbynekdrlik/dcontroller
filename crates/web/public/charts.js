// dcontroller's charts: a thin layer over uPlot (vendor/uplot, MIT). The
// Leptos app calls window.dcChart.latency(el, json) with a chart's data; this
// file keeps one uPlot per element, updates it in place, follows the
// element's width, and reports a drag-zoom as a "dc-zoom" event on the
// element ({from, to} in unix seconds) so the app can fetch finer data.
// Double-click asks for the app's range again ("dc-unzoom").
"use strict";
(() => {
  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

  function build(el, d) {
    // The legend reads the value under the cursor in the chart's unit.
    const fmt = (v) => (v == null ? "–" : d.unit === "%" ? `${v.toFixed(0)} %` : `${v.toFixed(2)} ms`);
    const grid = { stroke: css("--grid") || "#243049", width: 1 };
    const axis = { stroke: css("--muted") || "#8b97ad", grid, ticks: grid, font: "11px system-ui, sans-serif" };
    const series = [{}];
    const data = [d.t];
    // Late packets first, so the bars sit behind the flows' lines; faint,
    // and in the lower third of the chart (their own scale).
    if (d.late) {
      series.push({
        label: "late packets",
        stroke: "transparent",
        fill: (css("--bad") || "#f87171") + "40",
        scale: "late",
        paths: uPlot.paths.bars({ size: [0.9, 12] }),
        points: { show: false },
        value: (_, v) => (v == null ? "–" : String(v)),
      });
      data.push(d.late.map((v) => (v ? v : null)));
    }
    for (const f of d.flows) {
      series.push({
        label: f.label,
        stroke: f.color,
        width: 1.5,
        points: { show: d.t.length < 120 },
        spanGaps: false,
        value: (_, v) => fmt(v),
      });
      data.push(f.values);
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
      data.push(d.t.map(() => th.value));
    }
    const opts = {
      width: el.clientWidth || 600,
      height: d.height || 280,
      cursor: { drag: { x: true, y: false, setScale: false }, points: { size: 6 } },
      select: { show: true },
      legend: { live: true },
      scales: { x: { time: true }, y: { auto: true, range: (u, lo, hi) => [0, Math.max(hi * 1.1, d.floor || 0.1)] }, late: { auto: true, range: (u, lo, hi) => [0, Math.max(hi, 1) * 3] } },
      axes: [
        axis,
        { ...axis, label: d.unit === "%" ? "% of latency" : "ms", labelSize: 14, size: 52,
          values: (u, v) => v.map((x) => (d.unit === "%" ? x.toFixed(0) : x.toFixed(2))) },
        { ...axis, scale: "late", side: 1, grid: { show: false }, size: 36, show: !!d.late, values: (u, v) => v.map((x) => (Number.isInteger(x) ? x : "")) },
      ],
      series,
      hooks: {
        setSelect: [
          (u) => {
            if (u.select.width < 4) return;
            const from = Math.floor(u.posToVal(u.select.left, "x"));
            const to = Math.ceil(u.posToVal(u.select.left + u.select.width, "x"));
            u.setSelect({ left: 0, width: 0 }, false);
            el.dispatchEvent(new CustomEvent("dc-zoom", { detail: { from, to } }));
          },
        ],
      },
    };
    const u = new uPlot(opts, data, el);
    u.over.addEventListener("dblclick", () => el.dispatchEvent(new CustomEvent("dc-unzoom")));
    return u;
  }

  window.dcChart = {
    latency(el, json) {
      const d = JSON.parse(json);
      // The series set changed (flows came or went): build anew.
      // Labels, latency lines and the floor live in the chart's options: a
      // change to any of them builds it anew (a flow's latency changed).
      const shape = [d.unit, d.flows.map((f) => f.label).join("|"), d.thresholds.map((th) => th.label + "=" + th.value).join("|"), !!d.late, d.floor].join("#");
      if (el._dc && el._dcShape === shape) {
        const data = [d.t];
        if (d.late) data.push(d.late.map((v) => (v ? v : null)));
        data.push(...d.flows.map((f) => f.values), ...d.thresholds.map((th) => d.t.map(() => th.value)));
        el._dc.setData(data);
        return;
      }
      if (el._dc) el._dc.destroy();
      el.textContent = "";
      el._dc = build(el, d);
      el._dcShape = shape;
      if (!el._dcResize) {
        el._dcResize = new ResizeObserver(() => {
          if (el._dc && el.clientWidth > 0) el._dc.setSize({ width: el.clientWidth, height: el._dc.height });
        });
        el._dcResize.observe(el);
      }
    },
  };
})();
