<script lang="ts">
  import { onMount } from "svelte";
  import { EditorView, basicSetup } from "codemirror";
  import { sql } from "@codemirror/lang-sql";
  import { oneDark } from "@codemirror/theme-one-dark";
  import { keymap } from "@codemirror/view";
  import { Prec } from "@codemirror/state";

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
      ],
    });
    return () => view?.destroy();
  });
</script>

<div class="editor" bind:this={host}></div>

<style>
  .editor {
    height: 220px;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }
  .editor :global(.cm-editor) {
    height: 100%;
  }
</style>
