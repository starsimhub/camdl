import { useEffect } from 'react';
import { PanelGroup, Panel, PanelResizeHandle } from 'react-resizable-panels';
import Header from './components/Header';
import ModelCanvas from './components/ModelCanvas';
import EditorPanel from './components/EditorPanel';
import BottomPanel from './components/BottomPanel';
import { useStore } from './store';

export default function App() {
  const compile = useStore((s) => s.compile);

  // Compile the default model on mount
  useEffect(() => { compile(); }, [compile]);

  return (
    <div className="flex flex-col h-screen overflow-hidden bg-surface-0">
      <Header />

      {/* Main area: canvas + editor */}
      <PanelGroup direction="vertical" className="flex-1 min-h-0">
        {/* Top: canvas | DSL editor */}
        <Panel defaultSize={60} minSize={30}>
          <PanelGroup direction="horizontal">
            <Panel defaultSize={38} minSize={20}>
              <ModelCanvas />
            </Panel>
            <PanelResizeHandle className="w-1 bg-surface-border hover:bg-accent/40 transition-colors cursor-col-resize" />
            <Panel defaultSize={62} minSize={30}>
              <EditorPanel />
            </Panel>
          </PanelGroup>
        </Panel>

        <PanelResizeHandle className="h-1 bg-surface-border hover:bg-accent/40 transition-colors cursor-row-resize" />

        {/* Bottom: agent / run / split */}
        <Panel defaultSize={40} minSize={20}>
          <BottomPanel />
        </Panel>
      </PanelGroup>
    </div>
  );
}
