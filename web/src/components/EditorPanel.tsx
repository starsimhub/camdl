import { useState } from 'react';
import DslEditor from './DslEditor';
import ParamEditor from './ParamEditor';
import ScenariosPanel from './ScenariosPanel';

type EditorTab = 'dsl' | 'params' | 'scenarios';

const TAB_LABELS: Record<EditorTab, string> = {
  dsl:       'DSL',
  params:    'Params',
  scenarios: 'Scenarios',
};

export default function EditorPanel() {
  const [tab, setTab] = useState<EditorTab>('dsl');

  return (
    <div className="flex flex-col h-full">
      {/* Tab bar */}
      <div className="flex items-center gap-1 px-3 py-1 bg-surface-1 border-b border-surface-border flex-shrink-0">
        {(Object.keys(TAB_LABELS) as EditorTab[]).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`px-3 py-1 text-xs rounded transition-colors ${
              tab === t
                ? 'text-accent bg-accent/10 font-semibold'
                : 'text-gray-400 hover:text-gray-200'
            }`}
          >
            {TAB_LABELS[t]}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0">
        {tab === 'dsl'       && <DslEditor />}
        {tab === 'params'    && <ParamEditor />}
        {tab === 'scenarios' && <ScenariosPanel />}
      </div>
    </div>
  );
}
