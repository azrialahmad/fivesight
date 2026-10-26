<script lang="ts">
  import Icon from './Icon.svelte';
  import { session } from './session.svelte';
  import { api } from './api';
  import { presets, presetFor, isLocalUrl } from './providers';
  import { holdsSetup, missingModels, startsDownload } from './downloads';
  import type { AtlassianStatus, BridgeStatus, DownloadEvent, LocalServer, Settings, SetupInfo } from './api';
  import { onMount, untrack } from 'svelte';

  const steps = ['This PC', 'Speech model', 'AI model', 'Integrations'];
  let step = $state(0);
  let scroller = $state<HTMLElement>();
  $effect(() => {
    void step;
    scroller?.scrollTo(0, 0);
  });

  let form = $state<Settings>({ ...session.settings });
  let apiKey = $state('');

  let info = $state<SetupInfo | null>(null);
  let infoError = $state<string | null>(null);
  let locals = $state<LocalServer[]>([]);
  let atl = $state<AtlassianStatus | null>(null);
  let bridge = $state<BridgeStatus | null>(null);

  /** The speech model fivesight will use, and extra ones to fetch while we're at it. */
  let active = $state(session.settings.speechModel);
  let extras = $state<string[]>([]);

  onMount(() => {
    api
      .setupInfo()
      .then((i) => {
        info = i;
        // Keep a model that's already downloaded; otherwise start from the recommendation.
        const current = i.speechModels.find((m) => m.id === active);
        if (!(current?.present) && i.recommendation.speechModel) active = i.recommendation.speechModel;
      })
      .catch((e) => (infoError = String(e)));
    api.detectLocalLlms().then((l) => (locals = l)).catch(() => {});
    api.atlassianStatus().then((a) => (atl = a)).catch(() => {});
    // Poll, so "connected" shows up while the extension is being added.
    const pollBridge = () => api.getBridge().then((b) => (bridge = b)).catch(() => {});
    pollBridge();
    const bridgeTimer = setInterval(pollBridge, 3000);
    // Progress itself is tracked by the session; here, only notice what's on disk now.
    let off: (() => void) | undefined;
    api.onModelDownload((e) => (e.done || e.error) && refresh()).then((o) => (off = o));
    return () => {
      clearInterval(bridgeTimer);
      off?.();
    };
  });

  const gb = (mb: number) => (mb >= 10_240 ? Math.round(mb / 1024) : Math.round(mb / 102.4) / 10) + ' GB';
  const size = (bytes: number) => (bytes >= 1e9 ? (bytes / 1e9).toFixed(1) + ' GB' : Math.round(bytes / 1e6) + ' MB');

  // ---------- downloads ----------

  const wanted = $derived([active, ...extras.filter((x) => x !== active)]);
  const missing = $derived(info ? missingModels(info, wanted) : null);
  const activeReady = $derived(!!info && info.vadPresent && !!info.speechModels.find((m) => m.id === active)?.present);
  const dl = $derived(session.download);
  const pct = (e: DownloadEvent) => (e.total ? Math.floor((e.received / e.total) * 100) : 0);

  async function refresh() {
    try {
      info = await api.setupInfo();
    } catch {}
  }

  /** Paused by hand: leave it paused until Download is pressed again. */
  let paused = $state(false);
  const auto = $derived(!paused && startsDownload(info, wanted, session.downloading));

  function download() {
    paused = false;
    session.downloadModels(wanted);
  }

  function pause() {
    paused = true;
    api.cancelDownload();
  }

  /** Moving on from the speech model step (or finishing) downloads what was picked, unasked. */
  function go(to: number) {
    if (to > 1 && auto) download();
    waiting = false;
    step = to;
  }

  function toggleExtra(id: string) {
    extras = extras.includes(id) ? extras.filter((x) => x !== id) : [...extras, id];
  }

  // ---------- AI model ----------

  const selected = $derived(presetFor(form.baseUrl));
  const isLocal = $derived(isLocalUrl(form.baseUrl));
  let testing = $state(false);
  let test = $state<{ ok: boolean; text: string } | null>(null);

  function pick(url: string, model: string) {
    test = null;
    apiKey = '';
    form.baseUrl = url;
    form.model = model;
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

  // ---------- finish ----------

  let finishing = $state(false);
  let finishError = $state<string | null>(null);
  let copied = $state(false);

  /** Finish was pressed before the models were on disk: setup stays open until they are. */
  let waiting = $state(false);
  const held = $derived(holdsSetup(info, active));
  /** Leaving without finishing only once the app can record (or never could, on this PC). */
  const canLeave = $derived(!!session.modelsDir || !!infoError || (!!info && !info.recommendation.supported));

  $effect(() => {
    if (waiting && !held) {
      waiting = false;
      untrack(() => finish());
    }
  });

  async function finish(skip = false) {
    if (!skip && held) {
      waiting = true;
      // Finishing is a clear go-ahead, even after a pause.
      if (!session.downloading) download();
      return;
    }
    finishing = true;
    finishError = null;
    try {
      if (skip) await session.finishSetup(session.settings, null);
      else await session.finishSetup({ ...form, speechModel: active }, apiKey.trim() || null);
    } catch (e) {
      finishError = String(e);
    } finally {
      finishing = false;
    }
  }

  async function copyToken() {
    if (!bridge) return;
    try {
      await navigator.clipboard.writeText(bridge.token);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {}
  }

  const field = 'mt-2 w-full rounded-xl border border-line bg-paper px-3.5 py-2.5 text-sm outline-none focus:border-ink';
  const chip = 'rounded-full border px-3.5 py-1.5 text-[13px] font-medium transition-colors';
  const primary = 'rounded-full bg-ink px-6 py-2.5 text-sm font-semibold text-white disabled:opacity-40';
  const secondary = 'rounded-full border border-line px-4 py-2 text-[13px] font-medium hover:border-ink disabled:opacity-50';
</script>

<div class="flex h-full flex-col">
  <header class="flex items-center gap-2.5 px-8 pt-6">
    <span class="grid h-8 w-8 place-items-center rounded-[10px] bg-ink text-lime"><Icon name="logo" size={18} /></span>
    <span class="text-[19px] font-semibold tracking-tight">fivesight</span>
    {#if canLeave}
      <button class="ml-auto rounded-full px-3 py-1.5 text-[13px] text-muted hover:bg-soft disabled:opacity-50" disabled={finishing} onclick={() => finish(true)}>
        {session.settings.setupDone ? 'Close' : 'Skip setup'}
      </button>
    {/if}
  </header>

  <div class="min-h-0 flex-1 overflow-y-auto" bind:this={scroller}>
    <div class="mx-auto max-w-2xl px-8 pb-10 pt-6">
      <ol class="flex gap-2" aria-label="Setup steps">
        {#each steps as s, i}
          <li class="flex-1">
            <button class="w-full text-left" onclick={() => go(i)} aria-current={step === i ? 'step' : undefined}>
              <span class="block h-1 rounded-full {i <= step ? 'bg-ink' : 'bg-line'}"></span>
              <span class="mt-1.5 block text-xs {i === step ? 'font-semibold' : 'text-muted'}">{i + 1}. {s}</span>
            </button>
          </li>
        {/each}
      </ol>

      {#if waiting}
        <h1 class="mt-8 text-2xl font-semibold tracking-tight">Almost done</h1>
        <p class="mt-1 text-sm text-muted">
          fivesight needs the speech model and the voice detection model on this PC to record. It opens as soon as they're downloaded.
        </p>
        <div class="mt-6 rounded-2xl bg-soft px-5 py-4">
          {#if session.downloadError}
            <p class="break-words text-sm text-rec" role="alert">{session.downloadError}</p>
            <p class="mt-1 text-xs text-muted">Trying again picks up where it stopped.</p>
            <button class="{primary} mt-3" onclick={download}>Try again</button>
          {:else if dl}
            <p class="text-sm font-medium">Downloading {dl.file} <span class="text-muted">({dl.index + 1} of {dl.count})</span></p>
            <div class="mt-2 h-2 overflow-hidden rounded-full bg-line">
              <div class="h-full rounded-full bg-ink transition-[width]" style="width: {pct(dl)}%"></div>
            </div>
            <p class="mt-1.5 text-xs text-muted" role="status">{size(dl.received)} of {size(dl.total)} · {pct(dl)}%</p>
          {:else}
            <p class="text-sm text-muted" role="status">Starting the download…</p>
          {/if}
        </div>

      {:else if step === 0}
        <h1 class="mt-8 text-2xl font-semibold tracking-tight">Let's set up fivesight for this PC</h1>
        <p class="mt-1 text-sm text-muted">fivesight transcribes on your own graphics card, so the options depend on what's in this PC.</p>

        {#if infoError}
          <p class="mt-6 rounded-xl bg-rec-soft px-4 py-3 text-sm text-rec" role="alert">{infoError}</p>
        {:else if !info}
          <p class="mt-6 text-sm text-muted">Looking at this PC…</p>
        {:else}
          {@const sys = info.system}
          {@const rec = info.recommendation}
          <dl class="mt-6 grid grid-cols-[auto_1fr] gap-x-6 gap-y-2.5 rounded-2xl border border-line p-5 text-sm">
            <dt class="text-muted">Processor</dt>
            <dd>{sys.cpu || 'Unknown'} <span class="text-muted">· {sys.cores} threads</span></dd>
            <dt class="text-muted">Memory</dt>
            <dd>{gb(sys.ramMb)}</dd>
            <dt class="text-muted">Graphics</dt>
            <dd>
              {#each sys.gpus as g}
                <span class="block">{g.name} <span class="text-muted">· {gb(g.vramMb)}</span></span>
              {:else}
                <span class="text-muted">None found</span>
              {/each}
            </dd>
            <dt class="text-muted">Free space</dt>
            <dd>{sys.freeDiskMb === null ? 'Unknown' : gb(sys.freeDiskMb)}</dd>
          </dl>

          <div class="mt-4 flex items-start gap-3 rounded-2xl px-5 py-4 {rec.supported ? 'bg-soft' : 'bg-rec-soft'}">
            {#if rec.supported}
              <span class="mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full bg-lime text-ink"><Icon name="check" size={15} /></span>
            {:else}
              <span class="mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full bg-rec text-white"><Icon name="close" size={15} /></span>
            {/if}
            <div class="text-sm">
              <p class={rec.supported ? 'font-medium' : 'font-medium text-rec'}>{rec.summary}</p>
              {#each rec.warnings as w}<p class="mt-1 text-muted">{w}</p>{/each}
            </div>
          </div>
        {/if}

        <label class="mt-6 block text-sm font-medium" for="su-me">What should fivesight call you?</label>
        <input id="su-me" class={field} placeholder="Your name as people say it in calls" spellcheck="false" bind:value={form.userName} />
        <p class="mt-1.5 text-xs text-muted">
          Used instead of "You" in transcripts and AI notes, so it's clear when someone talks to you. Optional.
        </p>

      {:else if step === 1}
        <h1 class="mt-8 text-2xl font-semibold tracking-tight">Speech model</h1>
        <p class="mt-1 text-sm text-muted">
          Turns the call's audio into text, on this PC. Pick the one to use. You only download what you pick.
        </p>

        {#if info}
          <div class="mt-5 space-y-2" role="radiogroup" aria-label="Speech model">
            {#each info.speechModels as m}
              {@const isActive = active === m.id}
              {@const recommended = info.recommendation.speechModel === m.id}
              <div class="rounded-2xl border p-4 {isActive ? 'border-ink' : 'border-line'}">
                <button class="flex w-full items-start gap-3 text-left" role="radio" aria-checked={isActive} onclick={() => (active = m.id)}>
                  <span class="mt-0.5 grid h-5 w-5 shrink-0 place-items-center rounded-full border-2 {isActive ? 'border-ink' : 'border-line'}">
                    {#if isActive}<span class="h-2.5 w-2.5 rounded-full bg-ink"></span>{/if}
                  </span>
                  <span class="min-w-0 flex-1">
                    <span class="flex flex-wrap items-center gap-2">
                      <span class="text-sm font-semibold">{m.label}</span>
                      {#if recommended}<span class="rounded-full bg-lime px-2 py-0.5 text-[11px] font-semibold">Recommended</span>{/if}
                      {#if m.present}<span class="rounded-full bg-soft px-2 py-0.5 text-[11px] font-semibold">Downloaded</span>{/if}
                    </span>
                    <span class="mt-0.5 block text-[13px] text-muted">{m.note}</span>
                    <span class="mt-1 block text-xs text-faint">{size(m.bytes)} download · about {gb(m.vramMb)} of graphics memory</span>
                  </span>
                </button>
                {#if !isActive && !m.present}
                  <label class="ml-8 mt-2 flex cursor-pointer items-center gap-2 text-xs text-muted">
                    <input type="checkbox" class="h-3.5 w-3.5 accent-ink" checked={extras.includes(m.id)} onchange={() => toggleExtra(m.id)} />
                    Also download, to try later
                  </label>
                {/if}
              </div>
            {/each}
          </div>

          <p class="mt-4 text-xs text-muted">
            The voice detection model (2.3 MB) is always needed{info.vadPresent ? ' and is already here' : ' and downloads with it'}.
            Files go to <span class="break-all">{info.downloadDir || 'the local app data folder'}</span>.
          </p>

          <div class="mt-4 rounded-2xl bg-soft px-5 py-4">
            {#if session.downloading}
              {#if dl}
                <p class="text-sm font-medium">Downloading {dl.file} <span class="text-muted">({dl.index + 1} of {dl.count})</span></p>
                <div class="mt-2 h-2 overflow-hidden rounded-full bg-line">
                  <div class="h-full rounded-full bg-ink transition-[width]" style="width: {pct(dl)}%"></div>
                </div>
                <p class="mt-1.5 text-xs text-muted">{size(dl.received)} of {size(dl.total)} · {pct(dl)}%</p>
              {:else}
                <p class="text-sm text-muted">Starting…</p>
              {/if}
              <button class="{secondary} mt-3" onclick={pause}>Pause download</button>
            {:else if missing?.any}
              <p class="text-sm">
                {size(missing.bytes)} to download{auto ? ', starting when you continue' : ''}. If it stops, it picks up where it left off.
              </p>
              <button class="{primary} mt-3" onclick={download}>{session.downloadError ? 'Try again' : 'Download now'}</button>
            {:else}
              <p class="flex items-center gap-2 text-sm font-medium"><Icon name="check" size={18} /> Everything you picked is downloaded.</p>
            {/if}
            {#if session.downloadError}<p class="mt-2 break-words text-sm text-rec" role="alert">{session.downloadError}</p>{/if}
          </div>
        {:else}
          <p class="mt-6 text-sm text-muted">Looking at this PC…</p>
        {/if}

      {:else if step === 2}
        <h1 class="mt-8 text-2xl font-semibold tracking-tight">AI model</h1>
        <p class="mt-1 text-sm text-muted">
          Writes the AI notes and answers questions. Use a hosted service with your own key, or one running on this PC.
        </p>
        {#if info}<p class="mt-3 rounded-xl bg-soft px-4 py-3 text-sm">{info.recommendation.localLlmHint}</p>{/if}

        {#if locals.length}
          <h2 class="mt-5 text-sm font-semibold">Running on this PC</h2>
          {#each locals as l}
            <div class="mt-2 flex flex-wrap items-center gap-2">
              <span class="text-[13px] text-muted">{l.name}:</span>
              {#each l.models as m}
                {@const on = form.baseUrl === l.baseUrl && form.model === m}
                <button class="{chip} {on ? 'border-ink bg-ink text-white' : 'border-line hover:border-ink'}" onclick={() => pick(l.baseUrl, m)}>{m}</button>
              {:else}
                <span class="text-[13px] text-faint">no models pulled yet</span>
              {/each}
            </div>
          {/each}
        {/if}

        <h2 class="mt-5 text-sm font-semibold">Services</h2>
        <div class="mt-2 flex flex-wrap gap-2">
          {#each presets as p}
            <button class="{chip} {selected === p.id ? 'border-ink bg-ink text-white' : 'border-line hover:border-ink'}" onclick={() => pick(p.url, p.model)}>{p.label}</button>
          {/each}
          <button class="{chip} {selected === 'custom' ? 'border-ink bg-ink text-white' : 'border-line hover:border-ink'}" onclick={() => pick('', '')}>Custom endpoint</button>
        </div>

        <label class="mt-5 block text-sm font-medium" for="su-url">Server address</label>
        <input id="su-url" class={field} placeholder="https://your-gateway.example.com/v1" spellcheck="false" bind:value={form.baseUrl} />
        <label class="mt-4 block text-sm font-medium" for="su-model">Model</label>
        <input id="su-model" class={field} placeholder="model name as the provider spells it" spellcheck="false" bind:value={form.model} />
        <label class="mt-4 block text-sm font-medium" for="su-key">
          API key {#if isLocal}<span class="font-normal text-faint">(not needed for a local server)</span>{/if}
        </label>
        <input id="su-key" type="password" autocomplete="off" class={field} placeholder={isLocal ? 'Leave empty' : 'Paste your key (or leave empty if already saved)'} bind:value={apiKey} />
        <p class="mt-1.5 text-xs text-muted">Stored in Windows Credential Manager and only sent to this server.</p>

        <div class="mt-4 flex flex-wrap items-center gap-3">
          <button class={secondary} disabled={testing || !form.baseUrl.trim() || !form.model.trim()} onclick={runTest}>
            {testing ? 'Testing…' : 'Test connection'}
          </button>
          {#if test}
            <span class="flex items-start gap-1.5 text-sm {test.ok ? 'font-medium' : 'text-rec'}" role="status">
              {#if test.ok}<Icon name="check" size={18} />{/if}
              <span class="min-w-0 break-words">{test.text}</span>
            </span>
          {/if}
        </div>
        <p class="mt-3 text-xs text-muted">You can skip this and add it later in Settings. Recording and transcripts work without it.</p>

      {:else}
        <h1 class="mt-8 text-2xl font-semibold tracking-tight">Integrations <span class="text-base font-normal text-muted">(optional)</span></h1>

        <section class="mt-5 rounded-2xl border border-line p-5">
          <div class="flex items-center gap-3">
            <h2 class="font-semibold">Jira & Confluence</h2>
            <span class="rounded-full bg-soft px-2.5 py-0.5 text-[11px] font-semibold tracking-wide">READ-ONLY</span>
          </div>
          <p class="mt-0.5 text-sm text-muted">Lets Ask look up tickets and pages. It can never change anything.</p>
          {#if atl}
            <p class="mt-3 flex items-center gap-2 text-sm">
              <span class="h-2 w-2 shrink-0 rounded-full {atl.jiraSite ? 'bg-lime-deep' : 'bg-faint'}"></span>
              {#if atl.jiraSite}Jira: <span class="font-medium">{atl.jiraSite}</span>
              {:else if !atl.installed}<span class="text-muted">Jira: install Atlassian's acli first.</span>
              {:else}<span class="text-muted">Jira: run <code class="rounded bg-soft px-1.5 py-0.5 text-xs">acli jira auth login --web</code></span>{/if}
            </p>
            <p class="mt-1.5 flex items-center gap-2 text-sm">
              <span class="h-2 w-2 shrink-0 rounded-full {atl.confluence ? 'bg-lime-deep' : 'bg-faint'}"></span>
              {#if atl.confluence}Confluence: connected
              {:else if !atl.confluenceCli}<span class="text-muted">Confluence: <code class="rounded bg-soft px-1.5 py-0.5 text-xs">npm install -g confluence-cli</code></span>
              {:else}<span class="text-muted">Confluence: run <code class="rounded bg-soft px-1.5 py-0.5 text-xs">confluence init</code></span>{/if}
            </p>
          {:else}
            <p class="mt-3 text-sm text-muted">Checking…</p>
          {/if}
          <label class="mt-3 flex cursor-pointer items-center gap-2.5 text-sm">
            <input type="checkbox" class="h-4 w-4 accent-ink" bind:checked={form.atlassian} />
            Let Ask look things up in Jira and Confluence
          </label>
        </section>

        <section class="mt-4 rounded-2xl border border-line p-5">
          <h2 class="font-semibold">Google Meet extension</h2>
          <p class="mt-0.5 text-sm text-muted">
            Shows who's talking instead of "Others" and offers to record when you join a call. In Chrome, open chrome://extensions, turn on Developer mode, choose Load unpacked and pick the extension folder. Then paste this code into the extension.
          </p>
          <button class="{secondary} mt-3" disabled={!bridge?.folder} onclick={() => api.openExtensionFolder()}>Open extension folder</button>
          <div class="mt-3 flex items-center gap-2">
            <span class="h-2 w-2 rounded-full {bridge?.connected ? 'bg-lime-deep' : 'bg-faint'}"></span>
            <span class="text-sm {bridge?.connected ? 'font-medium' : 'text-muted'}">{bridge?.connected ? 'Extension connected' : 'Not connected yet'}</span>
            <button class="{secondary} ml-auto" disabled={!bridge} onclick={copyToken}>{copied ? 'Copied' : 'Copy pairing code'}</button>
          </div>
        </section>
      {/if}
    </div>
  </div>

  <footer class="border-t border-line">
    <div class="mx-auto flex max-w-2xl items-center gap-3 px-8 py-3">
      {#if waiting}
        <button class={secondary} onclick={() => (waiting = false)}>Back</button>
      {:else if step > 0}
        <button class={secondary} onclick={() => go(step - 1)}>Back</button>
      {/if}
      {#if waiting}
        <!-- Progress is on the page. -->
      {:else if step === 1 && info && !activeReady}
        <span class="text-xs text-muted">{session.downloading ? 'You can keep going while it downloads.' : 'Setup finishes once the picked model is downloaded.'}</span>
      {:else if step > 1 && session.downloadError}
        <span class="min-w-0 break-words text-xs text-rec" role="alert">Models not downloaded: {session.downloadError}</span>
        <button class="{secondary} shrink-0" onclick={() => session.retryDownload()}>Try again</button>
      {:else if step > 1 && session.downloading}
        <span class="text-xs text-muted" role="status">
          {dl ? `Downloading ${dl.file} (${dl.index + 1} of ${dl.count}) · ${pct(dl)}%` : 'Starting the download…'}
        </span>
      {/if}
      {#if finishError}<span class="min-w-0 break-words text-sm text-rec" role="alert">{finishError}</span>{/if}
      <span class="ml-auto"></span>
      {#if waiting}
        <button class={primary} disabled>Opening when done…</button>
      {:else if step < steps.length - 1}
        <button class={primary} onclick={() => go(step + 1)}>
          {step === 1 && auto ? 'Download and continue' : 'Continue'}
        </button>
      {:else}
        <button class={primary} disabled={finishing} onclick={() => finish()}>
          {finishing ? 'Saving…' : held ? 'Finish when downloaded' : 'Finish'}
        </button>
      {/if}
    </div>
  </footer>
</div>
