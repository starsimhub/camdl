import { useStore } from '../store';
import type { RunConfig } from '../types/experiment';

export default function RunConfigPanel() {
  const ir = useStore((s) => s.ir);
  const runConfig = useStore((s) => s.runConfig);
  const experimentStatus = useStore((s) => s.experimentStatus);
  const scenarios = useStore((s) => s.scenarios);
  const setRunConfig = useStore((s) => s.setRunConfig);
  const runExperiment = useStore((s) => s.runExperiment);
  const stopExperiment = useStore((s) => s.stopExperiment);

  const isRunning = experimentStatus === 'running';

  const inputCls = 'text-xs rounded px-1.5 py-0.5 focus:outline-none bg-white border border-gray-200 text-gray-700 dark:bg-surface-2 dark:border-surface-border dark:text-gray-300';

  return (
    <div className="flex flex-col h-full overflow-hidden bg-gray-50 border-r border-gray-200 dark:bg-surface-1 dark:border-surface-border">

      {/* ── Run config ────────────────────────────────────────────────────────── */}
      <div className="px-3 pt-3 pb-2 border-b border-gray-200 flex-shrink-0 dark:border-surface-border">
        <div className="text-xs text-gray-500 font-medium mb-2 uppercase tracking-wider">Run config</div>
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2">
            <span className="text-xs text-gray-500 w-16">Backend</span>
            <select
              value={runConfig.backend}
              onChange={(e) => setRunConfig({ backend: e.target.value as RunConfig['backend'] })}
              className={`flex-1 ${inputCls}`}
            >
              <option value="gillespie">Gillespie</option>
              <option value="tau_leap">Tau-leap</option>
              <option value="chain_binomial">Chain-binomial</option>
            </select>
          </div>

          {(runConfig.backend === 'tau_leap' || runConfig.backend === 'chain_binomial') && (
            <div className="flex items-center gap-2">
              <span className="text-xs text-gray-500 w-16">dt</span>
              <input
                type="number"
                value={runConfig.dt ?? 1}
                onChange={(e) => setRunConfig({ dt: parseFloat(e.target.value) })}
                className={`w-20 ${inputCls}`}
                step="0.1" min="0.01"
              />
            </div>
          )}

          <div className="flex items-center gap-2">
            <span className="text-xs text-gray-500 w-16">Seeds</span>
            <input
              type="number"
              value={runConfig.nSeeds}
              onChange={(e) => setRunConfig({ nSeeds: Math.max(1, parseInt(e.target.value) || 1) })}
              className={`w-16 ${inputCls}`}
              min="1"
            />
          </div>

          <div className="flex items-center gap-2">
            <span className="text-xs text-gray-500 w-16">Base seed</span>
            <input
              type="number"
              value={runConfig.baseSeed}
              onChange={(e) => setRunConfig({ baseSeed: parseInt(e.target.value) || 0 })}
              className={`w-16 ${inputCls}`}
              min="0"
            />
          </div>

          <div className="flex items-center gap-2">
            <span className="text-xs text-gray-500 w-16">t end</span>
            <input
              type="number"
              value={runConfig.tEnd ?? ''}
              placeholder="from model"
              onChange={(e) => {
                const v = parseFloat(e.target.value);
                setRunConfig({ tEnd: isNaN(v) ? undefined : v });
              }}
              className={`w-20 ${inputCls} placeholder-gray-400 dark:placeholder-gray-600`}
              min="0"
            />
          </div>
        </div>
      </div>

      {/* ── Run button ────────────────────────────────────────────────────────── */}
      <div className="px-3 py-3 flex-shrink-0">
        <button
          onClick={isRunning ? stopExperiment : runExperiment}
          disabled={!ir}
          className={`w-full py-1.5 text-xs rounded font-semibold transition-colors disabled:opacity-40 disabled:cursor-not-allowed ${
            isRunning
              ? 'bg-red-500/80 text-white hover:bg-red-600'
              : 'bg-accent text-white hover:bg-accent-dim'
          }`}
        >
          {isRunning ? '■ Stop' : `▶ Run All  (${runConfig.nSeeds} seed${runConfig.nSeeds !== 1 ? 's' : ''})`}
        </button>
      </div>

      {/* ── Per-scenario progress ─────────────────────────────────────────────── */}
      {scenarios.some((s) => s.seedsCompleted > 0) && (
        <div className="px-3 pb-2 flex flex-col gap-1 flex-shrink-0">
          {scenarios.map((sc) => (
            sc.seedsCompleted > 0 ? (
              <div key={sc.id} className="flex items-center gap-1.5">
                <div className="w-2 h-2 rounded-full flex-shrink-0" style={{ backgroundColor: sc.color }} />
                <div className="flex-1 h-0.5 bg-gray-200 rounded dark:bg-surface-2">
                  <div
                    className="h-full rounded transition-all"
                    style={{
                      width: `${(sc.seedsCompleted / runConfig.nSeeds) * 100}%`,
                      backgroundColor: sc.color,
                    }}
                  />
                </div>
                <span className="text-xs text-gray-500 dark:text-gray-600">{sc.seedsCompleted}</span>
              </div>
            ) : null
          ))}
        </div>
      )}

      {/* ── Errors ───────────────────────────────────────────────────────────── */}
      {scenarios.some((s) => s.status === 'error') && (
        <div className="px-3 pb-2">
          {scenarios.filter((s) => s.status === 'error').map((sc) => (
            <div key={sc.id} className="text-xs text-red-500 mt-1 dark:text-red-400">
              {sc.name}: {sc.error}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
