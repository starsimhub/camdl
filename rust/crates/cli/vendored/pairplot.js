// camdl survey pair-plot renderer.
//
// Reads:
//   <script type="application/json" id="landscape-data">  // TSV → JSON
//   <script type="application/json" id="landscape-bounds"> // [(lo,hi), ...]
//
// Visual style ported from
// camdl-book/.claude/worktrees/typhoid/_lib/identifiability.py
// (pairplot function): three-layer diagonal histograms, viridis_r
// off-diagonal scatter colored by |loglik - loglik_max| on a log
// scale, top-5 red stars.

(function () {
  "use strict";

  const COLORS = {
    GREY_BG:   "#e0e0e0",
    GREY_HIST: "#cccccc",
    TOP10_GREEN: "#16a085",
    TOP1_RED:    "#c0392b",
    REF_BLACK: "#000000",
  };

  const dataNode = document.getElementById("landscape-data");
  const boundsNode = document.getElementById("landscape-bounds");
  if (!dataNode || !boundsNode) {
    console.error("camdl survey: missing landscape data block");
    return;
  }
  const D = JSON.parse(dataNode.textContent);
  const declaredBounds = JSON.parse(boundsNode.textContent);

  const params = D.estimated;
  const nParams = params.length;
  const rows = D.rows.filter((r) => Number.isFinite(r.loglik));
  if (!rows.length) {
    document.getElementById("plot").innerText =
      "no finite-loglik rows in landscape.tsv — nothing to plot";
    return;
  }
  // Pick log axis when the declared bounds span > 2 decades
  // (matches the Python prototype's heuristic).
  const logAxis = params.map((_, i) => {
    const [lo, hi] = declaredBounds[i] || [NaN, NaN];
    return Number.isFinite(lo) && Number.isFinite(hi)
      && lo > 0 && hi / lo > 100;
  });

  // Top-K cutoff — the slider drives this; default 10%.
  const slider = document.getElementById("top-pct");
  const sliderDisplay = document.getElementById("top-pct-display");
  const colorBy = document.getElementById("color-by");

  // Hide the mean_ess option when not present.
  if (!D.has_mean_ess) {
    const opt = colorBy.querySelector('option[value="mean_ess"]');
    if (opt) opt.disabled = true;
  }

  // gh#46: disable loglik_se when the column carries no variation.
  // For `--eval simulate` the SE column is structurally zero (one
  // deterministic ODE solve, no replicate variance), so without
  // this guard the linear-normalisation fallback maps every point
  // to coord=0 → VIRIDIS_R[0] (yellow), which renders identically
  // to "every point is the optimum" — visually misleading. Same
  // pattern as mean_ess above; runs once at parse time, not on
  // every render.
  {
    const seVals = D.rows
      .map((r) => r.loglik_se)
      .filter(Number.isFinite);
    const seConstant = seVals.length === 0
      || (Math.max(...seVals) - Math.min(...seVals) < 1e-12);
    if (seConstant) {
      const opt = colorBy.querySelector('option[value="loglik_se"]');
      if (opt) {
        opt.disabled = true;
        opt.textContent = "loglik_se (no variation)";
      }
    }
  }

  function clamp01(x) { return Math.max(0, Math.min(1, x)); }

  // ── Color scaling helpers ────────────────────────────────────────
  // viridis_r palette (10 sample points, evenly spaced).
  const VIRIDIS_R = [
    "#fde725", "#b5de2b", "#6ece58", "#35b779", "#1f9e89",
    "#26828e", "#31688e", "#3e4989", "#482878", "#440154",
  ];
  function viridis(v) {
    const idx = Math.floor(clamp01(v) * (VIRIDIS_R.length - 1));
    return VIRIDIS_R[idx];
  }

  // Map a per-row scalar to a [0, 1] color coord. Normalisation is
  // computed over `displayedSet` only (the top-K subset that actually
  // gets colored on off-diagonals) so the full viridis range maps
  // onto the displayed points instead of being wasted on the gray-
  // and-not-displayed bottom of the distribution. Coords for rows
  // outside `displayedSet` are returned as 0 (unused — those rows
  // render as gray scatter, not viridis).
  //
  //  - `loglik` (gh#46 fix): rank-based normalisation over the
  //    displayed top-K. Best loglik → 0 (yellow), worst-of-top-K →
  //    1 (purple), interior linearly by rank. Distribution-free; the
  //    full viridis_r palette always exercises across the top-K
  //    regardless of loglik distribution shape.
  //
  //    The previous log-of-|loglik − loglik_max| scheme broke on
  //    long-tail surfaces with top-K spans of 4+ orders of magnitude:
  //    median-of-top-K mapped to ~0.93 of the palette (dark purple),
  //    so half the displayed set rendered visually identical and
  //    only points within ~5 nats of best got yellow. Rank-based
  //    sacrifices absolute-magnitude information to guarantee the
  //    diagnostic always shows useful colour variation across the
  //    displayed set, which is the right trade-off for an
  //    identifiability diagnostic — the question is the *ranking*
  //    of points across the box, not the absolute scale.
  //
  //  - other keys (`loglik_se`, `mean_ess`): linear over the top-K
  //    range. Constant columns (max == min) are detected upstream
  //    and the dropdown option is disabled, so the (hi <= lo)
  //    branch here is only hit on float-rounding edge cases at
  //    runtime — defensive fallback only.
  function colorCoord(rowsArr, displayedSet, key) {
    if (!displayedSet || displayedSet.size === 0) return rowsArr.map(() => 0);
    const idxs = [...displayedSet];
    if (key === "loglik") {
      // Build a list of (idx, loglik) for the displayed top-K and
      // sort descending. The position in this sorted list is the
      // rank coordinate.
      const topRows = idxs
        .map((k) => ({ k, ll: rowsArr[k].loglik }))
        .filter((o) => Number.isFinite(o.ll))
        .sort((a, b) => b.ll - a.ll);  // best first
      if (topRows.length < 2) return rowsArr.map(() => 0);
      const denom = topRows.length - 1;
      const rankCoord = new Map();
      topRows.forEach((o, r) => rankCoord.set(o.k, r / denom));
      return rowsArr.map((_, k) =>
        rankCoord.has(k) ? rankCoord.get(k) : 0);
    }
    // Linear normalisation over the displayed-set range.
    const topVals = idxs
      .map((k) => rowsArr[k][key])
      .filter(Number.isFinite);
    if (!topVals.length) return rowsArr.map(() => 0);
    const lo = Math.min(...topVals);
    const hi = Math.max(...topVals);
    if (hi <= lo) return rowsArr.map(() => 0);
    return rowsArr.map((r) => {
      const v = r[key];
      return Number.isFinite(v) ? clamp01((v - lo) / (hi - lo)) : 0;
    });
  }

  // ── Rebuild the pair-plot from the current top-K + color-by ─────
  function render() {
    const topPct = parseFloat(slider.value);
    sliderDisplay.textContent = (100.0 * topPct).toFixed(1) + "%";
    const colorKey = colorBy.value;

    // Sort indices by loglik desc, then split into:
    //  - bottom (1 - topPct) "gray"
    //  - middle topPct "green"  (top X%)
    //  - top 1% "red"           (always carved out; needed for the diagonals)
    //  - top 5 "stars"          (red stars on off-diagonals)
    const sortedIdx = [...rows.keys()].sort((a, b) => rows[b].loglik - rows[a].loglik);
    const nGreen = Math.max(1, Math.floor(rows.length * topPct));
    const nRed = Math.max(1, Math.floor(rows.length * 0.01));
    const greenIdx = new Set(sortedIdx.slice(0, nGreen));
    const redIdx = new Set(sortedIdx.slice(0, nRed));
    const starIdx = new Set(sortedIdx.slice(0, Math.min(5, rows.length)));

    const bandFor = (i) => {
      if (redIdx.has(i)) return "red";
      if (greenIdx.has(i)) return "green";
      return "gray";
    };

    // Color coords for the off-diagonal viridis layer. Normalised
    // over the displayed top-K (greenIdx) — not all rows — so the
    // full viridis range exercises across the colored points
    // regardless of whether the unselected tail is short or heavy.
    const cc = colorCoord(rows, greenIdx, colorKey);

    const traces = [];
    const layout = {
      grid: { rows: nParams, columns: nParams, pattern: "independent" },
      showlegend: false,
      hovermode: "closest",
      margin: { l: 60, r: 30, t: 30, b: 60 },
      paper_bgcolor: "#fff",
      plot_bgcolor: "#fff",
      annotations: [],
      // Diagonals are three histograms in subset relationship
      // (gray = all rows, green = top X%, red = top 1%). With
      // barmode "overlay" plus per-layer opacity, plotly draws
      // them on top of each other on a shared y-scale instead of
      // putting bars side-by-side per bin — matches the Python
      // prototype's `ax.hist(..., alpha=0.6)` layering.
      barmode: "overlay",
    };

    for (let i = 0; i < nParams; i++) {
      for (let j = 0; j < nParams; j++) {
        // Plotly grid + `pattern: "independent"` gives every subplot its
        // own (xaxis_N, yaxis_N) pair with the same N. Subplot 1 alone
        // uses the unsubscripted "x" / "y" / "xaxis" / "yaxis"; every
        // other subplot uses "xN" / "yN" / "xaxisN" / "yaxisN".
        //
        // The earlier conditioning on `j === 0` / `i === 0` (per-row /
        // per-column) silently routed every subplot in column 0 onto
        // shared default x-axis and every subplot in row 0 onto shared
        // default y-axis, which made traces in those panels render in
        // the figure's outer gutter strip. Symptom: scattered points
        // along the left edge spanning figure height + along the top
        // edge spanning figure width. The layout dict also clobbered
        // itself when multiple subplots wrote `layout["xaxis"] = ...`.
        const subplotId = i * nParams + j + 1;
        const isFirst = subplotId === 1;
        const xRef = isFirst ? "xaxis" : `xaxis${subplotId}`;
        const yRef = isFirst ? "yaxis" : `yaxis${subplotId}`;
        const xkey = isFirst ? "x" : `x${subplotId}`;
        const ykey = isFirst ? "y" : `y${subplotId}`;

        // Set axis layout for this subplot.
        layout[xRef] = {
          title: i === nParams - 1 ? params[j] : "",
          type: logAxis[j] ? "log" : "linear",
          showgrid: true, gridcolor: "#f0f0f0", zeroline: false,
        };
        layout[yRef] = {
          title: j === 0 ? params[i] : "",
          type: i === j ? "linear" : (logAxis[i] ? "log" : "linear"),
          showgrid: true, gridcolor: "#f0f0f0", zeroline: false,
        };

        if (i === j) {
          // ── Diagonal: three-layer histogram.
          const allVals = rows.map((r) => r.params[i]);
          // Bin edges: 40 bins across the declared bounds.
          const [lo, hi] = declaredBounds[i] || [
            Math.min(...allVals.filter(Number.isFinite)),
            Math.max(...allVals.filter(Number.isFinite)),
          ];
          const useLog = logAxis[i] && lo > 0;
          const nbins = 40;
          const edges = [];
          for (let b = 0; b <= nbins; b++) {
            const t = b / nbins;
            edges.push(useLog ? lo * Math.pow(hi / lo, t) : lo + t * (hi - lo));
          }
          // Three subset layers — each one is a strict subset of
          // the one below it, so at every bin red ≤ green ≤ gray.
          // With barmode=overlay + per-layer opacity, plotly draws
          // them stacked: gray bar = total height, green = subset
          // height visible against gray's translucency, red = top-1%
          // peak. Matches the Python prototype's matplotlib hist
          // layering exactly.
          const grayVals = rows.map((r) => r.params[i]);
          const greenVals = [], redVals = [];
          rows.forEach((r, k) => {
            const v = r.params[i];
            if (greenIdx.has(k)) greenVals.push(v);
            if (redIdx.has(k))   redVals.push(v);
          });
          // Plotly histograms with explicit xbins to share bins
          // across layers. The opacities mirror the Python
          // prototype (0.6 / 0.85 / 0.95) so gray reads as
          // background, green as foreground, red as peak marker.
          const histogram = (vals, color, opacity, name) => ({
            type: "histogram", x: vals,
            xbins: { start: edges[0], end: edges[edges.length - 1],
              size: useLog ? null : (edges[1] - edges[0]) },
            marker: { color },
            opacity, name,
            xaxis: xkey, yaxis: ykey,
          });
          traces.push(histogram(grayVals,  COLORS.GREY_HIST,   0.60, "all"));
          traces.push(histogram(greenVals, COLORS.TOP10_GREEN, 0.85, "top X%"));
          traces.push(histogram(redVals,   COLORS.TOP1_RED,    0.95, "top 1%"));
        } else {
          // ── Off-diagonal: gray scatter + top-K viridis + top-5 stars.
          const grayPts = { x: [], y: [], text: [] };
          const topPts  = { x: [], y: [], text: [], color: [] };
          const starPts = { x: [], y: [], text: [] };
          rows.forEach((r, k) => {
            const xv = r.params[j], yv = r.params[i];
            if (!Number.isFinite(xv) || !Number.isFinite(yv)) return;
            const tooltip = paramHover(r, params, j, i);
            if (starIdx.has(k)) {
              starPts.x.push(xv); starPts.y.push(yv); starPts.text.push(tooltip);
            }
            if (greenIdx.has(k) || redIdx.has(k)) {
              topPts.x.push(xv); topPts.y.push(yv); topPts.text.push(tooltip);
              topPts.color.push(viridis(cc[k]));
            } else {
              grayPts.x.push(xv); grayPts.y.push(yv); grayPts.text.push(tooltip);
            }
          });
          traces.push({
            type: "scattergl", mode: "markers",
            x: grayPts.x, y: grayPts.y, text: grayPts.text,
            marker: { color: COLORS.GREY_HIST, size: 3, opacity: 0.4,
              line: { width: 0 } },
            hoverinfo: "text",
            xaxis: xkey, yaxis: ykey, showlegend: false,
          });
          traces.push({
            type: "scattergl", mode: "markers",
            x: topPts.x, y: topPts.y, text: topPts.text,
            marker: { color: topPts.color, size: 5, opacity: 0.85,
              line: { color: "#000", width: 0.3 } },
            hoverinfo: "text",
            xaxis: xkey, yaxis: ykey, showlegend: false,
          });
          traces.push({
            type: "scattergl", mode: "markers",
            x: starPts.x, y: starPts.y, text: starPts.text,
            marker: { color: COLORS.TOP1_RED, size: 10, symbol: "star",
              line: { color: "#000", width: 1 } },
            hoverinfo: "text",
            xaxis: xkey, yaxis: ykey, showlegend: false,
          });
        }
      }
    }

    Plotly.react("plot", traces, layout, {
      responsive: true,
      displaylogo: false,
      modeBarButtonsToRemove: ["lasso2d", "autoScale2d"],
    });
  }

  function paramHover(r, paramNames, jx, iy) {
    const lines = [];
    paramNames.forEach((p, k) => {
      lines.push(`${p}: ${formatNum(r.params[k])}`);
    });
    lines.push(`loglik: ${formatNum(r.loglik)}`);
    if (Number.isFinite(r.loglik_se)) lines.push(`se: ${formatNum(r.loglik_se)}`);
    lines.push(`point_id: ${r.point_id}`);
    return lines.join("<br>");
  }
  function formatNum(v) {
    if (!Number.isFinite(v)) return "" + v;
    if (Math.abs(v) >= 1e5 || (Math.abs(v) > 0 && Math.abs(v) < 1e-3)) {
      return v.toExponential(3);
    }
    return v.toPrecision(4);
  }

  slider.addEventListener("input", render);
  colorBy.addEventListener("change", render);
  // Click on an axis title swaps log/linear for that parameter.
  document.getElementById("plot").addEventListener("plotly_clickannotation", () => render());
  // Cheap toggle: keyboard shortcut "L" toggles every axis.
  document.addEventListener("keydown", (e) => {
    if (e.key === "L" || e.key === "l") {
      for (let i = 0; i < nParams; i++) logAxis[i] = !logAxis[i];
      render();
    }
  });

  render();
})();
