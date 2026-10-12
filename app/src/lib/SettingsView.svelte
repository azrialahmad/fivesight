<script lang="ts">
  import Icon from './Icon.svelte';
  import { session, inTauri } from './session.svelte';
  import { api } from './api';
  import type { BridgeStatus, Settings } from './api';
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

  /** One-click starting points. All of them speak the OpenAI chat API; the address stays editable. */
  const presets = [
    { id: 'openrouter', label: 'OpenRouter', url: 'https://openrouter.ai/api/v1', model: 'google/gemini-2.5-flash' },
    { id: 'openai', label: 'OpenAI', url: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
    { id: 'gemini', label: 'Google Gemini', url: 'https://generativelanguage.googleapis.com/v1beta/openai', model: 'gemini-2.5-flash' },
    { id: 'groq', label: 'Groq', url: 'https://api.groq.com/openai/v1', model: 'llama-3.3-70b-versatile' },
    { id: 'together', label: 'Together', url: 'https://api.together.xyz/v1', model: 'meta-llama/Llama-3.3-70B-Instruct-Turbo' },
    { id: 'ollama', label: 'Ollama (local)', url: 'http://localhost:11434/v1', model: 'gemma3' },
    { id: 'lmstudio', label: 'LM Studio (local)', url: 'http://localhost:1234/v1', model: '' }
  ];

  const norm = (u: string) => u.trim().replace(/\/+$/, '').toLowerCase();
  const selected = $derived(presets.find((p) => norm(p.url) === norm(form.baseUrl))?.id ?? 'custom');
  const isLocal = $derived(/^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])([:/]|$)/i.test(form.baseUrl.trim()));
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

  async function save() {
    await session.saveSettings(form, apiKey.trim() || null);
    apiKey = '';
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
      <input id="fb" class={field} placeholder="Tried if the main model fails" spellcheck="false" bind:value={form.fallbackModel} />

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

      <p class="mt-3 text-xs text-muted">
        {isLocal ? 'Nothing leaves this PC.' : 'Meeting text is sent to this server when you enhance notes or ask a question.'}
      </p>
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
      <h2 class="font-semibold">Speech models</h2>
      <p class="mt-1 break-all text-sm text-muted">
        {session.modelsDir ?? (inTauri ? 'Not found. Put ggml-large-v3-turbo.bin and silero_vad.onnx in %LOCALAPPDATA%\\fivesight\\models.' : 'Not available in browser preview.')}
      </p>
    </section>

    <div class="flex items-center gap-3">
      <button class="rounded-full bg-ink px-6 py-2.5 text-sm font-semibold text-white" onclick={save}>Save settings</button>
      {#if saved}<span class="flex items-center gap-1.5 text-sm font-medium"><Icon name="check" size={18} /> Saved</span>{/if}
    </div>
  </div>
</div>
