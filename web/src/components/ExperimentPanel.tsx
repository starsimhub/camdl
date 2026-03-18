import { useState } from 'react';
import { Panel, PanelGroup, PanelResizeHandle } from 'react-resizable-panels';
import ExperimentSidebar from './ExperimentSidebar';
import RunConfigPanel from './RunConfigPanel';
import ResultsPanel from './ResultsPanel';

type ResultsTab = 'chart' | 'data';

export default function ExperimentPanel() {
  const [resultsTab, setResultsTab] = useState<ResultsTab>('chart');

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={18} minSize={14} maxSize={30}>
        <RunConfigPanel />
      </Panel>
      <PanelResizeHandle className="w-1 bg-surface-border hover:bg-accent/40 transition-colors cursor-col-resize" />

      <Panel defaultSize={22} minSize={15} maxSize={40}>
        <ExperimentSidebar />
      </Panel>
      <PanelResizeHandle className="w-1 bg-surface-border hover:bg-accent/40 transition-colors cursor-col-resize" />

      <Panel>
        <div className="flex flex-col h-full">
          <div className="flex items-center gap-1 px-3 py-1 bg-surface-1 border-b border-surface-border flex-shrink-0">
            {(['chart', 'data'] as ResultsTab[]).map((t) => (
              <button
                key={t}
                onClick={() => setResultsTab(t)}
                className={`px-2.5 py-0.5 text-xs rounded transition-colors capitalize ${
                  resultsTab === t
                    ? 'bg-surface-3 text-gray-100'
                    : 'text-gray-500 hover:text-gray-300'
                }`}
              >
                {t}
              </button>
            ))}
          </div>
          <div className="flex-1 min-h-0 overflow-hidden">
            {resultsTab === 'chart' && <ResultsPanel />}
            {resultsTab === 'data' && (
              <div className="flex items-center justify-center h-full">
                <span className="text-gray-600 text-sm">Data view — coming soon</span>
              </div>
            )}
          </div>
        </div>
      </Panel>
    </PanelGroup>
  );
}
