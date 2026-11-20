<script lang="ts">
  import { onMount } from "svelte";
  import { api, displayValue, onTransferEvent } from "./lib/api";
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

  let info = $state<AppInfo | null>(null);
  let connections = $state<ConnectionSummary[]>([]);
  let error = $state<string | null>(null);
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

  onMount(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
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
    catalog = null;
    selectedTable = null;
    rows = null;
    tools = [];
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
    busy = "Testing…";
    try {
      const v = await api.testConnection(name);
      error = null;
      alert(v);
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
  }

  function applyPreset(p: TransferPreset) {
    xferSource = p.source;
    xferTarget = p.target;
    xferMode = p.mode;
  }
</script>

<div class="layout">
  <aside class="sidebar">
    <div class="sidebar-header">
      <h1>MyDB <span>Manager</span></h1>
      {#if info}
        <p class="version">v{info.appVersion} · core {info.coreVersion}</p>
      {/if}
    </div>

    <h2>Connections</h2>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    {#if connections.length === 0}
      <p class="empty">No connections yet.</p>
    {:else}
      <ul class="connections">
        {#each connections as conn (conn.name)}
          <li class:active={selected === conn.name}>
            <button class="conn" onclick={() => selectConn(conn.name)}>
              <div class="conn-name">
                <span>{conn.env.split(" ")[0]}</span>
                {conn.name}
              </div>
              <div class="conn-target mono">{conn.kind} · {conn.target}</div>
              {#if conn.ssh}
                <div class="conn-ssh">ssh {conn.ssh}</div>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    <div class="side-actions">
      <button onclick={() => (showAdd = !showAdd)}>Add connection</button>
      {#if selected}
        <button onclick={() => testConn(selected!)}>Test</button>
      {/if}
    </div>

    {#if showAdd}
      <form
        class="add-form"
        onsubmit={(e) => {
          e.preventDefault();
          addConn();
        }}
      >
        <input placeholder="name" bind:value={addName} required />
        <select bind:value={addKind}>
          <option value="postgres">postgres</option>
          <option value="mysql">mysql</option>
          <option value="sqlite">sqlite</option>
        </select>
        <select bind:value={addEnv}>
          <option value="dev">dev</option>
          <option value="staging">staging</option>
          <option value="prod">prod</option>
          <option value="test">test</option>
        </select>
        <input placeholder="url or sqlite path" bind:value={addUrl} />
        <button type="submit">Save</button>
      </form>
    {/if}

    {#if catalog}
      <h2>Schema</h2>
      {#each catalog.schemas as schema (schema.name)}
        <details open>
          <summary>{schema.name}</summary>
          {#each schema.tables as table (table.name)}
            <button class="tree" onclick={() => openTable(schema, table)}>table {table.name}</button>
          {/each}
          {#each schema.views as v (v.name)}
            <div class="tree dim">view {v.name}</div>
          {/each}
          {#each schema.sequences as s (s.name)}
            <div class="tree dim">
              seq {s.name} = {s.last_value ?? "-"}
            </div>
          {/each}
          {#each schema.functions as f (f.name)}
            <div class="tree dim">fn {f.name}</div>
          {/each}
        </details>
      {/each}
    {/if}

    {#if info}
      <div class="config-path">
        config: <span class="mono">{info.configPath}</span>
      </div>
    {/if}
  </aside>

  <main>
    <nav class="tabs">
      <button class:on={view === "browse"} onclick={() => (view = "browse")}>Browse</button>
      <button class:on={view === "query"} onclick={() => (view = "query")}>SQL</button>
      <button class:on={view === "transfer"} onclick={() => (view = "transfer")}>Transfer</button>
      <button class:on={view === "diff"} onclick={() => (view = "diff")}>Diff</button>
      <button class:on={view === "verify"} onclick={() => (view = "verify")}>Verify</button>
      {#if busy}
        <span class="busy">{busy}</span>
      {/if}
    </nav>

    {#if view === "browse"}
      {#if selectedTable && rows}
        <section>
          <h2>{selectedTable.schema}.{selectedTable.table.name}</h2>
          <p class="dim">
            {selectedTable.table.columns.length} columns
            {#if selectedTable.table.primary_key}
              · PK {selectedTable.table.primary_key.columns.join(", ")}
            {/if}
            {#if selectedTable.table.row_estimate != null}
              · ~{selectedTable.table.row_estimate} rows
            {/if}
          </p>
          <pre class="ddl">{selectedTable.table.columns
              .map((c) => `${c.name} ${c.data_type}${c.nullable ? "" : " NOT NULL"}`)
              .join("\n")}</pre>
          <DataGrid result={rows} />
        </section>
      {:else}
        <section class="hero">
          <h2>Move databases between environments — without losing anything.</h2>
          <p>
            Select a connection to browse tables, sequences (with current values), views,
            functions and triggers. Transfers capture the full object graph and verify the result.
          </p>
        </section>
      {/if}
    {:else if view === "query"}
      <section>
        <SqlEditor bind:value={sql} onRun={runQuery} />
        <div class="actions">
          <button onclick={runQuery} disabled={!selected}>Run (⌘/Ctrl+Enter)</button>
        </div>
        {#if queryResult}
          <DataGrid result={queryResult} />
        {/if}
      </section>
    {:else if view === "transfer"}
      <section>
        <h2>Transfer wizard</h2>
        <div class="form-row">
          <label>Source <select bind:value={xferSource}>
              <option value="">—</option>
              {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
            </select></label>
          <label>Target <select bind:value={xferTarget}>
              <option value="">—</option>
              {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
            </select></label>
          <label>Mode <select bind:value={xferMode}>
              <option value="clean-recreate">clean-recreate</option>
              <option value="truncate-reload">truncate-reload</option>
              <option value="diff-apply">diff-apply</option>
            </select></label>
        </div>
        <label class="check"><input type="checkbox" bind:checked={xferYes} /> Confirm (-y)</label>
        {#if needsPhrase()}
          <label>Type TRANSFER <input bind:value={xferPhrase} placeholder="TRANSFER" /></label>
        {/if}
        {#if tools.length}
          <p class="dim">Tools: {tools.map((t) => t.statusLine).join(" · ") || "native engine"}</p>
        {:else if selected}
          <p class="dim">Tools: not found — using built-in engine</p>
        {/if}
        <div class="actions">
          <button onclick={buildPlan} disabled={!xferSource || !xferTarget}>Plan review</button>
          <button onclick={executeTransfer} disabled={!plan}>Execute</button>
          <button onclick={savePreset} disabled={!xferSource || !xferTarget}>Save preset</button>
        </div>
        {#if presets.length}
          <p>Presets</p>
          {#each presets as p (p.name)}
            <button class="tree" onclick={() => applyPreset(p)}>{p.name}: {p.source} → {p.target}</button>
          {/each}
        {/if}
        {#if plan}
          <pre class="ddl">
drop {plan.to_drop.length} · create {plan.to_create.length} · copy {plan.to_copy.length} tables
{#each plan.to_copy as t (t.schema + t.table)}{t.schema}.{t.table} (~{t.est_rows})
{/each}{#each plan.warnings as w (w)}warning: {w}
{/each}</pre>
        {/if}
        {#if events.length}
          <pre class="ddl">{events.join("\n")}</pre>
        {/if}
        {#if report}
          <div class="report" class:fail={!report.passed}>
            Verification {report.passed ? "passed" : "FAILED"}
          </div>
        {/if}
      </section>
    {:else if view === "diff"}
      <section>
        <h2>Schema diff</h2>
        <div class="form-row">
          <label>Source <select bind:value={xferSource}>
              {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
            </select></label>
          <label>Target <select bind:value={xferTarget}>
              {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
            </select></label>
        </div>
        <button onclick={doDiff} disabled={!xferSource || !xferTarget}>Compare</button>
        {#if diff}
          {#if diff.items.length === 0}
            <p>No differences.</p>
          {:else}
            <ul class="diff">
              {#each diff.items as item, i (i)}
                <li>
                  <strong>{item.action}</strong>
                  {item.kind} {item.schema}.{item.name} — {item.detail}
                  {#if item.sql}<pre class="ddl">{item.sql}</pre>{/if}
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </section>
    {:else if view === "verify"}
      <section>
        <h2>Verification report</h2>
        <div class="form-row">
          <label>Source <select bind:value={xferSource}>
              {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
            </select></label>
          <label>Target <select bind:value={xferTarget}>
              {#each connections as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
            </select></label>
        </div>
        <button onclick={doVerify} disabled={!xferSource || !xferTarget}>Verify</button>
        {#if report}
          <div class="report" class:fail={!report.passed}>
            {report.passed ? "Catalogs match" : "Mismatches found"}
          </div>
          {#each report.row_mismatches as [t, s, d] (t)}
            <div>{t}: source {s} / target {d}</div>
          {/each}
          {#each report.sequence_mismatches as [n, s, d] (n)}
            <div>{n}: source {displayValue(s)} / target {displayValue(d)}</div>
          {/each}
        {/if}
      </section>
    {/if}
  </main>
</div>

<style>
  .layout {
    display: flex;
    height: 100%;
  }
  .sidebar {
    width: 300px;
    min-width: 300px;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow: auto;
  }
  .sidebar-header h1 {
    margin: 0;
    font-size: 20px;
  }
  .sidebar-header span {
    color: var(--accent);
  }
  .version,
  .dim,
  .config-path,
  .empty,
  .conn-target,
  .conn-ssh {
    color: var(--text-dim);
    font-size: 12px;
  }
  h2 {
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    margin: 12px 0 4px;
  }
  .connections {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .conn {
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    padding: 8px;
  }
  li.active .conn,
  .conn:hover {
    background: var(--bg-raised);
  }
  .tree {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    padding: 4px 8px;
    font-size: 12px;
  }
  .tree.dim {
    color: var(--text-dim);
  }
  .error {
    color: #f87171;
    font-size: 12px;
  }
  main {
    flex: 1;
    overflow: auto;
    padding: 20px 28px;
  }
  .tabs {
    display: flex;
    gap: 8px;
    margin-bottom: 16px;
    align-items: center;
  }
  .tabs button.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .busy {
    color: var(--accent);
    font-size: 12px;
  }
  .hero h2 {
    text-transform: none;
    font-size: 28px;
    color: var(--text);
    letter-spacing: 0;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin: 12px 0;
  }
  .form-row {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
  input,
  select {
    font: inherit;
    background: var(--bg-raised);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 8px;
  }
  .ddl {
    background: var(--bg-raised);
    padding: 12px;
    border-radius: 8px;
    overflow: auto;
    font-size: 12px;
  }
  .report {
    margin-top: 12px;
    padding: 12px;
    border-radius: 8px;
    background: #134e4a;
  }
  .report.fail {
    background: #7f1d1d;
  }
  .add-form {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .side-actions {
    display: flex;
    gap: 6px;
  }
  .diff {
    padding-left: 16px;
  }
  .check {
    flex-direction: row;
    align-items: center;
    gap: 8px;
  }
</style>
