import { useStore } from '../store';
import { EXAMPLES } from '../lib/examples';

export default function Header() {
  const modelName     = useStore((s) => s.modelName);
  const setModelName  = useStore((s) => s.setModelName);
  const compileStatus = useStore((s) => s.compileStatus);
  const openFile      = useStore((s) => s.openFile);
  const saveFile      = useStore((s) => s.saveFile);
  const loadExample   = useStore((s) => s.loadExample);

  const statusDot =
    compileStatus === 'compiling' ? '⟳' :
    compileStatus === 'error'     ? '●' :
    compileStatus === 'ok'        ? '●' : '○';
  const statusColor =
    compileStatus === 'error' ? 'text-red-400' :
    compileStatus === 'ok'    ? 'text-accent' : 'text-gray-500';

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
    </header>
  );
}
