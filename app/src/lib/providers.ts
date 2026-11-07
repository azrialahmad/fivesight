import type { Profile, Settings } from './api';

/** Starting points for first-run setup and address suggestions. All speak the OpenAI chat API. */
export const presets = [
  { id: 'openrouter', label: 'OpenRouter', url: 'https://openrouter.ai/api/v1', model: 'google/gemini-2.5-flash' },
  { id: 'openai', label: 'OpenAI', url: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
  { id: 'gemini', label: 'Google Gemini', url: 'https://generativelanguage.googleapis.com/v1beta/openai', model: 'gemini-2.5-flash' },
  { id: 'groq', label: 'Groq', url: 'https://api.groq.com/openai/v1', model: 'llama-3.3-70b-versatile' },
  { id: 'together', label: 'Together', url: 'https://api.together.xyz/v1', model: 'meta-llama/Llama-3.3-70B-Instruct-Turbo' },
  { id: 'ollama', label: 'Ollama (local)', url: 'http://localhost:11434/v1', model: 'gemma3' },
  { id: 'lmstudio', label: 'LM Studio (local)', url: 'http://localhost:1234/v1', model: '' }
];

/** The profile Ask and the AI notes use. */
export const activeProfile = (s: Settings): Profile => s.profiles.find((p) => p.id === s.activeProfile) ?? s.profiles[0];

export const newProfile = (): Profile => ({ id: crypto.randomUUID(), name: '', baseUrl: '', model: '', fallbackModel: '' });

export const normUrl = (u: string) => u.trim().replace(/\/+$/, '').toLowerCase();

/** The preset whose address this is, or 'custom'. */
export const presetFor = (url: string) => presets.find((p) => normUrl(p.url) === normUrl(url))?.id ?? 'custom';

export const isLocalUrl = (url: string) => /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])([:/]|$)/i.test(url.trim());
