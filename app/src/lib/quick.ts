// The saved one-click questions above the Ask box. Kept in Settings, so they apply to every meeting.

export interface QuickPrompt {
  /** What the button says. Empty: it shows the prompt. */
  label: string;
  /** What the AI gets when you click it. */
  prompt: string;
}

/** What a new install starts with. They can be edited and removed like any other. */
export const defaultQuickPrompts: QuickPrompt[] = [
  { label: 'Decisions', prompt: 'What did we decide?' },
  { label: 'Action items', prompt: 'List the action items' },
  { label: 'Summary', prompt: 'Summarize in 3 bullets' }
];

export const chipLabel = (p: QuickPrompt) => p.label.trim() || p.prompt;

/**
 * The list with `draft` added at the end, or replacing the prompt at `index`. A blank prompt, or
 * one another entry already has, changes nothing.
 */
export function withPrompt(list: QuickPrompt[], index: number | null, draft: QuickPrompt): QuickPrompt[] {
  const next = { label: draft.label.trim(), prompt: draft.prompt.trim() };
  const taken = list.some((p, i) => i !== index && p.prompt.toLowerCase() === next.prompt.toLowerCase());
  if (!next.prompt || taken) return list;
  return index === null ? [...list, next] : list.map((p, i) => (i === index ? next : p));
}
