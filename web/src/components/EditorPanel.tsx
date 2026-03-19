import { useStore } from '../store';
import DslEditor from './DslEditor';
import AgentPanel from './AgentPanel';

export default function EditorPanel() {
  const activeTab = useStore((s) => s.activeTab);
  const setActiveTab = useStore((s) => s.setActiveTab);
  const agentPhase = useStore((s) => s.agentPhase);

  return (
    <div className="flex flex-col h-full">
      {/* Tab bar */}
      <div className="flex items-center gap-1 px-3 py-1 bg-gray-50 border-b border-gray-200 flex-shrink-0 dark:bg-surface-1 dark:border-surface-border">
        <button
          onClick={() => setActiveTab('dsl')}
          className={`px-3 py-1 text-xs rounded transition-colors ${
            activeTab === 'dsl'
              ? 'text-accent bg-accent/10 font-semibold'
              : 'text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200'
          }`}
        >
          DSL
        </button>
        <button
          onClick={() => setActiveTab('agent')}
          className={`px-3 py-1 text-xs rounded transition-colors ${
            activeTab === 'agent'
              ? 'text-accent bg-accent/10 font-semibold'
              : 'text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200'
          }`}
        >
          Agent
          {agentPhase !== 'idle' && (
            <span className="ml-1 text-accent animate-pulse">•</span>
          )}
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0 overflow-hidden">
        <div className={activeTab === 'dsl' ? 'h-full' : 'hidden h-full'}>
          <DslEditor />
        </div>
        <div className={activeTab === 'agent' ? 'h-full' : 'hidden h-full'}>
          <AgentPanel />
        </div>
      </div>
    </div>
  );
}
