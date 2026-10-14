import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "./tauri";

export interface KbDocument {
  id: string;
  name: string;
  origin: string;
  chars: number;
  chunks: number;
  embedder: string;
  created_at: number;
}

export interface KbHit {
  document_id: string;
  document_name: string;
  seq: number;
  text: string;
  score: number;
}

export type EmbedderMode = "local" | "remote";

export interface EmbeddingsConfig {
  mode: EmbedderMode;
  base_url: string;
  model: string;
}

export const DEFAULT_EMBEDDINGS: EmbeddingsConfig = {
  mode: "local",
  base_url: "",
  model: "text-embedding-3-small",
};

/** Merge stored settings over the defaults. */
export function configFromSettings(settings: Record<string, string>): EmbeddingsConfig {
  const mode = settings["embeddings.mode"];
  return {
    mode: mode === "remote" ? "remote" : "local",
    base_url: settings["embeddings.base_url"] ?? "",
    model: settings["embeddings.model"] ?? DEFAULT_EMBEDDINGS.model,
  };
}

/** The embedder tag the current config writes: "local" or "remote:<model>". */
export function tagForConfig(config: EmbeddingsConfig): string {
  return config.mode === "local" ? "local" : `remote:${config.model}`;
}

/** Largest file the import path accepts (keeps IPC payloads sane). */
export const MAX_IMPORT_BYTES = 2 * 1024 * 1024;

export function listDocuments(): Promise<KbDocument[]> {
  return invoke<KbDocument[]>("kb_list");
}

export function importDocument(
  name: string,
  text: string,
  origin: string,
): Promise<KbDocument> {
  return invoke<KbDocument>("kb_import", { name, text, origin });
}

export function deleteDocument(id: string): Promise<void> {
  return invoke<void>("kb_delete", { id });
}

export function searchKnowledge(query: string, topK: number): Promise<KbHit[]> {
  return invoke<KbHit[]>("kb_search", { query, topK });
}

export function reindexDocument(id: string): Promise<KbDocument> {
  return invoke<KbDocument>("kb_reindex", { id });
}

export function getSettings(): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("settings_get");
}

export function setSettings(pairs: Record<string, string>): Promise<void> {
  return invoke<void>("settings_set", { pairs });
}

export function setEmbeddingsKey(key: string): Promise<void> {
  return invoke<void>("embeddings_key_set", { key });
}

export function hasEmbeddingsKey(): Promise<boolean> {
  return invoke<boolean>("embeddings_key_has");
}

export function testEmbeddings(): Promise<string> {
  return invoke<string>("embeddings_test");
}

export { errorMessage };
