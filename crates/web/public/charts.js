// dcontroller's charts: a thin layer over uPlot (vendor/uplot, MIT). The
// Leptos app calls window.dcChart.latency(el, json) with a chart's data; this
// file keeps the charts in that element, updates them in place, follows the
// element's width, and reports a drag-zoom as a "dc-zoom" event on the
// element ({from, to} in unix seconds) so the app can fetch finer data.
// Double-click asks for the app's range again ("dc-unzoom").
//
// One series per source device: the typical peak against the latency. A
// bucket with late packets (they came after their flow's latency and were
// not played) is a vertical line at that time, in the colour of the source
// with the most of them, with the count at its top. The legend gives each
// source's level and late packets under the cursor.
"use strict";
(() => {
  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
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

  function chartData(d) {
    return [d.t, ...d.sources.map((s) => s.values), ...d.thresholds.map((th) => d.t.map(() => th.value))];
  }

  // Per bucket with late packets, among the sources showing: its total and
  // the colour of the source with the most.
  function lateMarks(u, d) {
    const marks = [];
    d.t.forEach((t, i) => {
      let total = 0;
      let most = 0;
      let color = "";
      d.sources.forEach((s, k) => {
        const n = s.late[i] || 0;
        if (!n || !u.series[k + 1].show) return;
        total += n;
        if (n > most) {
          most = n;
          color = s.color;
        }
      });
      if (total) marks.push({ t, total, color });
    });
    return marks;
  }

  function drawLate(u, el, d) {
    const marks = lateMarks(u, d);
    el.dataset.lateMarks = String(marks.length);
    const { left, top, width, height } = u.bbox;
    const px = uPlot.pxRatio;
    const ctx = u.ctx;
    ctx.save();
    ctx.lineWidth = 1.5 * px;
    ctx.font = `600 ${11 * px}px system-ui, sans-serif`;
    // uPlot leaves the last series' dash and the axes' alignment on the context.
    ctx.setLineDash([]);
    ctx.textAlign = "left";
    ctx.textBaseline = "top";
    let lastLabel = -Infinity;
    for (const m of marks) {
      const x = Math.round(u.valToPos(m.t, "x", true));
      if (x < left || x > left + width) continue;
      ctx.strokeStyle = m.color;
      ctx.beginPath();
      ctx.moveTo(x, top);
      ctx.lineTo(x, top + height);
      ctx.stroke();
      // Close marks share the first one's label room.
      if (x - lastLabel >= 32 * px) {
        ctx.fillStyle = m.color;
        ctx.fillText(String(m.total), x + 3 * px, top + 2 * px);
        lastLabel = x;
      }
    }
    ctx.restore();
  }

  function build(el, d, width) {
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
        value: (_, v, __, i) => fmt(v) + (i != null && s.late[i] ? ` · ${s.late[i]} late` : ""),
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
      cursor: { drag: { x: true, y: false, setScale: false }, points: { size: 6 } },
      select: { show: true },
      legend: { live: true },
      scales: { x: { time: true }, y: { auto: true, range: (u, lo, hi) => [0, Math.max(hi * 1.1, d.floor || 0.1)] } },
      axes: [
        axis,
        { ...axis, label: d.unit === "%" ? "typical peak, % of latency" : "typical peak, ms", labelSize: 14, size: 52,
          values: (u, v) => v.map((x) => (d.unit === "%" ? x.toFixed(0) : x.toFixed(2))) },
      ],
      series,
      hooks: { ...zoomHooks(el), draw: [(u) => drawLate(u, el, el._dcData)] },
    };
    return new uPlot(opts, chartData(d), el);
  }

  window.dcChart = {
    latency(el, json) {
      const d = JSON.parse(json);
      el._dcData = d;
      // Labels, latency lines and the floor live in the chart's options: a
      // change to any of them builds it anew.
      const shape = [d.unit, d.sources.map((s) => s.label).join("|"), d.thresholds.map((th) => th.label + "=" + th.value).join("|"), d.floor].join("#");
      if (el._dc && el._dcShape === shape) {
        el._dc.setData(chartData(d));
        return;
      }
      if (el._dc) el._dc.destroy();
      el.textContent = "";
      const u = build(el, d, el.clientWidth || 600);
      u.over.addEventListener("dblclick", () => el.dispatchEvent(new CustomEvent("dc-unzoom")));
      el._dc = u;
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
