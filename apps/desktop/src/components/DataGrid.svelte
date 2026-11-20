<script lang="ts">
  import { createVirtualizer } from "@tanstack/svelte-virtual";
  import { displayValue } from "../lib/api";
  import type { QueryResult } from "../lib/types";

  let { result }: { result: QueryResult } = $props();

  let parent = $state<HTMLDivElement | undefined>(undefined);
  let items = $state<{ key: unknown; index: number; start: number }[]>([]);
  let total = $state(0);

  $effect(() => {
    const el = parent;
    const count = result.rows.length;
    if (!el) return;
    const virtualizer = createVirtualizer({
      count,
      getScrollElement: () => el,
      estimateSize: () => 32,
      overscan: 16,
    });
    const unsub = virtualizer.subscribe((vz) => {
      items = vz.getVirtualItems();
      total = vz.getTotalSize();
    });
    return unsub;
  });
</script>

{#if result.columns.length === 0}
  <p class="ok">OK ({result.rows_affected} rows affected, {result.duration}ms)</p>
{:else}
  <div class="grid" bind:this={parent}>
    <div class="head">
      {#each result.columns as col (col.name)}
        <span>{col.name}</span>
      {/each}
    </div>
    <div class="body" style="height: {total}px;">
      {#each items as item (item.key)}
        <div class="row" style="transform: translateY({item.start}px);">
          {#each result.rows[item.index] ?? [] as cell, i (`${item.index}-${i}`)}
            <span class:nullish={displayValue(cell) === "NULL"}>{displayValue(cell)}</span>
          {/each}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .ok {
    color: var(--text-dim);
  }
  .grid {
    overflow: auto;
    max-height: min(60vh, 520px);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .head,
  .row {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(140px, 1fr);
    font-size: 12px;
  }
  .head {
    position: sticky;
    top: 0;
    background: var(--bg-panel);
    color: var(--text-dim);
    z-index: 1;
  }
  .head span,
  .row span {
    padding: 8px 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .body {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 32px;
    border-bottom: 1px solid var(--border);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .nullish {
    color: var(--text-dim);
    font-style: italic;
  }
</style>
