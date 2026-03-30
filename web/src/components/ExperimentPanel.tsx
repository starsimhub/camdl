import { useState } from "react";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import ExperimentSidebar from "./ExperimentSidebar";
import ResultsPanel from "./ResultsPanel";
import RunConfigPanel from "./RunConfigPanel";

type ResultsTab = "chart" | "data";

export default function ExperimentPanel() {
  const [resultsTab, setResultsTab] = useState<ResultsTab>("chart");

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={18} minSize={14} maxSize={30}>
        <RunConfigPanel />
      </Panel>
      <PanelResizeHandle className="w-1 bg-gray-200 hover:bg-accent/40 transition-colors cursor-col-resize dark:bg-surface-border" />

      <Panel defaultSize={22} minSize={15} maxSize={40}>
        <ExperimentSidebar />
      </Panel>
      <PanelResizeHandle className="w-1 bg-gray-200 hover:bg-accent/40 transition-colors cursor-col-resize dark:bg-surface-border" />

      <Panel>
        <div className="flex flex-col h-full">
          <div className="flex items-center gap-1 px-3 py-1 bg-gray-50 border-b border-gray-200 flex-shrink-0 dark:bg-surface-1 dark:border-surface-border">
            {(["chart", "data"] as ResultsTab[]).map((t) => (
              <button
                key={t}
                onClick={() => setResultsTab(t)}
                className={`px-2.5 py-0.5 text-xs rounded transition-colors capitalize ${
                  resultsTab === t
                    ? "bg-gray-200 text-gray-900 dark:bg-surface-3 dark:text-gray-100"
                    : "text-gray-500 hover:text-gray-700 dark:text-gray-500 dark:hover:text-gray-300"
                }`}
              >
                {t}
              </button>
            ))}
          </div>
          <div className="flex-1 min-h-0 overflow-hidden">
            {resultsTab === "chart" && <ResultsPanel />}
            {resultsTab === "data" && (
              <div className="flex items-center justify-center h-full">
                <span className="text-gray-500 text-sm dark:text-gray-600">Data view — coming soon</span>
              </div>
            )}
          </div>
        </div>
      </Panel>
    </PanelGroup>
  );
}
