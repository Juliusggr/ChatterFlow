import { listen } from "@tauri-apps/api/event";
import {
  BadgeDollarSign,
  Bell,
  BellOff,
  CheckCircle2,
  DollarSign,
  MessageSquare,
  Trash2,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  clearNotificationsHistory,
  dismissNotificationItem,
  getRecentNotifications,
  setNotificationDelivery,
  type AppNotification,
} from "./api";
import { avatarColor, formatMoney, initials } from "./types";

export type NotificationPrefs = {
  messagesEnabled: boolean;
  salesEnabled: boolean;
  osPushEnabled: boolean;
  soundEnabled: boolean;
};

const PREFS_STORAGE_KEY = "cf-notification-prefs-v1";
const TOAST_TTL_MS = 7500;
const SALE_TOAST_TTL_MS = 10000;

const DEFAULT_PREFS: NotificationPrefs = {
  messagesEnabled: true,
  salesEnabled: true,
  osPushEnabled: true,
  soundEnabled: true,
};

export function loadNotificationPrefs(): NotificationPrefs {
  try {
    const raw = localStorage.getItem(PREFS_STORAGE_KEY);
    if (!raw) return DEFAULT_PREFS;
    const parsed = JSON.parse(raw) as Partial<NotificationPrefs>;
    return {
      messagesEnabled: parsed.messagesEnabled !== false,
      salesEnabled: parsed.salesEnabled !== false,
      osPushEnabled: parsed.osPushEnabled !== false,
      soundEnabled: parsed.soundEnabled !== false,
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function saveNotificationPrefs(prefs: NotificationPrefs): void {
  try {
    localStorage.setItem(PREFS_STORAGE_KEY, JSON.stringify(prefs));
    window.dispatchEvent(new CustomEvent("cf-prefs-changed", { detail: prefs }));
  } catch {
    // Ignore storage quota errors
  }
  void setNotificationDelivery(prefs.messagesEnabled, prefs.salesEnabled, prefs.osPushEnabled).catch(
    () => undefined,
  );
}

let sharedAudioCtx: AudioContext | null = null;

function playAlertChime(isSale: boolean): void {
  try {
    const AudioCtor =
      window.AudioContext ||
      (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!AudioCtor) return;
    if (!sharedAudioCtx) {
      sharedAudioCtx = new AudioCtor();
    }
    const ctx = sharedAudioCtx;
    if (ctx.state === "suspended") {
      void ctx.resume();
    }
    const now = ctx.currentTime;
    const notes = isSale ? [523.25, 659.25, 783.99, 1046.5] : [587.33, 880];
    const spacing = isSale ? 0.075 : 0.085;
    notes.forEach((freq, index) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = isSale ? "triangle" : "sine";
      osc.frequency.setValueAtTime(freq, now + index * spacing);
      gain.gain.setValueAtTime(0.0001, now + index * spacing);
      gain.gain.exponentialRampToValueAtTime(isSale ? 0.14 : 0.09, now + index * spacing + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + index * spacing + (isSale ? 0.28 : 0.18));
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start(now + index * spacing);
      osc.stop(now + index * spacing + (isSale ? 0.3 : 0.2));
    });
  } catch {
    // Audio playback is optional if blocked by OS policy
  }
}

export function useNotificationPrefs(): [NotificationPrefs, (next: NotificationPrefs) => void] {
  const [prefs, setPrefs] = useState<NotificationPrefs>(() => loadNotificationPrefs());

  useEffect(() => {
    const sync = () => setPrefs(loadNotificationPrefs());
    window.addEventListener("storage", sync);
    window.addEventListener("cf-prefs-changed", sync);
    return () => {
      window.removeEventListener("storage", sync);
      window.removeEventListener("cf-prefs-changed", sync);
    };
  }, []);

  const update = useCallback((next: NotificationPrefs) => {
    setPrefs(next);
    saveNotificationPrefs(next);
  }, []);

  useEffect(() => {
    void setNotificationDelivery(prefs.messagesEnabled, prefs.salesEnabled, prefs.osPushEnabled).catch(
      () => undefined,
    );
  }, [prefs.messagesEnabled, prefs.osPushEnabled, prefs.salesEnabled]);

  return [prefs, update];
}

type LiveToast = AppNotification & { burst: number };

export type ToastBanner = {
  id: string;
  title: string;
  subtitle: string;
  tone: "green" | "brand";
  modelId?: string;
};

/**
 * Floating live toast stack rendered in AppShell (`enableSystemEffects=true`)
 * and in FanInsightsPanel (`enableSystemEffects=false` to avoid double sound/OS push).
 */
export function NotificationToastStack({
  enableSystemEffects = false,
  onOpenModel,
  onOpenChat,
  extraBanners = [],
  onDismissBanner,
}: {
  enableSystemEffects?: boolean;
  onOpenModel?: (modelId: string) => void;
  onOpenChat?: (modelId: string, ofFanId: string) => void;
  extraBanners?: ToastBanner[];
  onDismissBanner?: (id: string) => void;
}) {
  const [prefs] = useNotificationPrefs();
  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;
  const onOpenChatRef = useRef(onOpenChat);
  onOpenChatRef.current = onOpenChat;

  const [toasts, setToasts] = useState<LiveToast[]>([]);

  const dismissToast = useCallback((id: string) => {
    setToasts((current) => current.filter((item) => item.id !== id));
  }, []);

  useEffect(() => {
    let alive = true;
    let stopFn: (() => void) | undefined;
    const timers = new Map<string, number>();

    void listen<AppNotification>("cf-notification", (event) => {
      if (!alive || !event.payload) return;
      const item = event.payload;
      const currentPrefs = prefsRef.current;
      const isSale = item.kind === "ppv" || item.kind === "tip";

      if (isSale && !currentPrefs.salesEnabled) return;
      if (!isSale && !currentPrefs.messagesEnabled) return;

      setToasts((prev) => {
        const idx = prev.findIndex(
          (existing) =>
            existing.modelId === item.modelId &&
            existing.ofFanId === item.ofFanId &&
            existing.kind === item.kind,
        );
        if (idx >= 0) {
          const burst = prev[idx].burst + 1;
          const preview =
            item.kind === "message" && burst > 1
              ? `${burst} mensajes de ${item.fanName}`
              : item.preview;
          const next = prev.filter((_, index) => index !== idx);
          return [{ ...item, preview, burst }, ...next].slice(0, 4);
        }
        if (prev.some((existing) => existing.id === item.id)) return prev;
        return [{ ...item, burst: 1 }, ...prev].slice(0, 4);
      });

      const ttl = isSale ? SALE_TOAST_TTL_MS : TOAST_TTL_MS;
      const handle = window.setTimeout(() => {
        timers.delete(item.id);
        if (alive) {
          setToasts((prev) => prev.filter((t) => t.id !== item.id));
        }
      }, ttl);
      timers.set(item.id, handle);

      if (enableSystemEffects && currentPrefs.soundEnabled) {
        playAlertChime(isSale);
      }
    }).then((stop) => {
      if (alive) stopFn = stop;
      else stop();
    });

    return () => {
      alive = false;
      stopFn?.();
      timers.forEach((id) => window.clearTimeout(id));
      timers.clear();
    };
  }, [enableSystemEffects]);

  if (toasts.length === 0 && extraBanners.length === 0) {
    return null;
  }

  return (
    <div
      aria-live="polite"
      className="pointer-events-none fixed right-3.5 top-3.5 z-[80] flex w-[min(22rem,calc(100vw-1.5rem))] flex-col gap-2.5"
      style={{ color: "var(--text-primary)" }}
    >
      {extraBanners.map((banner) => (
        <div
          key={banner.id}
          className="pointer-events-auto flex items-start gap-3 rounded-xl p-3.5 shadow-lg transition-all"
          style={
            banner.tone === "green"
              ? {
                  background: "var(--bg-elevated)",
                  border: "1px solid var(--border)",
                  color: "var(--text-primary)",
                }
              : {
                  background: "var(--bg-elevated)",
                  border: "1px solid var(--border)",
                  color: "var(--text-primary)",
                }
          }
        >
          <CheckCircle2 size={18} className="mt-0.5 shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="truncate text-xs font-bold">{banner.title}</p>
            <p className="mt-0.5 text-xs opacity-90">{banner.subtitle}</p>
            {banner.modelId != null && onOpenModel ? (
              <button
                type="button"
                onClick={() => {
                  onOpenModel(banner.modelId!);
                  onDismissBanner?.(banner.id);
                }}
                className="cf-btn-primary mt-2 inline-flex items-center gap-1 px-2.5 py-1 text-[11px]"
              >
                <MessageSquare size={12} />
                Ir a Mensajes
              </button>
            ) : null}
          </div>
          {onDismissBanner ? (
            <button
              type="button"
              aria-label="Cerrar aviso"
              onClick={() => onDismissBanner(banner.id)}
              className="rounded p-0.5 opacity-75 hover:opacity-100"
              style={{ color: "inherit" }}
            >
              <X size={14} />
            </button>
          ) : null}
        </div>
      ))}

      {toasts.map((item) => (
        <NotificationCard
          key={item.id}
          item={item}
          onSelect={() => {
            onOpenChatRef.current?.(item.modelId, item.ofFanId);
            dismissToast(item.id);
          }}
          onDismiss={() => dismissToast(item.id)}
        />
      ))}
    </div>
  );
}

function NotificationCard({
  item,
  compact = false,
  onSelect,
  onDismiss,
}: {
  item: AppNotification;
  compact?: boolean;
  onSelect?: () => void;
  onDismiss?: () => void;
}) {
  const isSale = item.kind === "ppv" || item.kind === "tip";

  if (isSale) {
    // GREEN PURCHASE NOTIFICATION (PPV or Tip)
    return (
      <div
        role="status"
        onClick={onSelect}
        className={`pointer-events-auto relative overflow-hidden rounded-xl transition-all ${
          onSelect ? "cursor-pointer hover:brightness-105" : ""
        } ${compact ? "p-2.5" : "p-3.5 shadow-lg"}`}
        style={{
          background: "linear-gradient(135deg, #065f46 0%, #047857 55%, #059669 100%)",
          border: "1px solid #34d399",
          boxShadow: compact
            ? "none"
            : "0 12px 28px -6px rgba(5, 150, 105, 0.45), 0 4px 10px -2px rgba(0, 0, 0, 0.25)",
          color: "#ecfdf5",
        }}
      >
        <div className="flex items-start gap-3">
          <div
            className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full"
            style={{
              background: "rgba(16, 185, 129, 0.28)",
              border: "1px solid rgba(110, 231, 183, 0.55)",
            }}
          >
            <DollarSign size={18} strokeWidth={2.5} style={{ color: "#a7f3d0" }} />
          </div>

          <div className="min-w-0 flex-1">
            <div className="flex items-center justify-between gap-2">
              <span
                className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-bold uppercase tracking-wider"
                style={{
                  background: "rgba(236, 253, 245, 0.18)",
                  color: "#d1fae5",
                }}
              >
                <BadgeDollarSign size={11} strokeWidth={2.25} />
                {item.kind === "ppv" ? "Venta PPV" : "Propina (Tip)"}
              </span>

              {item.amount != null ? (
                <span
                  className="shrink-0 rounded-md px-2 py-0.5 text-xs font-extrabold tabular-nums"
                  style={{
                    background: "#ecfdf5",
                    color: "#065f46",
                  }}
                >
                  +{formatMoney(item.amount)}
                </span>
              ) : null}
            </div>

            <p className="mt-1.5 truncate text-xs font-semibold" style={{ color: "#ffffff" }}>
              {item.modelName}{" "}
              <span className="font-normal" style={{ color: "#a7f3d0" }}>← {item.fanName}</span>
            </p>

            <p className="mt-0.5 line-clamp-2 text-[11px] leading-snug" style={{ color: "#d1fae5" }}>
              {item.preview}
              {item.netAmount != null ? (
                <span className="ml-1.5 font-semibold" style={{ color: "#a7f3d0" }}>
                  ({formatMoney(item.netAmount)} neto)
                </span>
              ) : null}
            </p>
          </div>

          {onDismiss ? (
            <button
              type="button"
              aria-label="Cerrar notificación"
              onClick={(e) => {
                e.stopPropagation();
                onDismiss();
              }}
              className="shrink-0 rounded-md p-1 transition-colors hover:bg-white/15"
              style={{ color: "#a7f3d0" }}
            >
              <X size={14} strokeWidth={2.25} />
            </button>
          ) : null}
        </div>
      </div>
    );
  }

  // INCOMING CHAT MESSAGE NOTIFICATION
  return (
    <div
      role="status"
      onClick={onSelect}
      className={`pointer-events-auto relative overflow-hidden rounded-xl transition-all ${
        onSelect ? "cursor-pointer" : ""
      } ${compact ? "p-2.5" : "p-3.5 shadow-lg"}`}
      style={{
        background: "var(--bg-elevated)",
        border: "1px solid var(--border)",
        borderLeft: "4px solid var(--brand-500)",
        boxShadow: compact ? "none" : "var(--shadow-lg)",
        color: "var(--text-primary)",
      }}
    >
      <div className="flex items-start gap-3">
        <ModelAvatarThumb
          modelId={item.modelId}
          name={item.modelName}
          avatarUrl={item.modelAvatar}
        />

        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-2">
            <span
              className="inline-flex items-center gap-1 text-[11px] font-semibold"
              style={{ color: "var(--brand-600)" }}
            >
              <MessageSquare size={11} strokeWidth={2.25} />
              {item.modelName}
            </span>
            <span className="text-[10px]" style={{ color: "var(--text-tertiary)" }}>
              Nuevo mensaje
            </span>
          </div>

          <p className="mt-0.5 truncate text-xs font-semibold" style={{ color: "var(--text-primary)" }}>
            {item.fanName}
          </p>

          <p
            className="mt-0.5 line-clamp-2 text-xs leading-snug"
            style={{ color: "var(--text-secondary)" }}
          >
            {item.preview}
          </p>
        </div>

        {onDismiss ? (
          <button
            type="button"
            aria-label="Cerrar notificación"
            onClick={(e) => {
              e.stopPropagation();
              onDismiss();
            }}
            className="shrink-0 rounded-md p-1 transition-colors"
            style={{ color: "var(--text-tertiary)" }}
          >
            <X size={14} strokeWidth={2} />
          </button>
        ) : null}
      </div>
    </div>
  );
}

function ModelAvatarThumb({
  modelId,
  name,
  avatarUrl,
}: {
  modelId: string;
  name: string;
  avatarUrl: string | null;
}) {
  const [failed, setFailed] = useState(false);
  const url = avatarUrl?.trim() ?? "";
  if (url && !failed) {
    return (
      <img
        src={url}
        alt=""
        className="h-9 w-9 shrink-0 rounded-full object-cover"
        onError={() => setFailed(true)}
      />
    );
  }
  return (
    <span
      className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full text-xs font-semibold text-white"
      style={{ backgroundColor: avatarColor(modelId) }}
    >
      {initials(name)}
    </span>
  );
}

/**
 * Sidebar bell button + Notification Center popover for viewing recent alerts
 * and toggling Push / In-App / Sound / Green Purchase preferences.
 */
export function NotificationBellPopover({
  navOpen,
  onOpenChat,
}: {
  navOpen: boolean;
  onOpenChat: (modelId: string, ofFanId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [history, setHistory] = useState<AppNotification[]>([]);
  const [unseenCount, setUnseenCount] = useState(0);
  const [prefs, setPrefs] = useNotificationPrefs();
  const containerRef = useRef<HTMLDivElement | null>(null);

  const loadHistory = useCallback(async () => {
    try {
      const items = await getRecentNotifications();
      setHistory(items);
    } catch {
      // Ignore transient errors
    }
  }, []);

  useEffect(() => {
    void loadHistory();
  }, [loadHistory]);

  useEffect(() => {
    let alive = true;
    let stopFn: (() => void) | undefined;

    void listen<AppNotification>("cf-notification", (event) => {
      if (!alive || !event.payload) return;
      const item = event.payload;
      setHistory((prev) => {
        if (prev.some((existing) => existing.id === item.id)) return prev;
        return [item, ...prev].slice(0, 50);
      });
      setUnseenCount((c) => c + 1);
    }).then((stop) => {
      if (alive) stopFn = stop;
      else stop();
    });

    return () => {
      alive = false;
      stopFn?.();
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    setUnseenCount(0);
    const onDocClick = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, [open]);

  const anyActive = prefs.messagesEnabled || prefs.salesEnabled;

  return (
    <div ref={containerRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-label="Centro de notificaciones"
        title="Notificaciones y alertas"
        className={`relative flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs font-medium transition-colors ${
          navOpen ? "" : "justify-center"
        }`}
        style={{ color: "var(--nav-text)" }}
      >
        <span className="relative flex h-6 w-6 items-center justify-center">
          {anyActive ? <Bell size={16} strokeWidth={1.85} /> : <BellOff size={16} strokeWidth={1.85} />}
          {unseenCount > 0 ? (
            <span
              className="absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] font-bold text-white"
              style={{ background: "var(--brand-500)" }}
            >
              {unseenCount > 99 ? "99+" : unseenCount}
            </span>
          ) : null}
        </span>
        {navOpen ? <span>Alertas</span> : null}
      </button>

      {open ? (
        <div
          className={`fixed bottom-14 z-[80] flex max-h-[32rem] w-[23rem] flex-col overflow-hidden rounded-lg ${
            navOpen
              ? "left-[calc(3.75rem+15rem+0.75rem)]"
              : "left-[calc(3.75rem+4.5rem+0.75rem)]"
          }`}
          style={{
            background: "var(--bg-elevated)",
            border: "1px solid var(--border)",
            boxShadow: "var(--shadow-lg)",
            color: "var(--text-primary)",
          }}
        >
          <div
            className="flex items-center justify-between px-4 py-3"
            style={{ borderBottom: "1px solid var(--border)" }}
          >
            <div>
              <h2 className="text-sm font-semibold" style={{ color: "var(--text-primary)" }}>
                Notificaciones
              </h2>
              <p className="text-[11px]" style={{ color: "var(--text-tertiary)" }}>
                Mensajes entrantes y compras (PPV / Propinas)
              </p>
            </div>
            {history.length > 0 ? (
              <button
                type="button"
                onClick={() => {
                  setHistory([]);
                  void clearNotificationsHistory();
                }}
                className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium transition-colors hover:opacity-80"
                style={{ color: "var(--text-tertiary)" }}
              >
                <Trash2 size={12} />
                Limpiar
              </button>
            ) : null}
          </div>

          {/* Toggles */}
          <div
            className="grid grid-cols-2 gap-1.5 p-3 text-xs"
            style={{
              background: "var(--bg-subtle)",
              borderBottom: "1px solid var(--border)",
            }}
          >
            <PrefToggle
              active={prefs.messagesEnabled}
              label="Mensajes chat"
              accent="brand"
              onClick={() => setPrefs({ ...prefs, messagesEnabled: !prefs.messagesEnabled })}
            />
            <PrefToggle
              active={prefs.salesEnabled}
              label="Compras verdes"
              accent="green"
              onClick={() => setPrefs({ ...prefs, salesEnabled: !prefs.salesEnabled })}
            />
            <PrefToggle
              active={prefs.osPushEnabled}
              label="Push Windows"
              accent="brand"
              onClick={() => setPrefs({ ...prefs, osPushEnabled: !prefs.osPushEnabled })}
            />
            <PrefToggle
              active={prefs.soundEnabled}
              label={prefs.soundEnabled ? "Sonido activo" : "Silenciado"}
              accent="brand"
              icon={
                prefs.soundEnabled ? (
                  <Volume2 size={12} strokeWidth={2} />
                ) : (
                  <VolumeX size={12} strokeWidth={2} />
                )
              }
              onClick={() => setPrefs({ ...prefs, soundEnabled: !prefs.soundEnabled })}
            />
          </div>

          {/* History list */}
          <div className="min-h-0 flex-1 overflow-y-auto p-3">
            {history.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-10 text-center">
                <Bell size={24} strokeWidth={1.5} style={{ color: "var(--text-disabled)" }} />
                <p className="mt-2 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
                  Sin notificaciones recientes
                </p>
                <p className="mt-0.5 text-[11px]" style={{ color: "var(--text-tertiary)" }}>
                  Te avisaremos cuando entre un mensaje o una venta.
                </p>
              </div>
            ) : (
              <div className="flex flex-col gap-2">
                {history.map((item) => (
                  <NotificationCard
                    key={item.id}
                    item={item}
                    compact
                    onSelect={() => {
                      setOpen(false);
                      onOpenChat(item.modelId, item.ofFanId);
                    }}
                    onDismiss={() => {
                      setHistory((prev) => prev.filter((x) => x.id !== item.id));
                      void dismissNotificationItem(item.id);
                    }}
                  />
                ))}
              </div>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}

function PrefToggle({
  active,
  label,
  accent,
  icon,
  onClick,
}: {
  active: boolean;
  label: string;
  accent: "brand" | "green";
  icon?: React.ReactNode;
  onClick: () => void;
}) {
  const activeStyle =
    accent === "green"
      ? { background: "var(--success)", color: "#ffffff", borderColor: "var(--success)" }
      : { background: "var(--brand-500)", color: "#ffffff", borderColor: "var(--brand-500)" };

  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className="flex items-center justify-between gap-1.5 rounded-lg border px-2.5 py-1.5 text-[11px] font-semibold transition-all"
      style={
        active
          ? activeStyle
          : {
              background: "var(--bg-elevated)",
              color: "var(--text-tertiary)",
              borderColor: "var(--border)",
            }
      }
    >
      <span className="flex items-center gap-1 truncate">
        {icon}
        <span className="truncate">{label}</span>
      </span>
      <span
        className="h-2 w-2 shrink-0 rounded-full"
        style={{ background: active ? "#ffffff" : "var(--text-disabled)" }}
      />
    </button>
  );
}
