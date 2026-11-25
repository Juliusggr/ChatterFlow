import { listen } from "@tauri-apps/api/event";
import {
  CheckCircle2,
  ExternalLink,
  Loader2,
  MessageSquare,
  RefreshCw,
  Trash2,
  UserPlus,
} from "lucide-react";
import { useEffect, useState } from "react";
import { deleteModel, errorMessage, openModelSession, syncModelData } from "./api";
import {
  avatarColor,
  formatMoney,
  formatStamp,
  initials,
  modelLabel,
  type Model,
} from "./types";

export function ModelsView({
  models,
  selectedModelId,
  onChanged,
  onSelect,
  onOpenMessages,
}: {
  models: Model[];
  selectedModelId: number | null;
  onChanged: () => Promise<void>;
  onSelect: (id: number) => void;
  onOpenMessages?: (id: number) => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<number | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [noticeTone, setNoticeTone] = useState<"neutral" | "green">("neutral");
  const [formError, setFormError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    let stopFn: (() => void) | undefined;

    void listen<Model>("model-login-completed", (event) => {
      if (!alive || !event.payload) return;
      const connected = event.payload;
      const label = modelLabel(connected);
      setBusy(null);
      setNoticeTone("green");
      setNotice(
        connected.username
          ? `Sesión iniciada al instante: ${label} (@${connected.username}). Perfil y balance actualizados.`
          : `Sesión iniciada: ${label}.`,
      );
      void onChanged();
    })
      .then((stop) => {
        if (alive) stopFn = stop;
        else stop();
      })
      .catch(() => undefined);

    return () => {
      alive = false;
      stopFn?.();
    };
  }, [onChanged]);

  async function addModel() {
    setBusy("add");
    setFormError(null);
    setNotice(null);
    try {
      const created = await openModelSession(null);
      onSelect(created.id);
      setNoticeTone("neutral");
      setNotice(
        "Ventana de inicio de sesión abierta. Inicia sesión en OnlyFans y la cuenta se vinculará automáticamente.",
      );
      await onChanged();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function openSession(id: number) {
    setBusy(`open:${id}`);
    setFormError(null);
    try {
      onSelect(id);
      await openModelSession(id);
      await onChanged();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function sync(id: number) {
    setBusy(`sync:${id}`);
    setFormError(null);
    setNotice(null);
    try {
      const report = await syncModelData(id);
      await onChanged();
      setNoticeTone("neutral");
      setNotice(
        report.transactions_upserted > 0
          ? `Sincronizado. ${report.transactions_upserted} transacciones guardadas.`
          : "Perfil actualizado. Sin transacciones nuevas.",
      );
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function remove(id: number) {
    setBusy(`delete:${id}`);
    setFormError(null);
    try {
      await deleteModel(id);
      setPendingDeleteId(null);
      await onChanged();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  const isAdding = busy === "add";

  return (
    <section className="flex flex-col gap-6">
      {/* Header */}
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
            Modelos
          </h1>
          <p className="mt-1 max-w-2xl text-sm" style={{ color: "var(--text-secondary)" }}>
            Agrega una cuenta para iniciar sesión en su ventana aislada: el perfil y el balance se
            detectan automáticamente apenas entras.
          </p>
        </div>
        <button
          type="button"
          disabled={isAdding}
          className="cf-btn-primary"
          onClick={() => void addModel()}
        >
          {isAdding ? (
            <Loader2 size={15} strokeWidth={2} className="animate-spin" />
          ) : (
            <UserPlus size={15} strokeWidth={2} />
          )}
          {isAdding ? "Abriendo login…" : "Agregar modelo"}
        </button>
      </header>

      {/* Notifications */}
      {formError ? (
        <div
          role="alert"
          className="rounded-xl px-4 py-3 text-sm"
          style={{ background: "#fef2f2", border: "1px solid #fecaca", color: "#b91c1c" }}
        >
          {formError}
        </div>
      ) : null}
      {notice ? (
        <div
          className="flex items-center gap-2.5 rounded-xl px-4 py-3 text-sm"
          style={
            noticeTone === "green"
              ? {
                  background: "rgba(16, 185, 129, 0.12)",
                  border: "1px solid #34d399",
                  color: "var(--text-primary)",
                }
              : {
                  background: "var(--bg-subtle)",
                  border: "1px solid var(--border)",
                  color: "var(--text-secondary)",
                }
          }
        >
          {noticeTone === "green" ? (
            <CheckCircle2 size={16} strokeWidth={2.25} style={{ color: "#059669" }} className="shrink-0" />
          ) : null}
          <span>{notice}</span>
        </div>
      ) : null}

      {/* Models list */}
      {models.length === 0 ? (
        <div className="cf-card flex flex-col items-center justify-center gap-4 py-20 text-center">
          <div
            className="flex h-14 w-14 items-center justify-center rounded-full"
            style={{ background: "var(--bg-subtle)" }}
          >
            <UserPlus size={24} strokeWidth={1.5} style={{ color: "var(--text-disabled)" }} />
          </div>
          <div>
            <p className="font-semibold" style={{ color: "var(--text-primary)" }}>Sin modelos</p>
            <p className="mt-1 text-sm" style={{ color: "var(--text-tertiary)" }}>
              Agrega una modelo para comenzar.
            </p>
          </div>
        </div>
      ) : (
        <div className="cf-card overflow-hidden">
          <ul className="divide-y" style={{ "--divide-color": "var(--border-muted)" } as React.CSSProperties}>
            {models.map((model) => {
              const label = modelLabel(model);
              const selected = model.id === selectedModelId;
              const isBusySync = busy === `sync:${model.id}`;
              const isBusyOpen = busy === `open:${model.id}`;
              const isBusyDelete = busy === `delete:${model.id}`;
              const rowDisabled = isBusySync || isBusyOpen || isBusyDelete;
              const waitingLogin = model.window_open && !model.of_user_id;

              return (
                <li
                  key={model.id}
                  className="flex flex-wrap items-center gap-4 px-5 py-4 transition-colors"
                  style={{
                    background: selected
                      ? "color-mix(in srgb, var(--brand-500) 5%, transparent)"
                      : "transparent",
                    borderBottom: "1px solid var(--border-muted)",
                  }}
                >
                  {/* Avatar */}
                  <span
                    className="flex h-11 w-11 shrink-0 items-center justify-center overflow-hidden rounded-full text-xs font-semibold text-white"
                    style={{
                      backgroundColor: avatarColor(model.id),
                      boxShadow: selected ? `0 0 0 2px var(--brand-400)` : "none",
                    }}
                    aria-hidden="true"
                  >
                    <AvatarFace model={model} label={label} />
                  </span>

                  {/* Info */}
                  <span className="min-w-36 flex-1">
                    <span className="block text-sm font-semibold" style={{ color: "var(--text-primary)" }}>
                      {waitingLogin && label === "Unknown" ? "Nueva cuenta (esperando login…)" : label}
                    </span>
                    <span className="mt-0.5 block text-xs" style={{ color: "var(--text-tertiary)" }}>
                      {model.username ? `@${model.username}` : "Sin username"}
                    </span>
                    <span className="mt-1 inline-flex">
                      <SessionBadge model={model} />
                    </span>
                  </span>

                  {/* Balance */}
                  <span className="shrink-0 text-sm">
                    <span className="block font-semibold tabular-nums" style={{ color: "var(--text-primary)" }}>
                      {formatMoney(model.current_balance)}
                    </span>
                    <span className="mt-0.5 block text-xs" style={{ color: "var(--text-tertiary)" }}>
                      Pendiente {formatMoney(model.pending_balance)}
                    </span>
                    <span className="block text-xs" style={{ color: "var(--text-disabled)" }}>
                      Sync {formatStamp(model.last_synced_at)}
                    </span>
                  </span>

                  {/* Actions */}
                  <span className="flex shrink-0 flex-wrap gap-2">
                    {model.of_user_id && onOpenMessages ? (
                      <button
                        type="button"
                        disabled={rowDisabled}
                        className="cf-btn-primary"
                        style={{ padding: "0.375rem 0.75rem" }}
                        onClick={() => onOpenMessages(model.id)}
                      >
                        <MessageSquare size={13} strokeWidth={2} />
                        Mensajes
                      </button>
                    ) : null}

                    <button
                      type="button"
                      disabled={rowDisabled}
                      className="cf-btn-secondary"
                      style={{ padding: "0.375rem 0.75rem" }}
                      onClick={() => void openSession(model.id)}
                    >
                      <ExternalLink size={13} strokeWidth={2} />
                      {isBusyOpen ? "Abriendo…" : waitingLogin ? "Enfocar login" : "Abrir"}
                    </button>

                    <button
                      type="button"
                      disabled={rowDisabled}
                      className="cf-btn-live"
                      style={{ padding: "0.375rem 0.75rem" }}
                      onClick={() => void sync(model.id)}
                    >
                      <RefreshCw size={13} strokeWidth={2} className={isBusySync ? "animate-spin" : ""} />
                      {isBusySync ? "Sync…" : "Sincronizar"}
                    </button>

                    {pendingDeleteId === model.id ? (
                      <>
                        <button
                          type="button"
                          disabled={rowDisabled}
                          className="cf-btn-danger"
                          style={{ padding: "0.375rem 0.75rem" }}
                          onClick={() => void remove(model.id)}
                        >
                          Confirmar
                        </button>
                        <button
                          type="button"
                          disabled={rowDisabled}
                          className="cf-btn-secondary"
                          style={{ padding: "0.375rem 0.75rem" }}
                          onClick={() => setPendingDeleteId(null)}
                        >
                          Cancelar
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        disabled={rowDisabled}
                        className="cf-btn-secondary"
                        style={{ padding: "0.375rem 0.75rem", color: "var(--text-tertiary)" }}
                        onClick={() => setPendingDeleteId(model.id)}
                      >
                        <Trash2 size={13} strokeWidth={2} />
                        Eliminar
                      </button>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </section>
  );
}

function SessionBadge({ model }: { model: Model }) {
  if (model.window_open && model.of_user_id) {
    return (
      <span className="cf-badge cf-badge-live">
        <span className="h-1.5 w-1.5 rounded-full bg-orange-400 animate-pulse" aria-hidden="true" />
        Sesión abierta
      </span>
    );
  }
  if (model.window_open) {
    return (
      <span className="cf-badge cf-badge-brand">
        <span className="h-1.5 w-1.5 rounded-full bg-blue-500 animate-pulse" aria-hidden="true" />
        Esperando login
      </span>
    );
  }
  if (model.of_user_id) {
    return <span className="cf-badge cf-badge-brand">Sesión guardada</span>;
  }
  return <span className="cf-badge cf-badge-neutral">Sin sesión</span>;
}

function AvatarFace({ model, label }: { model: Model; label: string }) {
  const [failed, setFailed] = useState(false);
  const url = model.avatar_url?.trim() ?? "";
  if (url && !failed) {
    return (
      <img
        src={url}
        alt=""
        className="h-full w-full object-cover"
        onError={() => setFailed(true)}
      />
    );
  }
  return <span>{initials(label)}</span>;
}
