import { listen } from "@tauri-apps/api/event";
import { DollarSign, PanelLeft, PanelTop, Plus, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  closeMessageTab,
  getMessageTabs,
  getModels,
  openMessages,
  selectMessageTab,
  setMessageBarPlacement,
  type AppNotification,
  type MessageBarPlacement,
  type MessageTabsState,
} from "./api";
import { avatarColor, formatMoney, initials, modelLabel, type Model } from "./types";

const EMPTY_TABS: MessageTabsState = {
  openModelIds: [],
  activeModelId: null,
  barPlacement: "top",
  unreadCounts: {},
};

export function MessagesTabBar() {
  const [models, setModels] = useState<Model[]>([]);
  const [tabs, setTabs] = useState<MessageTabsState>(EMPTY_TABS);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [recentSales, setRecentSales] = useState<Record<number, number>>({});

  const reload = useCallback(async () => {
    try {
      const [nextModels, nextTabs] = await Promise.all([getModels(), getMessageTabs()]);
      setModels(nextModels);
      setTabs(nextTabs);
    } catch {
      // Keep last known tabs on transient errors.
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    let alive = true;
    const stops: Array<() => void> = [];
    const saleTimers = new Map<number, number>();

    void listen<MessageTabsState>("message-tabs-changed", (event) => {
      if (!alive) return;
      if (event.payload && Array.isArray(event.payload.openModelIds)) {
        setTabs({
          openModelIds: event.payload.openModelIds,
          activeModelId: event.payload.activeModelId ?? null,
          barPlacement: event.payload.barPlacement ?? "top",
          unreadCounts: event.payload.unreadCounts ?? {},
        });
      } else {
        void reload();
      }
    }).then((stop) => {
      if (alive) stops.push(stop);
      else stop();
    });

    void listen<Record<number, number>>("unread-counts-changed", (event) => {
      if (!alive || !event.payload) return;
      setTabs((prev) => ({ ...prev, unreadCounts: event.payload }));
    }).then((stop) => {
      if (alive) stops.push(stop);
      else stop();
    });

    void listen<AppNotification>("cf-notification", (event) => {
      if (!alive || !event.payload) return;
      const item = event.payload;
      if ((item.kind === "ppv" || item.kind === "tip") && item.amount != null) {
        const modelId = item.modelId;
        const amount = item.amount;
        setRecentSales((prev) => ({ ...prev, [modelId]: amount }));
        const prevTimer = saleTimers.get(modelId);
        if (prevTimer) window.clearTimeout(prevTimer);
        const timer = window.setTimeout(() => {
          saleTimers.delete(modelId);
          if (alive) {
            setRecentSales((prev) => {
              const copy = { ...prev };
              delete copy[modelId];
              return copy;
            });
          }
        }, 9000);
        saleTimers.set(modelId, timer);
      }
    }).then((stop) => {
      if (alive) stops.push(stop);
      else stop();
    });

    void listen("models-changed", () => {
      if (alive) void reload();
    }).then((stop) => {
      if (alive) stops.push(stop);
      else stop();
    });

    return () => {
      alive = false;
      stops.forEach((stop) => stop());
      saleTimers.forEach((t) => window.clearTimeout(t));
      saleTimers.clear();
    };
  }, [reload]);

  const modelById = useMemo(() => {
    const map = new Map<number, Model>();
    for (const model of models) {
      map.set(model.id, model);
    }
    return map;
  }, [models]);

  const openSet = useMemo(() => new Set(tabs.openModelIds), [tabs.openModelIds]);
  const availableModels = useMemo(
    () => models.filter((model) => !openSet.has(model.id)),
    [models, openSet],
  );

  useEffect(() => {
    if (pickerOpen && availableModels.length === 0) {
      setPickerOpen(false);
    }
  }, [pickerOpen, availableModels.length]);

  const togglePlacement = () => {
    const next: MessageBarPlacement = tabs.barPlacement === "sidebar" ? "top" : "sidebar";
    setTabs((prev) => ({ ...prev, barPlacement: next }));
    void setMessageBarPlacement(next).then((updated) => setTabs(updated)).catch(() => reload());
  };

  if (tabs.barPlacement === "sidebar") {
    return (
      <aside
        className="flex h-screen w-screen select-none flex-col overflow-hidden"
        style={{
          background: "#0f172a",
          borderRight: "1px solid #1e293b",
          color: "#e2e8f0",
        }}
      >
        {/* Sidebar Header + Toggle to Top Bar */}
        <div
          className="flex h-11 shrink-0 items-center justify-between gap-2 px-2.5"
          style={{ borderBottom: "1px solid #1e293b" }}
        >
          <span className="truncate text-[11px] font-semibold uppercase tracking-wider text-slate-400">
            Cuentas ({tabs.openModelIds.length})
          </span>
          <button
            type="button"
            onClick={togglePlacement}
            title="Cambiar a barra de arriba"
            aria-label="Cambiar a barra de arriba"
            className="inline-flex shrink-0 items-center gap-1 rounded-md bg-slate-800/90 px-2 py-1 text-[11px] font-medium text-slate-200 transition-colors hover:bg-blue-600 hover:text-white"
          >
            <PanelTop size={13} strokeWidth={2} />
            <span>Arriba</span>
          </button>
        </div>

        {/* Open Model Accounts List */}
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          <div className="flex flex-col gap-1">
            {tabs.openModelIds.map((id) => {
              const model = modelById.get(id);
              const label = model ? modelLabel(model) : `Modelo #${id}`;
              const active = tabs.activeModelId === id;
              const unread = tabs.unreadCounts?.[id] ?? 0;
              const saleFlash = recentSales[id];

              return (
                <div
                  key={id}
                  role="tab"
                  tabIndex={0}
                  aria-selected={active}
                  title={model?.username ? `${label} (@${model.username})` : label}
                  onClick={() => void selectMessageTab(id)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      void selectMessageTab(id);
                    }
                  }}
                  className="group flex cursor-pointer items-center gap-2 rounded-lg px-2 py-2 text-xs font-medium transition-colors"
                  style={{
                    background: active ? "#1e293b" : "transparent",
                    color: active ? "#f8fafc" : "#94a3b8",
                    borderLeft: active ? "3px solid #3b82f6" : "3px solid transparent",
                  }}
                >
                  <TabAvatar model={model} id={id} label={label} size={26} />
                  <div className="min-w-0 flex-1">
                    <span className="block truncate font-semibold">{label}</span>
                    {model?.username ? (
                      <span className="block truncate text-[10px] text-slate-400">
                        @{model.username}
                      </span>
                    ) : null}
                  </div>

                  {saleFlash != null ? (
                    <span
                      className="inline-flex shrink-0 items-center gap-0.5 rounded-full px-1.5 py-0.5 text-[10px] font-bold"
                      style={{ background: "#059669", color: "#ecfdf5" }}
                    >
                      <DollarSign size={10} strokeWidth={2.5} />
                      {formatMoney(saleFlash).replace("$", "")}
                    </span>
                  ) : unread > 0 ? (
                    <span className="flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full bg-blue-600 px-1 text-[10px] font-bold text-white">
                      {unread > 99 ? "99+" : unread}
                    </span>
                  ) : null}

                  <button
                    type="button"
                    aria-label={`Cerrar ${label}`}
                    title={`Cerrar ${label}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void closeMessageTab(id);
                    }}
                    className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-slate-400 opacity-60 transition-colors hover:bg-slate-700 hover:text-white group-hover:opacity-100"
                  >
                    <X size={12} strokeWidth={2.25} />
                  </button>
                </div>
              );
            })}
          </div>

          {/* Add model picker inside sidebar */}
          {availableModels.length > 0 ? (
            <div className="mt-2 pt-2" style={{ borderTop: "1px solid #1e293b" }}>
              {pickerOpen ? (
                <div className="flex flex-col gap-1 rounded-lg bg-slate-800/80 p-1.5">
                  <div className="flex items-center justify-between px-1 py-0.5">
                    <span className="text-[10px] font-semibold uppercase tracking-wider text-slate-400">
                      Añadir cuenta
                    </span>
                    <button
                      type="button"
                      aria-label="Cancelar"
                      onClick={() => setPickerOpen(false)}
                      className="rounded p-0.5 text-slate-400 hover:bg-slate-700 hover:text-white"
                    >
                      <X size={12} />
                    </button>
                  </div>
                  {availableModels.map((model) => {
                    const label = modelLabel(model);
                    return (
                      <button
                        key={model.id}
                        type="button"
                        onClick={() => {
                          setPickerOpen(false);
                          void openMessages(model.id);
                        }}
                        className="flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs font-medium text-slate-200 transition-colors hover:bg-blue-600 hover:text-white"
                      >
                        <TabAvatar model={model} id={model.id} label={label} size={20} />
                        <span className="truncate">{label}</span>
                      </button>
                    );
                  })}
                </div>
              ) : (
                <button
                  type="button"
                  onClick={() => setPickerOpen(true)}
                  className="flex w-full items-center justify-center gap-1.5 rounded-lg border border-dashed border-slate-700 py-1.5 text-xs font-medium text-slate-300 transition-colors hover:border-blue-500 hover:bg-slate-800/70 hover:text-white"
                >
                  <Plus size={14} strokeWidth={2.25} />
                  Añadir modelo
                </button>
              )}
            </div>
          ) : null}
        </div>
      </aside>
    );
  }

  /* ── Top Bar Mode (Barra de arriba) ─────────────────────────────── */
  return (
    <div
      className="flex h-screen w-screen select-none items-end gap-1 overflow-x-auto overflow-y-hidden px-2 pt-1.5"
      style={{
        background: "#0f172a",
        borderBottom: "1px solid #1e293b",
        color: "#e2e8f0",
      }}
    >
      {/* Toggle button: Top Bar <-> Left Sidebar */}
      <button
        type="button"
        onClick={togglePlacement}
        title="Cambiar a barra lateral"
        aria-label="Cambiar a barra lateral"
        className="mb-1 mr-1 inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-slate-800/90 px-2 text-[11px] font-medium text-slate-200 transition-colors hover:bg-blue-600 hover:text-white"
      >
        <PanelLeft size={13} strokeWidth={2} />
        <span>Lateral</span>
      </button>

      {tabs.openModelIds.map((id) => {
        const model = modelById.get(id);
        const label = model ? modelLabel(model) : `Modelo #${id}`;
        const active = tabs.activeModelId === id;
        const unread = tabs.unreadCounts?.[id] ?? 0;
        const saleFlash = recentSales[id];

        return (
          <div
            key={id}
            role="tab"
            tabIndex={0}
            aria-selected={active}
            title={model?.username ? `${label} (@${model.username})` : label}
            onClick={() => void selectMessageTab(id)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                void selectMessageTab(id);
              }
            }}
            className="group relative flex h-9 max-w-[220px] shrink-0 cursor-pointer items-center gap-2 rounded-t-lg px-2.5 text-xs font-medium transition-colors"
            style={{
              background: active ? "#1e293b" : "transparent",
              color: active ? "#f8fafc" : "#94a3b8",
              borderTop: saleFlash != null
                ? "2px solid #10b981"
                : active
                ? "2px solid #3b82f6"
                : "2px solid transparent",
              borderLeft: active ? "1px solid #334155" : "1px solid transparent",
              borderRight: active ? "1px solid #334155" : "1px solid transparent",
            }}
          >
            <TabAvatar model={model} id={id} label={label} />
            <span className="truncate">{label}</span>

            {saleFlash != null ? (
              <span
                className="inline-flex shrink-0 items-center rounded-full px-1.5 py-0.5 text-[10px] font-bold"
                style={{ background: "#059669", color: "#ecfdf5" }}
              >
                +{formatMoney(saleFlash)}
              </span>
            ) : unread > 0 ? (
              <span className="flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full bg-blue-600 px-1 text-[10px] font-bold text-white">
                {unread > 99 ? "99+" : unread}
              </span>
            ) : null}

            <button
              type="button"
              aria-label={`Cerrar ${label}`}
              title={`Cerrar ${label}`}
              onClick={(e) => {
                e.stopPropagation();
                void closeMessageTab(id);
              }}
              className="ml-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded text-slate-400 opacity-75 transition-colors hover:bg-slate-700 hover:text-white hover:opacity-100"
            >
              <X size={12} strokeWidth={2.25} />
            </button>
          </div>
        );
      })}

      {availableModels.length > 0 ? (
        pickerOpen ? (
          <div className="mb-1 ml-1 flex shrink-0 items-center gap-1.5 rounded-lg bg-slate-800/90 px-2 py-1">
            <span className="text-[11px] font-medium text-slate-400">Abrir:</span>
            {availableModels.map((model) => {
              const label = modelLabel(model);
              return (
                <button
                  key={model.id}
                  type="button"
                  onClick={() => {
                    setPickerOpen(false);
                    void openMessages(model.id);
                  }}
                  className="flex shrink-0 items-center gap-1.5 rounded-md bg-slate-700/80 px-2 py-1 text-xs font-medium text-slate-100 transition-colors hover:bg-blue-600"
                >
                  <TabAvatar model={model} id={model.id} label={label} size={18} />
                  <span className="max-w-[110px] truncate">{label}</span>
                </button>
              );
            })}
            <button
              type="button"
              aria-label="Cancelar"
              title="Cancelar"
              onClick={() => setPickerOpen(false)}
              className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-slate-400 hover:bg-slate-700 hover:text-white"
            >
              <X size={12} strokeWidth={2.25} />
            </button>
          </div>
        ) : (
          <button
            type="button"
            aria-label="Añadir cuenta a las pestañas"
            title="Añadir otra modelo"
            onClick={() => setPickerOpen(true)}
            className="mb-1 ml-1 flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-slate-300 transition-colors hover:bg-slate-800 hover:text-white"
          >
            <Plus size={15} strokeWidth={2.25} />
          </button>
        )
      ) : null}
    </div>
  );
}

function TabAvatar({
  model,
  id,
  label,
  size = 20,
}: {
  model: Model | undefined;
  id: number;
  label: string;
  size?: number;
}) {
  const [failed, setFailed] = useState(false);
  const url = model?.avatar_url?.trim() ?? "";

  if (url && !failed) {
    return (
      <img
        src={url}
        alt=""
        style={{ width: size, height: size }}
        className="shrink-0 rounded-full object-cover"
        onError={() => setFailed(true)}
      />
    );
  }

  return (
    <span
      aria-hidden="true"
      style={{ width: size, height: size, backgroundColor: avatarColor(id) }}
      className="flex shrink-0 items-center justify-center rounded-full text-[9px] font-semibold text-white"
    >
      {initials(label)}
    </span>
  );
}
