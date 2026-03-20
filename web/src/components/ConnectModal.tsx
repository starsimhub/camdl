import { useState } from 'react';

interface ConnectModalProps {
  onConnect: (url: string) => void;
  onClose: () => void;
  status: 'idle' | 'loading' | 'error';
  error?: string;
}

export default function ConnectModal({ onConnect, onClose, status, error }: ConnectModalProps) {
  const [url, setUrl] = useState('http://localhost:4280');

  const handleSubmit = () => {
    if (status !== 'loading') onConnect(url.trim());
  };

  return (
    <div
      className="fixed inset-0 bg-black/40 flex items-center justify-center z-50"
      onClick={onClose}
    >
      <div
        className="bg-white dark:bg-surface-1 rounded-lg shadow-xl p-6 w-96 max-w-[calc(100vw-2rem)]"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-gray-900 dark:text-gray-100 mb-1">
          Connect to camdl serve
        </h2>
        <p className="text-xs text-gray-500 dark:text-gray-400 mb-4">
          Load a completed experiment from a running{' '}
          <code className="bg-gray-100 dark:bg-surface-2 px-1 rounded">camdl serve</code> instance.
        </p>

        <label className="block text-xs text-gray-600 dark:text-gray-400 mb-1">
          Server URL
        </label>
        <input
          type="text"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') handleSubmit(); }}
          className="w-full text-sm border border-gray-200 dark:border-surface-border rounded px-3 py-2 bg-white dark:bg-surface-2 text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-1 focus:ring-accent"
          placeholder="http://localhost:4280"
          disabled={status === 'loading'}
          autoFocus
        />

        {error && (
          <p className="mt-2 text-xs text-red-500 dark:text-red-400 break-words">{error}</p>
        )}

        <div className="mt-4 flex gap-2 justify-end">
          <button
            onClick={onClose}
            disabled={status === 'loading'}
            className="px-3 py-1.5 text-xs text-gray-600 dark:text-gray-400 border border-gray-200 dark:border-surface-border rounded hover:border-gray-400 transition-colors disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            onClick={handleSubmit}
            disabled={status === 'loading'}
            className="px-3 py-1.5 text-xs bg-accent text-white rounded hover:bg-accent-dim disabled:opacity-50 disabled:cursor-not-allowed font-semibold transition-colors"
          >
            {status === 'loading' ? 'Connecting…' : 'Connect'}
          </button>
        </div>
      </div>
    </div>
  );
}
