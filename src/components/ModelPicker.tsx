import { OLLAMA_ID, type OllamaStatus, type ProviderInfo } from "../lib/api";
import { useIsAndroid } from "./useAndroid";

export interface ModelValue {
  provider_id: string | null;
  model: string;
}

interface ModelPickerProps {
  providers: ProviderInfo[];
  ollama: OllamaStatus;
  modelsByProvider: Record<string, string[]>;
  modelsLoading: boolean;
  value: ModelValue;
  disabled?: boolean;
  onChange: (value: ModelValue) => void;
  onRefreshModels: (providerId: string) => void;
}

const SEP = "::";

function optionValue(providerId: string, model: string): string {
  return `${providerId}${SEP}${model}`;
}

function parseOption(raw: string): { providerId: string; model: string } {
  const sep = raw.indexOf(SEP);
  if (sep < 0) return { providerId: "", model: raw };
  return { providerId: raw.slice(0, sep), model: raw.slice(sep + SEP.length) };
}

export default function ModelPicker({
  providers,
  ollama,
  modelsByProvider,
  modelsLoading,
  value,
  disabled,
  onChange,
  onRefreshModels,
}: ModelPickerProps) {
  // Local models are out of scope on phones (cloud + remote only): Android
  // hides the entry unless the conversation already uses it.
  const isAndroid = useIsAndroid();
  const showOllama = isAndroid
    ? value.provider_id === OLLAMA_ID
    : ollama.running || value.provider_id === OLLAMA_ID;
  const groups: Array<{ id: string; name: string; models: string[] }> = providers.map(
    (p) => ({ id: p.id, name: p.name, models: modelsByProvider[p.id] ?? [] }),
  );
  if (showOllama) {
    groups.push({
      id: OLLAMA_ID,
      name: ollama.running ? "Ollama (local)" : "Ollama (not running)",
      models: modelsByProvider[OLLAMA_ID] ?? ollama.models,
    });
  }

  // The current value always appears, even before its list loads.
  const currentCovered =
    value.provider_id != null &&
    value.model !== "" &&
    groups.some(
      (g) => g.id === value.provider_id && g.models.includes(value.model),
    );

  const selectedValue =
    value.provider_id != null && value.model !== ""
      ? optionValue(value.provider_id, value.model)
      : "";

  const activeProviderId =
    value.provider_id ?? groups[0]?.id ?? null;

  return (
    <div className="model-picker">
      <select
        className="select model-picker__select"
        aria-label="Model"
        disabled={disabled}
        value={selectedValue}
        onChange={(e) => {
          const parsed = parseOption(e.target.value);
          onChange({ provider_id: parsed.providerId || null, model: parsed.model });
        }}
      >
        <option value="" disabled>
          {groups.length === 0 ? "Add a provider first" : "Choose a model…"}
        </option>
        {groups.map((group) => (
          <optgroup key={group.id} label={group.name}>
            {group.models.map((model) => (
              <option key={model} value={optionValue(group.id, model)}>
                {model}
              </option>
            ))}
            {!currentCovered &&
              value.provider_id === group.id &&
              value.model !== "" && (
                <option value={optionValue(group.id, value.model)}>
                  {value.model}
                </option>
              )}
          </optgroup>
        ))}
      </select>
      <input
        type="text"
        className="input model-picker__manual"
        aria-label="Model id (manual)"
        title="Manual model id — overrides the dropdown"
        placeholder="Manual model id…"
        disabled={disabled}
        value={value.model}
        onChange={(e) =>
          onChange({
            provider_id: value.provider_id ?? activeProviderId,
            model: e.target.value,
          })
        }
        autoComplete="off"
        spellCheck={false}
      />
      <button
        type="button"
        className="btn"
        disabled={disabled || modelsLoading || activeProviderId == null}
        onClick={() => {
          if (activeProviderId) onRefreshModels(activeProviderId);
        }}
        title="Reload the model list for the selected provider"
      >
        {modelsLoading ? "Loading…" : "Reload"}
      </button>
      {value.provider_id === OLLAMA_ID && !ollama.running && !isAndroid && (
        <p className="settings__hint" role="note">
          Ollama is not running — open Settings for the setup guide.
        </p>
      )}
    </div>
  );
}
