import { listen } from "@tauri-apps/api/event";
import { FileText, PanelRightClose, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import { errorMessage, openIncomingChat, setInsightsOpen } from "./api";
import { NotificationToastStack } from "./Notifications";
import { loadFanNote, loadFanSpend, saveFanNote, subscribeFanNote, type FanSpendBreakdown } from "./agency";

export function FanInsightsPanel({ modelId }: { modelId: string }) {
  const [ofFanId, setOfFanId] = useState("");
  const [notes, setNotes] = useState("");
  const [spend, setSpend] = useState<FanSpendBreakdown | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [savedNoteSuccess, setSavedNoteSuccess] = useState(false);

  useEffect(() => {
    const stops: { current: Array<() => void> } = { current: [] };
    let alive = true;
    const onFan = (event: { payload: { modelId: string; ofFanId: string } }) => {
      if (event.payload.modelId !== modelId || !event.payload.ofFanId) return;
      setOfFanId(event.payload.ofFanId);
    };
    void listen<{ modelId: string; ofFanId: string }>("fan-selected", onFan)
      .then((stop) => {
        if (alive) stops.current.push(stop);
        else stop();
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      stops.current.forEach((stop) => stop());
    };
  }, [modelId]);

  useEffect(() => {
    if (!modelId || !ofFanId) {
      if (!ofFanId) {
        setNotes("");
        setSpend(null);
      }
      return;
    }
    let alive = true;
    void loadFanNote(modelId, ofFanId)
      .then((remote) => {
        if (alive) setNotes(remote);
      })
      .catch(() => undefined);
    void loadFanSpend(modelId, ofFanId)
      .then((next) => {
        if (!alive) return;
        if (next && (next.totalSpendGross > 0 || next.totalEarnedNet > 0 || next.lastSpendAt)) setSpend(next);
        else setSpend(null);
      })
      .catch(() => {
        if (alive) setSpend(null);
      });
    return () => {
      alive = false;
    };
  }, [modelId, ofFanId]);

  useEffect(() => {
    if (!modelId || !ofFanId) return;
    return subscribeFanNote(modelId, ofFanId, setNotes);
  }, [modelId, ofFanId]);

  useEffect(() => {
    setOfFanId("");
    setNotes("");
    setSpend(null);
  }, [modelId]);

  async function persistNotes() {
    if (!ofFanId) return;
    try {
      await saveFanNote({
        modelId,
        fanPlatformId: ofFanId,
        fanUsername: "",
        notes,
      });
      setSavedNoteSuccess(true);
      setTimeout(() => setSavedNoteSuccess(false), 2500);
      setError(null);
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }

  return (
    <aside
      className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden"
      style={{
        background: "var(--bg)",
        color: "var(--text-primary)",
        fontFamily: "var(--font-ui)",
      }}
    >
      <NotificationToastStack
        enableSystemEffects={false}
        onOpenChat={(targetModelId, targetFanId) => {
          void openIncomingChat(targetModelId, targetFanId);
        }}
      />
      <header
        className="flex items-center justify-between px-4 py-3.5"
        style={{
          background: "var(--bg-elevated)",
          borderBottom: "1px solid var(--border)",
        }}
      >
        <div className="flex items-center gap-2">
          <Sparkles size={16} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
          <h1 className="text-sm font-semibold tracking-tight">Fan Insights</h1>
        </div>
        <div className="flex items-center gap-2">
          {ofFanId ? (
            <span className="cf-badge cf-badge-brand text-xs font-medium">Fan #{ofFanId}</span>
          ) : (
            <span className="cf-badge cf-badge-neutral text-xs">Sin fan activo</span>
          )}
          <button
            type="button"
            title="Ocultar Fan Insights"
            aria-label="Ocultar Fan Insights"
            className="inline-flex h-7 w-7 items-center justify-center rounded-lg"
            style={{ color: "var(--text-tertiary)", background: "var(--bg-subtle)" }}
            onClick={() => void setInsightsOpen(false)}
          >
            <PanelRightClose size={14} strokeWidth={2} />
          </button>
        </div>
      </header>

      {error ? (
        <div
          role="alert"
          className="m-3 rounded-lg px-3 py-2 text-xs"
          style={{ background: "var(--danger-bg)", color: "var(--danger)", border: "1px solid var(--danger-border)" }}
        >
          {error}
        </div>
      ) : null}

      <form
        className="flex min-h-0 flex-1 flex-col gap-3 p-3"
        onSubmit={(event) => {
          event.preventDefault();
          void persistNotes();
        }}
      >
        {spend ? (
          <div
            className="rounded-lg px-3 py-2.5 text-xs leading-relaxed"
            style={{ background: "var(--bg-subtle)", border: "1px solid var(--border)" }}
          >
            <p>
              <span className="font-semibold">Gastó el fan (bruto): </span>
              {money(spend.totalSpendGross)}
            </p>
            {spend.ppvSpent > 0 ? <p>PPV: {money(spend.ppvSpent)}</p> : null}
            {spend.tipSpent > 0 ? <p>Propinas: {money(spend.tipSpent)}</p> : null}
            {spend.subSpent > 0 ? <p>Suscripciones: {money(spend.subSpent)}</p> : null}
            {spend.otherSpent > 0 ? <p>Otros: {money(spend.otherSpent)}</p> : null}
            {spend.highestSpend > 0 ? <p>Compra más alta: {money(spend.highestSpend)}</p> : null}
            {spend.lastSpendAt ? <p>Última compra: {when(spend.lastSpendAt)}</p> : null}
            {spend.totalEarnedNet > 0 ? (
              <p>
                <span className="font-semibold">Ingreso neto: </span>
                {money(spend.totalEarnedNet)}
              </p>
            ) : null}
            <p style={{ color: "var(--text-tertiary)" }}>
              El bruto es lo que pagó el fan. El neto es lo que quedó en las transacciones después de la comisión de OnlyFans.
            </p>
          </div>
        ) : null}
        <label className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider" style={{ color: "var(--text-tertiary)" }}>
          <FileText size={14} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
          Notas del fan
        </label>
        <textarea
          value={notes}
          disabled={!ofFanId}
          placeholder={
            ofFanId ? "Escribe notas o preferencias sobre este fan…" : "Selecciona un fan para escribir notas"
          }
          className="cf-input min-h-0 w-full flex-1 resize-none p-3 leading-relaxed"
          style={{ fontSize: "0.875rem" }}
          onChange={(event) => setNotes(event.target.value)}
        />
        <div className="flex items-center justify-between gap-2">
          {savedNoteSuccess ? (
            <span className="text-xs font-semibold" style={{ color: "var(--success-text)" }}>
              Guardado
            </span>
          ) : (
            <span />
          )}
          <button type="submit" disabled={!ofFanId} className="cf-btn-primary" style={{ padding: "0.4rem 0.8rem" }}>
            <FileText size={14} strokeWidth={2} />
            Guardar notas
          </button>
        </div>
      </form>
    </aside>
  );
}

function money(amount: number) {
  return new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(amount);
}

function when(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat("es", { dateStyle: "medium", timeStyle: "short" }).format(date);
}
