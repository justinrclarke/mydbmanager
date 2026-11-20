<script lang="ts">
  import { onMount } from "svelte";
  import { EditorView, basicSetup } from "codemirror";
  import { sql } from "@codemirror/lang-sql";
  import { keymap } from "@codemirror/view";
  import { Prec, EditorState } from "@codemirror/state";
  import { oneDark } from "@codemirror/theme-one-dark";

  let {
    value = $bindable(""),
    onRun,
  }: {
    value?: string;
    onRun?: () => void;
  } = $props();

  let host = $state<HTMLDivElement | null>(null);
  let view: EditorView | undefined;

  onMount(() => {
    if (!host) return;
    view = new EditorView({
      doc: value,
      parent: host,
      extensions: [
        basicSetup,
        sql(),
        oneDark,
        EditorView.theme({
          "&": {
            height: "100%",
            fontSize: "12px",
            backgroundColor: "#1e1e20",
          },
          ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "1.45" },
          ".cm-gutters": {
            backgroundColor: "#2a2a2c",
            color: "#6e6e73",
            borderRight: "1px solid #000",
          },
          ".cm-activeLine": { backgroundColor: "#2c2c2e" },
          ".cm-activeLineGutter": { backgroundColor: "#2c2c2e" },
        }),
        Prec.highest(
          keymap.of([
            {
              key: "Mod-Enter",
              run: () => {
                onRun?.();
                return true;
              },
            },
          ]),
        ),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) value = u.state.doc.toString();
        }),
        EditorState.tabSize.of(2),
      ],
    });
    return () => view?.destroy();
  });
</script>

<div class="editor" bind:this={host}></div>

<style>
  .editor {
    height: 100%;
    min-height: 0;
    overflow: hidden;
    background: #1e1e20;
  }
  .editor :global(.cm-editor) {
    height: 100%;
  }
  .editor :global(.cm-focused) {
    outline: none;
  }
</style>
