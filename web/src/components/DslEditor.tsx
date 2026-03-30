import MonacoEditor, { useMonaco } from "@monaco-editor/react";
import type { Monaco } from "@monaco-editor/react";
import { useEffect, useRef } from "react";
import { useIsDark } from "../lib/theme";
import { useStore } from "../store";

const KEYWORDS = [
  "time_unit",
  "compartments",
  "parameters",
  "tables",
  "functions",
  "transitions",
  "observations",
  "interventions",
  "ode",
  "output",
  "simulate",
  "init",
  "timepoints",
  "scenarios",
  "stratify",
  "let",
  "from",
  "to",
  "where",
  "sum",
  "consecutive",
  "in",
  "by",
  "values",
  "only",
  "real",
  "integer",
  "rate",
  "probability",
  "positive",
  "count",
  "and",
  "or",
  "not",
  "if",
  "then",
  "else",
  "coupling",
  "every",
  "at",
  "format",
  "description",
  "tag",
  "null",
  "transfer",
];

function registerCamdl(monaco: Monaco) {
  if (monaco.languages.getLanguages().some((l: { id: string }) => l.id === "camdl")) return;

  monaco.languages.register({ id: "camdl" });
  monaco.languages.setMonarchTokensProvider("camdl", {
    keywords: KEYWORDS,
    tokenizer: {
      root: [
        [/#.*$/, "comment"],
        [/'[a-zA-Z_]+/, "type"],
        [/[a-zA-Z_]\w*/, { cases: { "@keywords": "keyword", "@default": "identifier" } }],
        [/-->/, "operator"],
        [/@/, "operator"],
        [/[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?/, "number"],
        [/"[^"]*"/, "string"],
        [/[{}[\]()]/, "delimiter"],
        [/[+\-*/^=<>!&|]/, "operator"],
      ],
    },
  });

  monaco.editor.defineTheme("camdl-dark", {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "comment", foreground: "6b7280", fontStyle: "italic" },
      { token: "keyword", foreground: "2dd4bf", fontStyle: "bold" },
      { token: "type", foreground: "a78bfa" },
      { token: "operator", foreground: "f9a8d4" },
      { token: "number", foreground: "fbbf24" },
      { token: "string", foreground: "86efac" },
      { token: "identifier", foreground: "e5e7eb" },
      { token: "delimiter", foreground: "9ca3af" },
    ],
    colors: {
      "editor.background": "#0f1117",
      "editor.foreground": "#e5e7eb",
      "editorLineNumber.foreground": "#4b5563",
      "editor.lineHighlightBackground": "#161b22",
      "editorCursor.foreground": "#2dd4bf",
      "editor.selectionBackground": "#1c2128",
      "editorGutter.background": "#0f1117",
    },
  });

  monaco.editor.defineTheme("camdl-light", {
    base: "vs",
    inherit: true,
    rules: [
      { token: "comment", foreground: "94a3b8", fontStyle: "italic" },
      { token: "keyword", foreground: "0d9488", fontStyle: "bold" },
      { token: "type", foreground: "7c3aed" },
      { token: "operator", foreground: "db2777" },
      { token: "number", foreground: "d97706" },
      { token: "string", foreground: "16a34a" },
      { token: "identifier", foreground: "1e293b" },
      { token: "delimiter", foreground: "64748b" },
    ],
    colors: {
      "editor.background": "#ffffff",
      "editor.foreground": "#1e293b",
      "editorLineNumber.foreground": "#94a3b8",
      "editor.lineHighlightBackground": "#f1f5f9",
      "editorCursor.foreground": "#0d9488",
      "editor.selectionBackground": "#e0f2fe",
      "editorGutter.background": "#ffffff",
    },
  });
}

export default function DslEditor() {
  const dslSource = useStore((s) => s.dslSource);
  const setDslSource = useStore((s) => s.setDslSource);
  const diagnostics = useStore((s) => s.diagnostics);
  const highlightedSpan = useStore((s) => s.highlightedSpan);
  const isDark = useIsDark();

  const editorRef = useRef<Monaco["editor"]["IStandaloneCodeEditor"] | null>(null);
  const monaco = useMonaco();

  // Push error markers when diagnostics change
  useEffect(() => {
    if (!monaco || !editorRef.current) return;
    const model = editorRef.current.getModel();
    if (!model) return;

    const markers: Monaco["editor"]["IMarkerData"][] = diagnostics.map((d) => ({
      severity: d.severity === "error"
        ? monaco.MarkerSeverity.Error
        : monaco.MarkerSeverity.Warning,
      message: `[${d.code}] ${d.message}`,
      startLineNumber: d.loc.line || 1,
      startColumn: d.loc.col || 1,
      endLineNumber: d.loc.end_line || d.loc.line || 1,
      endColumn: d.loc.end_col || d.loc.col + 1 || 2,
    }));

    monaco.editor.setModelMarkers(model, "camdl", markers);
  }, [diagnostics, monaco]);

  // Switch Monaco theme when light/dark changes
  useEffect(() => {
    if (!monaco) return;
    monaco.editor.setTheme(isDark ? "camdl-dark" : "camdl-light");
  }, [isDark, monaco]);

  // Scroll to + select highlighted span when canvas node is clicked
  useEffect(() => {
    if (!editorRef.current || !highlightedSpan) return;
    const editor = editorRef.current;
    const { startLine, startCol, endLine, endCol } = highlightedSpan;
    editor.revealLineInCenter(startLine);
    editor.setSelection({
      startLineNumber: startLine,
      startColumn: startCol,
      endLineNumber: endLine,
      endColumn: endCol,
    });
  }, [highlightedSpan]);

  return (
    <MonacoEditor
      height="100%"
      language="camdl"
      theme={isDark ? "camdl-dark" : "camdl-light"}
      value={dslSource}
      beforeMount={registerCamdl}
      onChange={(v) => {
        if (v !== undefined) setDslSource(v);
      }}
      onMount={(editor) => {
        editorRef.current = editor;
        // Stop key events from bubbling to React Flow (which captures Space for panning)
        editor.getDomNode()?.addEventListener("keydown", (e: KeyboardEvent) => e.stopPropagation());
      }}
      options={{
        fontSize: 13,
        lineHeight: 20,
        fontFamily: "\"JetBrains Mono\", \"Fira Code\", Menlo, monospace",
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        renderLineHighlight: "line",
        padding: { top: 12, bottom: 12 },
        wordWrap: "off",
        folding: true,
        glyphMargin: false,
        overviewRulerBorder: false,
        hideCursorInOverviewRuler: true,
        scrollbar: { vertical: "auto", horizontal: "auto", verticalScrollbarSize: 6 },
      }}
    />
  );
}
