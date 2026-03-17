import { Panel, PanelGroup, PanelResizeHandle } from 'react-resizable-panels';
import type { ActiveTab } from '../store';
import { useStore } from '../store';
import AgentPanel from './AgentPanel';
import RunPanel from './RunPanel';

export default function BottomPanel() {
  const activeTab  = useStore((s) => s.activeTab);
  const setActiveTab = useStore((s) => s.setActiveTab);
  const agentStatus = useStore((s) => s.agentStatus);
  const trajectory  = useStore((s) => s.trajectory);

  const tabs: { id: ActiveTab; label: string }[] = [
    { id: 'agent', label: 'Agent' },
    { id: 'run',   label: 'Run' },
    { id: 'split', label: 'Split' },
  ];

  return (
    <div className="flex flex-col h-full bg-surface-0">
      {/* Tab bar */}
      <div className="flex items-center gap-1 px-3 py-1 border-b border-surface-border flex-shrink-0 bg-surface-1">
        {tabs.map(({ id, label }) => (
          <button
            key={id}
            onClick={() => setActiveTab(id)}
            className={`px-3 py-1 text-xs rounded transition-colors ${
              activeTab === id
                ? 'text-accent bg-accent/10 font-semibold'
                : 'text-gray-400 hover:text-gray-200'
            }`}
          >
            {label}
            {id === 'agent' && agentStatus === 'streaming' && (
              <span className="ml-1 text-accent animate-pulse">•</span>
            )}
            {id === 'run' && trajectory && (
              <span className="ml-1 text-accent">•</span>
            )}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0 overflow-hidden">
        {activeTab === 'agent' && <AgentPanel />}
        {activeTab === 'run'   && <RunPanel />}
        {activeTab === 'split' && (
          <PanelGroup direction="horizontal">
            <Panel defaultSize={50} minSize={25}>
              <RunPanel />
            </Panel>
            <PanelResizeHandle className="w-1 bg-surface-border hover:bg-accent/40 transition-colors cursor-col-resize" />
            <Panel defaultSize={50} minSize={25}>
              <AgentPanel />
            </Panel>
          </PanelGroup>
        )}
      </div>
    </div>
  );
}
