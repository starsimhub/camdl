import { useRef, useEffect, useState } from 'react';
import MonacoEditor from '@monaco-editor/react';
import { MarkdownHooks as Markdown } from 'react-markdown';
import { useStore } from '../store';
import { sendMessage } from '../lib/agentClient';
import { useIsDark } from '../lib/theme';

// ── Diff Viewer ───────────────────────────────────────────────────────────────

function DiffViewer() {
  const pendingDiff = useStore((s) => s.pendingDiff);
  const acceptDiff  = useStore((s) => s.acceptDiff);
  const rejectDiff  = useStore((s) => s.rejectDiff);
  const isDark = useIsDark();

  if (!pendingDiff) return null;

  return (
    <div className="border border-accent/40 rounded-lg overflow-hidden mx-3 mb-3 flex-shrink-0">
      {/* Explanation */}
      <div className="px-3 py-2 bg-gray-100 border-b border-gray-200 dark:bg-surface-2 dark:border-surface-border">
        <p className="text-xs text-gray-700 leading-relaxed dark:text-gray-300">{pendingDiff.explanation}</p>
      </div>

      {/* Monaco diff editor */}
      <div style={{ height: 220 }}>
        <MonacoEditor
          height="100%"
          language="camdl"
          theme={isDark ? 'camdl-dark' : 'camdl-light'}
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
      <div className="flex gap-2 px-3 py-2 bg-gray-100 border-t border-gray-200 dark:bg-surface-2 dark:border-surface-border">
        <button
          onClick={acceptDiff}
          className="px-3 py-1 text-xs bg-accent text-white rounded font-semibold hover:bg-accent-dim transition-colors"
        >
          Accept
        </button>
        <button
          onClick={rejectDiff}
          className="px-3 py-1 text-xs bg-gray-200 text-gray-700 rounded hover:bg-gray-300 transition-colors dark:bg-surface-3 dark:text-gray-300 dark:hover:bg-surface-border"
        >
          Reject
        </button>
        <span className="text-xs text-gray-400 self-center ml-1 dark:text-gray-600">proposed edit</span>
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
            ? 'bg-accent/15 text-gray-800 border border-accent/20 dark:text-gray-200'
            : 'bg-gray-100 text-gray-700 dark:bg-surface-2 dark:text-gray-300'
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
                  'bg-gray-200 text-gray-500 animate-pulse dark:bg-surface-3 dark:text-gray-400'
                }`}
              >
                {tc.status === 'running' ? `⟳ ${tc.name}` : tc.summary}
              </span>
            ))}
          </div>
        )}
        {/* Message text */}
        {isUser ? (
          <pre className="whitespace-pre-wrap break-words font-mono text-xs">{msg.content}</pre>
        ) : (
          <div className="text-xs leading-relaxed
            [&_p]:mb-2 [&_p:last-child]:mb-0
            [&_code]:bg-gray-200 [&_code]:px-1 [&_code]:py-0.5 [&_code]:rounded [&_code]:text-accent [&_code]:font-mono [&_code]:text-xs dark:[&_code]:bg-surface-3
            [&_pre]:bg-gray-200 [&_pre]:rounded [&_pre]:p-2 [&_pre]:overflow-x-auto [&_pre]:my-2 dark:[&_pre]:bg-surface-3
            [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_pre_code]:text-gray-700 dark:[&_pre_code]:text-gray-200
            [&_ul]:list-disc [&_ul]:pl-4 [&_ul]:mb-2
            [&_ol]:list-decimal [&_ol]:pl-4 [&_ol]:mb-2
            [&_li]:mb-0.5
            [&_strong]:text-gray-900 [&_strong]:font-semibold dark:[&_strong]:text-gray-100
            [&_h3]:text-gray-800 [&_h3]:font-semibold [&_h3]:mt-2 [&_h3]:mb-1 dark:[&_h3]:text-gray-200"
          >
            <Markdown>{msg.content}</Markdown>
          </div>
        )}
      </div>
    </div>
  );
}

// ── Typing indicator ──────────────────────────────────────────────────────────

function TypingDots() {
  return (
    <div className="flex justify-start mb-3">
      <div className="bg-gray-100 rounded-xl px-3 py-2.5 flex items-center gap-1 dark:bg-surface-2">
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="block w-1.5 h-1.5 rounded-full bg-gray-400 dark:bg-gray-500"
            style={{ animation: `bounce 1.2s ease-in-out ${i * 0.2}s infinite` }}
          />
        ))}
      </div>
    </div>
  );
}

// ── Agent Panel ───────────────────────────────────────────────────────────────

export default function AgentPanel() {
  const messages    = useStore((s) => s.messages);
  const agentPhase  = useStore((s) => s.agentPhase);
  const pendingDiff = useStore((s) => s.pendingDiff);
  const [input, setInput] = useState('');
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const submit = () => {
    const text = input.trim();
    if (!text || agentPhase !== 'idle') return;
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
          <div className="flex flex-col items-center justify-center h-full gap-2 text-gray-500 dark:text-gray-600">
            <p className="text-sm">Ask the agent to build or modify a model.</p>
            <p className="text-xs">Example: "add waning immunity from R back to S"</p>
          </div>
        )}
        {messages
          .filter((msg) => msg.role === 'user' || msg.content || (msg.toolCalls?.length ?? 0) > 0)
          .map((msg) => (
            <MessageBubble key={msg.id} msg={msg} />
          ))}
        {(agentPhase === 'waiting' || agentPhase === 'tool_calling') && <TypingDots />}
        <div ref={bottomRef} />
      </div>

      {/* Diff viewer (if pending) */}
      {pendingDiff && <DiffViewer />}

      {/* Input */}
      <div className="flex gap-2 px-3 py-2 border-t border-gray-200 flex-shrink-0 dark:border-surface-border">
        <textarea
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Ask the agent… (Enter to send, Shift+Enter for newline)"
          rows={2}
          disabled={agentPhase !== 'idle'}
          className="flex-1 resize-none bg-gray-100 border border-gray-200 rounded-lg px-3 py-2 text-xs text-gray-700 placeholder-gray-400 focus:outline-none focus:border-accent/40 disabled:opacity-50 font-mono dark:bg-surface-2 dark:border-surface-border dark:text-gray-300 dark:placeholder-gray-600"
          style={{ fontFamily: 'JetBrains Mono, monospace' }}
        />
        <button
          onClick={submit}
          disabled={!input.trim() || agentPhase !== 'idle'}
          className="px-3 self-end py-2 text-xs bg-accent text-white rounded-lg font-semibold hover:bg-accent-dim disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          Send
        </button>
      </div>
    </div>
  );
}
