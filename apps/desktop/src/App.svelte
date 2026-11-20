<script lang="ts">
  import { onMount } from "svelte";
  import { api, displayValue, onTransferEvent } from "./lib/api";
  import { envKind, envLabel, kindShort, NAV } from "./lib/ui";
  import type {
    AppInfo,
    ConnectionSummary,
    QueryResult,
    Schema,
    SchemaCatalog,
    SchemaDiff,
    Table,
    ToolStatus,
    TransferPlan,
    TransferPreset,
    VerificationReport,
    ViewId,
  } from "./lib/types";
  import SqlEditor from "./components/SqlEditor.svelte";
  import DataGrid from "./components/DataGrid.svelte";
  import {
    demoCatalog,
    demoConnections,
    demoDiff,
    demoInfo,
    demoPlan,
    demoPresets,
    demoReport,
    demoRows,
    demoTools,
    isTauriRuntime,
  } from "./lib/demo";

  let info = $state<AppInfo | null>(null);
  let connections = $state<ConnectionSummary[]>([]);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let view = $state<ViewId>("browse");
  let selected = $state<string | null>(null);
  let catalog = $state<SchemaCatalog | null>(null);
  let selectedTable = $state<{ schema: string; table: Table } | null>(null);
  let rows = $state<QueryResult | null>(null);
  let sql = $state("SELECT 1");
  let queryResult = $state<QueryResult | null>(null);
  let busy = $state<string | null>(null);

  let xferSource = $state("");
  let xferTarget = $state("");
  let xferMode = $state("clean-recreate");
  let xferYes = $state(false);
  let xferPhrase = $state("");
  let plan = $state<TransferPlan | null>(null);
  let report = $state<VerificationReport | null>(null);
  let events = $state<string[]>([]);
  let tools = $state<ToolStatus[]>([]);
  let presets = $state<TransferPreset[]>([]);
  let diff = $state<SchemaDiff | null>(null);
  let showAdd = $state(false);
  let addName = $state("");
  let addUrl = $state("");
  let addKind = $state("postgres");
  let addEnv = $state("dev");
  let schemaFilter = $state("");
  let preview = $state(false);

  const selectedConn = $derived(connections.find((c) => c.name === selected) ?? null);

  const statusLeft = $derived.by(() => {
    if (!selectedConn) return "No connection";
    const bits = [selectedConn.name, selectedConn.kind, selectedConn.target];
    if (selectedConn.ssh) bits.push(`ssh ${selectedConn.ssh}`);
    return bits.join("  ·  ");
  });

  const statusRight = $derived.by(() => {
    const bits: string[] = [];
    if (busy) bits.push(busy);
    if (view === "browse" && rows) bits.push(`${rows.rows.length} rows  ·  ${rows.duration}ms`);
    if (view === "query" && queryResult) {
      bits.push(
        queryResult.columns.length
          ? `${queryResult.rows.length} rows  ·  ${queryResult.duration}ms`
          : `${queryResult.rows_affected} affected  ·  ${queryResult.duration}ms`,
      );
    }
    if (info) bits.push(`v${info.appVersion}`);
    return bits.join("  ·  ");
  });

  const filteredSchemas = $derived.by(() => {
    if (!catalog) return [];
    const q = schemaFilter.trim().toLowerCase();
    if (!q) return catalog.schemas;
    return catalog.schemas
      .map((schema) => ({
        ...schema,
        tables: schema.tables.filter((t) => t.name.toLowerCase().includes(q)),
        views: schema.views.filter((v) => v.name.toLowerCase().includes(q)),
        sequences: schema.sequences.filter((s) => s.name.toLowerCase().includes(q)),
        functions: schema.functions.filter((f) => f.name.toLowerCase().includes(q)),
      }))
      .filter(
        (s) =>
          s.name.toLowerCase().includes(q) ||
          s.tables.length ||
          s.views.length ||
          s.sequences.length ||
          s.functions.length,
      );
  });

  onMount(() => {
    if (navigator.userAgent.includes("Mac")) document.documentElement.classList.add("mac");
    if (isTauriRuntime()) document.documentElement.classList.add("tauri");
    let unlisten: (() => void) | undefined;
    (async () => {
      if (!isTauriRuntime()) {
        preview = true;
        info = demoInfo;
        connections = demoConnections;
        presets = demoPresets;
        selected = connections[0]?.name ?? null;
        catalog = demoCatalog;
        tools = demoTools;
        xferSource = "prod-pg";
        xferTarget = "staging-pg";
        return;
      }
      try {
        info = await api.appInfo();
        connections = await api.listConnections();
        presets = await api.listPresets();
        selected = connections[0]?.name ?? null;
      } catch (e) {
        error = String(e);
      }
      unlisten = await onTransferEvent((ev) => {
        events = [`${ev.kind}: ${ev.message}`, ...events].slice(0, 40);
      });
    })();
    return () => unlisten?.();
  });

  async function refresh() {
    connections = await api.listConnections();
    presets = await api.listPresets();
  }

  async function selectConn(name: string) {
    selected = name;
    error = null;
    notice = null;
    catalog = null;
    selectedTable = null;
    rows = null;
    tools = [];
    if (preview) {
      catalog = demoCatalog;
      tools = demoTools;
      return;
    }
    busy = "Connecting…";
    try {
      catalog = await api.introspect(name);
      tools = await api.detectTools(name);
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function openTable(schema: Schema, table: Table) {
    if (!selected) return;
    selectedTable = { schema: schema.name, table };
    view = "browse";
    if (preview) {
      rows = demoRows;
      return;
    }
    busy = "Loading rows…";
    try {
      rows = await api.fetchRows(selected, schema.name, table.name, 200, 0);
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function runQuery() {
    if (!selected) return;
    if (preview) {
      queryResult = demoRows;
      return;
    }
    busy = "Running query…";
    try {
      queryResult = await api.executeSql(selected, sql);
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function testConn(name: string) {
    if (preview) {
      notice = `${name} is reachable`;
      return;
    }
    busy = "Testing…";
    try {
      const v = await api.testConnection(name);
      error = null;
      notice = v;
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function addConn() {
    await api.addConnection({
      name: addName,
      kind: addKind,
      env: addEnv,
      url: addUrl || null,
    });
    showAdd = false;
    addName = "";
    addUrl = "";
    await refresh();
  }

  function needsPhrase(): boolean {
    const tgt = connections.find((c) => c.name === xferTarget);
    return Boolean(tgt?.env.includes("production")) && xferMode !== "diff-apply";
  }

  async function buildPlan() {
    if (preview) {
      plan = demoPlan;
      view = "transfer";
      return;
    }
    busy = "Planning…";
    try {
      plan = await api.previewPlan(xferSource, xferTarget, {
        mode: xferMode,
        "schema-only": false,
        "dry-run": true,
        "assume-yes": xferYes,
      });
      view = "transfer";
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function executeTransfer() {
    if (needsPhrase() && xferPhrase !== "TRANSFER") {
      error = "Type TRANSFER to confirm a destructive production operation.";
      return;
    }
    busy = "Transferring…";
    events = [];
    try {
      report = await api.runTransfer(xferSource, xferTarget, {
        mode: xferMode,
        "schema-only": false,
        "dry-run": false,
        "assume-yes": xferYes || xferPhrase === "TRANSFER",
      });
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function doDiff() {
    if (preview) {
      diff = demoDiff;
      return;
    }
    busy = "Diffing…";
    try {
      diff = await api.runDiff(xferSource, xferTarget);
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function doVerify() {
    if (preview) {
      report = demoReport;
      return;
    }
    busy = "Verifying…";
    try {
      report = await api.runVerify(xferSource, xferTarget);
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  async function savePreset() {
    await api.savePreset({
      name: `${xferSource}-to-${xferTarget}`,
      source: xferSource,
      target: xferTarget,
      mode: xferMode,
      exclude_tables: [],
      schema_only: false,
    });
    await refresh();
    notice = "Preset saved";
  }

  function applyPreset(p: TransferPreset) {
    xferSource = p.source;
    xferTarget = p.target;
    xferMode = p.mode;
  }
</script>

<div class="shell">
  <div class="titlebar" data-tauri-drag-region>
    <div class="app-title">MyDB Manager</div>
    <nav class="tabs" aria-label="Views">
      {#each NAV as item (item.id)}
        <button class:on={view === item.id} onclick={() => (view = item.id)}>{item.label}</button>
      {/each}
    </nav>
    <div class="title-actions">
      {#if selected}
        <button onclick={() => testConn(selected!)}>Test</button>
      {/if}
    </div>
  </div>

  <aside class="sidebar">
    <div class="side-head">
      <span>Connections</span>
      <button class="icon-btn" onclick={() => (showAdd = !showAdd)} title="Add connection" aria-label="Add connection">
        {showAdd ? "−" : "+"}
      </button>
    </div>

    {#if showAdd}
      <form
        class="add-form"
        onsubmit={(e) => {
          e.preventDefault();
          addConn();
        }}
      >
        <input placeholder="Name" bind:value={addName} required />
        <div class="row-2">
          <select bind:value={addKind}>
            <option value="postgres">PostgreSQL</option>
            <option value="mysql">MySQL</option>
            <option value="sqlite">SQLite</option>
          </select>
          <select bind:value={addEnv}>
            <option value="dev">Dev</option>
            <option value="staging">Staging</option>
            <option value="prod">Production</option>
            <option value="test">Test</option>
          </select>
        </div>
        <input placeholder="URL or SQLite path" bind:value={addUrl} />
        <button type="submit" class="primary">Save</button>
      </form>
    {/if}

    {#if connections.length === 0}
      <p class="empty">No connections. Click + to add one.</p>
    {:else}
      <ul class="connections">
        {#each connections as conn (conn.name)}
          {@const ek = envKind(conn.env)}
          <li class:active={selected === conn.name}>
            <button class="conn" onclick={() => selectConn(conn.name)}>
              <span class="env-dot {ek}" title={envLabel(conn.env)}></span>
              <span class="conn-name">{conn.name}</span>
              <span class="kind">{kindShort(conn.kind)}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if catalog}
      <div class="side-head schema-head">
        <span>Schema</span>
      </div>
      <input class="filter" placeholder="Filter" bind:value={schemaFilter} />
      <div class="tree-scroll">
        {#each filteredSchemas as schema (schema.name)}
          <details class="schema-group" open>
            <summary>{schema.name}</summary>
            {#each schema.tables as table (table.name)}
              <button
                class="tree"
                class:on={selectedTable?.schema === schema.name && selectedTable?.table.name === table.name}
                onclick={() => openTable(schema, table)}
              >
                {table.name}
              </button>
            {/each}
            {#each schema.views as v (v.name)}
              <div class="tree dim">{v.name}</div>
            {/each}
            {#each schema.sequences as s (s.name)}
              <div class="tree dim seq">
                {s.name}
                <span>{s.last_value ?? "—"}</span>
              </div>
            {/each}
            {#each schema.functions as f (f.name)}
              <div class="tree dim">{f.name}()</div>
            {/each}
          </details>
        {/each}
      </div>
    {/if}
  </aside>

  <div class="workspace">
    {#if error}
      <div class="banner error" role="alert">
        <span>{error}</span>
        <button class="ghost" onclick={() => (error = null)}>Dismiss</button>
      </div>
    {/if}
    {#if notice}
      <div class="banner ok" role="status">
        <span>{notice}</span>
        <button class="ghost" onclick={() => (notice = null)}>Dismiss</button>
      </div>
    {/if}

    {#if view === "browse"}
      <div class="toolbar">
        {#if selectedTable}
          <span class="path">{selectedTable.schema}.{selectedTable.table.name}</span>
          <span class="sep">{selectedTable.table.columns.length} columns</span>
          {#if selectedTable.table.primary_key}
            <span class="sep">PK {selectedTable.table.primary_key.columns.join(", ")}</span>
          {/if}
          {#if selectedTable.table.row_estimate != null}
            <span class="sep">~{selectedTable.table.row_estimate.toLocaleString()} rows</span>
          {/if}
        {:else}
          <span class="muted">Select a table</span>
        {/if}
      </div>
      {#if selectedTable && rows}
        <DataGrid result={rows} />
      {:else}
        <div class="empty-pane">Select a table in the sidebar</div>
      {/if}
    {:else if view === "query"}
      <div class="toolbar">
        <button class="primary" onclick={runQuery} disabled={!selected}>Run</button>
        <span class="muted">⌘↩</span>
      </div>
      <div class="split">
        <div class="split-top">
          <SqlEditor bind:value={sql} onRun={runQuery} />
        </div>
        <div class="split-bottom">
          {#if queryResult}
            <DataGrid result={queryResult} />
          {:else}
            <div class="empty-pane">Run a statement to see results</div>
          {/if}
        </div>
      </div>
    {:else if view === "transfer"}
      <div class="toolbar wrap">
        <label>Source
          <select bind:value={xferSource}>
            <option value="">—</option>
            {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
          </select>
        </label>
        <span class="arrow">→</span>
        <label>Target
          <select bind:value={xferTarget}>
            <option value="">—</option>
            {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
          </select>
        </label>
        <label>Mode
          <select bind:value={xferMode}>
            <option value="clean-recreate">Clean recreate</option>
            <option value="truncate-reload">Truncate reload</option>
            <option value="diff-apply">Diff apply</option>
          </select>
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={xferYes} />
          Confirm
        </label>
        {#if needsPhrase()}
          <input class="phrase" bind:value={xferPhrase} placeholder="TRANSFER" />
        {/if}
        <button class="primary" onclick={buildPlan} disabled={!xferSource || !xferTarget}>Plan</button>
        <button onclick={executeTransfer} disabled={!plan}>Execute</button>
        <button onclick={savePreset} disabled={!xferSource || !xferTarget}>Save preset</button>
      </div>
      {#if presets.length}
        <div class="toolbar sub">
          {#each presets as p (p.name)}
            <button onclick={() => applyPreset(p)}>{p.source} → {p.target}</button>
          {/each}
          {#if tools.length}
            <span class="muted">{tools.map((t) => t.statusLine).join(" · ")}</span>
          {/if}
        </div>
      {/if}
      {#if plan}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Action</th>
                <th>Object</th>
                <th>Rows</th>
              </tr>
            </thead>
            <tbody>
              {#each plan.to_copy as t (t.schema + t.table)}
                <tr>
                  <td>copy</td>
                  <td>{t.schema}.{t.table}</td>
                  <td class="num">~{t.est_rows.toLocaleString()}</td>
                </tr>
              {/each}
              {#each plan.warnings as w (w)}
                <tr class="warn"><td colspan="3">{w}</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
        <pre class="log">{`drop ${plan.to_drop.length}  create ${plan.to_create.length}  copy ${plan.to_copy.length}  sequences ${plan.sequences_to_set.length}`}
{#if events.length}{events.join("\n")}{/if}</pre>
      {:else}
        <div class="empty-pane">Choose source and target, then Plan</div>
      {/if}
      {#if report}
        <div class="report" class:fail={!report.passed}>
          Verification {report.passed ? "passed" : "failed"}
        </div>
      {/if}
    {:else if view === "diff"}
      <div class="toolbar">
        <label>Source
          <select bind:value={xferSource}>
            {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
          </select>
        </label>
        <span class="arrow">→</span>
        <label>Target
          <select bind:value={xferTarget}>
            {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
          </select>
        </label>
        <button class="primary" onclick={doDiff} disabled={!xferSource || !xferTarget}>Compare</button>
      </div>
      {#if diff}
        {#if diff.items.length === 0}
          <div class="empty-pane">No differences</div>
        {:else}
          <div class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Action</th>
                  <th>Kind</th>
                  <th>Object</th>
                  <th>Detail</th>
                </tr>
              </thead>
              <tbody>
                {#each diff.items as item, i (i)}
                  <tr>
                    <td>{item.action}</td>
                    <td>{item.kind}</td>
                    <td>{item.schema}.{item.name}</td>
                    <td>{item.detail}</td>
                  </tr>
                  {#if item.sql}
                    <tr><td colspan="4"><pre class="sql">{item.sql}</pre></td></tr>
                  {/if}
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else}
        <div class="empty-pane">Compare two connections</div>
      {/if}
    {:else if view === "verify"}
      <div class="toolbar">
        <label>Source
          <select bind:value={xferSource}>
            {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
          </select>
        </label>
        <span class="arrow">→</span>
        <label>Target
          <select bind:value={xferTarget}>
            {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
          </select>
        </label>
        <button class="primary" onclick={doVerify} disabled={!xferSource || !xferTarget}>Verify</button>
      </div>
      {#if report}
        <div class="report" class:fail={!report.passed}>
          {report.passed ? "Catalogs match" : "Mismatches found"}
        </div>
        {#if report.row_mismatches.length || report.sequence_mismatches.length}
          <div class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Object</th>
                  <th>Source</th>
                  <th>Target</th>
                </tr>
              </thead>
              <tbody>
                {#each report.row_mismatches as [t, s, d] (t)}
                  <tr><td>{t}</td><td class="num">{s}</td><td class="num">{d}</td></tr>
                {/each}
                {#each report.sequence_mismatches as [n, s, d] (n)}
                  <tr>
                    <td>{n}</td>
                    <td class="num">{displayValue(s)}</td>
                    <td class="num">{displayValue(d)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else}
        <div class="empty-pane">Verify source against target</div>
      {/if}
    {/if}
  </div>

  <footer class="statusbar">
    <span>{statusLeft}</span>
    <span>{statusRight}</span>
  </footer>
</div>

<style>
  .shell {
    display: grid;
    grid-template-columns: 240px 1fr;
    grid-template-rows: 38px 1fr 22px;
    height: 100%;
    background: var(--bg);
  }
  .titlebar {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: 220px 1fr auto;
    align-items: center;
    background: var(--bg-title);
    border-bottom: 1px solid #000;
    padding: 0 10px 0 12px;
    -webkit-app-region: drag;
  }
  :global(html.mac.tauri) .titlebar {
    padding-left: 78px;
    grid-template-columns: 160px 1fr auto;
  }
  .app-title {
    font-weight: 600;
    font-size: 13px;
    pointer-events: none;
  }
  .title-actions {
    display: flex;
    gap: 6px;
    -webkit-app-region: no-drag;
  }
  .tabs {
    display: flex;
    height: 100%;
    -webkit-app-region: no-drag;
  }
  .tabs button {
    height: 100%;
    border: none;
    border-radius: 0;
    background: transparent;
    padding: 0 14px;
    color: var(--text-dim);
    border-bottom: 2px solid transparent;
  }
  .tabs button:hover:not(:disabled) {
    background: transparent;
    color: var(--text);
  }
  .tabs button.on {
    color: var(--text);
    background: #3a3a3c;
    border-bottom-color: var(--accent);
  }
  .sidebar {
    grid-row: 2;
    background: var(--bg-sidebar);
    border-right: 1px solid #000;
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    resize: horizontal;
  }
  .side-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 8px 4px;
    color: var(--text-faint);
    font-size: 11px;
    font-weight: 600;
  }
  .schema-head {
    border-top: 1px solid #000;
    margin-top: 6px;
  }
  .icon-btn {
    width: 18px;
    height: 18px;
    padding: 0;
    line-height: 16px;
  }
  .connections {
    list-style: none;
    margin: 0;
    padding: 0 4px;
  }
  .conn {
    width: 100%;
    height: 24px;
    display: flex;
    align-items: center;
    gap: 6px;
    text-align: left;
    background: transparent;
    border: none;
    padding: 0 6px;
    border-radius: 4px;
  }
  li.active .conn {
    background: var(--bg-selected);
    color: #fff;
  }
  .conn-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind {
    color: var(--text-faint);
    font-size: 10px;
    font-variant: all-small-caps;
  }
  li.active .kind {
    color: rgba(255, 255, 255, 0.7);
  }
  .env-dot {
    width: 7px;
    height: 7px;
    border-radius: 99px;
    background: var(--text-faint);
    flex-shrink: 0;
  }
  .env-dot.prod {
    background: var(--prod);
  }
  .env-dot.staging {
    background: var(--staging);
  }
  .env-dot.dev {
    background: var(--dev);
  }
  .env-dot.test {
    background: var(--test);
  }
  .empty {
    color: var(--text-dim);
    font-size: 11px;
    padding: 4px 10px;
    margin: 0;
  }
  .add-form {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 4px 8px 8px;
  }
  .row-2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .filter {
    margin: 0 8px 6px;
    width: calc(100% - 16px);
  }
  .tree-scroll {
    flex: 1;
    overflow: auto;
    padding: 0 4px 8px;
  }
  .schema-group {
    margin: 0;
  }
  .schema-group summary {
    cursor: default;
    padding: 3px 6px;
    color: var(--text-dim);
    font-size: 11px;
    font-weight: 600;
  }
  .tree {
    display: flex;
    align-items: center;
    width: 100%;
    height: 20px;
    text-align: left;
    background: transparent;
    border: none;
    padding: 0 6px 0 18px;
    font-size: 12px;
    border-radius: 3px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tree:hover {
    background: var(--bg-hover);
  }
  .tree.on {
    background: var(--bg-selected);
    color: #fff;
  }
  .tree.dim {
    color: var(--text-dim);
  }
  .tree.seq span {
    margin-left: auto;
    color: var(--text-faint);
    font-family: var(--mono);
    font-size: 10px;
  }
  .workspace {
    grid-row: 2;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--bg-content);
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 32px;
    padding: 0 8px;
    background: var(--bg-toolbar);
    border-bottom: 1px solid #000;
    flex-shrink: 0;
  }
  .toolbar.wrap {
    flex-wrap: wrap;
    padding: 4px 8px;
    gap: 6px 10px;
  }
  .toolbar.sub {
    background: #323234;
  }
  .toolbar label {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
    font-size: 11px;
  }
  .toolbar .check {
    color: var(--text);
  }
  .path {
    font-weight: 600;
    font-family: var(--mono);
  }
  .sep,
  .muted {
    color: var(--text-dim);
  }
  .arrow {
    color: var(--text-faint);
  }
  .phrase {
    width: 110px;
  }
  .banner {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 4px 8px;
    font-size: 12px;
    flex-shrink: 0;
  }
  .banner.error {
    background: #5c1210;
    color: #ffd4d1;
  }
  .banner.ok {
    background: #0d3d16;
    color: #c6f5ce;
  }
  .empty-pane {
    flex: 1;
    display: grid;
    place-items: center;
    color: var(--text-faint);
  }
  .split {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-rows: minmax(140px, 38%) 1fr;
  }
  .split-top {
    min-height: 0;
    border-bottom: 1px solid #000;
  }
  .split-bottom {
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .table-wrap {
    flex: 1;
    overflow: auto;
    min-height: 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  th,
  td {
    text-align: left;
    padding: 4px 8px;
    border-bottom: 1px solid #2a2a2c;
    user-select: text;
  }
  th {
    position: sticky;
    top: 0;
    background: #3a3a3c;
    color: var(--text-dim);
    font-weight: 600;
    border-bottom: 1px solid #000;
  }
  td.num {
    font-family: var(--mono);
    text-align: right;
  }
  tr.warn td {
    color: var(--warn);
  }
  .sql,
  .log {
    margin: 0;
    padding: 8px;
    font-family: var(--mono);
    font-size: 11px;
    background: #161618;
    color: var(--text-dim);
    white-space: pre-wrap;
    user-select: text;
  }
  .report {
    padding: 6px 8px;
    background: #0d3d16;
    color: #c6f5ce;
    flex-shrink: 0;
  }
  .report.fail {
    background: #5c1210;
    color: #ffd4d1;
  }
  .statusbar {
    grid-column: 1 / -1;
    grid-row: 3;
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0 8px;
    background: var(--bg-title);
    border-top: 1px solid #000;
    color: var(--text-dim);
    font-size: 11px;
    overflow: hidden;
    white-space: nowrap;
    gap: 12px;
  }
  .statusbar span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
