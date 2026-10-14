<script lang="ts">
  import Icon from './Icon.svelte';
  import { session, inTauri } from './session.svelte';
  import { api, looksLikeAddress } from './api';
  import { presets, presetFor, isLocalUrl } from './providers';
  import type { AtlassianStatus, BridgeStatus, Settings } from './api';
  import { onMount } from 'svelte';

  let form = $state<Settings>({ ...session.settings });
  let apiKey = $state('');
  let saved = $state(false);
  let keySaved = $state(false);
  let testing = $state(false);
  let test = $state<{ ok: boolean; text: string } | null>(null);

  $effect(() => {
    // Pick up settings that finish loading after this view mounts.
    form = { ...session.settings };
  });

  // Keys are stored per server, so ask again whenever the address changes.
  $effect(() => {
    const url = form.baseUrl;
    let stale = false;
    api.keySaved(url).then((v) => {
      if (!stale) keySaved = v;
    });
    return () => (stale = true);
  });

  let bridge = $state<BridgeStatus | null>(null);
  let atl = $state<AtlassianStatus | null>(null);
  let atlChecking = $state(false);

  async function checkAtlassian() {
    atlChecking = true;
    try {
      atl = await api.atlassianStatus();
    } finally {
      atlChecking = false;
    }
  }
  onMount(() => void checkAtlassian());
  let copied = $state(false);
  let showToken = $state(false);

  onMount(() => {
    const poll = () => api.getBridge().then((b) => (bridge = b)).catch(() => {});
    poll();
    const t = setInterval(poll, 3000);
    return () => clearInterval(t);
  });

  async function copyToken() {
    if (!bridge) return;
    try {
      await navigator.clipboard.writeText(bridge.token);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      showToken = true;
    }
  }

  const languages: [string, string][] = [
    ['auto', 'Indonesian + English (switches per sentence)'],
    ['id', 'Indonesian'],
    ['en', 'English']
  ];

  const speechLabels: Record<string, string> = {
    'large-v3-turbo': 'Large v3 Turbo',
    'large-v3-turbo-q8_0': 'Large v3 Turbo (8-bit)',
    'large-v3-turbo-q5_0': 'Large v3 Turbo (5-bit)',
    'large-v3': 'Large v3',
    'small-q5_1': 'Small (5-bit)'
  };
  const speechLabel = $derived(speechLabels[session.settings.speechModel] ?? session.settings.speechModel);

  const selected = $derived(presetFor(form.baseUrl));
  const isLocal = $derived(isLocalUrl(form.baseUrl));
  const insecure = $derived(/^http:\/\//i.test(form.baseUrl.trim()) && !isLocal);

  function pick(id: string) {
    test = null;
    apiKey = '';
    const p = presets.find((x) => x.id === id);
    if (p) {
      form.baseUrl = p.url;
      form.model = p.model;
    } else {
      form.baseUrl = '';
      form.model = '';
    }
  }

  let notice = $state<string | null>(null);
  onMount(() => void api.settingsNotice().then((n) => (notice = n)).catch(() => {}));

  // Nothing is kept until Save, so say when the form differs from what's saved.
  const dirty = $derived.by(() => {
    const s = session.settings;
    return (
      form.baseUrl.trim() !== s.baseUrl.trim() ||
      form.model.trim() !== s.model.trim() ||
      form.fallbackModel.trim() !== s.fallbackModel.trim() ||
      form.language !== s.language ||
      form.atlassian !== s.atlassian ||
      form.autoNotes !== s.autoNotes ||
      form.vocabulary.trim() !== s.vocabulary.trim() ||
      apiKey.trim() !== ''
    );
  });
  const modelIsAddress = $derived(looksLikeAddress(form.model));
  const fallbackIsAddress = $derived(looksLikeAddress(form.fallbackModel));
  let saveError = $state<string | null>(null);

  async function save() {
    saveError = null;
    try {
      await session.saveSettings(form, apiKey.trim() || null);
    } catch (e) {
      saveError = String(e);
      return;
    }
    apiKey = '';
    notice = null;
    keySaved = await api.keySaved(form.baseUrl);
    saved = true;
    setTimeout(() => (saved = false), 2000);
  }

  async function runTest() {
    testing = true;
    test = null;
    try {
      test = { ok: true, text: await api.testLlm(form, apiKey.trim() || null) };
    } catch (e) {
      test = { ok: false, text: String(e) };
    } finally {
      testing = false;
    }
  }

  async function forget() {
    await api.forgetKey(form.baseUrl);
    keySaved = false;
    test = null;
  }

  const field = 'mt-2 w-full rounded-xl border border-line bg-paper px-3.5 py-2.5 text-sm outline-none focus:border-ink';
  const chip = 'rounded-full border px-3.5 py-1.5 text-[13px] font-medium transition-colors';
</script>

<div class="h-full overflow-y-auto">
  <div class="mx-auto max-w-xl space-y-8 px-8 pb-12 pt-8">
    <h1 class="text-2xl font-semibold tracking-tight">Settings</h1>
    {#if notice}
      <p class="break-words rounded-xl bg-rec-soft px-4 py-3 text-sm text-rec" role="alert">{notice}</p>
    {/if}

    <section>
      <label class="font-semibold" for="lang">Language for new meetings</label>
      <p class="mt-0.5 text-sm text-muted">
        You can change it per meeting, right in the meeting. If your calls are mostly one language, pinning it is the most accurate.
      </p>
      <select id="lang" class={field} bind:value={form.language}>
        {#each languages as [v, label]}<option value={v}>{label}</option>{/each}
      </select>
    </section>

    <section>
      <label class="font-semibold" for="vocab">Words to listen for</label>
      <p class="mt-0.5 text-sm text-muted">
        Names, products and jargon the transcript keeps getting wrong. Separate them with commas or new lines.
        The meeting title, people on the call and names in your notes are picked up on their own.
      </p>
      <textarea
        id="vocab"
        rows="3"
        class="{field} resize-y"
        placeholder="Playwright, Claude, backend engineer"
        spellcheck="false"
        bind:value={form.vocabulary}
      ></textarea>
    </section>

    <section>
      <h2 class="font-semibold">AI model</h2>
      <p class="mt-0.5 text-sm text-muted">
        Bring your own key. Works with any service that speaks the OpenAI chat API. Audio is always transcribed on this PC.
      </p>

      <div class="mt-3 flex flex-wrap gap-2" role="radiogroup" aria-label="AI provider">
        {#each presets as p}
          <button
            class="{chip} {selected === p.id ? 'border-ink bg-ink text-white' : 'border-line hover:border-ink'}"
            role="radio"
            aria-checked={selected === p.id}
            onclick={() => pick(p.id)}
          >
            {p.label}
          </button>
        {/each}
        <button
          class="{chip} {selected === 'custom' ? 'border-ink bg-ink text-white' : 'border-line hover:border-ink'}"
          role="radio"
          aria-checked={selected === 'custom'}
          onclick={() => pick('custom')}
        >
          Custom endpoint
        </button>
      </div>

      <label class="mt-5 block text-sm font-medium" for="url">Server address</label>
      <input id="url" class={field} placeholder="https://your-gateway.example.com/v1" spellcheck="false" bind:value={form.baseUrl} />
      <p class="mt-1.5 text-xs text-muted">
        The base address, usually ending in /v1. Pasting the full /chat/completions address also works.
      </p>
      {#if insecure}
        <p class="mt-1.5 text-xs font-medium text-rec">This address isn't encrypted (http). Your key and meeting text would travel in the clear.</p>
      {/if}

      <label class="mt-4 block text-sm font-medium" for="model">Model</label>
      <input id="model" class={field} placeholder="model name as the provider spells it" spellcheck="false" bind:value={form.model} />
      {#if modelIsAddress}
        <p class="mt-1.5 text-xs font-medium text-rec">This is a server address. Put it in Server address above; this field takes a model name.</p>
      {/if}

      <label class="mt-4 block text-sm font-medium" for="key">
        API key {#if isLocal}<span class="font-normal text-faint">(not needed for a local server)</span>{/if}
      </label>
      <input
        id="key"
        type="password"
        autocomplete="off"
        class={field}
        placeholder={keySaved ? 'Saved for this server. Type to replace.' : isLocal ? 'Leave empty' : 'Paste your key'}
        bind:value={apiKey}
      />
      <p class="mt-1.5 text-xs text-muted">
        Stored in Windows Credential Manager, one per server, and only ever sent to that server.
        {#if keySaved}
          <button class="ml-1 font-medium text-ink underline" onclick={forget}>Remove saved key</button>
        {/if}
      </p>

      <label class="mt-4 block text-sm font-medium" for="fb">Fallback model <span class="font-normal text-faint">(optional)</span></label>
      <input id="fb" class={field} placeholder="Another model on the same server, tried if the main one fails" spellcheck="false" bind:value={form.fallbackModel} />
      {#if fallbackIsAddress}
        <p class="mt-1.5 text-xs font-medium text-rec">
          This is a server address. The fallback is another model name on the same server; put the address in Server address above.
        </p>
      {/if}

      <div class="mt-4 flex flex-wrap items-center gap-3">
        <button
          class="rounded-full border border-line px-4 py-2 text-[13px] font-medium hover:border-ink disabled:opacity-50"
          disabled={testing || !form.baseUrl.trim() || !form.model.trim()}
          onclick={runTest}
        >
          {testing ? 'Testing…' : 'Test connection'}
        </button>
        {#if test}
          <span class="flex items-start gap-1.5 text-sm {test.ok ? 'font-medium' : 'text-rec'}" role="status">
            {#if test.ok}<Icon name="check" size={18} />{/if}
            <span class="min-w-0 break-words">{test.text}</span>
          </span>
        {/if}
      </div>

      <label class="mt-5 flex cursor-pointer items-start gap-2.5 text-sm">
        <input type="checkbox" class="mt-0.5 h-4 w-4 accent-ink" bind:checked={form.autoNotes} />
        <span>
          Update AI notes automatically while recording
          <span class="mt-0.5 block text-xs text-muted">
            When you pause typing and as the meeting goes on: about one AI request a minute while things change, and one when you stop.
          </span>
        </span>
      </label>

      <p class="mt-3 text-xs text-muted">
        {isLocal ? 'Nothing leaves this PC.' : 'Meeting text is sent to this server when AI notes are written or you ask a question.'}
      </p>
    </section>

    <section>
      <div class="flex items-center gap-3">
        <h2 class="font-semibold">Jira & Confluence</h2>
        <span class="rounded-full bg-soft px-2.5 py-0.5 text-[11px] font-semibold tracking-wide">READ-ONLY</span>
      </div>
      <p class="mt-0.5 text-sm text-muted">
        Ask can search and read Jira tickets (through acli) and Confluence pages with their comments (through confluence-cli). It can never create, edit, comment on, assign or move anything.
      </p>
      <div class="mt-3 space-y-1.5 text-sm">
        {#if atlChecking && !atl}
          <p class="text-muted">Checking…</p>
        {:else if atl}
          <p class="flex items-center gap-2">
            <span class="h-2 w-2 shrink-0 rounded-full {atl.jiraSite ? 'bg-lime-deep' : 'bg-faint'}"></span>
            {#if atl.jiraSite}<span>Jira: <span class="font-medium">{atl.jiraSite}</span></span>
            {:else if !atl.installed}<span class="text-muted">Jira: acli isn't installed. See Atlassian's install guide.</span>
            {:else}<span class="text-muted">Jira: not logged in. Run <code class="rounded bg-soft px-1.5 py-0.5 text-xs">acli jira auth login --web</code></span>{/if}
          </p>
          <p class="flex items-center gap-2">
            <span class="h-2 w-2 shrink-0 rounded-full {atl.confluence ? 'bg-lime-deep' : 'bg-faint'}"></span>
            {#if atl.confluence}<span>Confluence: connected</span>
            {:else if !atl.confluenceCli}<span class="text-muted">Confluence: install it with <code class="rounded bg-soft px-1.5 py-0.5 text-xs">npm install -g confluence-cli</code></span>
            {:else}<span class="text-muted">Confluence: not set up. Run <code class="rounded bg-soft px-1.5 py-0.5 text-xs">confluence init</code></span>{/if}
          </p>
        {/if}
      </div>
      <div class="mt-3 flex flex-wrap items-center gap-3">
        <label class="flex cursor-pointer items-center gap-2.5 text-sm">
          <input type="checkbox" class="h-4 w-4 accent-ink" bind:checked={form.atlassian} />
          Let Ask look things up in Jira and Confluence
        </label>
        <button class="ml-auto rounded-full border border-line px-4 py-1.5 text-[13px] font-medium hover:border-ink disabled:opacity-50" disabled={atlChecking} onclick={checkAtlassian}>
          {atlChecking ? 'Checking…' : 'Check again'}
        </button>
      </div>
      <p class="mt-1.5 text-xs text-muted">Ticket text goes to your AI model only when a question needs it. Each lookup is shown under the answer.</p>
    </section>

    <section>
      <h2 class="font-semibold">Google Meet</h2>
      <p class="mt-0.5 text-sm text-muted">
        The fivesight browser extension tells the app who is talking, so the transcript shows names instead of "Others". It also lets fivesight offer to record when you join a call. It never sends audio or video, and it shows nothing on the Meet page.
      </p>
      <div class="mt-3 flex items-center gap-2 text-sm">
        {#if bridge?.error}
          <span class="h-2 w-2 rounded-full bg-rec"></span><span class="text-rec">{bridge.error}</span>
        {:else if bridge?.connected}
          <span class="h-2 w-2 rounded-full bg-lime-deep"></span><span class="font-medium">Extension connected{bridge.inCall ? ' · in a call' : ''}</span>
        {:else}
          <span class="h-2 w-2 rounded-full bg-faint"></span><span class="text-muted">Extension not connected</span>
        {/if}
      </div>
      <ol class="mt-3 list-decimal space-y-1 pl-5 text-sm text-muted">
        <li>In Chrome, open <span class="font-medium text-ink">chrome://extensions</span>, turn on Developer mode, and choose Load unpacked.</li>
        <li>Pick the <span class="font-medium text-ink">extension</span> folder from the fivesight project.</li>
        <li>Open the extension's icon, paste the pairing code below, and press Connect.</li>
      </ol>
      <div class="mt-3 flex items-center gap-2">
        <input
          class="{field} mt-0 flex-1 font-mono text-xs"
          readonly
          type={showToken ? 'text' : 'password'}
          value={bridge?.token ?? ''}
          aria-label="Pairing code"
          onfocus={() => (showToken = true)}
        />
        <button class="shrink-0 rounded-full border border-line px-4 py-2.5 text-[13px] font-medium hover:border-ink" onclick={copyToken}>
          {copied ? 'Copied' : 'Copy code'}
        </button>
      </div>
      <p class="mt-1.5 text-xs text-muted">Only your browser extension can use this code. It stays on this PC.</p>
    </section>

    <section>
      <div class="flex items-center gap-3">
        <h2 class="font-semibold">Speech model</h2>
        <button class="ml-auto rounded-full border border-line px-4 py-1.5 text-[13px] font-medium hover:border-ink" onclick={() => session.openSetup()}>
          Run setup again
        </button>
      </div>
      <p class="mt-1 text-sm">{speechLabel}</p>
      <p class="mt-0.5 break-all text-sm text-muted">
        {session.modelsDir ?? (inTauri ? 'Not downloaded yet. Run setup to pick and download it.' : 'Not available in browser preview.')}
      </p>
    </section>

  </div>
  <!-- Stays in view while scrolling, so changes at the top aren't left unsaved by accident. -->
  <div class="sticky bottom-0 border-t border-line bg-card/95 backdrop-blur">
    <div class="mx-auto flex max-w-xl items-center gap-3 px-8 py-3">
      <button
        class="shrink-0 rounded-full bg-ink px-6 py-2.5 text-sm font-semibold text-white disabled:opacity-40"
        disabled={modelIsAddress || fallbackIsAddress}
        onclick={save}
      >
        Save settings
      </button>
      {#if saveError}
        <span class="min-w-0 break-words text-sm font-medium text-rec" role="alert">{saveError}</span>
      {:else if saved}
        <span class="flex items-center gap-1.5 text-sm font-medium"><Icon name="check" size={18} /> Saved</span>
      {:else if dirty}
        <span class="flex items-center gap-2 text-sm font-medium"><span class="h-2 w-2 rounded-full bg-lime-deep"></span>Unsaved changes</span>
      {/if}
    </div>
  </div>
</div>
