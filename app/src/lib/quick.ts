// The saved one-click questions above the Ask box. Kept in Settings, so they apply to every meeting.

/** What a new install starts with. They can be removed like any other. */
export const defaultQuickPrompts = ['What did we decide?', 'List the action items', 'Summarize in 3 bullets'];

/** The list with `text` added at the end. Blank text or a prompt already there changes nothing. */
export function withPrompt(list: string[], text: string): string[] {
  const t = text.trim().replace(/\s+/g, ' ');
  if (!t || list.some((p) => p.toLowerCase() === t.toLowerCase())) return list;
  return [...list, t];
}
