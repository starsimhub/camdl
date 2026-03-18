// Claude API agent with tool use — proxied through compiler-server to keep key server-side.

import { useStore } from '../store';
import { compile as compileApi } from './compilerClient';
import { simulate as wasmSimulate } from './wasm';

const SYSTEM_PROMPT = `You are an expert in stochastic compartmental epidemic modelling using the camdl DSL.

The camdl language is a minimal spec DSL for compartmental models. Key syntax:

  time_unit = 'days

  compartments { S, E, I, R }   # disease states

  let N = S + I + R              # derived quantities

  parameters {
    beta  : rate                 # transmission rate
    gamma : rate                 # recovery rate
    N0    : count
    I0    : count
  }

  transitions {
    infection : S --> I  @ beta * S * (I / N)   # rate expression after @
    recovery  : I --> R  @ gamma * I
  }

  # Stratification (age groups etc.)
  stratify(by = age, values = [child, adult])
  # Then use compartments like S[child], I[adult] and sum(a in age, ...)

  init {
    S = N0 - I0
    I = I0
  }

  simulate {
    from = 0 'days
    to   = 120 'days
  }

You have three tools:
1. compile(dsl) — compile DSL to IR JSON, returns IR or error with line numbers
2. simulate(ir_json, backend?, seed?) — run simulation, returns trajectory summary
3. propose_edit(modified, explanation) — propose a DSL change as a diff for the user to accept/reject

Rules:
- Always call compile to verify your DSL is valid before calling propose_edit
- Never output modified DSL in your chat message — always use propose_edit
- If compile fails, read the error, fix the DSL, try again (max 3 attempts)
- Explain what you changed and why in the explanation field
- When asked about simulation results, use the trajectory data to give specific numbers
- Note: editing the DSL (via propose_edit) clears all experiment results. The user will need to re-run their experiment after accepting any edits.`;

interface Message {
  role: 'user' | 'assistant';
  content: string | ContentBlock[];
}

interface ContentBlock {
  type: 'text' | 'tool_use' | 'tool_result';
  [key: string]: unknown;
}

const TOOLS = [
  {
    name: 'compile',
    description: 'Compile camdl DSL source to IR JSON. Use this to check validity before propose_edit.',
    input_schema: {
      type: 'object',
      properties: { dsl: { type: 'string', description: 'camdl DSL source code' } },
      required: ['dsl'],
    },
  },
  {
    name: 'simulate',
    description: 'Run simulation given IR JSON. Returns trajectory summary statistics.',
    input_schema: {
      type: 'object',
      properties: {
        ir_json:  { type: 'string' },
        backend:  { type: 'string', enum: ['gillespie', 'tau_leap', 'chain_binomial'], default: 'gillespie' },
        seed:     { type: 'number', default: 42 },
      },
      required: ['ir_json'],
    },
  },
  {
    name: 'propose_edit',
    description: 'Propose a change to the current DSL. The user will see a diff and can accept or reject.',
    input_schema: {
      type: 'object',
      properties: {
        modified:    { type: 'string', description: 'Complete modified DSL source' },
        explanation: { type: 'string', description: 'Plain English explanation of what changed and why' },
      },
      required: ['modified', 'explanation'],
    },
  },
];

// Execute a tool call and return the result string.
async function executeTool(name: string, input: Record<string, unknown>): Promise<string> {
  const store = useStore.getState();

  if (name === 'compile') {
    const result = await compileApi(String(input.dsl ?? ''));
    if (result.ok) {
      return JSON.stringify({ ok: true, ir: result.ir });
    } else {
      return JSON.stringify({ ok: false, diagnostics: result.diagnostics });
    }
  }

  if (name === 'simulate') {
    try {
      const traj = await wasmSimulate(String(input.ir_json ?? ''), {
        backend: (input.backend as 'gillespie') ?? 'gillespie',
        seed: Number(input.seed ?? 42),
      });
      // Return summary stats instead of full trajectory
      const last = traj.snapshots[traj.snapshots.length - 1];
      const summary: Record<string, number> = {};
      traj.int_compartment_names.forEach((n, i) => { summary[n] = last?.counts[i] ?? 0; });
      traj.real_compartment_names.forEach((n, i) => { summary[n] = last?.values[i] ?? 0; });
      return JSON.stringify({ ok: true, final_state: summary, n_snapshots: traj.snapshots.length });
    } catch (e) {
      return JSON.stringify({ ok: false, error: String(e) });
    }
  }

  if (name === 'propose_edit') {
    store.setPendingDiff({
      modified:    String(input.modified ?? ''),
      explanation: String(input.explanation ?? ''),
    });
    return JSON.stringify({ ok: true, message: 'Diff shown to user for review.' });
  }

  return JSON.stringify({ error: `Unknown tool: ${name}` });
}

export async function sendMessage(userText: string) {
  const store = useStore.getState();
  store.addUserMessage(userText);
  store.setAgentStatus('streaming');

  const assistantMsgId = crypto.randomUUID();
  store.startAssistantMessage(assistantMsgId);

  // Build conversation history from store messages (simplified — last 20)
  const history: Message[] = store.messages
    .slice(-20)
    .filter((m) => m.role === 'user' || m.role === 'assistant')
    .map((m) => ({ role: m.role, content: m.content }));

  // Inject current DSL as context
  const currentDsl = store.dslSource;
  history[history.length - 1] = {
    role: 'user',
    content: `${userText}\n\n---\nCurrent model DSL:\n\`\`\`camdl\n${currentDsl}\n\`\`\``,
  };

  const messages: Message[] = [...history];

  try {
    let iterations = 0;
    while (iterations++ < 10) {
      const body = {
        model: 'claude-opus-4-6',
        max_tokens: 4096,
        system: SYSTEM_PROMPT,
        tools: TOOLS,
        messages,
      };

      const res = await fetch('/api/agent/stream', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });

      if (!res.ok) {
        const err = await res.text();
        store.appendAssistantChunk(assistantMsgId, `\n\n[Error: ${err}]`);
        break;
      }

      // Parse SSE stream
      const reader = res.body!.getReader();
      const decoder = new TextDecoder();
      let buf = '';

      let stopReason = '';
      const toolUses: { id: string; name: string; inputStr: string }[] = [];
      let currentToolId = '';

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });

        const lines = buf.split('\n');
        buf = lines.pop() ?? '';

        for (const line of lines) {
          if (!line.startsWith('data: ')) continue;
          const data = line.slice(6).trim();
          if (data === '[DONE]' || !data) continue;

          let event: Record<string, unknown>;
          try { event = JSON.parse(data); } catch { continue; }

          const t = event.type as string;

          if (t === 'content_block_start') {
            const block = event.content_block as { type: string; id?: string; name?: string };
            if (block.type === 'tool_use') {
              currentToolId = block.id ?? '';
              toolUses.push({ id: currentToolId, name: block.name ?? '', inputStr: '' });
              store.addToolCall(assistantMsgId, block.name ?? '', 'running', '');
            }
          }

          if (t === 'content_block_delta') {
            const delta = event.delta as { type: string; text?: string; partial_json?: string };
            if (delta.type === 'text_delta' && delta.text) {
              store.appendAssistantChunk(assistantMsgId, delta.text);
            }
            if (delta.type === 'input_json_delta' && delta.partial_json) {
              const tu = toolUses.find((t) => t.id === currentToolId);
              if (tu) tu.inputStr += delta.partial_json;
            }
          }

          if (t === 'message_delta') {
            const d = event.delta as { stop_reason?: string };
            stopReason = d.stop_reason ?? '';
          }
        }
      }

      if (stopReason !== 'tool_use' || toolUses.length === 0) break;

      // Execute tool calls
      const toolResults = [];
      for (const tu of toolUses) {
        let input: Record<string, unknown> = {};
        try { input = JSON.parse(tu.inputStr); } catch {}

        store.addToolCall(assistantMsgId, tu.name, 'running', `${tu.name}(…)`);
        const result = await executeTool(tu.name, input);
        store.addToolCall(assistantMsgId, tu.name, 'done', `${tu.name} ✓`);

        toolResults.push({ type: 'tool_result', tool_use_id: tu.id, content: result });
      }

      // Continue conversation with tool results
      messages.push({
        role: 'assistant',
        content: toolUses.map((tu) => ({
          type: 'tool_use',
          id: tu.id,
          name: tu.name,
          input: JSON.parse(tu.inputStr || '{}'),
        })),
      });
      messages.push({ role: 'user', content: toolResults as unknown as string });
    }
  } catch (e) {
    store.appendAssistantChunk(assistantMsgId, `\n\n[Error: ${e}]`);
  } finally {
    store.setAgentStatus('idle');
  }
}
