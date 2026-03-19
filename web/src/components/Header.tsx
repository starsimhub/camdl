import { useStore } from '../store';
import { EXAMPLES } from '../lib/examples';
import { useTheme } from '../lib/theme';

const THEME_LABELS: Record<string, string> = { dark: '🌙', light: '☀', auto: '⬤' };

export default function Header() {
  const modelName        = useStore((s) => s.modelName);
  const setModelName     = useStore((s) => s.setModelName);
  const compileStatus    = useStore((s) => s.compileStatus);
  const experimentStatus = useStore((s) => s.experimentStatus);
  const runExperiment    = useStore((s) => s.runExperiment);
  const openFile         = useStore((s) => s.openFile);
  const saveFile         = useStore((s) => s.saveFile);
  const loadExample      = useStore((s) => s.loadExample);
  const { theme, cycle } = useTheme();

  const statusDot =
    compileStatus === 'compiling' ? '⟳' :
    compileStatus === 'error'     ? '●' :
    compileStatus === 'ok'        ? '●' : '○';
  const statusColor =
    compileStatus === 'error' ? 'text-red-400' :
    compileStatus === 'ok'    ? 'text-accent' : 'text-gray-400';

  return (
    <header className="flex items-center gap-3 px-4 h-11 bg-white border-b border-gray-200 flex-shrink-0 dark:bg-surface-1 dark:border-surface-border">
      {/* Logo */}
      <span className="text-accent font-semibold tracking-tight text-sm">camdl</span>
      <span className="text-gray-300 dark:text-surface-border">·</span>

      {/* Model name */}
      <input
        value={modelName}
        onChange={(e) => setModelName(e.target.value)}
        className="bg-transparent text-gray-700 text-sm focus:outline-none focus:text-gray-900 w-32 dark:text-gray-300 dark:focus:text-white"
        spellCheck={false}
      />

      {/* Compile status dot */}
      <span className={`text-xs ${statusColor}`} title={compileStatus}>
        {statusDot}
      </span>

      {/* Examples dropdown */}
      <select
        value={EXAMPLES.find((e) => e.name === modelName) ? modelName : ''}
        onChange={(e) => { if (e.target.value) loadExample(e.target.value); }}
        className="text-xs bg-gray-100 border border-gray-200 text-gray-600 hover:text-gray-900 rounded px-2 py-1 focus:outline-none cursor-pointer transition-colors dark:bg-surface-2 dark:border-surface-border dark:text-gray-400 dark:hover:text-gray-200"
        title="Load an example model"
      >
        <option value="" disabled>examples ▾</option>
        {EXAMPLES.map((ex) => (
          <option key={ex.name} value={ex.name} title={ex.description}>
            {ex.label}
          </option>
        ))}
      </select>

      {/* Run All */}
      <button
        onClick={() => { runExperiment(); }}
        disabled={experimentStatus === 'running' || compileStatus !== 'ok'}
        title="Run all scenarios"
        className={`px-2.5 py-1 text-xs rounded font-semibold transition-colors disabled:opacity-40 disabled:cursor-not-allowed ${
          experimentStatus === 'running'
            ? 'bg-accent/15 text-accent border border-accent/30'
            : 'bg-accent text-white hover:bg-accent-dim'
        }`}
      >
        {experimentStatus === 'running' ? '● Running…' : '▶ Run All'}
      </button>

      <div className="flex-1" />

      {/* Theme toggle */}
      <button
        onClick={cycle}
        title={`Theme: ${theme} (click to cycle)`}
        className="px-2 py-1 text-xs text-gray-400 hover:text-gray-600 transition-colors dark:text-gray-500 dark:hover:text-gray-300"
      >
        {THEME_LABELS[theme]}
      </button>

      {/* File ops */}
      <button
        onClick={openFile}
        className="px-2 py-1 text-xs text-gray-600 border border-gray-200 hover:text-gray-900 hover:border-gray-400 rounded transition-colors dark:text-gray-300 dark:border-surface-border dark:hover:text-white dark:hover:border-gray-500"
      >
        Open
      </button>
      <button
        onClick={saveFile}
        className="px-2 py-1 text-xs text-gray-600 border border-gray-200 hover:text-gray-900 hover:border-gray-400 rounded transition-colors dark:text-gray-300 dark:border-surface-border dark:hover:text-white dark:hover:border-gray-500"
      >
        Save
      </button>
    </header>
  );
}
