<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  interface AppInfo {
    appVersion: string;
    coreVersion: string;
    configPath: string;
  }

  interface ConnectionSummary {
    name: string;
    env: string;
    kind: string;
    target: string;
    ssh: string | null;
  }

  let info = $state<AppInfo | null>(null);
  let connections = $state<ConnectionSummary[]>([]);
  let error = $state<string | null>(null);

  onMount(async () => {
    try {
      info = await invoke<AppInfo>("app_info");
      connections = await invoke<ConnectionSummary[]>("list_connections");
    } catch (e) {
      error = String(e);
    }
  });

  const roadmap = [
    { phase: "Phase 1", label: "Introspection, schema browser, query execution" },
    { phase: "Phase 2", label: "Hybrid dump / restore + verification reports" },
    { phase: "Phase 3", label: "Transfer wizard (prod → staging → dev) + schema diff" },
    { phase: "Phase 4/5", label: "Data grid, SQL editor, transfer & diff UI" },
  ];
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
    {:else if connections.length === 0}
      <p class="empty">
        No connections yet.<br />
        Add one from the terminal:
      </p>
      <code class="hint">mydb conn add prod --url postgres://…</code>
    {:else}
      <ul class="connections">
        {#each connections as conn (conn.name)}
          <li>
            <div class="conn-name">
              <span>{conn.env.split(" ")[0]}</span>
              {conn.name}
            </div>
            <div class="conn-target mono">{conn.kind} · {conn.target}</div>
            {#if conn.ssh}
              <div class="conn-ssh">ssh {conn.ssh}</div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if info}
      <div class="config-path">
        config: <span class="mono">{info.configPath}</span>
      </div>
    {/if}
  </aside>

  <main>
    <section class="hero">
      <h2>Move databases between environments — without losing anything.</h2>
      <p>
        Tables, sequences (with their current values), views, functions,
        triggers, indexes and constraints: the transfer engine captures the
        full object graph, replays it in dependency order, and verifies the
        result.
      </p>
      <div class="actions">
        <button disabled title="Arrives in Phase 5">Transfer…</button>
        <button disabled title="Arrives in Phase 4">New query</button>
      </div>
    </section>

    <section class="roadmap">
      <h3>Roadmap</h3>
      <ul>
        {#each roadmap as item (item.phase)}
          <li><strong>{item.phase}</strong> — {item.label}</li>
        {/each}
      </ul>
    </section>
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
    gap: 12px;
    overflow-y: auto;
  }

  .sidebar-header h1 {
    margin: 0;
    font-size: 1.25rem;
    letter-spacing: 0.02em;
  }

  .sidebar-header h1 span {
    color: var(--accent);
    font-weight: 400;
  }

  .version {
    margin: 4px 0 0;
    color: var(--text-dim);
    font-size: 0.75rem;
  }

  h2 {
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-dim);
    margin: 8px 0 0;
  }

  .empty {
    color: var(--text-dim);
    font-size: 0.85rem;
    line-height: 1.5;
  }

  .hint {
    display: block;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px;
    font-size: 0.72rem;
    color: var(--accent);
    word-break: break-all;
  }

  .error {
    color: #fca5a5;
    font-size: 0.85rem;
  }

  .connections {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .connections li {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px;
    cursor: pointer;
  }

  .connections li:hover {
    border-color: var(--accent-dim);
  }

  .conn-name {
    font-weight: 600;
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .conn-target {
    color: var(--text-dim);
    font-size: 0.72rem;
    margin-top: 4px;
    word-break: break-all;
  }

  .conn-ssh {
    color: var(--text-dim);
    font-size: 0.7rem;
    margin-top: 2px;
  }

  .config-path {
    margin-top: auto;
    color: var(--text-dim);
    font-size: 0.68rem;
    word-break: break-all;
  }

  main {
    flex: 1;
    padding: 40px;
    overflow-y: auto;
  }

  .hero h2 {
    margin: 0 0 12px;
    font-size: 1.5rem;
    max-width: 640px;
    line-height: 1.3;
  }

  .hero p {
    color: var(--text-dim);
    max-width: 640px;
    line-height: 1.6;
    margin: 0 0 20px;
  }

  .actions {
    display: flex;
    gap: 10px;
  }

  .actions button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .roadmap {
    margin-top: 48px;
    max-width: 640px;
  }

  .roadmap h3 {
    color: var(--text-dim);
    text-transform: uppercase;
    font-size: 0.75rem;
    letter-spacing: 0.08em;
  }

  .roadmap ul {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    color: var(--text-dim);
    font-size: 0.88rem;
  }

  .roadmap strong {
    color: var(--text);
  }
</style>
