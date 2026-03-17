import { useStore } from '../store';
import { EXAMPLES } from '../lib/examples';

export default function Header() {
  const modelName       = useStore((s) => s.modelName);
  const setModelName    = useStore((s) => s.setModelName);
  const compileStatus   = useStore((s) => s.compileStatus);
  const simStatus       = useStore((s) => s.simStatus);
  const openFile        = useStore((s) => s.openFile);
  const saveFile        = useStore((s) => s.saveFile);
  const runSimulation   = useStore((s) => s.runSimulation);
  const ir              = useStore((s) => s.ir);
  const anyRunning      = useStore((s) =>
    s.simStatus === 'running' || s.scenarios.some((sc) => sc.status === 'running')
  );
  const setDslSource      = useStore((s) => s.setDslSource);
  const resetOverrides    = useStore((s) => s.resetParamOverrides);
  const setParamOverride  = useStore((s) => s.setParamOverride);
  const setSimConfig      = useStore((s) => s.setSimConfig);
  const resetRun          = useStore((s) => s.resetRun);

  function loadExample(name: string) {
    const ex = EXAMPLES.find((e) => e.name === name);
    if (!ex) return;
    setModelName(ex.name);
    resetRun();
    resetOverrides();
    const defaults = ex.paramSets[0];
    if (defaults) {
      for (const [k, v] of Object.entries(defaults.values)) setParamOverride(k, v);
      setSimConfig({ tEnd: defaults.tEnd ?? undefined });
    } else {
      setSimConfig({ tEnd: undefined });
    }
    setDslSource(ex.dsl);
  }

  const statusDot =
    compileStatus === 'compiling' ? '⟳' :
    compileStatus === 'error'     ? '●' :
    compileStatus === 'ok'        ? '●' : '○';
  const statusColor =
    compileStatus === 'error'     ? 'text-red-400' :
    compileStatus === 'ok'        ? 'text-accent' : 'text-gray-500';

  return (
    <header className="flex items-center gap-3 px-4 h-11 bg-surface-1 border-b border-surface-border flex-shrink-0">
      {/* Logo */}
      <span className="text-accent font-semibold tracking-tight text-sm">camdl</span>
      <span className="text-surface-border">·</span>

      {/* Model name */}
      <input
        value={modelName}
        onChange={(e) => setModelName(e.target.value)}
        className="bg-transparent text-gray-300 text-sm focus:outline-none focus:text-white w-32"
        spellCheck={false}
      />

      {/* Compile status dot */}
      <span className={`text-xs ${statusColor}`} title={compileStatus}>
        {statusDot}
      </span>

      {/* Examples dropdown */}
      <select
        value=""
        onChange={(e) => { if (e.target.value) loadExample(e.target.value); }}
        className="text-xs bg-surface-2 border border-surface-border text-gray-400 hover:text-gray-200 rounded px-2 py-1 focus:outline-none cursor-pointer transition-colors"
        title="Load an example model"
      >
        <option value="" disabled>examples ▾</option>
        {EXAMPLES.map((ex) => (
          <option key={ex.name} value={ex.name} title={ex.description}>
            {ex.label}
          </option>
        ))}
      </select>

      <div className="flex-1" />

      {/* File ops */}
      <button
        onClick={openFile}
        className="px-2 py-1 text-xs text-gray-300 border border-surface-border hover:text-white hover:border-gray-500 rounded transition-colors"
      >
        Open
      </button>
      <button
        onClick={saveFile}
        className="px-2 py-1 text-xs text-gray-300 border border-surface-border hover:text-white hover:border-gray-500 rounded transition-colors"
      >
        Save
      </button>

      {/* Run button */}
      <button
        onClick={runSimulation}
        disabled={!ir || anyRunning}
        className={`px-3 py-1 text-xs rounded font-semibold transition-colors disabled:cursor-not-allowed ${
          anyRunning
            ? 'bg-red-500/80 text-white animate-pulse'
            : 'bg-accent text-surface-0 hover:bg-accent-dim disabled:opacity-40'
        }`}
      >
        {simStatus === 'running' ? '● Running…' : anyRunning ? '● Running…' : '▶ Run'}
      </button>
    </header>
  );
}
