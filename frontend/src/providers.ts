/** Supported LLM / tool providers. Shared across components. */
export const SUPPORTED_PROVIDERS = [
  { id: 'openai', label: 'OpenAI' },
  { id: 'anthropic', label: 'Anthropic' },
  { id: 'perplexity', label: 'Perplexity' },
  { id: 'xai', label: 'X.AI' },
  { id: 'google', label: 'Google' },
  { id: 'tavily', label: 'Tavily' },
] as const;

export type ProviderId = (typeof SUPPORTED_PROVIDERS)[number]['id'];
