import { useRef, useEffect, useState } from 'react';
import MonacoEditor from '@monaco-editor/react';
import { useStore } from '../store';
import { sendMessage } from '../lib/agentClient';

// ── Diff Viewer ───────────────────────────────────────────────────────────────

function DiffViewer() {
  const pendingDiff = useStore((s) => s.pendingDiff);
  const dslSource   = useStore((s) => s.dslSource);
  const acceptDiff  = useStore((s) => s.acceptDiff);
  const rejectDiff  = useStore((s) => s.rejectDiff);

  if (!pendingDiff) return null;

  return (
    <div className="border border-accent/40 rounded-lg overflow-hidden mx-3 mb-3 flex-shrink-0">
      {/* Explanation */}
      <div className="px-3 py-2 bg-surface-2 border-b border-surface-border">
        <p className="text-xs text-gray-300 leading-relaxed">{pendingDiff.explanation}</p>
      </div>

      {/* Monaco diff editor */}
      <div style={{ height: 220 }}>
        <MonacoEditor
          height="100%"
          language="camdl"
          theme="camdl-dark"
          value={pendingDiff.modified}
          options={{
            readOnly: true,
            minimap: { enabled: false },
            fontSize: 12,
            lineHeight: 18,
            padding: { top: 8 },
            scrollBeyondLastLine: false,
            fontFamily: '"JetBrains Mono", monospace',
          }}
        />
      </div>

      {/* Accept / Reject */}
      <div className="flex gap-2 px-3 py-2 bg-surface-2 border-t border-surface-border">
        <button
          onClick={acceptDiff}
          className="px-3 py-1 text-xs bg-accent text-surface-0 rounded font-semibold hover:bg-accent-dim transition-colors"
        >
          Accept
        </button>
        <button
          onClick={rejectDiff}
          className="px-3 py-1 text-xs bg-surface-3 text-gray-300 rounded hover:bg-surface-border transition-colors"
        >
          Reject
        </button>
        <span className="text-xs text-gray-600 self-center ml-1">proposed edit</span>
      </div>
    </div>
  );
}

// ── Message Bubble ────────────────────────────────────────────────────────────

function MessageBubble({ msg }: { msg: ReturnType<typeof useStore.getState>['messages'][0] }) {
  const isUser = msg.role === 'user';
  return (
    <div className={`flex ${isUser ? 'justify-end' : 'justify-start'} mb-3`}>
      <div
        className={`max-w-[85%] rounded-xl px-3 py-2 text-sm leading-relaxed ${
          isUser
            ? 'bg-accent/15 text-gray-200 border border-accent/20'
            : 'bg-surface-2 text-gray-300'
        }`}
        style={{ fontFamily: 'JetBrains Mono, monospace', fontSize: 12 }}
      >
        {/* Tool call badges */}
        {msg.toolCalls && msg.toolCalls.length > 0 && (
          <div className="flex flex-wrap gap-1 mb-2">
            {msg.toolCalls.map((tc, i) => (
              <span
                key={i}
                className={`text-xs px-2 py-0.5 rounded-full ${
                  tc.status === 'done'    ? 'bg-accent/10 text-accent' :
                  tc.status === 'error'   ? 'bg-red-500/10 text-red-400' :
                  'bg-surface-3 text-gray-400 animate-pulse'
                }`}
              >
                {tc.status === 'running' ? `⟳ ${tc.name}` : tc.summary}
              </span>
            ))}
          </div>
        )}
        {/* Message text — preserve whitespace */}
        <pre className="whitespace-pre-wrap break-words font-mono text-xs">{msg.content}</pre>
      </div>
    </div>
  );
}

// ── Agent Panel ───────────────────────────────────────────────────────────────

export default function AgentPanel() {
  const messages     = useStore((s) => s.messages);
  const agentStatus  = useStore((s) => s.agentStatus);
  const pendingDiff  = useStore((s) => s.pendingDiff);
  const [input, setInput] = useState('');
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const submit = () => {
    const text = input.trim();
    if (!text || agentStatus === 'streaming') return;
    setInput('');
    sendMessage(text);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  return (
    <div className="flex flex-col h-full">
      {/* Message list */}
      <div className="flex-1 overflow-y-auto px-3 py-3 min-h-0">
        {messages.length === 0 && (
          <div className="flex flex-col items-center justify-center h-full gap-2 text-gray-600">
            <p className="text-sm">Ask the agent to build or modify a model.</p>
            <p className="text-xs">Example: "add waning immunity from R back to S"</p>
          </div>
        )}
        {messages.map((msg) => (
          <MessageBubble key={msg.id} msg={msg} />
        ))}
        {agentStatus === 'streaming' && messages[messages.length - 1]?.role !== 'assistant' && (
          <div className="flex justify-start mb-3">
            <div className="bg-surface-2 rounded-xl px-3 py-2">
              <span className="text-xs text-gray-500 animate-pulse">thinking…</span>
            </div>
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      {/* Diff viewer (if pending) */}
      {pendingDiff && <DiffViewer />}

      {/* Input */}
      <div className="flex gap-2 px-3 py-2 border-t border-surface-border flex-shrink-0">
        <textarea
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Ask the agent… (Enter to send, Shift+Enter for newline)"
          rows={2}
          disabled={agentStatus === 'streaming'}
          className="flex-1 resize-none bg-surface-2 border border-surface-border rounded-lg px-3 py-2 text-xs text-gray-300 placeholder-gray-600 focus:outline-none focus:border-accent/40 disabled:opacity-50 font-mono"
          style={{ fontFamily: 'JetBrains Mono, monospace' }}
        />
        <button
          onClick={submit}
          disabled={!input.trim() || agentStatus === 'streaming'}
          className="px-3 self-end py-2 text-xs bg-accent text-surface-0 rounded-lg font-semibold hover:bg-accent-dim disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          Send
        </button>
      </div>
    </div>
  );
}
