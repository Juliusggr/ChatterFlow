import {
  BarChart3,
  LayoutDashboard,
  MessageSquare,
  Moon,
  PanelLeft,
  PanelTop,
  Plus,
  Sun,
  UserRound,
  Users,
} from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, onAction, requestPermission } from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  errorMessage,
  getMessageTabs,
  getModels,
  openIncomingChat,
  openMessages,
  openModelSession,
  setMessageBarPlacement,
  type MessageBarPlacement,
  type MessageTabsState,
} from "./api";
import { isManager, signOut, type AgencyProfile } from "./agency";
import { startSaleRecording } from "./saleRecording";
import { AuthGate } from "./AuthGate";
import { AnalyticsView } from "./AnalyticsView";
import { DashboardView } from "./DashboardView";
import { FanInsightsPanel } from "./FanInsights";
import { MessagesTabBar } from "./MessagesTabBar";
import { ModelsView } from "./ModelsView";
import {
  NotificationBellPopover,
  NotificationToastStack,
  type ToastBanner,
} from "./Notifications";
import { TeamsView } from "./TeamsView";
import { syncDropperShiftRecords } from "./reports/data";
import { avatarColor, initials, modelLabel, type Model } from "./types";

type Section = "dashboard" | "teams" | "models" | "messages" | "analytics";

const NAV: { id: Section; label: string; icon: typeof LayoutDashboard }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "teams", label: "Equipos y Roles", icon: Users },
  { id: "models", label: "Modelos", icon: UserRound },
  { id: "messages", label: "Messages", icon: MessageSquare },
  { id: "analytics", label: "Analytics", icon: BarChart3 },
];

/* ── Dark-mode hook ───────────────────────────────────────────────── */
function useDarkMode(): [boolean, () => void] {
  const [dark, setDark] = useState(() => {
    const stored = localStorage.getItem("cf-theme");
    if (stored) return stored === "dark";
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  });

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("cf-theme", dark ? "dark" : "light");
  }, [dark]);

  return [dark, () => setDark((d) => !d)];
}

/* ── Entry point ──────────────────────────────────────────────────── */
export function App() {
  return (
    <AuthGate>
      {(profile) => {
        if (window.__CF_VIEW__ === "messages-tabs") {
          return <MessagesTabBar />;
        }
        const embeddedModelId = window.__CF_VIEW__ === "insights" ? window.__CF_MODEL_ID__ : undefined;
        if (embeddedModelId) {
          return <FanInsightsPanel modelId={embeddedModelId} />;
        }
        return <AppShell profile={profile} />;
      }}
    </AuthGate>
  );
}

declare global {
  interface Window {
    __CF_VIEW__?: string;
    __CF_MODEL_ID__?: string;
    __CF_PARTITION_ID__?: string;
  }
}

/* ── AppShell ─────────────────────────────────────────────────────── */
function AppShell({ profile }: { profile: AgencyProfile }) {
  const canManage = isManager(profile.role);
  const canOpenMessages = profile.role !== "dropper";
  const [section, setSection] = useState<Section>("dashboard");
  const [navOpen, setNavOpen] = useState(() => window.innerWidth >= 1100);
  const [models, setModels] = useState<Model[]>([]);
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dark, toggleDark] = useDarkMode();
  const [barPlacement, setBarPlacement] = useState<MessageBarPlacement>("top");
  const [unreadCounts, setUnreadCounts] = useState<Record<string, number>>({});
  const [loginBanners, setLoginBanners] = useState<ToastBanner[]>([]);
  const modelsRef = useRef(models);
  modelsRef.current = models;

  const reload = useCallback(async () => {
    try {
      const [visible, tabsState] = await Promise.all([getModels(), getMessageTabs()]);
      setModels(visible);
      setBarPlacement(tabsState.barPlacement ?? "top");
      setUnreadCounts(tabsState.unreadCounts ?? {});
      setSelectedModelId((current) =>
        current != null && visible.some((model) => model.id === current) ? current : null,
      );
      setError(null);
      setLoaded(true);
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  useEffect(() => { void reload(); }, [reload]);

  useEffect(() => {
    if (canOpenMessages && section === "messages" && selectedModelId) {
      void openMessages(selectedModelId).catch((caught) => setError(errorMessage(caught)));
    }
  }, [canOpenMessages, section, selectedModelId]);

  useEffect(() => {
    if ((!canManage && section === "teams") || (!canOpenMessages && section === "messages")) {
      setSection("dashboard");
    }
  }, [canManage, canOpenMessages, section]);

  useEffect(() => {
    if (profile.role !== "dropper" && !canManage) return;
    let alive = true;
    let stop = () => {};
    void listen("dropper-activity-changed", () => {
      if (alive) void syncDropperShiftRecords();
    })
      .then((unlisten) => {
        if (alive) stop = unlisten;
        else unlisten();
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      stop();
    };
  }, [profile.role, canManage]);

  useEffect(() => {
    if (profile.role === "dropper") return;
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
  }, [profile.role]);

  useEffect(() => {
    let alive = true;
    const stops: Array<() => void> = [];

    void listen("models-changed", () => {
      if (alive) void reload();
    })
      .then((stop) => {
        if (alive) stops.push(stop);
        else stop();
      })
      .catch(() => undefined);

    void listen<MessageTabsState>("message-tabs-changed", (event) => {
      if (!alive || !event.payload) return;
      if (event.payload.barPlacement) {
        setBarPlacement(event.payload.barPlacement);
      }
      if (event.payload.unreadCounts) {
        setUnreadCounts(event.payload.unreadCounts);
      }
    })
      .then((stop) => {
        if (alive) stops.push(stop);
        else stop();
      })
      .catch(() => undefined);

    void listen<Record<string, number>>("unread-counts-changed", (event) => {
      if (!alive || !event.payload) return;
      setUnreadCounts(event.payload);
    })
      .then((stop) => {
        if (alive) stops.push(stop);
        else stop();
      })
      .catch(() => undefined);

    void listen<Model>("model-login-completed", (event) => {
      if (!alive || !event.payload) return;
      void reload();
      const connected = event.payload;
      const label = modelLabel(connected);
      const id = `login-${connected.id}-${Date.now()}`;
      setLoginBanners((prev) => [
        {
          id,
          title: `Sesión iniciada · ${label}`,
          subtitle: connected.username
            ? `Cuenta @${connected.username} lista y sincronizando balance.`
            : "Cuenta conectada correctamente.",
          tone: "green",
          modelId: connected.id,
        },
        ...prev.slice(0, 2),
      ]);
      window.setTimeout(() => {
        setLoginBanners((prev) => prev.filter((item) => item.id !== id));
      }, 7000);
    })
      .then((stop) => {
        if (alive) stops.push(stop);
        else stop();
      })
      .catch(() => undefined);

    return () => {
      alive = false;
      stops.forEach((s) => s());
    };
  }, [reload]);

  const handleOpenModelChat = useCallback((modelId: string) => {
    if (!canOpenMessages) return;
    setSelectedModelId(modelId);
    setSection("messages");
    void openMessages(modelId).catch((caught) => setError(errorMessage(caught)));
  }, [canOpenMessages]);

  const handleOpenChat = useCallback((modelId: string, ofFanId: string) => {
    if (!canOpenMessages) return;
    setSelectedModelId(modelId);
    setSection("messages");
    void openIncomingChat(modelId, ofFanId).catch((caught) => setError(errorMessage(caught)));
  }, [canOpenMessages]);

  useEffect(() => {
    let alive = true;
    let stop: { unregister: () => Promise<void> } | undefined;
    void (async () => {
      try {
        if (!(await isPermissionGranted())) {
          await requestPermission();
        }
      } catch {
        // The desktop shell may already be allowed to show toasts.
      }
      try {
        const unlisten = await onAction((event) => {
          const extra = event.notification?.extra ?? {};
          const modelId = typeof extra.modelId === "string" ? extra.modelId.trim() : "";
          const ofFanId = typeof extra.ofFanId === "string" ? extra.ofFanId : "";
          if (canOpenMessages && modelId) {
            setSelectedModelId(modelId);
            setSection("messages");
            void openIncomingChat(modelId, ofFanId);
          }
        });
        if (alive) stop = unlisten;
        else void unlisten.unregister();
      } catch {
        // Notification actions are only available inside the Tauri shell.
      }
    })();
    return () => {
      alive = false;
      void stop?.unregister();
    };
  }, [canOpenMessages]);

  const handleChangeBarPlacement = useCallback((next: MessageBarPlacement) => {
    setBarPlacement(next);
    void setMessageBarPlacement(next)
      .then((updated) => {
        setBarPlacement(updated.barPlacement);
      })
      .catch((caught) => setError(errorMessage(caught)));
  }, []);

  return (
    <div
      className="flex h-screen min-h-0"
      style={{ background: "var(--bg)", color: "var(--text-primary)" }}
    >
      <NotificationToastStack
        enableSystemEffects
        onOpenModel={handleOpenModelChat}
        onOpenChat={handleOpenChat}
        extraBanners={loginBanners}
        onDismissBanner={(id) =>
          setLoginBanners((prev) => prev.filter((item) => item.id !== id))
        }
      />

      {/* Model Rail */}
      <ModelRail
        models={models}
        selectedModelId={selectedModelId}
        unreadCounts={unreadCounts}
        onOpenModels={() => setSection("models")}
        onSelect={(id) => {
          setSelectedModelId(id);
          const open =
            canOpenMessages && section === "messages" ? openMessages(id) : openModelSession(id);
          void open.catch((caught) => setError(errorMessage(caught)));
        }}
      />

      {/* Sidebar nav */}
      <aside
        className={`flex shrink-0 flex-col transition-[width] duration-200 ease-out ${navOpen ? "w-60" : "w-[4.5rem]"}`}
        style={{
          background: "var(--nav-bg)",
          borderRight: "1px solid var(--nav-border)",
          color: "var(--nav-text)",
        }}
      >
        {/* Logo / toggle */}
        <div
          className={`flex items-center gap-3 px-3 py-4 ${navOpen ? "" : "justify-center"}`}
          style={{ borderBottom: "1px solid var(--nav-border)" }}
        >
          <button
            type="button"
            className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors"
            style={{ color: "var(--nav-text)" }}
            aria-expanded={navOpen}
            aria-controls="app-nav"
            aria-label={navOpen ? "Contraer menú" : "Expandir menú"}
            onClick={() => setNavOpen((open) => !open)}
            onMouseEnter={(e) => {
              (e.currentTarget as HTMLButtonElement).style.background = "var(--nav-hover)";
            }}
            onMouseLeave={(e) => {
              (e.currentTarget as HTMLButtonElement).style.background = "transparent";
            }}
          >
            {/* Hamburger SVG */}
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round">
              <line x1="2" y1="5" x2="16" y2="5"/>
              <line x1="2" y1="9" x2="16" y2="9"/>
              <line x1="2" y1="13" x2="16" y2="13"/>
            </svg>
          </button>
          {navOpen && (
            <span className="truncate text-sm font-semibold tracking-tight" style={{ color: "#fff" }}>
              CreatorFloww
            </span>
          )}
        </div>

        {/* Nav items */}
        <nav id="app-nav" aria-label="Secciones" className="flex flex-col gap-0.5 p-2 flex-1">
          {NAV.filter((item) => (canManage || item.id !== "teams") && (canOpenMessages || item.id !== "messages")).map((item) => {
            const Icon = item.icon;
            const active = section === item.id;
            return (
              <button
                key={item.id}
                type="button"
                title={item.label}
                aria-label={item.label}
                aria-current={active ? "page" : undefined}
                className={`flex items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm font-medium transition-colors ${navOpen ? "" : "justify-center px-0"} ${active ? "cf-nav-active" : ""}`}
                style={
                  !active
                    ? { color: "var(--nav-text)" }
                    : undefined
                }
                onMouseEnter={(e) => {
                  if (!active) {
                    (e.currentTarget as HTMLButtonElement).style.background = "var(--nav-hover)";
                    (e.currentTarget as HTMLButtonElement).style.color = "#fff";
                  }
                }}
                onMouseLeave={(e) => {
                  if (!active) {
                    (e.currentTarget as HTMLButtonElement).style.background = "transparent";
                    (e.currentTarget as HTMLButtonElement).style.color = "var(--nav-text)";
                  }
                }}
                onClick={() => setSection(item.id)}
              >
                <Icon size={17} strokeWidth={active ? 2 : 1.75} />
                {navOpen ? <span className="truncate">{item.label}</span> : null}
              </button>
            );
          })}
        </nav>

        {/* Notifications & Dark mode controls at bottom */}
        <div
          className={`flex flex-col gap-1.5 p-2 pb-4 ${navOpen ? "px-3" : "items-center"}`}
          style={{ borderTop: "1px solid var(--nav-border)" }}
        >
          <button
            type="button"
            className={`flex items-center gap-3 rounded-lg px-3 py-2 text-left text-sm ${navOpen ? "" : "justify-center px-0"}`}
            style={{ color: "var(--nav-text)" }}
            onClick={() => void signOut()}
          >
            {navOpen ? <span className="truncate">{profile.username}</span> : null}
            <span className="text-xs">Salir</span>
          </button>
          <NotificationBellPopover navOpen={navOpen} onOpenChat={handleOpenChat} />
          <div className={`flex items-center ${navOpen ? "px-1" : "justify-center"}`}>
            <button
              type="button"
              onClick={toggleDark}
              aria-label={dark ? "Modo claro" : "Modo oscuro"}
              title={dark ? "Modo claro" : "Modo oscuro"}
              className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors"
              style={{ color: "var(--nav-text)" }}
              onMouseEnter={(e) => {
                (e.currentTarget as HTMLButtonElement).style.background = "var(--nav-hover)";
                (e.currentTarget as HTMLButtonElement).style.color = "#fff";
              }}
              onMouseLeave={(e) => {
                (e.currentTarget as HTMLButtonElement).style.background = "transparent";
                (e.currentTarget as HTMLButtonElement).style.color = "var(--nav-text)";
              }}
            >
              {dark ? <Sun size={16} strokeWidth={1.75} /> : <Moon size={16} strokeWidth={1.75} />}
            </button>
            {navOpen && (
              <span className="ml-1 self-center text-xs" style={{ color: "var(--nav-muted)" }}>
                {dark ? "Modo claro" : "Modo oscuro"}
              </span>
            )}
          </div>
        </div>
      </aside>

      {/* Main content */}
      <main
        className="min-w-0 flex-1 overflow-auto"
        style={section === "analytics" ? { background: "var(--report-canvas)" } : undefined}
      >
        {loaded && section === "analytics" ? (
          <AnalyticsView />
        ) : (
        <div className="mx-auto flex w-full max-w-6xl flex-col gap-6 px-6 py-8">
          {error ? (
            <div
              role="alert"
              className="flex flex-wrap items-center justify-between gap-3 rounded-xl px-4 py-3 text-sm"
              style={{
                background: "var(--danger-bg)",
                border: "1px solid var(--danger-border)",
                color: "var(--danger)",
              }}
            >
              <p>{error}</p>
              <button
                type="button"
                className="cf-btn-secondary text-xs"
                style={{ color: "var(--danger)", borderColor: "var(--danger-border)" }}
                onClick={() => void reload()}
              >
                Reintentar
              </button>
            </div>
          ) : null}

          {!loaded && !error ? (
            <div className="flex flex-col gap-4">
              <div className="cf-skeleton h-8 w-48" />
              <div className="cf-skeleton h-32 w-full" />
              <div className="grid gap-4 lg:grid-cols-3">
                {[0,1,2].map(i => <div key={i} className="cf-skeleton h-40" />)}
              </div>
            </div>
          ) : null}

          {loaded && section === "dashboard" ? <DashboardView canDeleteShifts={profile.role === "owner"} /> : null}
          {loaded && section === "teams" && canManage ? <TeamsView models={models} /> : null}
          {loaded && section === "models" ? (
            <ModelsView
              models={models}
              canManage={canManage}
              selectedModelId={selectedModelId}
              onChanged={reload}
              onSelect={setSelectedModelId}
              onOpenMessages={canOpenMessages ? handleOpenModelChat : undefined}
            />
          ) : null}
          {loaded && canOpenMessages && section === "messages" ? (
            <MessagesSection
              models={models}
              barPlacement={barPlacement}
              unreadCounts={unreadCounts}
              onChangePlacement={handleChangeBarPlacement}
              onOpen={handleOpenModelChat}
            />
          ) : null}
        </div>
        )}
      </main>
    </div>
  );
}

/* ── MessagesSection ──────────────────────────────────────────────── */
function MessagesSection({
  models,
  barPlacement,
  unreadCounts,
  onChangePlacement,
  onOpen,
}: {
  models: Model[];
  barPlacement: MessageBarPlacement;
  unreadCounts: Record<string, number>;
  onChangePlacement: (placement: MessageBarPlacement) => void;
  onOpen: (modelId: string) => void;
}) {
  return (
    <section className="flex flex-col gap-5">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
            Messages
          </h1>
          <p className="mt-1 text-sm" style={{ color: "var(--text-secondary)" }}>
            {barPlacement === "sidebar"
              ? "Las cuentas se muestran en una barra lateral izquierda dentro de la ventana de mensajes."
              : "Las cuentas se muestran en la barra superior de pestañas dentro de la ventana de mensajes."}
          </p>
        </div>

        {/* Toggle between Top Bar and Sidebar for Messages account selector */}
        <div
          role="group"
          aria-label="Posición del selector de cuentas en Mensajes"
          className="inline-flex items-center rounded-xl p-1"
          style={{
            background: "var(--bg-subtle)",
            border: "1px solid var(--border)",
          }}
        >
          <button
            type="button"
            aria-pressed={barPlacement === "top"}
            onClick={() => onChangePlacement("top")}
            className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-semibold transition-all"
            style={
              barPlacement === "top"
                ? {
                    background: "var(--bg-elevated)",
                    color: "var(--brand-700)",
                    boxShadow: "var(--shadow-sm)",
                  }
                : { color: "var(--text-secondary)" }
            }
          >
            <PanelTop size={14} strokeWidth={2} />
            Barra de arriba
          </button>
          <button
            type="button"
            aria-pressed={barPlacement === "sidebar"}
            onClick={() => onChangePlacement("sidebar")}
            className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-semibold transition-all"
            style={
              barPlacement === "sidebar"
                ? {
                    background: "var(--bg-elevated)",
                    color: "var(--brand-700)",
                    boxShadow: "var(--shadow-sm)",
                  }
                : { color: "var(--text-secondary)" }
            }
          >
            <PanelLeft size={14} strokeWidth={2} />
            Barra lateral
          </button>
        </div>
      </header>
      {models.length === 0 ? (
        <div className="cf-card flex max-w-sm flex-col items-center justify-center gap-3 py-16 text-center">
          <MessageSquare size={32} strokeWidth={1.25} style={{ color: "var(--text-disabled)" }} />
          <p className="text-sm font-medium" style={{ color: "var(--text-secondary)" }}>
            No hay modelos disponibles.
          </p>
        </div>
      ) : (
        <ul className="grid gap-3 sm:grid-cols-2">
          {models.map((model) => {
            const label = modelLabel(model);
            const unread = unreadCounts[model.id] ?? 0;
            return (
              <li key={model.id}>
                <button
                  type="button"
                  className="cf-card cf-card-hover flex w-full items-center gap-3 p-4 text-left"
                  onClick={() => onOpen(model.id)}
                >
                  <span
                    className="relative flex h-10 w-10 shrink-0 items-center justify-center overflow-hidden rounded-full text-xs font-semibold text-white"
                    style={{ backgroundColor: avatarColor(model.id) }}
                  >
                    <AvatarFace model={model} label={label} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-semibold" style={{ color: "var(--text-primary)" }}>
                      {label}
                    </span>
                    <span className="block truncate text-xs" style={{ color: "var(--text-tertiary)" }}>
                      {model.username ? `@${model.username}` : "Sin username"}
                    </span>
                  </span>
                  {unread > 0 ? (
                    <span
                      className="flex h-5 min-w-5 items-center justify-center rounded-full px-1.5 text-[11px] font-bold text-white"
                      style={{ background: "var(--brand-600)" }}
                    >
                      {unread > 99 ? "99+" : unread}
                    </span>
                  ) : null}
                  <MessageSquare size={16} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/* ── ModelRail ────────────────────────────────────────────────────── */
function ModelRail({
  models,
  selectedModelId,
  unreadCounts,
  onOpenModels,
  onSelect,
}: {
  models: Model[];
  selectedModelId: string | null;
  unreadCounts: Record<string, number>;
  onOpenModels: () => void;
  onSelect: (id: string) => void;
}) {
  return (
    <aside
      className="flex w-[3.75rem] shrink-0 flex-col items-center px-2 py-3 gap-1"
      style={{
        background: "var(--nav-bg)",
        borderRight: "1px solid var(--nav-border)",
      }}
    >
      <div className="flex min-h-0 w-full flex-1 flex-col items-center gap-2 overflow-y-auto pb-2">
        {models.map((model) => {
          const label = modelLabel(model);
          const selected = model.id === selectedModelId;
          const unread = unreadCounts[model.id] ?? 0;
          return (
            <div key={model.id} className="relative">
              <button
                type="button"
                title={label === "Unknown" ? "Unknown" : `@${model.username || label}`}
                aria-label={label}
                aria-pressed={selected}
                className="relative flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-full text-xs font-semibold text-white transition-all"
                style={{
                  backgroundColor: avatarColor(model.id),
                  boxShadow: selected
                    ? `0 0 0 2px var(--bg-elevated), 0 0 0 4px var(--brand-500)`
                    : "none",
                  opacity: selected ? 1 : 0.85,
                }}
                onClick={() => onSelect(model.id)}
              >
                <AvatarFace model={model} label={label} />
              </button>
              {unread > 0 ? (
                <span
                  className="pointer-events-none absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[9px] font-bold text-white"
                  style={{ background: "var(--brand-600)" }}
                >
                  {unread > 9 ? "9+" : unread}
                </span>
              ) : null}
            </div>
          );
        })}

        <button
          type="button"
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-dashed transition-colors"
          style={{
            borderColor: "var(--nav-text)",
            color: "var(--nav-text)",
          }}
          aria-label="Ir a Modelos"
          title="Modelos"
          onClick={onOpenModels}
          onMouseEnter={(e) => {
            (e.currentTarget as HTMLButtonElement).style.borderColor = "#fff";
            (e.currentTarget as HTMLButtonElement).style.color = "#fff";
          }}
          onMouseLeave={(e) => {
            (e.currentTarget as HTMLButtonElement).style.borderColor = "var(--nav-text)";
            (e.currentTarget as HTMLButtonElement).style.color = "var(--nav-text)";
          }}
        >
          <Plus size={16} strokeWidth={2} />
        </button>
      </div>
    </aside>
  );
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
  return <span aria-hidden="true">{initials(label)}</span>;
}
