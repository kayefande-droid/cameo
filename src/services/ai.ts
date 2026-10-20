import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  AIProviderId,
  ChatEvent,
  ChatMessage,
  GenOpts,
  ModelInfo,
  ProviderStatus,
} from "../types/ai";

const BACKEND_KEYS: Record<AIProviderId, string> = {
  openrouter: "openrouter",
  nvidia: "nvidia",
  ollama: "ollama",
  compatible: "compatible",
};

export async function saveKey(provider: AIProviderId, key: string) {
  return invoke<boolean>("save_provider_key", {
    provider: BACKEND_KEYS[provider],
    key,
  });
}

export async function deleteKey(provider: AIProviderId) {
  return invoke<boolean>("delete_provider_key", { provider: BACKEND_KEYS[provider] });
}

export async function hasKey(provider: AIProviderId) {
  return invoke<boolean>("has_provider_key", { provider: BACKEND_KEYS[provider] });
}

export async function listModels(provider: AIProviderId): Promise<ModelInfo[]> {
  return invoke<ModelInfo[]>("list_models", { provider: BACKEND_KEYS[provider] });
}

export function testConnection(provider: AIProviderId): Promise<ProviderStatus> {
  return invoke<ProviderStatus>("test_connection", { provider: BACKEND_KEYS[provider] });
}

/**
 * Streamed chat against the active provider/model.
 * Calls onEvent for every delta/usage/done/error event.
 * Resolves when the stream finishes; rejects only on command-level failure.
 */
export async function streamChat(
  provider: AIProviderId,
  model: string,
  messages: ChatMessage[],
  opts: GenOpts,
  onEvent: (ev: ChatEvent) => void,
  requestId = newRequestId(),
  timeoutMs?: number,
): Promise<void> {
  const selectedModel = model.trim();
  if (!selectedModel) {
    throw new Error("Choose a model before sending a chat request. Open Settings, select a model, then press Use.");
  }
  const ch = new Channel<ChatEvent>();
  ch.onmessage = onEvent;
  await invoke("chat", {
    requestId,
    provider: BACKEND_KEYS[provider],
    model: selectedModel,
    messages,
    opts: { ...opts, timeout_ms: timeoutMs },
    onEvent: ch,
  });
}

export async function cancelChat(requestId: string) {
  return invoke<boolean>("chat_cancel", { requestId });
}

export function newRequestId(): string {
  return `req-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
