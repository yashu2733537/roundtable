import { useCallback, useEffect, useState } from "react";
import { errorMessage, fetchAppInfo } from "../lib/tauri";
import {
  remoteClientStatus,
  remoteConnect,
  remoteDisconnect,
  remotePairingCode,
  remoteRevoke,
  remoteStart,
  remoteStatus,
  remoteStop,
  remoteTokens,
  type RemoteClientStatus,
  type RemoteStatus,
  type RemoteToken,
} from "../lib/api";

function formatExpiry(expiresAt: number, now: number): string {
  const target = expiresAt > 1e12 ? expiresAt : expiresAt * 1000;
  const secs = Math.max(0, Math.round((target - now) / 1000));
  if (secs <= 0) return "expired";
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return m > 0 ? `${m}m ${s}s left` : `${s}s left`;
}

export default function RemoteSettings() {
  const [isAndroid, setIsAndroid] = useState(false);

  // Server (desktop only).
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [statusLoading, setStatusLoading] = useState(true);
  const [bind, setBind] = useState("127.0.0.1");
  const [port, setPort] = useState("8787");
  const [ack, setAck] = useState(false);
  const [serverBusy, setServerBusy] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);

  const [pairing, setPairing] = useState<{ code: string; expires_at: number } | null>(null);
  const [pairingError, setPairingError] = useState<string | null>(null);
  const [pairingBusy, setPairingBusy] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  const [tokens, setTokens] = useState<RemoteToken[]>([]);
  const [tokensError, setTokensError] = useState<string | null>(null);
  const [tokensLoading, setTokensLoading] = useState(false);
  const [revoking, setRevoking] = useState<string | null>(null);

  // Client (all platforms).
  const [client, setClient] = useState<RemoteClientStatus | null>(null);
  const [clientError, setClientError] = useState<string | null>(null);
  const [clientLoading, setClientLoading] = useState(true);
  const [url, setUrl] = useState("");
  const [label, setLabel] = useState("");
  const [code, setCode] = useState("");
  const [clientBusy, setClientBusy] = useState(false);
  const [clientNotice, setClientNotice] = useState<string | null>(null);

  useEffect(() => {
    fetchAppInfo()
      .then((info) => {
        setIsAndroid(info.platform.toLowerCase().includes("android"));
      })
      .catch(() => setIsAndroid(false));
  }, []);

  const loadServer = useCallback(async () => {
    setStatusLoading(true);
    setStatusError(null);
    try {
      setStatus(await remoteStatus());
    } catch (err: unknown) {
      setStatusError(errorMessage(err));
    } finally {
      setStatusLoading(false);
    }
  }, []);

  const loadTokens = useCallback(async () => {
    setTokensLoading(true);
    setTokensError(null);
    try {
      setTokens(await remoteTokens());
    } catch (err: unknown) {
      setTokensError(errorMessage(err));
    } finally {
      setTokensLoading(false);
    }
  }, []);

  const loadClient = useCallback(async () => {
    setClientLoading(true);
    setClientError(null);
    try {
      setClient(await remoteClientStatus());
    } catch (err: unknown) {
      setClientError(errorMessage(err));
    } finally {
      setClientLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadServer();
    void loadTokens();
    void loadClient();
  }, [loadServer, loadTokens, loadClient]);

  // Tick the pairing-code countdown once a second while a code is shown.
  useEffect(() => {
    if (!pairing) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [pairing]);

  const portNumber = Number(port);
  const portValid =
    Number.isInteger(portNumber) && portNumber >= 1 && portNumber <= 65535;
  const bindTrimmed = bind.trim() === "" ? "127.0.0.1" : bind.trim();
  const isLoopback =
    bindTrimmed === "127.0.0.1" || bindTrimmed.toLowerCase() === "localhost";
  const needsAck = !isLoopback && !status?.running;
  const canStart = portValid && (!needsAck || ack) && !serverBusy;

  const doStart = async () => {
    if (!portValid) return;
    setServerBusy(true);
    setServerError(null);
    try {
      const next = await remoteStart(bindTrimmed, portNumber);
      setStatus(next);
      void loadTokens();
    } catch (err: unknown) {
      setServerError(errorMessage(err));
    } finally {
      setServerBusy(false);
    }
  };

  const doStop = async () => {
    setServerBusy(true);
    setServerError(null);
    try {
      setStatus(await remoteStop());
    } catch (err: unknown) {
      setServerError(errorMessage(err));
    } finally {
      setServerBusy(false);
    }
  };

  const doPairing = async () => {
    setPairingBusy(true);
    setPairingError(null);
    try {
      setPairing(await remotePairingCode());
      setNow(Date.now());
    } catch (err: unknown) {
      setPairingError(errorMessage(err));
    } finally {
      setPairingBusy(false);
    }
  };

  const doRevoke = async (id: string) => {
    setRevoking(id);
    setTokensError(null);
    try {
      await remoteRevoke(id);
      setTokens((prev) => prev.filter((t) => t.id !== id));
    } catch (err: unknown) {
      setTokensError(errorMessage(err));
    } finally {
      setRevoking(null);
    }
  };

  const doConnect = async () => {
    if (url.trim() === "" || code.trim() === "") return;
    setClientBusy(true);
    setClientError(null);
    setClientNotice(null);
    try {
      await remoteConnect(url.trim(), code.trim(), label.trim());
      setClient(await remoteClientStatus());
      setClientNotice("Connected.");
      setCode("");
    } catch (err: unknown) {
      setClientError(errorMessage(err));
    } finally {
      setClientBusy(false);
    }
  };

  const doDisconnect = async () => {
    setClientBusy(true);
    setClientError(null);
    setClientNotice(null);
    try {
      await remoteDisconnect();
      setClient(await remoteClientStatus());
      setClientNotice("Disconnected.");
    } catch (err: unknown) {
      setClientError(errorMessage(err));
    } finally {
      setClientBusy(false);
    }
  };

  return (
    <div className="p4-stack">
      {!isAndroid && (
        <section className="provider-form" aria-labelledby="remote-server-heading">
          <h4 className="settings__label" id="remote-server-heading">
            This device as server (desktop)
          </h4>
          {statusLoading && (
            <p className="muted" role="status">
              <span className="spinner" aria-hidden="true" /> Checking remote status…
            </p>
          )}
          {statusError && !statusLoading && (
            <div className="notice notice--error" role="alert">
              <p className="notice__title">Couldn’t read remote status.</p>
              <p className="notice__body">{statusError}</p>
              <p className="notice__body">
                <button type="button" className="btn" onClick={() => void loadServer()}>
                  Retry
                </button>
              </p>
            </div>
          )}
          {status && (
            <p className="muted" role="status">
              {status.running
                ? `Running on ${status.bind}:${status.port}${status.tls ? " (TLS)" : " (no TLS)"} · ${status.token_count} device(s)`
                : "Server stopped."}
            </p>
          )}
          <div className="builder__grid">
            <div className="settings__field">
              <label className="settings__label" htmlFor="remote-bind">
                Bind address
              </label>
              <input
                id="remote-bind"
                className="input"
                value={bind}
                onChange={(e) => setBind(e.target.value)}
                autoComplete="off"
                spellCheck={false}
                placeholder="127.0.0.1"
              />
            </div>
            <div className="settings__field">
              <label className="settings__label" htmlFor="remote-port">
                Port
              </label>
              <input
                id="remote-port"
                className="input"
                inputMode="numeric"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </div>
          </div>
          {!portValid && (
            <p className="settings__hint" role="note">
              Port must be a number from 1 to 65535.
            </p>
          )}
          <div className="notice" role="note">
            <p className="notice__body">
              The backend refuses plaintext LAN access without TLS or a tunnel.
              Keep 127.0.0.1 for local-only use, or serve behind TLS / an SSH
              tunnel before exposing another interface.
            </p>
          </div>
          {needsAck && (
            <label className="check-row">
              <input
                type="checkbox"
                checked={ack}
                onChange={(e) => setAck(e.target.checked)}
              />
              I understand this exposes plaintext HTTP on the LAN
            </label>
          )}
          {serverError && (
            <div className="notice notice--error" role="alert">
              <p className="notice__body">{serverError}</p>
            </div>
          )}
          <div className="provider-form__actions">
            {status?.running ? (
              <button
                type="button"
                className="btn"
                disabled={serverBusy}
                onClick={() => void doStop()}
              >
                {serverBusy ? "Stopping…" : "Stop server"}
              </button>
            ) : (
              <button
                type="button"
                className="btn btn--primary"
                disabled={!canStart}
                onClick={() => void doStart()}
              >
                {serverBusy ? "Starting…" : "Start server"}
              </button>
            )}
          </div>

          <h4 className="settings__label">Pairing</h4>
          <div className="provider-form__actions">
            <button
              type="button"
              className="btn"
              disabled={pairingBusy}
              onClick={() => void doPairing()}
            >
              {pairingBusy ? "Generating…" : "New pairing code"}
            </button>
          </div>
          {pairingError && (
            <div className="notice notice--error" role="alert">
              <p className="notice__body">{pairingError}</p>
              <p className="notice__body">
                <button type="button" className="btn" onClick={() => void doPairing()}>
                  Retry
                </button>
              </p>
            </div>
          )}
          {pairing && (
            <p className="muted" role="status">
              Code <code>{pairing.code}</code> · {formatExpiry(pairing.expires_at, now)}
            </p>
          )}

          <h4 className="settings__label">Paired devices</h4>
          {tokensLoading && (
            <p className="muted" role="status">
              <span className="spinner" aria-hidden="true" /> Loading devices…
            </p>
          )}
          {tokensError && (
            <div className="notice notice--error" role="alert">
              <p className="notice__body">{tokensError}</p>
              <p className="notice__body">
                <button type="button" className="btn" onClick={() => void loadTokens()}>
                  Retry
                </button>
              </p>
            </div>
          )}
          {!tokensLoading && !tokensError && tokens.length === 0 && (
            <p className="muted">No paired devices.</p>
          )}
          {!tokensLoading && !tokensError && tokens.length > 0 && (
            <ul className="provider-list">
              {tokens.map((t) => (
                <li key={t.id} className="provider-row">
                  <div className="provider-row__main">
                    <span className="provider-row__name">{t.label || "Unnamed device"}</span>
                    <span className="provider-row__base">{t.id}</span>
                  </div>
                  <div className="provider-row__actions">
                    <button
                      type="button"
                      className="btn btn--sm"
                      disabled={revoking === t.id}
                      onClick={() => void doRevoke(t.id)}
                    >
                      {revoking === t.id ? "Revoking…" : "Revoke"}
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <section className="provider-form" aria-labelledby="remote-client-heading">
        <h4 className="settings__label" id="remote-client-heading">
          Connect to a server{isAndroid ? " (this device)" : ""}
        </h4>
        {clientLoading && (
          <p className="muted" role="status">
            <span className="spinner" aria-hidden="true" /> Checking connection…
          </p>
        )}
        {clientError && (
          <div className="notice notice--error" role="alert">
            <p className="notice__title">Connection failed.</p>
            <p className="notice__body">{clientError}</p>
            <p className="notice__body">
              <button type="button" className="btn" onClick={() => void loadClient()}>
                Retry
              </button>
            </p>
          </div>
        )}
        {client && !clientLoading && (
          <p className="muted" role="status">
            {client.configured
              ? `Configured${client.url ? ` for ${client.url}` : ""}${client.has_token ? " · token saved" : " · no token"}`
              : "Not connected."}
          </p>
        )}
        {clientNotice && (
          <p className="settings__ok" role="status">
            {clientNotice}
          </p>
        )}
        <label className="settings__label" htmlFor="remote-url">
          Server URL
        </label>
        <input
          id="remote-url"
          className="input"
          type="url"
          placeholder="http://192.168.1.10:8787"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
        <label className="settings__label" htmlFor="remote-label">
          Device label
        </label>
        <input
          id="remote-label"
          className="input"
          placeholder="My phone"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          autoComplete="off"
        />
        <label className="settings__label" htmlFor="remote-code">
          Pairing code
        </label>
        <input
          id="remote-code"
          className="input"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
        <div className="provider-form__actions">
          <button
            type="button"
            className="btn btn--primary"
            disabled={clientBusy || url.trim() === "" || code.trim() === ""}
            onClick={() => void doConnect()}
          >
            {clientBusy ? "Connecting…" : "Connect"}
          </button>
          <button
            type="button"
            className="btn"
            disabled={clientBusy}
            onClick={() => void doDisconnect()}
          >
            Disconnect
          </button>
        </div>
      </section>
    </div>
  );
}
