<script lang="ts">
  import Icon from './Icon.svelte';
  import { session, inTauri } from './session.svelte';
  import type { Settings } from './api';

  let form = $state<Settings>({ ...session.settings });
  let apiKey = $state('');
  let saved = $state(false);

  $effect(() => {
    // Pick up settings that finish loading after this view mounts.
    form = { ...session.settings };
  });

  const languages: [string, string][] = [
    ['auto', 'Auto-detect (Indonesian + English mixed)'],
    ['id', 'Indonesian'],
    ['en', 'English']
  ];

  async function save() {
    await session.saveSettings(form, apiKey.trim() || null);
    apiKey = '';
    saved = true;
    setTimeout(() => (saved = false), 2000);
  }

  const field = 'mt-2 w-full rounded-xl border border-line bg-paper px-3.5 py-2.5 text-sm outline-none focus:border-ink';
  const seg = 'flex-1 rounded-full px-4 py-2 text-sm font-medium transition-colors';
</script>

<div class="h-full overflow-y-auto">
  <div class="mx-auto max-w-xl space-y-8 px-8 pb-12 pt-8">
    <h1 class="text-2xl font-semibold tracking-tight">Settings</h1>

    <section>
      <label class="font-semibold" for="lang">Spoken language</label>
      <p class="mt-0.5 text-sm text-muted">Pinning one language is more accurate than auto-detect.</p>
      <select id="lang" class={field} bind:value={form.language}>
        {#each languages as [v, label]}<option value={v}>{label}</option>{/each}
      </select>
    </section>

    <section>
      <h2 class="font-semibold">AI model</h2>
      <p class="mt-0.5 text-sm text-muted">Used for AI notes and questions. Audio is always transcribed on this PC.</p>
      <div class="mt-3 flex gap-1 rounded-full bg-soft p-1" role="radiogroup" aria-label="AI provider">
        <button class="{seg} {form.provider === 'openrouter' ? 'bg-ink text-white' : 'text-muted'}" role="radio" aria-checked={form.provider === 'openrouter'} onclick={() => (form.provider = 'openrouter')}>
          OpenRouter (cloud)
        </button>
        <button class="{seg} {form.provider === 'local' ? 'bg-ink text-white' : 'text-muted'}" role="radio" aria-checked={form.provider === 'local'} onclick={() => (form.provider = 'local')}>
          Local model
        </button>
      </div>

      {#if form.provider === 'openrouter'}
        <label class="mt-5 block text-sm font-medium" for="key">API key</label>
        <input id="key" type="password" autocomplete="off" class={field} placeholder={form.hasKey ? 'Saved. Type to replace.' : 'sk-or-…'} bind:value={apiKey} />
        <p class="mt-1.5 text-xs text-muted">Stored in Windows Credential Manager, not in a file.</p>
        <label class="mt-4 block text-sm font-medium" for="model">Model</label>
        <input id="model" class={field} bind:value={form.openrouterModel} />
        <label class="mt-4 block text-sm font-medium" for="fb">Fallback model <span class="font-normal text-faint">(optional)</span></label>
        <input id="fb" class={field} placeholder="Used if the main model fails" bind:value={form.fallbackModel} />
        <p class="mt-3 text-xs text-muted">Meeting text is sent to the model you pick here.</p>
      {:else}
        <label class="mt-5 block text-sm font-medium" for="url">Server address</label>
        <input id="url" class={field} bind:value={form.localUrl} />
        <p class="mt-1.5 text-xs text-muted">Any OpenAI-compatible server: Ollama, LM Studio, llama.cpp.</p>
        <label class="mt-4 block text-sm font-medium" for="lm">Model name</label>
        <input id="lm" class={field} bind:value={form.localModel} />
        <p class="mt-3 text-xs text-muted">Nothing leaves this PC.</p>
      {/if}
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
