export type AIProviderId = "openrouter" | "nvidia" | "ollama" | "compatible";

export type ModelCapability =
  | "text"
  | "vision"
  | "tools"
  | "streaming"
  | "structured-output";

export interface ModelInfo {
  id: string;
  name: string;
  provider: AIProviderId;
  capabilities: ModelCapability[];
  context_length?: number;
  pricing_in_per_m?: number;
  pricing_out_per_m?: number;
}

export interface ChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

export interface GenOpts {
  temperature?: number;
  max_tokens?: number;
  base_url?: string;
  proxy_profile_id?: string;
  timeout_ms?: number;
}

export interface ProviderStatus {
  ok: boolean;
  message: string;
}

export type ChatEvent =
  | { type: "delta"; text: string }
  | { type: "usage"; prompt_tokens: number; completion_tokens: number }
  | { type: "done" }
  | { type: "error"; message: string };

export interface AIProviderMeta {
  id: AIProviderId;
  label: string;
  description: string;
  requiresKey: boolean;
}

export const PROVIDERS: AIProviderMeta[] = [
  {
    id: "openrouter",
    label: "OpenRouter",
    description: "280+ hosted models behind one API key (openrouter.ai)",
    requiresKey: true,
  },
  {
    id: "nvidia",
    label: "NVIDIA NIM",
    description: "NVIDIA-hosted inference API (integrate.api.nvidia.com)",
    requiresKey: true,
  },
  {
    id: "ollama",
    label: "Ollama (local)",
    description: "Local models served by Ollama on this machine — no key needed",
    requiresKey: false,
  },
  {
    id: "compatible",
    label: "Compatible endpoint",
    description: "Any OpenAI-compatible /v1 chat API you host or subscribe to",
    requiresKey: true,
  },
];
