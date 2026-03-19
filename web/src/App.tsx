import { useEffect } from 'react';
import { PanelGroup, Panel, PanelResizeHandle } from 'react-resizable-panels';
import Header from './components/Header';
import ModelCanvas from './components/ModelCanvas';
import EditorPanel from './components/EditorPanel';
import BottomPanel from './components/BottomPanel';
import { useStore } from './store';

export default function App() {
  const loadExample = useStore((s) => s.loadExample);

  // Load sir_five_age with its baseline preset on mount
  useEffect(() => { loadExample('sir_five_age'); }, [loadExample]);

  return (
    <div className="flex flex-col h-screen overflow-hidden bg-gray-50 dark:bg-surface-0">
      <Header />

      {/* Main area: canvas + editor */}
      <PanelGroup direction="vertical" className="flex-1 min-h-0">
        {/* Top: canvas | DSL editor */}
        <Panel defaultSize={50} minSize={25}>
          <PanelGroup direction="horizontal">
            <Panel defaultSize={50} minSize={20}>
              <ModelCanvas />
            </Panel>
            <PanelResizeHandle className="w-1 bg-gray-200 hover:bg-accent/40 transition-colors cursor-col-resize dark:bg-surface-border" />
            <Panel defaultSize={50} minSize={25}>
              <EditorPanel />
            </Panel>
          </PanelGroup>
        </Panel>

        <PanelResizeHandle className="h-1 bg-gray-200 hover:bg-accent/40 transition-colors cursor-row-resize dark:bg-surface-border" />

        {/* Bottom: experiment */}
        <Panel defaultSize={50} minSize={20}>
          <BottomPanel />
        </Panel>
      </PanelGroup>
    </div>
  );
}
