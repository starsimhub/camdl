import { useState } from "react";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { useStore } from "../store";
import AgentPanel from "./AgentPanel";
import DslEditor from "./DslEditor";

export default function EditorPanel() {
  const activeTab = useStore((s) => s.activeTab);
  const setActiveTab = useStore((s) => s.setActiveTab);
  const agentPhase = useStore((s) => s.agentPhase);
  const [split, setSplit] = useState(false);

  return (
    <div className="flex flex-col h-full">
      {/* Tab bar */}
      <div className="flex items-center gap-1 px-3 py-1 bg-gray-50 border-b border-gray-200 flex-shrink-0 dark:bg-surface-1 dark:border-surface-border">
        {split
          ? (
            <>
              <span className="px-3 py-1 text-xs text-accent font-semibold">DSL</span>
              <span className="text-gray-300 dark:text-surface-border text-xs">|</span>
              <span className="px-3 py-1 text-xs text-accent font-semibold">
                Agent{agentPhase !== "idle" && <span className="ml-1 animate-pulse">•</span>}
              </span>
            </>
          )
          : (
            <>
              <button
                onClick={() => setActiveTab("dsl")}
                className={`px-3 py-1 text-xs rounded transition-colors ${
                  activeTab === "dsl"
                    ? "text-accent bg-accent/10 font-semibold"
                    : "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
                }`}
              >
                DSL
              </button>
              <button
                onClick={() => setActiveTab("agent")}
                className={`px-3 py-1 text-xs rounded transition-colors ${
                  activeTab === "agent"
                    ? "text-accent bg-accent/10 font-semibold"
                    : "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
                }`}
              >
                Agent{agentPhase !== "idle" && <span className="ml-1 text-accent animate-pulse">•</span>}
              </button>
            </>
          )}
        <div className="flex-1" />
        <button
          onClick={() => setSplit((s) => !s)}
          title={split ? "Switch to tabs" : "Split DSL + Agent"}
          className="px-2 py-0.5 text-xs text-gray-400 hover:text-gray-600 transition-colors dark:text-gray-500 dark:hover:text-gray-300"
        >
          {split ? "⊟" : "⊞"}
        </button>
      </div>

      {/* Content */}
      {split
        ? (
          <PanelGroup direction="horizontal" className="flex-1 min-h-0">
            <Panel defaultSize={55} minSize={30}>
              <DslEditor />
            </Panel>
            <PanelResizeHandle className="w-1 bg-gray-200 hover:bg-accent/40 transition-colors cursor-col-resize dark:bg-surface-border" />
            <Panel defaultSize={45} minSize={25}>
              <AgentPanel />
            </Panel>
          </PanelGroup>
        )
        : (
          // Use visibility:hidden (not display:none) so Monaco always has a sized container
          // and its keyboard handlers stay active across tab switches.
          <div className="relative flex-1 min-h-0 overflow-hidden">
            <div className={`absolute inset-0 ${activeTab === "dsl" ? "" : "invisible pointer-events-none"}`}>
              <DslEditor />
            </div>
            <div className={`absolute inset-0 ${activeTab === "agent" ? "" : "invisible pointer-events-none"}`}>
              <AgentPanel />
            </div>
          </div>
        )}
    </div>
  );
}
