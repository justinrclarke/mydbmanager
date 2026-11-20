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
      estimateSize: () => 24,
      overscan: 24,
    });
    const unsub = virtualizer.subscribe((vz) => {
      items = vz.getVirtualItems();
      total = vz.getTotalSize();
    });
    return unsub;
  });
</script>

{#if result.columns.length === 0}
  <div class="ok">OK — {result.rows_affected} rows affected ({result.duration}ms)</div>
{:else}
  <div class="grid" bind:this={parent}>
    <div class="head" style="grid-template-columns: repeat({result.columns.length}, minmax(120px, 1fr));">
      {#each result.columns as col (col.name)}
        <span title={col.type_name}>{col.name}</span>
      {/each}
    </div>
    <div class="body" style="height: {total}px;">
      {#each items as item (item.key)}
        <div
          class="row"
          class:alt={item.index % 2 === 1}
          style="transform: translateY({item.start}px); grid-template-columns: repeat({result.columns.length}, minmax(120px, 1fr));"
        >
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
    padding: 8px 10px;
  }
  .grid {
    overflow: auto;
    flex: 1;
    min-height: 0;
    background: var(--bg-content);
  }
  .head,
  .row {
    display: grid;
    font-size: 12px;
  }
  .head {
    position: sticky;
    top: 0;
    background: #3a3a3c;
    color: var(--text-dim);
    z-index: 1;
    border-bottom: 1px solid #000;
    font-weight: 600;
  }
  .head span,
  .row span {
    padding: 4px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-right: 1px solid #3a3a3c;
    user-select: text;
  }
  .body {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 24px;
    font-family: var(--mono);
    font-size: 11px;
    border-bottom: 1px solid #2a2a2c;
  }
  .row.alt {
    background: #242426;
  }
  .nullish {
    color: var(--text-faint);
    font-style: italic;
    font-family: var(--font);
  }
</style>
