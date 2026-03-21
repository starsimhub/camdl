/**
 * MapPanel — geographic choropleth for patch-stratified models.
 *
 * Shows median value of a selected compartment type per patch, colored on a
 * sequential scale.  Supports two display modes:
 *
 *   1. Grid mode (default): flat colored squares, one per patch.  Works without
 *      any geographic data — useful for development and quick inspection.
 *
 *   2. Map mode: Leaflet choropleth with user-loaded GeoJSON.  Features must
 *      have a `patch_index` integer property, or are matched by array index.
 *
 * In both modes: time slider, metric/scenario selectors, and click-to-chart
 * for per-patch ensemble trajectory.
 */

import { useState, useMemo, useCallback, useRef } from 'react';
import 'leaflet/dist/leaflet.css';
import { MapContainer, TileLayer, GeoJSON as LeafletGeoJSON } from 'react-leaflet';
import type L from 'leaflet';
import { LineChart, Line, XAxis, YAxis, Tooltip, ResponsiveContainer } from 'recharts';
import type { FeatureCollection, Feature } from 'geojson';
import { useStore } from '../store';
import { detectPatches, allPatchMedians, patchTimeSeries } from '../lib/patchStats';

// ── Color scale (YlOrRd) ──────────────────────────────────────────────────────

function choroplethColor(t: number): string {
  // 0 → #ffffb2, 0.5 → #fd8d3c, 1 → #bd0026
  const clamp = Math.max(0, Math.min(1, t));
  if (clamp < 0.5) {
    const s = clamp * 2;
    const r = Math.round(255 + s * (253 - 255));
    const g = Math.round(255 + s * (141 - 255));
    const b = Math.round(178 + s * (60 - 178));
    return `rgb(${r},${g},${b})`;
  } else {
    const s = (clamp - 0.5) * 2;
    const r = Math.round(253 + s * (189 - 253));
    const g = Math.round(141 + s * (0 - 141));
    const b = Math.round(60 + s * (38 - 60));
    return `rgb(${r},${g},${b})`;
  }
}

// ── Patch detail chart ────────────────────────────────────────────────────────

interface PatchChartProps {
  patchIdx: number;
  compType: string;
  scenarioName: string;
  color: string;
  data: { t: number; median: number; [key: string]: number }[];
  onClose: () => void;
}

function PatchChart({ patchIdx, compType, scenarioName, color, data, onClose }: PatchChartProps) {
  const seedKeys = Object.keys(data[0] ?? {}).filter((k) => k.startsWith('seed_'));

  return (
    <div className="absolute right-3 top-3 z-[1000] bg-white dark:bg-surface-1 border border-gray-200 dark:border-surface-border rounded-lg shadow-xl p-3 w-64">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs font-semibold text-gray-700 dark:text-gray-200">
          Patch {patchIdx} — {compType}
        </span>
        <button
          onClick={onClose}
          className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 text-xs"
        >
          ✕
        </button>
      </div>
      <div className="text-xs text-gray-500 dark:text-gray-400 mb-2">{scenarioName}</div>
      <ResponsiveContainer width="100%" height={110}>
        <LineChart data={data} margin={{ top: 2, right: 4, bottom: 2, left: 4 }}>
          <XAxis dataKey="t" tick={{ fontSize: 9 }} tickLine={false} axisLine={false} />
          <YAxis tick={{ fontSize: 9 }} tickLine={false} axisLine={false} width={28} />
          <Tooltip
            contentStyle={{ fontSize: 10, padding: '4px 8px' }}
            formatter={(v: number) => [v.toFixed(0), compType]}
          />
          {seedKeys.map((k) => (
            <Line
              key={k}
              type="monotone"
              dataKey={k}
              stroke={color}
              strokeOpacity={0.25}
              strokeWidth={1}
              dot={false}
              isAnimationActive={false}
              legendType="none"
              name=""
            />
          ))}
          <Line
            type="monotone"
            dataKey="median"
            stroke={color}
            strokeWidth={2}
            dot={false}
            isAnimationActive={false}
            name="median"
          />
        </LineChart>
      </ResponsiveContainer>
    </div>
  );
}

// ── Patch grid (no GeoJSON) ───────────────────────────────────────────────────

interface PatchGridProps {
  patchIndices: number[];
  values: number[];
  minV: number;
  maxV: number;
  selectedPatch: number | null;
  onSelect: (p: number) => void;
}

function PatchGrid({ patchIndices, values, minV, maxV, selectedPatch, onSelect }: PatchGridProps) {
  const cols = Math.max(4, Math.ceil(Math.sqrt(patchIndices.length)));
  return (
    <div
      className="overflow-auto flex-1 p-3"
      style={{ display: 'flex', alignItems: 'flex-start' }}
    >
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: `repeat(${cols}, 1fr)`,
          gap: 3,
          width: '100%',
        }}
      >
        {patchIndices.map((p, i) => {
          const v = values[i] ?? 0;
          const t = maxV > minV ? (v - minV) / (maxV - minV) : 0;
          const bg = choroplethColor(t);
          const isSelected = p === selectedPatch;
          return (
            <div
              key={p}
              title={`Patch ${p}: ${v.toFixed(0)}`}
              onClick={() => onSelect(p)}
              style={{
                backgroundColor: bg,
                aspectRatio: '1',
                borderRadius: 2,
                cursor: 'pointer',
                border: isSelected ? '2px solid #1a1a1a' : '1px solid rgba(0,0,0,0.1)',
                minWidth: 0,
              }}
            />
          );
        })}
      </div>
    </div>
  );
}

// ── GeoJSON map ───────────────────────────────────────────────────────────────

interface GeoMapProps {
  geoJson: FeatureCollection;
  patchIndices: number[];
  values: number[];
  minV: number;
  maxV: number;
  selectedPatch: number | null;
  onSelect: (p: number) => void;
}

function GeoMap({ geoJson, patchIndices, values, minV, maxV, selectedPatch, onSelect }: GeoMapProps) {
  // Build a lookup: patch index → color
  const colorByPatch = useMemo(() => {
    const m = new Map<number, string>();
    patchIndices.forEach((p, i) => {
      const v = values[i] ?? 0;
      const t = maxV > minV ? (v - minV) / (maxV - minV) : 0;
      m.set(p, choroplethColor(t));
    });
    return m;
  }, [patchIndices, values, minV, maxV]);

  const featurePatchIndex = useCallback(
    (feature: Feature, arrayIdx: number): number => {
      const pi = (feature.properties as Record<string, unknown>)?.patch_index;
      if (typeof pi === 'number') return pi;
      return arrayIdx;
    },
    [],
  );

  const style = useCallback(
    (feature?: Feature): L.PathOptions => {
      if (!feature) return {};
      const idx = geoJson.features.indexOf(feature);
      const pi = featurePatchIndex(feature, idx);
      const color = colorByPatch.get(pi) ?? '#e5e7eb';
      const isSelected = pi === selectedPatch;
      return {
        fillColor: color,
        fillOpacity: 0.75,
        weight: isSelected ? 2 : 0.5,
        color: isSelected ? '#1a1a1a' : '#666',
      };
    },
    [colorByPatch, featurePatchIndex, selectedPatch, geoJson],
  );

  const onEachFeature = useCallback(
    (feature: Feature, layer: L.Layer) => {
      const idx = geoJson.features.indexOf(feature);
      const pi = featurePatchIndex(feature, idx);
      const name =
        (feature.properties as Record<string, string>)?.name ??
        (feature.properties as Record<string, string>)?.lga_name ??
        `Patch ${pi}`;
      (layer as L.Path).bindTooltip(name, { sticky: true, className: 'text-xs' });
      layer.on('click', () => onSelect(pi));
    },
    [geoJson, featurePatchIndex, onSelect],
  );

  // Use key based on values to force GeoJSON re-render on data change
  const geoKey = useMemo(() => values.join(',') + (selectedPatch ?? ''), [values, selectedPatch]);

  return (
    <MapContainer
      style={{ width: '100%', height: '100%' }}
      center={[10, 8]}
      zoom={6}
      scrollWheelZoom={true}
    >
      <TileLayer
        url="https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png"
        attribution='© <a href="https://openstreetmap.org">OSM</a>'
        opacity={0.4}
      />
      <LeafletGeoJSON
        key={geoKey}
        data={geoJson}
        style={style}
        onEachFeature={onEachFeature}
      />
    </MapContainer>
  );
}

// ── Color legend ──────────────────────────────────────────────────────────────

function ColorLegend({ minV, maxV }: { minV: number; maxV: number }) {
  const stops = Array.from({ length: 5 }, (_, i) => i / 4);
  return (
    <div className="flex items-center gap-2 mt-2">
      <span className="text-xs text-gray-500 dark:text-gray-400">{minV.toFixed(0)}</span>
      <div
        className="flex-1 h-2.5 rounded"
        style={{
          background: `linear-gradient(to right, ${stops.map((t) => choroplethColor(t)).join(', ')})`,
        }}
      />
      <span className="text-xs text-gray-500 dark:text-gray-400">{maxV.toFixed(0)}</span>
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────────────────

export default function MapPanel() {
  const scenarios = useStore((s) => s.scenarios);
  const ir = useStore((s) => s.ir);

  const [scenarioIdx, setScenarioIdx] = useState(0);
  const sc = scenarios[Math.min(scenarioIdx, scenarios.length - 1)];
  const firstTraj = sc?.runs[0]?.trajectory;
  const patchInfo = useMemo(
    () => (firstTraj ? detectPatches(firstTraj, ir) : null),
    [firstTraj, ir],
  );

  const [compType, setCompType] = useState(() => patchInfo?.compTypes[0] ?? 'I');
  const effectiveCompType = patchInfo?.compTypes.includes(compType)
    ? compType
    : (patchInfo?.compTypes[0] ?? 'I');

  const maxSnap = (patchInfo?.nSnapshots ?? 1) - 1;
  const [snapIdx, setSnapIdx] = useState(0);
  const effectiveSnap = Math.min(snapIdx, maxSnap);

  const remoteGeo = useStore((s) => s.remoteGeo);
  const [geoJson, setGeoJson] = useState<FeatureCollection | null>(null);
  const effectiveGeo = geoJson ?? remoteGeo;
  const [selectedPatch, setSelectedPatch] = useState<number | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Compute median values for all patches at current time
  const values = useMemo(() => {
    if (!sc || !patchInfo) return [];
    return allPatchMedians(sc, patchInfo.indices, effectiveCompType, effectiveSnap, patchInfo.names);
  }, [sc, patchInfo, effectiveCompType, effectiveSnap]);

  const minV = useMemo(() => Math.min(0, ...values), [values]);
  const maxV = useMemo(() => Math.max(1, ...values), [values]);

  // Per-patch time series for chart
  const chartData = useMemo(() => {
    if (selectedPatch === null || !sc || !patchInfo) return [];
    return patchTimeSeries(sc, effectiveCompType, selectedPatch, patchInfo.names);
  }, [selectedPatch, sc, effectiveCompType, patchInfo]);

  const handleLoadGeoJson = () => fileInputRef.current?.click();

  const handleFileChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    file.text().then((text) => {
      try {
        const parsed = JSON.parse(text) as FeatureCollection;
        setGeoJson(parsed);
      } catch {
        alert('Could not parse GeoJSON file.');
      }
    });
    e.target.value = '';
  };

  if (!patchInfo || scenarios.every((s) => s.runs.length === 0)) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-2 text-gray-500 text-sm">
        <span className="dark:text-gray-600">Map view requires a patch-stratified model</span>
        <span className="text-xs text-gray-400 dark:text-gray-700">
          Add a <code className="font-mono">patch</code> dimension or use the{' '}
          <code className="font-mono">_p{'{N}'}</code> suffix convention
        </span>
      </div>
    );
  }

  const tCurrent = patchInfo.tValues[effectiveSnap];

  return (
    <div className="flex flex-col h-full relative">
      {/* ── Controls bar ────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-gray-200 dark:border-surface-border bg-white dark:bg-surface-0 flex-shrink-0 flex-wrap">
        {/* Scenario */}
        <select
          value={scenarioIdx}
          onChange={(e) => setScenarioIdx(Number(e.target.value))}
          className="text-xs bg-gray-100 dark:bg-surface-2 border border-gray-200 dark:border-surface-border rounded px-1.5 py-0.5 text-gray-700 dark:text-gray-300 focus:outline-none"
        >
          {scenarios.map((s, i) => (
            <option key={s.id} value={i}>
              {s.name}
            </option>
          ))}
        </select>

        {/* Compartment type */}
        <select
          value={effectiveCompType}
          onChange={(e) => setCompType(e.target.value)}
          className="text-xs bg-gray-100 dark:bg-surface-2 border border-gray-200 dark:border-surface-border rounded px-1.5 py-0.5 text-gray-700 dark:text-gray-300 focus:outline-none"
        >
          {patchInfo.compTypes.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>

        {/* Time display */}
        <span className="text-xs text-gray-500 dark:text-gray-400">
          t = {tCurrent?.toFixed(1) ?? '—'}
        </span>

        <div className="flex-1" />

        {/* Load GeoJSON */}
        <button
          onClick={handleLoadGeoJson}
          className="text-xs px-2 py-0.5 border border-gray-200 dark:border-surface-border rounded text-gray-600 dark:text-gray-400 hover:border-gray-400 transition-colors"
        >
          {geoJson
            ? `GeoJSON loaded (${geoJson.features.length} features)`
            : remoteGeo
              ? `GeoJSON from server (${remoteGeo.features.length} features)`
              : 'Load GeoJSON…'}
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept=".geojson,.json"
          className="hidden"
          onChange={handleFileChange}
        />
      </div>

      {/* ── Time slider ─────────────────────────────────────────────────────── */}
      <div className="px-3 py-1.5 flex items-center gap-2 flex-shrink-0 border-b border-gray-100 dark:border-surface-border bg-white dark:bg-surface-0">
        <span className="text-xs text-gray-400 dark:text-gray-600 w-5">t₀</span>
        <input
          type="range"
          min={0}
          max={maxSnap}
          value={effectiveSnap}
          onChange={(e) => setSnapIdx(Number(e.target.value))}
          className="flex-1 accent-accent h-1.5 cursor-pointer"
        />
        <span className="text-xs text-gray-400 dark:text-gray-600 w-5 text-right">t₁</span>
        <ColorLegend minV={minV} maxV={maxV} />
      </div>

      {/* ── Main display ────────────────────────────────────────────────────── */}
      <div className="flex-1 min-h-0 relative">
        {effectiveGeo ? (
          <GeoMap
            geoJson={effectiveGeo}
            patchIndices={patchInfo.indices}
            values={values}
            minV={minV}
            maxV={maxV}
            selectedPatch={selectedPatch}
            onSelect={setSelectedPatch}
          />
        ) : (
          <div className="flex flex-col h-full">
            <div className="text-xs text-gray-400 dark:text-gray-600 px-3 pt-1 pb-0.5">
              Grid view — {patchInfo.indices.length} patches (load GeoJSON for geographic map)
            </div>
            <PatchGrid
              patchIndices={patchInfo.indices}
              values={values}
              minV={minV}
              maxV={maxV}
              selectedPatch={selectedPatch}
              onSelect={setSelectedPatch}
            />
          </div>
        )}

        {/* Patch chart overlay */}
        {selectedPatch !== null && chartData.length > 0 && sc && (
          <PatchChart
            patchIdx={selectedPatch}
            compType={effectiveCompType}
            scenarioName={sc.name}
            color={sc.color}
            data={chartData}
            onClose={() => setSelectedPatch(null)}
          />
        )}
      </div>
    </div>
  );
}
