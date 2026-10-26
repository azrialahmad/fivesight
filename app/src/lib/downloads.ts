// What the first-run setup still has to download. Kept apart from Setup.svelte so it can be tested.
import type { SetupInfo } from './api';

/** Silero VAD, always needed (see VAD in models.rs). */
export const VAD_BYTES = 2_327_524;

/** The models in `wanted` that aren't on disk yet, with the voice detection model if it's missing. */
export function missingModels(info: SetupInfo, wanted: string[]) {
  const speech = info.speechModels.filter((m) => wanted.includes(m.id) && !m.present);
  const vad = !info.vadPresent;
  return { speech, vad, bytes: speech.reduce((n, m) => n + m.bytes, 0) + (vad ? VAD_BYTES : 0), any: vad || speech.length > 0 };
}

/**
 * Whether moving on in setup (past the speech model step, or Finish) should start the download.
 * Setup used to download only when the Download button was pressed, so going through it with
 * Continue left the PC without models. Not on a PC that can't transcribe, where it would be wasted.
 */
export function startsDownload(info: SetupInfo | null, wanted: string[], downloading: boolean) {
  return !!info && info.recommendation.supported && !downloading && missingModels(info, wanted).any;
}
