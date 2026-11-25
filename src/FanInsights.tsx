import { listen } from "@tauri-apps/api/event";
import { FileText, PanelRightClose, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import { errorMessage, openIncomingChat, getModels, setInsightsOpen } from "./api";
import { NotificationToastStack } from "./Notifications";
import { loadFanNote, modelExternalKey, saveFanNote, subscribeFanNote, syncAgencyModels } from "./agency";

export function FanInsightsPanel({ modelId }: { modelId: number }) {
  const [ofFanId, setOfFanId] = useState("");
  const [notes, setNotes] = useState("");
  const [externalKey, setExternalKey] = useState<string | null>(() =>
    window.__CF_PARTITION_ID__ ? modelExternalKey({ partition_id: window.__CF_PARTITION_ID__ }) : null,
  );
  const [error, setError] = useState<string | null>(null);
  const [savedNoteSuccess, setSavedNoteSuccess] = useState(false);

  useEffect(() => {
    const stops: { current: Array<() => void> } = { current: [] };
    let alive = true;
    const onFan = (event: { payload: { modelId: number; ofFanId: string } }) => {
      if (event.payload.modelId !== modelId || !event.payload.ofFanId) return;
      setOfFanId(event.payload.ofFanId);
    };
    void listen<{ modelId: number; ofFanId: string }>("fan-selected", onFan)
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
    let alive = true;
    void getModels()
      .then(async (models) => {
        if (!alive) return;
        const model = models.find((item) => item.id === modelId);
        if (!model?.partition_id) return;
        setExternalKey(modelExternalKey(model));
        await syncAgencyModels([model]);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [modelId]);

  useEffect(() => {
    if (!externalKey || !ofFanId) {
      if (!ofFanId) setNotes("");
      return;
    }
    let alive = true;
    void loadFanNote(externalKey, ofFanId)
      .then((remote) => {
        if (alive) setNotes(remote);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [externalKey, ofFanId]);

  useEffect(() => {
    if (!externalKey || !ofFanId) return;
    return subscribeFanNote(externalKey, ofFanId, setNotes);
  }, [externalKey, ofFanId]);

  useEffect(() => {
    setOfFanId("");
    setNotes("");
  }, [modelId]);

  async function persistNotes() {
    if (!ofFanId) return;
    try {
      let key = externalKey;
      if (!key) {
        const models = await getModels();
        const model = models.find((item) => item.id === modelId);
        if (!model?.partition_id) {
          throw new Error("No se encontró la modelo en esta sesión");
        }
        await syncAgencyModels([model]);
        key = modelExternalKey(model);
        setExternalKey(key);
      }
      try {
        await saveFanNote({
          externalKey: key,
          fanPlatformId: ofFanId,
          fanUsername: "",
          notes,
        });
      } catch (caught) {
        if (!/todavía no está en Supabase/i.test(errorMessage(caught))) throw caught;
        const models = await getModels();
        const model = models.find((item) => item.id === modelId);
        if (!model) throw caught;
        await syncAgencyModels([model]);
        await saveFanNote({
          externalKey: key,
          fanPlatformId: ofFanId,
          fanUsername: "",
          notes,
        });
      }
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
        fontFamily: "Satoshi, sans-serif",
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
          style={{ background: "#fef2f2", color: "#b91c1c", border: "1px solid #fecaca" }}
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
            <span className="text-xs font-semibold" style={{ color: "#059669" }}>
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
