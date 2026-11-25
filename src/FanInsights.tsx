import { listen } from "@tauri-apps/api/event";
import {
  ChevronDown,
  ChevronUp,
  CreditCard,
  DollarSign,
  FileText,
  ListFilter,
  Plus,
  ShieldAlert,
  Sparkles,
  User,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import {
  errorMessage,
  getFanInsights,
  insertChatText,
  openIncomingChat,
  saveFanNotes,
  updateFanLists,
  updateFanProfile,
  type FanInsights,
  type FanListItem,
  type FanLists,
  type ScriptRecord,
} from "./api";
import { NotificationToastStack } from "./Notifications";
import { PpvTimeline } from "./messages/ppv/PpvTimeline";
import { ScriptPopover } from "./messages/scripts/ScriptPopover";
import { resetScriptStore } from "./messages/scripts/scriptStore";
import { applyFanName } from "./messages/types";
import { formatMoney, formatStamp } from "./types";

type Tab = "insights" | "sales" | "notes";
type ListTab = "fans" | "following";

const EMPTY_LISTS: FanLists = { fans: [], following: [] };

export function FanInsightsPanel({ modelId }: { modelId: number }) {
  const [ofFanId, setOfFanId] = useState("");
  const [insights, setInsights] = useState<FanInsights | null>(null);
  const [tab, setTab] = useState<Tab>("insights");
  const [listTab, setListTab] = useState<ListTab>("fans");
  const [listQuery, setListQuery] = useState("");
  const [newList, setNewList] = useState("");
  const [notes, setNotes] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [scriptsOpen, setScriptsOpen] = useState(false);
  const [scriptNotice, setScriptNotice] = useState<string | null>(null);
  const [scriptDraft, setScriptDraft] = useState<string | null>(null);
  const [savedNoteSuccess, setSavedNoteSuccess] = useState(false);
  const [ppvReload, setPpvReload] = useState(0);
  const [open, setOpen] = useState({
    spending: true,
    organization: true,
    subscription: true,
    profile: true,
    lists: true,
  });

  useEffect(() => {
    const stops: { current: Array<() => void> } = { current: [] };
    let alive = true;
    const onFan = (event: { payload: { modelId: number; ofFanId: string } }) => {
      if (event.payload.modelId !== modelId || !event.payload.ofFanId) {
        return;
      }
      setOfFanId(event.payload.ofFanId);
      setPpvReload((current) => current + 1);
      void getFanInsights(modelId, event.payload.ofFanId)
        .then((next) => {
          if (!alive) return;
          setInsights(next);
          setNotes(next.notes);
          setError(null);
        })
        .catch((caught) => {
          if (alive) {
            setError(errorMessage(caught));
          }
        });
    };
    void listen<{ modelId: number; ofFanId: string }>("fan-selected", onFan)
      .then((stop) => {
        if (alive) stops.current.push(stop);
        else stop();
      })
      .catch(() => undefined);
    void listen<{ modelId: number; ofFanId: string }>("fan-insights-changed", onFan)
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
    if (!ofFanId) {
      setInsights(null);
      setNotes("");
      return;
    }
    let alive = true;
    void getFanInsights(modelId, ofFanId)
      .then((next) => {
        if (!alive) return;
        setInsights(next);
        setNotes(next.notes);
        setError(null);
      })
      .catch((caught) => {
        if (alive) setError(errorMessage(caught));
      });
    return () => {
      alive = false;
    };
  }, [modelId, ofFanId]);

  useEffect(() => {
    setOfFanId("");
    setInsights(null);
    setNotes("");
    setScriptsOpen(false);
    setScriptNotice(null);
    setScriptDraft(null);
    resetScriptStore(String(modelId), "");
  }, [modelId]);

  useEffect(() => {
    resetScriptStore(String(modelId), ofFanId);
  }, [modelId, ofFanId]);

  async function saveProfile(
    patch: Partial<Pick<FanInsights, "nickname" | "birthday" | "location" | "country">>,
  ) {
    if (!ofFanId || !insights) return;
    const next = { ...insights, ...patch };
    setInsights(next);
    try {
      await updateFanProfile({
        modelId,
        ofFanId,
        ...patch,
      });
      setError(null);
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }

  async function useScript(script: ScriptRecord) {
    if (!ofFanId) {
      setScriptNotice("Selecciona un chat en OnlyFans antes de insertar un script.");
      setScriptDraft(null);
      return;
    }
    const text = applyFanName(script.scriptTextContent, insights?.nickname ?? "");
    if (!text.trim()) {
      setScriptNotice("Ese script no tiene texto para insertar.");
      setScriptDraft(null);
      return;
    }
    const price = script.mediaPrice == null ? "" : ` Precio del script: ${script.mediaPrice.toFixed(2)}.`;
    const media =
      script.media.length === 0
        ? ""
        : ` Hay ${script.media.length} media en el script; adjúntala en OnlyFans.`;
    try {
      const status = await insertChatText(modelId, text);
      if (status === "inserted") {
        setScriptNotice(`Texto insertado en el chat de OnlyFans.${price}${media}`);
        setScriptDraft(null);
        setError(null);
        return;
      }
    } catch (caught) {
      setError(errorMessage(caught));
    }
    try {
      await navigator.clipboard.writeText(text);
      setScriptNotice(`No se encontró el cuadro de mensaje. El texto quedó copiado.${price}${media}`);
      setScriptDraft(null);
    } catch {
      setScriptNotice(`No se pudo insertar ni copiar.${price}${media}`);
      setScriptDraft(text);
    }
  }

  async function persistNotes() {
    if (!ofFanId) return;
    try {
      await saveFanNotes(modelId, ofFanId, notes);
      setSavedNoteSuccess(true);
      setTimeout(() => setSavedNoteSuccess(false), 2500);
      setError(null);
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }

  async function changeLists(lists: FanLists) {
    if (!ofFanId || !insights) return;
    setInsights({ ...insights, lists });
    try {
      await updateFanLists(modelId, ofFanId, lists);
      setError(null);
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }

  const data = insights;
  const lists = data?.lists ?? EMPTY_LISTS;
  const visibleLists = (listTab === "fans" ? lists.fans : lists.following).filter((item) =>
    item.name.toLowerCase().includes(listQuery.trim().toLowerCase()),
  );

  return (
    <aside
      className="flex h-full min-h-0 flex-col"
      style={{
        background: "var(--bg)",
        color: "var(--text-primary)",
        fontFamily: "Satoshi, sans-serif",
      }}
    >
      <NotificationToastStack
        enableSystemEffects={false}
        onOpenChat={(targetModelId, ofFanId) => {
          void openIncomingChat(targetModelId, ofFanId);
        }}
      />
      {/* Header */}
      <header
        className="flex items-center justify-between px-4 py-3.5"
        style={{
          background: "var(--bg-elevated)",
          borderBottom: "1px solid var(--border)",
        }}
      >
        <div className="flex items-center gap-2">
          <Sparkles size={16} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
          <h1 className="text-sm font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
            Fan Insights
          </h1>
        </div>
        {ofFanId ? (
          <span className="cf-badge cf-badge-brand text-xs font-medium">
            Fan #{ofFanId}
          </span>
        ) : (
          <span className="cf-badge cf-badge-neutral text-xs">
            Sin fan activo
          </span>
        )}
      </header>

      <div className="relative z-20 flex flex-col gap-2 px-3 py-2" style={{ borderBottom: "1px solid var(--border)" }}>
        <ScriptPopover
          modelId={modelId}
          open={scriptsOpen}
          onOpenChange={setScriptsOpen}
          onPick={(script) => {
            void useScript(script);
          }}
        />
        {scriptNotice ? (
          <p className="text-xs" style={{ color: "var(--text-secondary)" }}>
            {scriptNotice}
          </p>
        ) : null}
        {scriptDraft ? (
          <textarea
            readOnly
            value={scriptDraft}
            className="w-full resize-none rounded-lg border border-slate-200 bg-white p-2 text-xs text-slate-800 shadow-sm"
            rows={4}
          />
        ) : null}
      </div>

      {error ? (
        <div
          role="alert"
          className="m-3 rounded-lg px-3 py-2 text-xs"
          style={{ background: "#fef2f2", color: "#b91c1c", border: "1px solid #fecaca" }}
        >
          {error}
        </div>
      ) : null}

      {/* Main scrollable body */}
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {tab === "insights" ? (
          <div className="flex flex-col gap-3">
            {/* Spending behavior */}
            <Section
              title="Comportamiento de gasto"
              icon={DollarSign}
              open={open.spending}
              onToggle={() => setOpen((current) => ({ ...current, spending: !current.spending }))}
            >
              <Metric label="Gasto total" value={formatMoney(data?.totalSpend ?? 0)} isHighlight />
              <Metric label="Último pago" value={data?.lastSpend ? formatStamp(data.lastSpend) : "—"} />
              <div
                className="mt-2.5 rounded-xl p-3 flex flex-col gap-2"
                style={{ background: "var(--bg-subtle)" }}
              >
                <SpendLine
                  label="PPV"
                  total={data?.ppvTotal ?? 0}
                  count={data?.ppvCount ?? 0}
                  showAverage={data?.spendSource !== "onlyfans"}
                />
                <SpendLine
                  label="Propinas (Tips)"
                  total={data?.tipTotal ?? 0}
                  count={data?.tipCount ?? 0}
                  showAverage={data?.spendSource !== "onlyfans"}
                />
                <div className="pt-2" style={{ borderTop: "1px solid var(--border-muted)" }}>
                  <Metric
                    label="Gasto más alto"
                    value={data?.highestSpend == null ? "—" : formatMoney(data.highestSpend)}
                  />
                </div>
              </div>
            </Section>

            {/* Organization metrics */}
            <Section
              title="Métricas de perfil"
              icon={ShieldAlert}
              open={open.organization}
              onToggle={() =>
                setOpen((current) => ({ ...current, organization: !current.organization }))
              }
            >
              <Metric
                label="Poder adquisitivo"
                value={data?.spendingPower ?? "Datos insuficientes"}
                accent
              />
              <Metric
                label="Riesgo de chargeback"
                value={data?.chargebackRisk ?? "Desconocido"}
                accent
              />
            </Section>

            {/* Subscription */}
            <Section
              title="Suscripción"
              icon={CreditCard}
              open={open.subscription}
              onToggle={() =>
                setOpen((current) => ({ ...current, subscription: !current.subscription }))
              }
            >
              <Metric label="Estado" value={data?.subStatus ?? "Desconocido"} accent />
              <Metric
                label="Costo mensual"
                value={
                  data?.subCost == null ? "Desconocido" : data.subCost === 0 ? "Gratis" : formatMoney(data.subCost)
                }
                accent
              />
              <Metric label="Duración acumulada" value={data?.subDuration ?? "Desconocido"} accent />
              <Metric label="Auto-renovación" value={data?.autoRenewal ?? "Desconocido"} accent />
            </Section>

            {/* Profile */}
            <Section
              title="Información personal"
              icon={User}
              open={open.profile}
              onToggle={() => setOpen((current) => ({ ...current, profile: !current.profile }))}
            >
              <label className="mt-2 flex flex-col gap-1 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
                Apodo / Alias
                <input
                  value={data?.nickname ?? ""}
                  disabled={!ofFanId}
                  placeholder="Ej: John"
                  className="cf-input"
                  style={{ padding: "0.4rem 0.65rem", fontSize: "0.8125rem" }}
                  onChange={(event) =>
                    setInsights((current) =>
                      current ? { ...current, nickname: event.target.value } : current,
                    )
                  }
                  onBlur={(event) => void saveProfile({ nickname: event.target.value })}
                />
              </label>

              <label className="mt-2.5 flex flex-col gap-1 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
                Cumpleaños
                <input
                  type="date"
                  value={data?.birthday ?? ""}
                  disabled={!ofFanId}
                  className="cf-input"
                  style={{ padding: "0.4rem 0.65rem", fontSize: "0.8125rem" }}
                  onChange={(event) => void saveProfile({ birthday: event.target.value })}
                />
              </label>

              <label className="mt-2.5 flex flex-col gap-1 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
                Ubicación
                <input
                  value={data?.location ?? ""}
                  disabled={!ofFanId}
                  placeholder="Ej: Miami, FL"
                  className="cf-input"
                  style={{ padding: "0.4rem 0.65rem", fontSize: "0.8125rem" }}
                  onChange={(event) =>
                    setInsights((current) =>
                      current ? { ...current, location: event.target.value } : current,
                    )
                  }
                  onBlur={(event) => void saveProfile({ location: event.target.value })}
                />
              </label>

              <label className="mt-2.5 flex flex-col gap-1 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
                País
                <input
                  value={data?.country ?? ""}
                  disabled={!ofFanId}
                  placeholder="Ej: España"
                  className="cf-input"
                  style={{ padding: "0.4rem 0.65rem", fontSize: "0.8125rem" }}
                  onChange={(event) =>
                    setInsights((current) =>
                      current ? { ...current, country: event.target.value } : current,
                    )
                  }
                  onBlur={(event) => void saveProfile({ country: event.target.value })}
                />
              </label>

              <div className="mt-2 pt-2" style={{ borderTop: "1px solid var(--border-muted)" }}>
                <Metric label="Hora local" value="—" />
              </div>
            </Section>

            {/* Lists */}
            <Section
              title="Listas de OnlyFans"
              icon={ListFilter}
              open={open.lists}
              onToggle={() => setOpen((current) => ({ ...current, lists: !current.lists }))}
            >
              <div
                className="mt-2 inline-flex w-full rounded-lg p-0.5"
                style={{ background: "var(--bg-subtle)" }}
              >
                <button
                  type="button"
                  className="flex-1 rounded-md py-1 text-xs font-semibold transition-all"
                  style={
                    listTab === "fans"
                      ? {
                          background: "var(--bg-elevated)",
                          color: "var(--text-primary)",
                          boxShadow: "var(--shadow-sm)",
                        }
                      : { color: "var(--text-tertiary)" }
                  }
                  onClick={() => setListTab("fans")}
                >
                  Fans
                </button>
                <button
                  type="button"
                  className="flex-1 rounded-md py-1 text-xs font-semibold transition-all"
                  style={
                    listTab === "following"
                      ? {
                          background: "var(--bg-elevated)",
                          color: "var(--text-primary)",
                          boxShadow: "var(--shadow-sm)",
                        }
                      : { color: "var(--text-tertiary)" }
                  }
                  onClick={() => setListTab("following")}
                >
                  Siguiendo
                </button>
              </div>

              <input
                value={listQuery}
                placeholder="Filtrar listas…"
                className="cf-input mt-2.5"
                style={{ padding: "0.35rem 0.65rem", fontSize: "0.8125rem" }}
                onChange={(event) => setListQuery(event.target.value)}
              />

              {visibleLists.length === 0 ? (
                <p className="mt-3 text-center text-xs py-2" style={{ color: "var(--text-disabled)" }}>
                  No hay listas disponibles
                </p>
              ) : (
                <ul className="mt-2.5 flex flex-col gap-1.5 max-h-48 overflow-y-auto">
                  {visibleLists.map((item) => (
                    <li key={item.name}>
                      <label
                        className="flex items-center gap-2 rounded-lg px-2 py-1 text-xs cursor-pointer select-none transition-colors"
                        style={{
                          background: item.checked ? "var(--bg-subtle)" : "transparent",
                          color: item.checked ? "var(--text-primary)" : "var(--text-secondary)",
                        }}
                      >
                        <input
                          type="checkbox"
                          checked={item.checked}
                          disabled={!ofFanId}
                          className="rounded"
                          style={{ accentColor: "var(--brand-600)" }}
                          onChange={(event) => {
                            const next = toggleList(lists, listTab, item.name, event.target.checked);
                            void changeLists(next);
                          }}
                        />
                        <span className="truncate">{item.name}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              )}

              <form
                className="mt-3 flex gap-1.5"
                onSubmit={(event) => {
                  event.preventDefault();
                  const name = newList.trim();
                  if (!name || !ofFanId) return;
                  const bucket = lists[listTab];
                  if (bucket.some((item) => item.name.toLowerCase() === name.toLowerCase())) {
                    setNewList("");
                    return;
                  }
                  void changeLists({
                    ...lists,
                    [listTab]: [...bucket, { name, checked: true }],
                  });
                  setNewList("");
                }}
              >
                <input
                  value={newList}
                  disabled={!ofFanId}
                  placeholder="Nueva lista…"
                  className="cf-input min-w-0 flex-1"
                  style={{ padding: "0.35rem 0.65rem", fontSize: "0.8125rem" }}
                  onChange={(event) => setNewList(event.target.value)}
                />
                <button
                  type="submit"
                  disabled={!ofFanId || !newList.trim()}
                  className="cf-btn-primary shrink-0"
                  style={{ padding: "0.35rem 0.65rem", fontSize: "0.75rem" }}
                >
                  <Plus size={13} strokeWidth={2.5} />
                  Crear
                </button>
              </form>
            </Section>
          </div>
        ) : null}

        {/* SALES TAB */}
        {tab === "sales" ? (
          <div className="flex flex-col gap-3">
            <PpvTimeline modelId={modelId} ofFanId={ofFanId || null} reloadKey={ppvReload} />
            {(data?.sales.length ?? 0) === 0 ? (
              <div className="flex flex-col items-center justify-center py-12 text-center">
                <CreditCard size={28} strokeWidth={1.5} style={{ color: "var(--text-disabled)" }} />
                <p className="mt-2 text-xs" style={{ color: "var(--text-tertiary)" }}>
                  Sin historial de transacciones para este fan
                </p>
              </div>
            ) : (
              <ul className="flex flex-col gap-2">
                {data?.sales.map((sale) => (
                  <li
                    key={sale.ofTransactionId}
                    className="cf-card p-3 flex items-center justify-between gap-3"
                  >
                    <div>
                      <span className="block font-semibold text-xs" style={{ color: "var(--text-primary)" }}>
                        {sale.kind}
                      </span>
                      <span className="block text-[11px] mt-0.5" style={{ color: "var(--text-tertiary)" }}>
                        {saleStamp(sale.createdAt)}
                      </span>
                    </div>
                    <span className="tabular-nums font-bold text-sm" style={{ color: "var(--brand-600)" }}>
                      {formatMoney(sale.netAmount)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        ) : null}

        {/* NOTES TAB */}
        {tab === "notes" ? (
          <form
            className="flex h-full flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              void persistNotes();
            }}
          >
            <div className="relative flex-1">
              <textarea
                value={notes}
                disabled={!ofFanId}
                placeholder={ofFanId ? "Escribe notas o preferencias sobre este fan…" : "Selecciona un fan para escribir notas"}
                className="cf-input h-full min-h-60 w-full resize-none p-3.5 leading-relaxed"
                style={{ fontSize: "0.875rem" }}
                onChange={(event) => setNotes(event.target.value)}
              />
            </div>
            <div className="flex items-center justify-between">
              {savedNoteSuccess ? (
                <span className="text-xs font-semibold" style={{ color: "#059669" }}>
                  ✓ Guardado exitosamente
                </span>
              ) : <span />}
              <button
                type="submit"
                disabled={!ofFanId}
                className="cf-btn-primary"
                style={{ padding: "0.45rem 0.9rem" }}
              >
                <FileText size={14} strokeWidth={2} />
                Guardar notas
              </button>
            </div>
          </form>
        ) : null}
      </div>

      {/* Bottom Segmented Tab Navigation */}
      <nav
        className="grid grid-cols-3 p-1.5 gap-1 shrink-0"
        style={{
          background: "var(--bg-elevated)",
          borderTop: "1px solid var(--border)",
        }}
        aria-label="Secciones Fan Insights"
      >
        <button
          type="button"
          aria-pressed={tab === "insights"}
          className="flex items-center justify-center gap-1.5 rounded-lg py-2 text-xs font-semibold transition-all"
          style={
            tab === "insights"
              ? {
                  background: "var(--brand-50)",
                  color: "var(--brand-700)",
                }
              : { color: "var(--text-tertiary)" }
          }
          onClick={() => setTab("insights")}
        >
          <Sparkles size={13} strokeWidth={2} />
          Insights
        </button>

        <button
          type="button"
          aria-pressed={tab === "sales"}
          className="flex items-center justify-center gap-1.5 rounded-lg py-2 text-xs font-semibold transition-all"
          style={
            tab === "sales"
              ? {
                  background: "var(--brand-50)",
                  color: "var(--brand-700)",
                }
              : { color: "var(--text-tertiary)" }
          }
          onClick={() => setTab("sales")}
        >
          <CreditCard size={13} strokeWidth={2} />
          Ventas
        </button>

        <button
          type="button"
          aria-pressed={tab === "notes"}
          className="flex items-center justify-center gap-1.5 rounded-lg py-2 text-xs font-semibold transition-all"
          style={
            tab === "notes"
              ? {
                  background: "var(--brand-50)",
                  color: "var(--brand-700)",
                }
              : { color: "var(--text-tertiary)" }
          }
          onClick={() => setTab("notes")}
        >
          <FileText size={13} strokeWidth={2} />
          Notas
        </button>
      </nav>
    </aside>
  );
}

/* ── UI Helpers ───────────────────────────────────────────────────── */

function SpendLine({
  label,
  total,
  count,
  showAverage,
}: {
  label: string;
  total: number;
  count: number;
  showAverage: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-3 text-xs">
      <span style={{ color: "var(--text-secondary)" }}>{label}</span>
      <span className="text-right tabular-nums font-semibold" style={{ color: "var(--text-primary)" }}>
        {formatMoney(total)}
        {showAverage ? (
          <span className="ml-1 text-[11px] font-normal" style={{ color: "var(--text-tertiary)" }}>
            ({count === 0 ? "—" : `${formatMoney(total / count)} prom.`})
          </span>
        ) : null}
      </span>
    </div>
  );
}

function Metric({
  label,
  value,
  accent = false,
  isHighlight = false,
}: {
  label: string;
  value: string;
  accent?: boolean;
  isHighlight?: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-3 py-1 text-xs">
      <span style={{ color: "var(--text-secondary)" }}>{label}</span>
      <span
        className={`tabular-nums ${
          isHighlight
            ? "font-bold text-sm"
            : accent
            ? "font-semibold"
            : "font-medium"
        }`}
        style={
          accent
            ? { color: "var(--brand-600)" }
            : { color: "var(--text-primary)" }
        }
      >
        {value}
      </span>
    </div>
  );
}

function Section({
  title,
  icon: Icon,
  open,
  onToggle,
  children,
}: {
  title: string;
  icon: typeof DollarSign;
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <section className="cf-card p-3.5">
      <button
        type="button"
        className="flex w-full items-center justify-between text-left cursor-pointer"
        onClick={onToggle}
      >
        <span className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider" style={{ color: "var(--text-tertiary)" }}>
          <Icon size={14} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
          {title}
        </span>
        {open ? (
          <ChevronUp size={15} strokeWidth={2} style={{ color: "var(--text-tertiary)" }} />
        ) : (
          <ChevronDown size={15} strokeWidth={2} style={{ color: "var(--text-tertiary)" }} />
        )}
      </button>
      {open ? <div className="mt-3 pt-3" style={{ borderTop: "1px solid var(--border-muted)" }}>{children}</div> : null}
    </section>
  );
}

function saleStamp(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime()) || date.getUTCFullYear() < 2016) {
    return "—";
  }
  return formatStamp(value);
}

function toggleList(lists: FanLists, tab: ListTab, name: string, checked: boolean): FanLists {
  const update = (items: FanListItem[]) =>
    items.map((item) => (item.name === name ? { ...item, checked } : item));
  return tab === "fans"
    ? { ...lists, fans: update(lists.fans) }
    : { ...lists, following: update(lists.following) };
}
