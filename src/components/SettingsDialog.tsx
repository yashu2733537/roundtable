import { useEffect, useRef, useState } from "react";
import Modal from "./Modal";
import ToolsSettings from "./ToolsSettings";
import RemoteSettings from "./RemoteSettings";
import AuditLog from "./AuditLog";
import { useIsAndroid } from "./useAndroid";
import { IconClose } from "./Icon";
import type { ThemeChoice } from "../lib/theme";
import {
  OLLAMA_ID,
  addProvider,
  errorMessage,
  exportData,
  fetchDataDir,
  fetchOllamaStatus,
  importData,
  listProviders,
  openDataDir,
  removeProvider,
  testProvider,
  updateProvider,
  type OllamaStatus,
  type ProviderInfo,
} from "../lib/api";

interface SettingsDialogProps {
  open: boolean;
  onClose: () => void;
  theme: ThemeChoice;
  onThemeChange: (choice: ThemeChoice) => void;
  defaultProviderId: string | null;
  defaultModel: string;
  onDefaultModelChange: (providerId: string | null, model: string) => void;
  onProvidersChanged: () => void;
}

const THEME_OPTIONS: Array<{ value: ThemeChoice; label: string }> = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
];

type Busy = "idle" | "save" | "test" | "data";

/// One-click starters for OpenAI-compatible endpoints. Anthropic speaks a
/// different protocol and gets its own adapter + preset (not this list).
const PRESETS: Array<{ name: string; base: string }> = [
  { name: "OpenAI", base: "https://api.openai.com/v1" },
  { name: "Google (Gemini)", base: "https://generativelanguage.googleapis.com/v1beta/openai" },
  { name: "DeepSeek", base: "https://api.deepseek.com/v1" },
  { name: "OpenRouter", base: "https://openrouter.ai/api/v1" },
  { name: "Together", base: "https://api.together.xyz/v1" },
  { name: "Groq", base: "https://api.groq.com/openai/v1" },
  { name: "Mistral", base: "https://api.mistral.ai/v1" },
  { name: "xAI", base: "https://api.x.ai/v1" },
];

export default function SettingsDialog({
  open,
  onClose,
  theme,
  onThemeChange,
  defaultProviderId,
  defaultModel,
  onDefaultModelChange,
  onProvidersChanged,
}: SettingsDialogProps) {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [providersError, setProvidersError] = useState<string | null>(null);
  const [ollama, setOllama] = useState<OllamaStatus>({ running: false, models: [] });
  const [ollamaChecking, setOllamaChecking] = useState(false);

  // Add / edit form.
  const [editingId, setEditingId] = useState<string | null>(null);
  const [formName, setFormName] = useState("");
  const [formBase, setFormBase] = useState("");
  const [formKey, setFormKey] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>("idle");

  // Data section.
  const [dataDir, setDataDir] = useState<string>("");
  const [dataMessage, setDataMessage] = useState<string | null>(null);
  const [dataError, setDataError] = useState<string | null>(null);
  const [importText, setImportText] = useState<string | null>(null);
  const [importName, setImportName] = useState<string>("");
  const fileRef = useRef<HTMLInputElement>(null);
  const isAndroid = useIsAndroid();

  const refreshProviders = async () => {
    setProvidersError(null);
    try {
      setProviders(await listProviders());
    } catch (err: unknown) {
      setProvidersError(errorMessage(err));
    }
  };

  const checkOllama = async () => {
    setOllamaChecking(true);
    try {
      setOllama(await fetchOllamaStatus());
    } catch {
      setOllama({ running: false, models: [] });
    } finally {
      setOllamaChecking(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    void refreshProviders();
    void checkOllama();
    fetchDataDir()
      .then(setDataDir)
      .catch(() => setDataDir(""));
    setFormError(null);
    setTestResult(null);
    setDataMessage(null);
    setDataError(null);
    setImportText(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open ]);

  const startAdd = () => {
    setEditingId(null);
    setFormName("");
    setFormBase("https://api.openai.com/v1");
    setFormKey("");
    setFormError(null);
  };

  const startEdit = (provider: ProviderInfo) => {
    setEditingId(provider.id);
    setFormName(provider.name);
    setFormBase(provider.base_url);
    setFormKey("");
    setFormError(null);
    setPendingDeleteId(null);
  };

  const saveProvider = async () => {
    setBusy("save");
    setFormError(null);
    try {
      if (editingId) {
        // Empty key field = keep whatever is stored.
        const key = formKey === "" ? undefined : formKey;
        await updateProvider(editingId, {
          name: formName,
          base_url: formBase,
          key,
        });
      } else {
        await addProvider({ name: formName, base_url: formBase, key: formKey });
      }
      setEditingId(null);
      setFormName("");
      setFormBase("");
      setFormKey("");
      await refreshProviders();
      onProvidersChanged();
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    } finally {
      setBusy("idle");
    }
  };

  const confirmDelete = async (id: string) => {
    setBusy("save");
    setFormError(null);
    try {
      await removeProvider(id);
      setPendingDeleteId(null);
      if (editingId === id) {
        setEditingId(null);
        setFormName("");
        setFormBase("");
        setFormKey("");
      }
      await refreshProviders();
      onProvidersChanged();
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    } finally {
      setBusy("idle");
    }
  };

  const runTest = async (id: string) => {
    setTestingId(id);
    setTestResult(null);
    setFormError(null);
    try {
      setTestResult(await testProvider(id));
    } catch (err: unknown) {
      setFormError(errorMessage(err));
    } finally {
      setTestingId(null);
    }
  };

  const runExport = async () => {
    setBusy("data");
    setDataMessage(null);
    setDataError(null);
    try {
      const json = await exportData();
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const stamp = new Date().toISOString().slice(0, 10);
      const link = document.createElement("a");
      link.href = url;
      link.download = `roundtable-export-${stamp}.json`;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(url);
      setDataMessage("Export downloaded. Provider keys are never exported.");
    } catch (err: unknown) {
      setDataError(errorMessage(err));
    } finally {
      setBusy("idle");
    }
  };

  const onImportFile = async (file: File | undefined) => {
    if (!file) return;
    setDataMessage(null);
    setDataError(null);
    try {
      setImportText(await file.text());
      setImportName(file.name);
    } catch (err: unknown) {
      setDataError(`Could not read ${file.name}: ${errorMessage(err)}`);
    }
  };

  const confirmImport = async () => {
    if (importText == null) return;
    setBusy("data");
    setDataMessage(null);
    setDataError(null);
    try {
      setDataMessage(await importData(importText));
      setImportText(null);
      setImportName("");
      await refreshProviders();
      onProvidersChanged();
    } catch (err: unknown) {
      setDataError(errorMessage(err));
    } finally {
      setBusy("idle");
    }
  };

  const showingForm = editingId !== null || formName !== "" || formBase !== "";

  return (
    <Modal open={open} onClose={onClose} label="Settings" className="settings">
      <div className="modal__header">
        <h2 className="modal__title">Settings</h2>
        <button
          type="button"
          className="btn btn--icon"
          onClick={onClose}
          aria-label="Close settings"
        >
          <IconClose />
        </button>
      </div>

      <section className="settings__section" aria-labelledby="theme-heading">
        <h3 className="settings__heading" id="theme-heading">
          Appearance
        </h3>
        <fieldset className="segmented-field">
          <legend className="settings__label">Theme</legend>
          <div className="segmented">
            {THEME_OPTIONS.map((option) => (
              <label
                key={option.value}
                className={`segmented__option${
                  theme === option.value ? " segmented__option--active" : ""
                }`}
              >
                <input
                  type="radio"
                  name="theme"
                  value={option.value}
                  checked={theme === option.value}
                  onChange={() => onThemeChange(option.value)}
                  className="segmented__input"
                />
                <span>{option.label}</span>
              </label>
            ))}
          </div>
          <p className="settings__hint">
            “System” follows your operating system’s light/dark preference.
          </p>
        </fieldset>
      </section>

      <section className="settings__section" aria-labelledby="providers-heading">
        <h3 className="settings__heading" id="providers-heading">
          Providers
        </h3>
        {providersError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Couldn’t load providers.</p>
            <p className="notice__body">{providersError}</p>
          </div>
        )}
        <ul className="provider-list">
          {providers.map((provider) => (
            <li key={provider.id} className="provider-row">
              <div className="provider-row__main">
                <span className="provider-row__name">{provider.name}</span>
                <span className="provider-row__base">{provider.base_url}</span>
                <span
                  className={`provider-row__key${
                    provider.has_key ? " provider-row__key--saved" : ""
                  }`}
                >
                  {provider.has_key ? "Key saved" : "No key"}
                </span>
              </div>
              <div className="provider-row__actions">
                <button
                  type="button"
                  className="btn btn--sm"
                  disabled={testingId === provider.id}
                  onClick={() => void runTest(provider.id)}
                >
                  {testingId === provider.id ? "Testing…" : "Test"}
                </button>
                <button
                  type="button"
                  className="btn btn--sm"
                  onClick={() => startEdit(provider)}
                >
                  Edit
                </button>
                {pendingDeleteId === provider.id ? (
                  <button
                    type="button"
                    className="btn btn--danger btn--sm"
                    disabled={busy !== "idle"}
                    onClick={() => void confirmDelete(provider.id)}
                  >
                    Confirm?
                  </button>
                ) : (
                  <button
                    type="button"
                    className="btn btn--sm"
                    onClick={() => {
                      setPendingDeleteId(provider.id);
                      window.setTimeout(() => {
                        setPendingDeleteId((current) =>
                          current === provider.id ? null : current,
                        );
                      }, 4000);
                    }}
                  >
                    Delete
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
        {testResult && (
          <p className="settings__ok" role="status">
            {testResult}
          </p>
        )}

        {!editingId && (
          <div className="provider-presets">
            <span className="settings__label" id="preset-label">
              Start from a preset:
            </span>
            <ul className="chip-list" aria-labelledby="preset-label">
              {PRESETS.map((preset) => (
                <li key={preset.name}>
                  <button
                    type="button"
                    className="btn btn--sm"
                    title={preset.base}
                    onClick={() => {
                      setEditingId(null);
                      setFormName(preset.name);
                      setFormBase(preset.base);
                      setFormKey("");
                      setFormError(null);
                    }}
                  >
                    {preset.name}
                  </button>
                </li>
              ))}
            </ul>
            <p className="settings__hint">
              Presets fill in the endpoint — you still add your own API key.
              Anthropic needs its own adapter and arrives separately.
            </p>
          </div>
        )}

        {editingId ? (
          <form
            className="provider-form"
            onSubmit={(e) => {
              e.preventDefault();
              void saveProvider();
            }}
          >
            <h4 className="settings__label">Edit provider</h4>
            <label className="settings__label" htmlFor="provider-name">
              Name
            </label>
            <input
              id="provider-name"
              className="input"
              value={formName}
              onChange={(e) => setFormName(e.target.value)}
              autoComplete="off"
            />
            <label className="settings__label" htmlFor="provider-base">
              Base URL
            </label>
            <input
              id="provider-base"
              className="input"
              type="url"
              value={formBase}
              onChange={(e) => setFormBase(e.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
            <label className="settings__label" htmlFor="provider-key">
              API key
            </label>
            <input
              id="provider-key"
              className="input"
              type="password"
              placeholder="Leave empty to keep the stored key"
              value={formKey}
              onChange={(e) => setFormKey(e.target.value)}
              autoComplete="new-password"
              spellCheck={false}
            />
            {formError && (
              <div className="notice notice--error" role="alert">
                <p className="notice__body">{formError}</p>
              </div>
            )}
            <div className="provider-form__actions">
              <button type="submit" className="btn btn--primary" disabled={busy !== "idle"}>
                {busy === "save" ? "Saving…" : "Save"}
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  setEditingId(null);
                  setFormError(null);
                }}
              >
                Cancel
              </button>
            </div>
            <p className="settings__hint">
              Keys are stored in the OS credential store — never in files or
              the database.
            </p>
          </form>
        ) : showingForm ? (
          <form
            className="provider-form"
            onSubmit={(e) => {
              e.preventDefault();
              void saveProvider();
            }}
          >
            <h4 className="settings__label">Add provider</h4>
            <label className="settings__label" htmlFor="provider-name">
              Name
            </label>
            <input
              id="provider-name"
              className="input"
              placeholder="OpenAI"
              value={formName}
              onChange={(e) => setFormName(e.target.value)}
              autoComplete="off"
            />
            <label className="settings__label" htmlFor="provider-base">
              Base URL
            </label>
            <input
              id="provider-base"
              className="input"
              type="url"
              placeholder="https://api.openai.com/v1"
              value={formBase}
              onChange={(e) => setFormBase(e.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
            <label className="settings__label" htmlFor="provider-key">
              API key
            </label>
            <input
              id="provider-key"
              className="input"
              type="password"
              placeholder="Optional for keyless endpoints"
              value={formKey}
              onChange={(e) => setFormKey(e.target.value)}
              autoComplete="new-password"
              spellCheck={false}
            />
            {formError && (
              <div className="notice notice--error" role="alert">
                <p className="notice__body">{formError}</p>
              </div>
            )}
            <div className="provider-form__actions">
              <button type="submit" className="btn btn--primary" disabled={busy !== "idle"}>
                {busy === "save" ? "Saving…" : "Add"}
              </button>
              <button type="button" className="btn" onClick={startAdd}>
                Clear
              </button>
            </div>
          </form>
        ) : (
          <button type="button" className="btn" onClick={startAdd}>
            Add provider
          </button>
        )}
      </section>

      <section className="settings__section" aria-labelledby="ollama-heading">
        <h3 className="settings__heading" id="ollama-heading">
          Local models (Ollama)
        </h3>
        {ollamaChecking ? (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Checking for Ollama…
          </p>
        ) : ollama.running ? (
          <>
            <p className="settings__ok" role="status">
              Ollama is running
              {ollama.models.length > 0
                ? ` — ${ollama.models.length} model${ollama.models.length === 1 ? "" : "s"} installed.`
                : " but no models are installed yet."}
            </p>
            {ollama.models.length > 0 && (
              <ul className="chip-list">
                {ollama.models.map((model) => (
                  <li key={model}>
                    <code>{model}</code>
                  </li>
                ))}
              </ul>
            )}
            <button type="button" className="btn" onClick={() => void checkOllama()}>
              Retry
            </button>
          </>
        ) : (
          <>
            <div className="notice notice--error" role="alert">
              <p className="notice__title">Ollama was not found.</p>
              <p className="notice__body">
                Install it from <code>ollama.com</code>, then start it and pull
                a model:
              </p>
              <p className="notice__body">
                <code>ollama serve</code> and <code>ollama pull llama3.1</code>
              </p>
              <p className="notice__body">
                Roundtable talks to it at <code>http://localhost:11434</code> —
                no API key needed.
              </p>
            </div>
            <p>
              <button type="button" className="btn" onClick={() => void checkOllama()}>
                Retry
              </button>
            </p>
          </>
        )}
      </section>

      <section className="settings__section" aria-labelledby="default-model-heading">
        <h3 className="settings__heading" id="default-model-heading">
          Default model
        </h3>
        <div className="provider-form">
          <label className="settings__label" htmlFor="default-provider">
            Provider
          </label>
          <select
            id="default-provider"
            className="select"
            value={defaultProviderId ?? ""}
            onChange={(e) =>
              onDefaultModelChange(e.target.value === "" ? null : e.target.value, defaultModel)
            }
          >
            <option value="">None</option>
            {providers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
            <option value={OLLAMA_ID}>Ollama (local)</option>
          </select>
          <label className="settings__label" htmlFor="default-model">
            Model id
          </label>
          <input
            id="default-model"
            className="input"
            placeholder="e.g. gpt-4o-mini"
            value={defaultModel}
            onChange={(e) =>
              onDefaultModelChange(defaultProviderId, e.target.value)
            }
            autoComplete="off"
            spellCheck={false}
          />
          <p className="settings__hint">
            New conversations start with this provider and model.
          </p>
        </div>
      </section>

      {open && !isAndroid && (
        <section className="settings__section" aria-labelledby="tools-heading">
          <h3 className="settings__heading" id="tools-heading">
            Tools
          </h3>
          <ToolsSettings />
        </section>
      )}

      {open && (
        <section className="settings__section" aria-labelledby="remote-heading">
          <h3 className="settings__heading" id="remote-heading">
            Remote
          </h3>
          <RemoteSettings />
        </section>
      )}

      {open && (
        <section className="settings__section" aria-labelledby="audit-heading">
          <h3 className="settings__heading" id="audit-heading">
            Audit
          </h3>
          <AuditLog roomId={null} />
        </section>
      )}

      <section className="settings__section" aria-labelledby="data-heading">
        <h3 className="settings__heading" id="data-heading">
          Data
        </h3>
        <div className="provider-form">
          <div className="provider-form__actions">
            <button
              type="button"
              className="btn"
              disabled={busy !== "idle"}
              onClick={() => void runExport()}
            >
              {busy === "data" ? "Working…" : "Export (JSON)"}
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => fileRef.current?.click()}
            >
              Import…
            </button>
            <input
              ref={fileRef}
              type="file"
              accept="application/json,.json"
              hidden
              aria-label="Choose an export file to import"
              onChange={(e) => {
                void onImportFile(e.target.files?.[0]);
                e.target.value = "";
              }}
            />
          </div>
          {importText != null && (
            <div className="notice" role="alert">
              <p className="notice__title">Replace everything?</p>
              <p className="notice__body">
                Importing <code>{importName}</code> replaces all conversations,
                providers, and settings on this device. This cannot be undone.
              </p>
              <p className="notice__body">
                <button
                  type="button"
                  className="btn btn--danger"
                  disabled={busy !== "idle"}
                  onClick={() => void confirmImport()}
                >
                  Replace my data
                </button>{" "}
                <button
                  type="button"
                  className="btn"
                  onClick={() => {
                    setImportText(null);
                    setImportName("");
                  }}
                >
                  Cancel
                </button>
              </p>
            </div>
          )}
          {dataMessage && (
            <p className="settings__ok" role="status">
              {dataMessage}
            </p>
          )}
          {dataError && (
            <div className="notice notice--error" role="alert">
              <p className="notice__body">{dataError}</p>
            </div>
          )}
          <p className="settings__hint">
            Data folder: <code>{dataDir || "…"}</code>
          </p>
          <p>
            <button
              type="button"
              className="btn"
              onClick={() =>
                openDataDir().catch((err: unknown) => setDataError(errorMessage(err)))
              }
            >
              Open data folder
            </button>
          </p>
        </div>
      </section>
    </Modal>
  );
}
