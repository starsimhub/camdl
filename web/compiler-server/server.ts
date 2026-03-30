import { execFile } from "child_process";
import cors from "cors";
import express from "express";
import { unlink, writeFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";

const app = express();
app.use(cors());
app.use(express.json({ limit: "2mb" }));

// Find camdlc: CAMDLC env var, then PATH, then project default.
const CAMDLC = process.env.CAMDLC
  ?? join(import.meta.dirname, "../../ocaml/_build/default/bin/camdlc.exe");

const PORT = parseInt(process.env.PORT ?? "3001", 10);

// ── POST /compile ────────────────────────────────────────────────────────────

app.post("/compile", async (req, res) => {
  const { source, name } = req.body as { source?: string; name?: string };
  if (typeof source !== "string") {
    return res.status(400).json({ ok: false, diagnostics: [], error: "missing source" });
  }

  const modelName = name ?? "model";
  const tmpFile = join(tmpdir(), `camdl_${process.pid}_${Date.now()}.camdl`);

  try {
    await writeFile(tmpFile, source, "utf8");

    const result = await new Promise<{ stdout: string; stderr: string; code: number }>(
      (resolve) => {
        execFile(
          CAMDLC,
          [tmpFile, "--json-errors"],
          { timeout: 10_000 },
          (err, stdout, stderr) => {
            resolve({ stdout, stderr, code: (err as any)?.code ?? 0 });
          },
        );
      },
    );

    if (result.code === 0) {
      try {
        const ir = JSON.parse(result.stdout);
        // Rename model to user-supplied name if provided.
        if (name) ir.name = modelName;
        return res.json({ ok: true, ir });
      } catch {
        return res.status(500).json({ ok: false, diagnostics: [], error: "compiler produced invalid JSON" });
      }
    }

    // Non-zero exit: try to parse JSON diagnostics from stderr.
    const stderrTrimmed = result.stderr.trim();
    try {
      const diagnostics = JSON.parse(stderrTrimmed);
      return res.status(422).json({ ok: false, diagnostics });
    } catch {
      // Fallback: wrap raw stderr as a single diagnostic.
      return res.status(422).json({
        ok: false,
        diagnostics: [{
          severity: "error",
          code: "E000",
          message: stderrTrimmed || "compilation failed",
          loc: { file: "", line: 0, col: 0, end_line: 0, end_col: 0 },
        }],
      });
    }
  } finally {
    unlink(tmpFile).catch(() => {});
  }
});

// ── GET /health ──────────────────────────────────────────────────────────────

app.get("/health", (_req, res) => {
  res.json({ status: "ok", camdlc: CAMDLC });
});

// ── POST /agent/stream ───────────────────────────────────────────────────────
// Proxy to Anthropic API so the API key stays server-side.

app.post("/agent/stream", async (req, res) => {
  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) {
    return res.status(500).json({ error: "ANTHROPIC_API_KEY not set on server" });
  }

  res.setHeader("Content-Type", "text/event-stream");
  res.setHeader("Cache-Control", "no-cache");
  res.setHeader("Connection", "keep-alive");

  try {
    const upstream = await fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "x-api-key": apiKey,
        "anthropic-version": "2023-06-01",
        "anthropic-beta": "interleaved-thinking-2025-05-14",
      },
      body: JSON.stringify({ ...req.body, stream: true }),
    });

    if (!upstream.ok) {
      const err = await upstream.text();
      res.write(`data: ${JSON.stringify({ type: "error", error: err })}\n\n`);
      return res.end();
    }

    const reader = upstream.body!.getReader();
    const decoder = new TextDecoder();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      res.write(decoder.decode(value, { stream: true }));
    }
    res.end();
  } catch (err) {
    res.write(`data: ${JSON.stringify({ type: "error", error: String(err) })}\n\n`);
    res.end();
  }
});

app.listen(PORT, () => {
  console.log(`camdl compiler server listening on http://localhost:${PORT}`);
  console.log(`camdlc: ${CAMDLC}`);
});
