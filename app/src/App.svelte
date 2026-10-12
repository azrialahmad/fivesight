<script lang="ts">
  import { onMount } from 'svelte';
  import Home from './lib/Home.svelte';
  import Icon from './lib/Icon.svelte';
  import MeetingView from './lib/MeetingView.svelte';
  import SettingsView from './lib/SettingsView.svelte';
  import { session, inTauri } from './lib/session.svelte';

  onMount(() => session.init());

  const nav = 'flex w-full items-center gap-3 rounded-full px-4 py-2.5 text-left text-sm transition-colors';
</script>

<div class="flex h-full">
  <aside class="flex w-56 shrink-0 flex-col px-3 py-4">
    <button class="flex items-center gap-2.5 px-3 pb-5 pt-1" onclick={() => session.go('home')} aria-label="fivesight home">
      <span class="grid h-8 w-8 place-items-center rounded-[10px] bg-ink text-lime"><Icon name="spark" size={18} /></span>
      <span class="text-[19px] font-semibold tracking-tight">fivesight</span>
    </button>

    <button
      class="mb-5 flex items-center justify-center gap-2 rounded-full bg-ink px-4 py-3 text-sm font-semibold text-white transition-transform active:scale-[0.98]"
      onclick={() => session.newMeeting()}
    >
      <span class="grid h-5 w-5 place-items-center rounded-full bg-lime text-ink"><Icon name="plus" size={14} /></span>
      New meeting
    </button>

    <nav class="flex flex-col gap-0.5" aria-label="Main">
      <button class="{nav} {session.view === 'home' ? 'bg-card font-semibold shadow-sm' : 'text-muted hover:bg-soft'}" aria-current={session.view === 'home' ? 'page' : undefined} onclick={() => session.go('home')}>
        <Icon name="home" size={19} /> Meetings
      </button>
      <button class="{nav} {session.view === 'settings' ? 'bg-card font-semibold shadow-sm' : 'text-muted hover:bg-soft'}" aria-current={session.view === 'settings' ? 'page' : undefined} onclick={() => session.go('settings')}>
        <Icon name="settings" size={19} /> Settings
      </button>
    </nav>

    <div class="mt-auto space-y-3 px-1">
      {#if session.recording}
        <button
          class="flex w-full items-center gap-2 rounded-2xl bg-rec-soft px-3.5 py-2.5 text-left text-xs font-semibold text-rec"
          onclick={() => session.recordingMeetingId !== null && session.openMeeting(session.recordingMeetingId)}
        >
          <span class="rec-dot h-2 w-2 rounded-full bg-rec"></span> Recording now
          <Icon name="chevron" size={16} />
        </button>
      {/if}
      {#if inTauri}
        <button class="flex w-full items-center gap-2 rounded-full px-3.5 py-2 text-xs text-muted hover:bg-soft" onclick={() => session.openLivePanel()}>
          <Icon name="pip" size={16} /> Mini panel
        </button>
      {/if}
      <p class="px-2.5 text-[11px] leading-relaxed text-faint">Transcribes on this PC. Tell everyone in the call you're taking notes.</p>
    </div>
  </aside>

  <main class="my-3 mr-3 min-w-0 flex-1 overflow-hidden rounded-3xl border border-line bg-card">
    {#if session.view === 'meeting' && session.current}
      <MeetingView />
    {:else if session.view === 'settings'}
      <SettingsView />
    {:else}
      <Home />
    {/if}
  </main>
</div>
