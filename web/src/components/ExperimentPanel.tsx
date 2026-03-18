import { Panel, PanelGroup, PanelResizeHandle } from 'react-resizable-panels';
import ExperimentSidebar from './ExperimentSidebar';
import ResultsPanel from './ResultsPanel';

export default function ExperimentPanel() {
  return (
    <PanelGroup direction="horizontal">
      <Panel defaultSize={28} minSize={18} maxSize={45}>
        <ExperimentSidebar />
      </Panel>
      <PanelResizeHandle className="w-1 bg-surface-border hover:bg-accent/40 transition-colors cursor-col-resize" />
      <Panel>
        <ResultsPanel />
      </Panel>
    </PanelGroup>
  );
}
