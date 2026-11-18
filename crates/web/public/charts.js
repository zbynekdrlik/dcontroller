// dcontroller's charts: a thin layer over uPlot (vendor/uplot, MIT). The
// Leptos app calls window.dcChart.latency(el, json) with a chart's data; this
// file keeps the charts in that element, updates them in place, follows the
// element's width, and reports a drag-zoom as a "dc-zoom" event on the
// element ({from, to} in unix seconds) so the app can fetch finer data.
// Double-click asks for the app's range again ("dc-unzoom").
//
// One series per source device: the typical peak against the latency. A
// bucket with late packets (they came after their flow's latency and were
// not played) is a red bar in a band at the bottom of the plot, its height
// by its count against the range's most (#120: full-height lines in the
// lines' colours made a wall of them). The legend gives each source's
// level and late packets under the cursor.
"use strict";
(() => {
  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  // Times read in Slovak format and Slovak time, wherever the browser is.
  const ZONE = "Europe/Bratislava";
  const format = (opts) => new Intl.DateTimeFormat("sk-SK", { timeZone: ZONE, ...opts });
  const clock = { hour: "2-digit", minute: "2-digit", hourCycle: "h23" };
  const fmtMinutes = format(clock);
  const fmtSeconds = format({ ...clock, second: "2-digit" });
  const fmtDay = format({ day: "numeric", month: "numeric" });
  const fmtDate = format({ day: "numeric", month: "numeric", year: "numeric" });
  const when = (fmt, t) => fmt.format(new Date(t * 1000));

  // An axis tick: the date for a day's step or at midnight, else the clock.
  function tick(t, step) {
    if (step >= 86400) return when(fmtDay, t);
    if (step < 60) return when(fmtSeconds, t);
    const text = when(fmtMinutes, t);
    return text === "00:00" ? when(fmtDay, t) : text;
  }

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

  // The late band: the bottom fifth of the plot.
  const BAND = 0.2;

  function drawLate(u, el, d) {
    const marks = lateMarks(u, d);
    el.dataset.lateMarks = String(marks.length);
    const { left, top, width, height } = u.bbox;
    const px = uPlot.pxRatio;
    const ctx = u.ctx;
    const most = marks.reduce((m, k) => Math.max(m, k.total), 0);
    el.dataset.lateMost = String(most);
    if (!most) return;
    const band = height * BAND;
    const bottom = top + height;
    const red = css("--bad") || "#f87171";
    ctx.save();
    ctx.setLineDash([]);
    // The band's floor and its name, so the bars read as a count, not a level.
    ctx.globalAlpha = 0.35;
    ctx.strokeStyle = red;
    ctx.lineWidth = 1 * px;
    ctx.beginPath();
    ctx.moveTo(left, bottom - band);
    ctx.lineTo(left + width, bottom - band);
    ctx.stroke();
    ctx.globalAlpha = 1;
    ctx.fillStyle = red;
    ctx.font = `600 ${11 * px}px system-ui, sans-serif`;
    ctx.textAlign = "left";
    ctx.textBaseline = "bottom";
    ctx.fillText(`${d.lateName || "late packets"}, most ${most}`, left + 4 * px, bottom - band - 2 * px);
    // One bar per bucket, at least 2 px wide, at most the bucket's width.
    const step = d.t.length > 1 ? Math.abs(u.valToPos(d.t[1], "x", true) - u.valToPos(d.t[0], "x", true)) : 4 * px;
    const w = Math.max(2 * px, Math.min(step * 0.8, 8 * px));
    for (const m of marks) {
      const x = u.valToPos(m.t, "x", true);
      if (x < left || x > left + width) continue;
      const h = Math.max(2 * px, band * Math.sqrt(m.total / most));
      ctx.fillRect(Math.round(x - w / 2), Math.round(bottom - h), Math.round(w), Math.round(h));
    }
    ctx.restore();
  }

  // Each dashed latency line says on the chart what it is: its value and
  // the sources whose flows run at it (the legend alone did not, #105).
  function drawThresholds(u, el, d) {
    const { left, top, width } = u.bbox;
    const px = uPlot.pxRatio;
    const ctx = u.ctx;
    ctx.save();
    ctx.setLineDash([]);
    ctx.font = `600 ${11 * px}px system-ui, sans-serif`;
    ctx.textAlign = "right";
    ctx.textBaseline = "bottom";
    let drawn = 0;
    for (const th of d.thresholds) {
      const y = Math.round(u.valToPos(th.value, "y", true));
      if (y < top) continue;
      ctx.fillStyle = th.color;
      ctx.fillText(th.label, left + width - 4 * px, y - 3 * px);
      drawn++;
    }
    el.dataset.thresholdLabels = String(drawn);
    ctx.restore();
  }

  function build(el, d, width) {
    const fmt = (v) => (v == null ? "–" : `${v.toFixed(2)} ms`);
    const axis = axisStyle();
    const series = [{ value: (_, t) => (t == null ? "–" : `${when(fmtDate, t)} ${when(fmtSeconds, t)}`) }];
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
      // Near a line (or over its legend entry) that line stands out, the rest dim.
      cursor: { drag: { x: true, y: false, setScale: false }, points: { size: 6 }, focus: { prox: 16 } },
      focus: { alpha: 0.25 },
      select: { show: true },
      legend: { live: true },
      scales: { x: { time: true }, y: { auto: true, range: (u, lo, hi) => [0, Math.max(hi * 1.1, d.floor || 0.1)] } },
      tzDate: (t) => uPlot.tzDate(new Date(t * 1000), ZONE),
      axes: [
        { ...axis, values: (u, splits, _, __, step) => splits.map((t) => tick(t, step)) },
        { ...axis, label: d.axis || "typical peak, ms", labelSize: 14, size: 52, values: (u, v) => v.map((x) => x.toFixed(2)) },
      ],
      series,
      hooks: { ...zoomHooks(el), draw: [(u) => drawLate(u, el, el._dcData), (u) => drawThresholds(u, el, el._dcData)] },
    };
    return new uPlot(opts, chartData(d), el);
  }

  window.dcChart = {
    latency(el, json) {
      const d = JSON.parse(json);
      el._dcData = d;
      // Labels, colours, latency lines and the floor live in the chart's
      // options: a change to any of them builds it anew.
      const shape = [d.unit, d.sources.map((s) => s.label + "=" + s.color).join("|"), d.thresholds.map((th) => th.label + "=" + th.value).join("|"), d.floor].join("#");
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
