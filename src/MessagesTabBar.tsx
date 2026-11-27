import { listen } from "@tauri-apps/api/event";
import { DollarSign, PanelLeft, PanelRightClose, PanelRightOpen, PanelTop, Plus, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  closeMessageTab,
  getMessageTabs,
  getModels,
  openMessages,
  selectMessageTab,
  setInsightsOpen,
  setMessageBarPlacement,
  type AppNotification,
  type MessageBarPlacement,
  type MessageTabsState,
} from "./api";
import { startSaleRecording } from "./saleRecording";
import { avatarColor, formatMoney, initials, modelLabel, type Model } from "./types";

const EMPTY_TABS: MessageTabsState = {
  openModelIds: [],
  activeModelId: null,
  barPlacement: "top",
  insightsOpen: true,
  unreadCounts: {},
};

export function MessagesTabBar() {
  const [models, setModels] = useState<Model[]>([]);
  const [tabs, setTabs] = useState<MessageTabsState>(EMPTY_TABS);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [recentSales, setRecentSales] = useState<Record<string, number>>({});
  const modelsRef = useRef(models);
  modelsRef.current = models;

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
    let stop = () => {};
    void startSaleRecording(() => modelsRef.current).then((unlisten) => {
      if (alive) stop = unlisten;
      else unlisten();
    });
    return () => {
      alive = false;
      stop();
    };
  }, []);

  useEffect(() => {
    let alive = true;
    const stops: Array<() => void> = [];
    const saleTimers = new Map<string, number>();

    void listen<MessageTabsState>("message-tabs-changed", (event) => {
      if (!alive) return;
      if (event.payload && Array.isArray(event.payload.openModelIds)) {
        setTabs({
          openModelIds: event.payload.openModelIds,
          activeModelId: event.payload.activeModelId ?? null,
          barPlacement: event.payload.barPlacement ?? "top",
          insightsOpen: event.payload.insightsOpen ?? true,
          unreadCounts: event.payload.unreadCounts ?? {},
        });
      } else {
        void reload();
      }
    }).then((stop) => {
      if (alive) stops.push(stop);
      else stop();
    });

    void listen<Record<string, number>>("unread-counts-changed", (event) => {
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
    const map = new Map<string, Model>();
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

  const changePlacement = (next: MessageBarPlacement) => {
    if (next === tabs.barPlacement) return;
    setTabs((prev) => ({ ...prev, barPlacement: next }));
    void setMessageBarPlacement(next).then((updated) => setTabs(updated)).catch(() => reload());
  };

  const toggleInsights = () => {
    const next = !tabs.insightsOpen;
    setTabs((prev) => ({ ...prev, insightsOpen: next }));
    void setInsightsOpen(next).then((updated) => setTabs(updated)).catch(() => reload());
  };

  if (tabs.barPlacement === "sidebar") {
    return (
      <aside
        className="flex h-screen min-h-0 w-screen min-w-0 select-none flex-col items-center overflow-hidden"
        style={{
          background: "var(--nav-bg)",
          borderRight: "1px solid var(--nav-border)",
          color: "var(--nav-text)",
        }}
      >
        <div className="flex w-full shrink-0 justify-center py-2" style={{ borderBottom: "1px solid var(--nav-border)" }}>
          <HubChrome
            stacked
            placement={tabs.barPlacement}
            insightsOpen={tabs.insightsOpen}
            onPlacement={changePlacement}
            onToggleInsights={toggleInsights}
          />
        </div>
        <div className="flex min-h-0 w-full flex-1 flex-col items-center gap-2 overflow-y-auto overflow-x-hidden py-2">
          {tabs.openModelIds.map((id) => {
            const model = modelById.get(id);
            const label = model ? modelLabel(model) : `Modelo #${id}`;
            const active = tabs.activeModelId === id;
            const unread = tabs.unreadCounts?.[id] ?? 0;
            const saleFlash = recentSales[id];
            const tip = model?.username ? `${label} (@${model.username})` : label;
            return (
              <div key={id} className="group relative">
                <button
                  type="button"
                  role="tab"
                  aria-selected={active}
                  title={tip}
                  onClick={() => void selectMessageTab(id)}
                  className="relative flex h-11 w-11 items-center justify-center rounded-full"
                  style={{
                    boxShadow: active ? "0 0 0 2px var(--brand-500)" : "none",
                  }}
                >
                  <TabAvatar model={model} id={id} label={label} size={40} />
                  {saleFlash != null ? (
                    <span
                      className="absolute -bottom-1 left-1/2 -translate-x-1/2 rounded-full px-1 text-[9px] font-bold"
                      style={{ background: "var(--success)", color: "#fff" }}
                    >
                      <DollarSign size={8} className="inline" />
                      {formatMoney(saleFlash).replace("$", "")}
                    </span>
                  ) : unread > 0 ? (
                    <span className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[9px] font-bold text-white" style={{ background: "var(--brand-500)" }}>
                      {unread > 99 ? "99+" : unread}
                    </span>
                  ) : null}
                </button>
                <button
                  type="button"
                  aria-label={`Cerrar ${label}`}
                  title={`Cerrar ${label}`}
                  onClick={() => void closeMessageTab(id)}
                  className="cf-chrome-chip absolute -left-0.5 -top-0.5 hidden h-4 w-4 items-center justify-center rounded-full group-hover:flex"
                >
                  <X size={10} />
                </button>
              </div>
            );
          })}
          {pickerOpen
            ? availableModels.map((model) => {
                const label = modelLabel(model);
                return (
                  <button
                    key={model.id}
                    type="button"
                    title={label}
                    onClick={() => {
                      setPickerOpen(false);
                      void openMessages(model.id);
                    }}
                    className="rounded-full opacity-70 hover:opacity-100"
                  >
                    <TabAvatar model={model} id={model.id} label={label} size={36} />
                  </button>
                );
              })
            : null}
        </div>
        {availableModels.length > 0 ? (
          <button
            type="button"
            title={pickerOpen ? "Cancelar" : "Añadir modelo"}
            aria-label={pickerOpen ? "Cancelar" : "Añadir modelo"}
            onClick={() => setPickerOpen((open) => !open)}
            className="cf-chrome-icon mb-2 flex h-9 w-9 shrink-0 items-center justify-center rounded-full"
          >
            {pickerOpen ? <X size={16} /> : <Plus size={16} />}
          </button>
        ) : null}
      </aside>
    );
  }

  /* ── Top Bar Mode (Barra de arriba) ─────────────────────────────── */
  return (
    <div
      className="flex h-screen min-h-0 w-screen min-w-0 select-none items-end gap-1 overflow-x-auto overflow-y-hidden px-2 pt-1.5"
      style={{
        background: "var(--nav-bg)",
        borderBottom: "1px solid var(--nav-border)",
        color: "var(--nav-text)",
      }}
    >
      <div className="mb-1 mr-1 shrink-0">
        <HubChrome
          placement={tabs.barPlacement}
          insightsOpen={tabs.insightsOpen}
          onPlacement={changePlacement}
          onToggleInsights={toggleInsights}
        />
      </div>

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
              background: active ? "var(--nav-active-bg)" : "transparent",
              color: active ? "#fff" : "var(--nav-muted)",
              borderTop: saleFlash != null
                ? "2px solid var(--success)"
                : active
                ? "2px solid var(--brand-500)"
                : "2px solid transparent",
              borderLeft: active ? "1px solid var(--nav-border)" : "1px solid transparent",
              borderRight: active ? "1px solid var(--nav-border)" : "1px solid transparent",
            }}
          >
            <TabAvatar model={model} id={id} label={label} />
            <span className="truncate">{label}</span>

            {saleFlash != null ? (
              <span
                className="inline-flex shrink-0 items-center rounded-full px-1.5 py-0.5 text-[10px] font-bold"
                style={{ background: "var(--success)", color: "#fff" }}
              >
                +{formatMoney(saleFlash)}
              </span>
            ) : unread > 0 ? (
              <span className="flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full px-1 text-[10px] font-bold text-white" style={{ background: "var(--brand-500)" }}>
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
              className="cf-chrome-icon ml-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded opacity-75"
            >
              <X size={12} strokeWidth={2.25} />
            </button>
          </div>
        );
      })}

      {availableModels.length > 0 ? (
        pickerOpen ? (
          <div className="mb-1 ml-1 flex shrink-0 items-center gap-1.5 rounded px-2 py-1" style={{ background: "var(--nav-active-bg)" }}>
            <span className="text-[11px] font-medium" style={{ color: "var(--nav-muted)" }}>Abrir:</span>
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
                  className="cf-chrome-chip flex shrink-0 items-center gap-1.5 rounded px-2 py-1 text-xs font-medium"
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
              className="cf-chrome-icon flex h-5 w-5 shrink-0 items-center justify-center rounded"
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
            className="cf-chrome-icon mb-1 ml-1 flex h-7 w-7 shrink-0 items-center justify-center rounded"
          >
            <Plus size={15} strokeWidth={2.25} />
          </button>
        )
      ) : null}
    </div>
  );
}

function HubChrome({
  placement,
  insightsOpen,
  stacked = false,
  onPlacement,
  onToggleInsights,
}: {
  placement: MessageBarPlacement;
  insightsOpen: boolean;
  stacked?: boolean;
  onPlacement: (next: MessageBarPlacement) => void;
  onToggleInsights: () => void;
}) {
  const item = (active: boolean) =>
    `inline-flex h-7 w-7 items-center justify-center rounded transition-colors ${
      active ? "cf-chrome-active" : "cf-chrome-chip"
    }`;
  return (
    <div
      className={`inline-flex gap-1 ${stacked ? "flex-col items-center" : "items-center"}`}
      role="group"
      aria-label="Distribución de Messages"
    >
      <button
        type="button"
        className={item(placement === "top")}
        aria-pressed={placement === "top"}
        title="Barra de arriba"
        aria-label="Barra de arriba"
        onClick={() => onPlacement("top")}
      >
        <PanelTop size={14} strokeWidth={2} />
      </button>
      <button
        type="button"
        className={item(placement === "sidebar")}
        aria-pressed={placement === "sidebar"}
        title="Barra lateral"
        aria-label="Barra lateral"
        onClick={() => onPlacement("sidebar")}
      >
        <PanelLeft size={14} strokeWidth={2} />
      </button>
      <button
        type="button"
        className={item(insightsOpen)}
        aria-pressed={insightsOpen}
        title={insightsOpen ? "Ocultar Fan Insights" : "Mostrar Fan Insights"}
        aria-label={insightsOpen ? "Ocultar Fan Insights" : "Mostrar Fan Insights"}
        onClick={onToggleInsights}
      >
        {insightsOpen ? <PanelRightClose size={14} strokeWidth={2} /> : <PanelRightOpen size={14} strokeWidth={2} />}
      </button>
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
  id: string;
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
