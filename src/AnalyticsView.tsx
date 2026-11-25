import { listen } from "@tauri-apps/api/event";
import {
  ArrowDownRight,
  ArrowUpRight,
  FileSpreadsheet,
  FileText,
  Filter,
  RefreshCw,
  Search,
  TrendingUp,
  Users,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Legend,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  errorMessage,
  exportAnalytics,
  getAnalytics,
  syncAllModels,
  type AnalyticsRange,
  type AnalyticsReport,
  type TrendBucket,
} from "./api";
import { avatarColor, formatMoney, formatStamp, initials, modelLabel, type Model } from "./types";

const QUICK_RANGES: { id: AnalyticsRange; label: string; defaultBucket?: TrendBucket }[] = [
  { id: "24h", label: "24 horas", defaultBucket: "hour" },
  { id: "7", label: "7 días", defaultBucket: "day" },
  { id: "30", label: "30 días", defaultBucket: "day" },
  { id: "90", label: "90 días", defaultBucket: "day" },
  { id: "180", label: "180 días", defaultBucket: "day" },
  { id: "all", label: "Histórico", defaultBucket: "month" },
];

const CURRENT_YEAR = new Date().getFullYear();
const YEAR_OPTIONS = Array.from(
  { length: CURRENT_YEAR + 2 - 2018 + 1 },
  (_, i) => CURRENT_YEAR + 2 - i,
);

const SPANISH_MONTHS = [
  "Enero",
  "Febrero",
  "Marzo",
  "Abril",
  "Mayo",
  "Junio",
  "Julio",
  "Agosto",
  "Septiembre",
  "Octubre",
  "Noviembre",
  "Diciembre",
];

const MONTH_OPTIONS: { value: string; label: string }[] = (() => {
  const options: { value: string; label: string }[] = [];
  for (let y = CURRENT_YEAR + 1; y >= 2020; y--) {
    for (let m = 12; m >= 1; m--) {
      const mStr = m < 10 ? `0${m}` : `${m}`;
      options.push({
        value: `${y}-${mStr}`,
        label: `${SPANISH_MONTHS[m - 1]} ${y}`,
      });
    }
  }
  return options;
})();

const BUCKETS: { id: TrendBucket; label: string }[] = [
  { id: "hour", label: "Hora" },
  { id: "day", label: "Día" },
  { id: "week", label: "Semana" },
  { id: "month", label: "Mes" },
];

const COLORS = ["#4f72f0", "#ea580c", "#0f766e", "#8b5cf6", "#ec4899", "#3b82f6"];

export function AnalyticsView({ models }: { models: Model[] }) {
  const [tab, setTab] = useState<"overview" | "creators">("overview");
  const [range, setRange] = useState<AnalyticsRange>("7");
  const [bucket, setBucket] = useState<TrendBucket>("day");
  const [net, setNet] = useState(true);
  const [creatorFilter, setCreatorFilter] = useState<number[]>([]);
  const [selected, setSelected] = useState<number[]>([]);
  const [query, setQuery] = useState("");
  const [report, setReport] = useState<AnalyticsReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let alive = true;
    void getAnalytics(range, creatorFilter, selected, net, bucket)
      .then((next) => {
        if (!alive) return;
        setReport(next);
        setError(null);
      })
      .catch((caught) => {
        if (alive) setError(errorMessage(caught));
      });
    return () => {
      alive = false;
    };
  }, [range, creatorFilter, selected, net, bucket, reload]);

  useEffect(() => {
    let alive = true;
    let stop = () => {};
    void listen("models-changed", () => setReload((current) => current + 1)).then((unlisten) => {
      if (alive) stop = unlisten;
      else unlisten();
    });
    return () => {
      alive = false;
      stop();
    };
  }, []);

  const filteredModels = models.filter((model) =>
    modelLabel(model).toLowerCase().includes(query.trim().toLowerCase()),
  );

  const compareChart = useMemo(() => {
    if (!report) return [];
    return report.comparisonLabels.map((label, index) => {
      const point: Record<string, string | number> = { label };
      for (const series of report.comparisonSeries) {
        point[`c${series.id}`] = series.values[index] ?? 0;
      }
      return point;
    });
  }, [report]);

  async function syncAll() {
    setBusy("sync");
    setNotice(null);
    setError(null);
    try {
      const result = await syncAllModels();
      setReload((current) => current + 1);
      setNotice(
        result.errors.length === 0
          ? `Sincronizadas ${result.synced}. Sin sesión: ${result.skipped}.`
          : result.errors.join(" "),
      );
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function exportFile(format: "xlsx" | "pdf" | "csv") {
    setBusy(format);
    setError(null);
    try {
      const result = await exportAnalytics(range, creatorFilter, format);
      if (result.saved && result.path) {
        setNotice(`Exportado con éxito en: ${result.path}`);
      }
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="flex flex-col gap-6">
      {/* Page Header */}
      <header className="flex flex-wrap items-center justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
            Analytics & Reports
          </h1>
          <p className="mt-1 text-sm" style={{ color: "var(--text-secondary)" }}>
            Métricas de rendimiento, ingresos desglosados y comparativas entre creadoras.
          </p>
        </div>

        {/* Global actions: Sync & Export */}
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            disabled={busy !== null}
            className="cf-btn-primary"
            onClick={() => void syncAll()}
          >
            <RefreshCw
              size={14}
              strokeWidth={2}
              className={busy === "sync" ? "animate-spin" : ""}
            />
            {busy === "sync" ? "Sincronizando…" : "Sincronizar datos"}
          </button>

          <button
            type="button"
            disabled={busy !== null}
            className="cf-btn-secondary"
            onClick={() => void exportFile("xlsx")}
          >
            <FileSpreadsheet size={14} strokeWidth={2} style={{ color: "var(--text-secondary)" }} />
            {busy === "xlsx" ? "Exportando…" : "Excel"}
          </button>

          <button
            type="button"
            disabled={busy !== null}
            className="cf-btn-secondary"
            onClick={() => void exportFile("pdf")}
          >
            <FileText size={14} strokeWidth={2} style={{ color: "var(--text-secondary)" }} />
            {busy === "pdf" ? "Exportando…" : "PDF"}
          </button>
        </div>
      </header>

      {/* Segmented Control Navigation Tabs */}
      <div
        className="inline-flex w-fit rounded-xl p-1 gap-1"
        style={{ background: "var(--bg-subtle)", border: "1px solid var(--border)" }}
      >
        <button
          type="button"
          aria-pressed={tab === "overview"}
          className="rounded-lg px-4 py-2 text-sm font-semibold transition-all"
          style={
            tab === "overview"
              ? {
                  background: "var(--bg-elevated)",
                  color: "var(--text-primary)",
                  boxShadow: "var(--shadow-sm)",
                }
              : { color: "var(--text-tertiary)" }
          }
          onClick={() => setTab("overview")}
        >
          Overview general
        </button>
        <button
          type="button"
          aria-pressed={tab === "creators"}
          className="rounded-lg px-4 py-2 text-sm font-semibold transition-all"
          style={
            tab === "creators"
              ? {
                  background: "var(--bg-elevated)",
                  color: "var(--text-primary)",
                  boxShadow: "var(--shadow-sm)",
                }
              : { color: "var(--text-tertiary)" }
          }
          onClick={() => setTab("creators")}
        >
          Comparativa de creadoras
        </button>
      </div>

      {/* Filter Toolbar Card */}
      <div
        className="cf-card flex flex-wrap items-center gap-3 p-3.5"
      >
        {/* Quick Range Pills */}
        <div
          className="inline-flex rounded-lg p-0.5 gap-0.5"
          style={{ background: "var(--bg-subtle)" }}
        >
          {QUICK_RANGES.map((item) => (
            <button
              key={item.id}
              type="button"
              aria-pressed={range === item.id}
              className="rounded-md px-2.5 py-1.5 text-xs font-medium transition-all"
              style={
                range === item.id
                  ? {
                      background: "var(--bg-elevated)",
                      color: "var(--brand-600)",
                      boxShadow: "var(--shadow-sm)",
                    }
                  : { color: "var(--text-tertiary)" }
              }
              onClick={() => {
                setRange(item.id);
                if (item.defaultBucket) setBucket(item.defaultBucket);
              }}
            >
              {item.label}
            </button>
          ))}
        </div>

        <div className="h-4 w-px" style={{ background: "var(--border)" }} />

        {/* Year and Month dropdowns */}
        <div className="flex items-center gap-2">
          <select
            aria-label="Seleccionar año"
            value={range.startsWith("year:") ? range.replace("year:", "") : ""}
            onChange={(e) => {
              if (e.target.value) {
                setRange(`year:${e.target.value}`);
                setBucket("month");
              }
            }}
            className="cf-input"
            style={{
              padding: "0.35rem 0.65rem",
              fontSize: "0.8125rem",
              width: "auto",
              cursor: "pointer",
            }}
          >
            <option value="">Año…</option>
            {YEAR_OPTIONS.map((y) => (
              <option key={y} value={y}>
                {y}
              </option>
            ))}
          </select>

          <select
            aria-label="Seleccionar mes"
            value={range.startsWith("month:") ? range.replace("month:", "") : ""}
            onChange={(e) => {
              if (e.target.value) {
                setRange(`month:${e.target.value}`);
                setBucket("day");
              }
            }}
            className="cf-input"
            style={{
              padding: "0.35rem 0.65rem",
              fontSize: "0.8125rem",
              width: "auto",
              cursor: "pointer",
            }}
          >
            <option value="">Mes…</option>
            {MONTH_OPTIONS.map((m) => (
              <option key={m.value} value={m.value}>
                {m.label}
              </option>
            ))}
          </select>
        </div>

        <div className="h-4 w-px" style={{ background: "var(--border)" }} />

        {/* Creators filter pills */}
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--text-tertiary)" }}>
            <Filter size={12} strokeWidth={2} /> Creadoras:
          </span>
          <button
            type="button"
            aria-pressed={creatorFilter.length === 0}
            className="cf-badge transition-all"
            style={
              creatorFilter.length === 0
                ? {
                    background: "var(--brand-50)",
                    color: "var(--brand-700)",
                    border: "1px solid var(--brand-100)",
                  }
                : {
                    background: "transparent",
                    color: "var(--text-tertiary)",
                    border: "1px solid var(--border)",
                  }
            }
            onClick={() => setCreatorFilter([])}
          >
            Todas
          </button>
          {models.map((model) => {
            const active = creatorFilter.includes(model.id);
            return (
              <button
                key={model.id}
                type="button"
                aria-pressed={active}
                className="cf-badge transition-all"
                style={
                  active
                    ? {
                        background: "var(--brand-50)",
                        color: "var(--brand-700)",
                        border: "1px solid var(--brand-200)",
                      }
                    : {
                        background: "transparent",
                        color: "var(--text-tertiary)",
                        border: "1px solid var(--border)",
                      }
                }
                onClick={() =>
                  setCreatorFilter((current) =>
                    current.includes(model.id)
                      ? current.filter((id) => id !== model.id)
                      : [...current, model.id],
                  )
                }
              >
                {modelLabel(model)}
              </button>
            );
          })}
        </div>

        {/* Net / Gross toggle */}
        <label
          className="ml-auto flex items-center gap-2 cursor-pointer text-xs font-semibold select-none"
          style={{ color: "var(--text-secondary)" }}
        >
          <input
            type="checkbox"
            checked={net}
            className="rounded"
            style={{ accentColor: "var(--brand-600)" }}
            onChange={(event) => setNet(event.target.checked)}
          />
          Ingresos netos
        </label>
      </div>

      {/* Notifications */}
      {error ? (
        <div
          role="alert"
          className="rounded-xl px-4 py-3 text-sm"
          style={{ background: "#fef2f2", border: "1px solid #fecaca", color: "#b91c1c" }}
        >
          {error}
        </div>
      ) : null}
      {notice ? (
        <div
          className="rounded-xl px-4 py-3 text-sm"
          style={{ background: "var(--bg-subtle)", border: "1px solid var(--border)", color: "var(--text-secondary)" }}
        >
          {notice}
        </div>
      ) : null}

      {/* TAB CONTENT */}
      {tab === "overview" ? (
        <>
          {/* Top Metric Cards */}
          <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
            {(report?.cards ?? []).map((card) => {
              const isPositive = (card.changePercent ?? 0) >= 0;
              return (
                <article
                  key={card.key}
                  className="cf-card p-5 flex flex-col justify-between"
                >
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-semibold uppercase tracking-wider" style={{ color: "var(--text-tertiary)" }}>
                      {card.label}
                    </span>
                    {card.changePercent != null && (
                      <span
                        className="inline-flex items-center gap-0.5 text-xs font-semibold px-2 py-0.5 rounded-full"
                        style={
                          isPositive
                            ? { background: "#ecfdf5", color: "#059669" }
                            : { background: "#fef2f2", color: "#dc2626" }
                        }
                      >
                        {isPositive ? (
                          <ArrowUpRight size={13} strokeWidth={2.5} />
                        ) : (
                          <ArrowDownRight size={13} strokeWidth={2.5} />
                        )}
                        {Math.abs(card.changePercent).toFixed(1)}%
                      </span>
                    )}
                  </div>
                  <div className="mt-3">
                    <p className="text-2xl font-bold tabular-nums tracking-tight" style={{ color: "var(--text-primary)" }}>
                      {formatMoney(card.amount)}
                    </p>
                    <p className="mt-1 text-xs" style={{ color: "var(--text-disabled)" }}>
                      {card.changePercent == null ? "Primer registro" : "vs período anterior"}
                    </p>
                  </div>
                </article>
              );
            })}
          </div>

          {/* Creators mini profiles */}
          <div className="grid gap-4 md:grid-cols-2">
            {(report?.creators ?? []).map((creator) => (
              <article key={creator.id} className="cf-card p-5">
                <div className="flex items-center gap-3">
                  <span
                    className="flex h-11 w-11 items-center justify-center rounded-full text-xs font-bold text-white shrink-0"
                    style={{ backgroundColor: avatarColor(creator.id) }}
                  >
                    {initials(creator.name || "Unknown")}
                  </span>
                  <div className="min-w-0 flex-1">
                    <span className="block font-semibold text-sm truncate" style={{ color: "var(--text-primary)" }}>
                      {creator.name || "Unknown"}
                    </span>
                    <span className="block text-xs mt-0.5 truncate" style={{ color: "var(--text-tertiary)" }}>
                      {creator.username ? `@${creator.username}` : "Sin username"} · Sincronizado {formatStamp(creator.lastSyncedAt)}
                    </span>
                  </div>
                </div>

                <div className="mt-4 pt-4 flex items-baseline justify-between" style={{ borderTop: "1px solid var(--border-muted)" }}>
                  <div>
                    <span className="text-[11px] font-semibold uppercase tracking-wider" style={{ color: "var(--text-tertiary)" }}>
                      Saldo actual
                    </span>
                    <p className="text-xl font-bold tabular-nums" style={{ color: "var(--text-primary)" }}>
                      {formatMoney(creator.currentBalance)}
                    </p>
                  </div>
                  <div className="text-right">
                    <span className="text-xs" style={{ color: "var(--text-tertiary)" }}>
                      Pendiente: <strong style={{ color: "var(--text-secondary)" }}>{formatMoney(creator.pendingBalance)}</strong>
                    </span>
                  </div>
                </div>

                <div className="mt-3 grid grid-cols-3 gap-2">
                  {creator.channels.map((channel) => (
                    <div
                      key={channel.key}
                      className="rounded-lg p-2.5 flex flex-col justify-between"
                      style={{ background: "var(--bg-subtle)" }}
                    >
                      <p className="text-[11px] font-medium truncate" style={{ color: "var(--text-tertiary)" }}>
                        {channel.label}
                      </p>
                      <p className="text-sm font-semibold tabular-nums mt-1" style={{ color: "var(--text-primary)" }}>
                        {formatMoney(channel.amount)}
                      </p>
                    </div>
                  ))}
                </div>
              </article>
            ))}
          </div>

          {(report?.creators.length ?? 0) === 0 ? (
            <p className="text-sm text-center py-6" style={{ color: "var(--text-tertiary)" }}>
              No hay creadoras disponibles en este filtro.
            </p>
          ) : null}

          {/* Earnings Trends Bar Chart */}
          <article className="cf-card p-6">
            <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
              <div className="flex items-center gap-2">
                <TrendingUp size={16} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
                <h2 className="text-base font-semibold" style={{ color: "var(--text-primary)" }}>
                  Tendencia de ingresos
                </h2>
              </div>
              <div
                className="inline-flex rounded-lg p-0.5 gap-0.5"
                style={{ background: "var(--bg-subtle)" }}
              >
                {BUCKETS.map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    aria-pressed={bucket === item.id}
                    className="rounded-md px-2.5 py-1 text-xs font-medium transition-all"
                    style={
                      bucket === item.id
                        ? {
                            background: "var(--bg-elevated)",
                            color: "var(--brand-600)",
                            boxShadow: "var(--shadow-sm)",
                          }
                        : { color: "var(--text-tertiary)" }
                    }
                    onClick={() => setBucket(item.id)}
                  >
                    {item.label}
                  </button>
                ))}
              </div>
            </div>

            {(report?.trends.length ?? 0) === 0 ? (
              <div className="flex h-64 items-center justify-center text-sm" style={{ color: "var(--text-tertiary)" }}>
                Sin datos de tendencia para este período
              </div>
            ) : (
              <ResponsiveContainer width="100%" height={280}>
                <BarChart data={report?.trends ?? []}>
                  <CartesianGrid vertical={false} stroke="var(--border-muted)" strokeDasharray="3 3" />
                  <XAxis
                    dataKey="label"
                    axisLine={false}
                    tickLine={false}
                    tick={{ fill: "var(--text-tertiary)", fontSize: 12 }}
                  />
                  <YAxis
                    axisLine={false}
                    tickLine={false}
                    tick={{ fill: "var(--text-tertiary)", fontSize: 12 }}
                    tickFormatter={(val) => `$${val}`}
                  />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: "var(--bg-elevated)",
                      borderColor: "var(--border)",
                      borderRadius: "0.75rem",
                      boxShadow: "var(--shadow-md)",
                      color: "var(--text-primary)",
                      fontFamily: "Satoshi, sans-serif",
                    }}
                    formatter={(val) => [formatMoney(Number(val)), "Ingresos"]}
                  />
                  <Bar
                    dataKey="amount"
                    fill="var(--brand-500)"
                    radius={[6, 6, 0, 0]}
                  />
                </BarChart>
              </ResponsiveContainer>
            )}
          </article>

          {/* Channels charts: Area Chart & Donut */}
          <div className="grid gap-5 lg:grid-cols-[1.4fr_0.8fr]">
            <article className="cf-card p-6">
              <h2 className="mb-4 text-base font-semibold" style={{ color: "var(--text-primary)" }}>
                Ingresos por canal a lo largo del tiempo
              </h2>
              {(report?.channelsOverTime.length ?? 0) === 0 ? (
                <div className="flex h-60 items-center justify-center text-sm" style={{ color: "var(--text-tertiary)" }}>
                  Sin datos de canales
                </div>
              ) : (
                <ResponsiveContainer width="100%" height={270}>
                  <AreaChart data={report?.channelsOverTime ?? []}>
                    <defs>
                      <linearGradient id="gradSubs" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="5%" stopColor={COLORS[0]} stopOpacity={0.3} />
                        <stop offset="95%" stopColor={COLORS[0]} stopOpacity={0.0} />
                      </linearGradient>
                      <linearGradient id="gradTips" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="5%" stopColor={COLORS[1]} stopOpacity={0.3} />
                        <stop offset="95%" stopColor={COLORS[1]} stopOpacity={0.0} />
                      </linearGradient>
                      <linearGradient id="gradPosts" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="5%" stopColor={COLORS[2]} stopOpacity={0.3} />
                        <stop offset="95%" stopColor={COLORS[2]} stopOpacity={0.0} />
                      </linearGradient>
                      <linearGradient id="gradPpv" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="5%" stopColor={COLORS[3]} stopOpacity={0.3} />
                        <stop offset="95%" stopColor={COLORS[3]} stopOpacity={0.0} />
                      </linearGradient>
                    </defs>
                    <CartesianGrid vertical={false} stroke="var(--border-muted)" strokeDasharray="3 3" />
                    <XAxis
                      dataKey="label"
                      axisLine={false}
                      tickLine={false}
                      tick={{ fill: "var(--text-tertiary)", fontSize: 12 }}
                    />
                    <YAxis
                      axisLine={false}
                      tickLine={false}
                      tick={{ fill: "var(--text-tertiary)", fontSize: 12 }}
                      tickFormatter={(val) => `$${val}`}
                    />
                    <Tooltip
                      contentStyle={{
                        backgroundColor: "var(--bg-elevated)",
                        borderColor: "var(--border)",
                        borderRadius: "0.75rem",
                        boxShadow: "var(--shadow-md)",
                        color: "var(--text-primary)",
                      }}
                      formatter={(val) => formatMoney(Number(val))}
                    />
                    <Legend
                      wrapperStyle={{
                        paddingTop: 12,
                        fontSize: 12,
                        fontFamily: "Satoshi, sans-serif",
                      }}
                    />
                    <Area type="monotone" dataKey="subs" stroke={COLORS[0]} strokeWidth={2} fillOpacity={1} fill="url(#gradSubs)" name="Suscripciones" />
                    <Area type="monotone" dataKey="tips" stroke={COLORS[1]} strokeWidth={2} fillOpacity={1} fill="url(#gradTips)" name="Tips" />
                    <Area type="monotone" dataKey="posts" stroke={COLORS[2]} strokeWidth={2} fillOpacity={1} fill="url(#gradPosts)" name="Posts" />
                    <Area type="monotone" dataKey="ppv" stroke={COLORS[3]} strokeWidth={2} fillOpacity={1} fill="url(#gradPpv)" name="PPV" />
                    <Area type="monotone" dataKey="referrals" stroke={COLORS[4]} strokeWidth={2} fill="transparent" name="Referidos" />
                    <Area type="monotone" dataKey="streams" stroke={COLORS[5]} strokeWidth={2} fill="transparent" name="Streams" />
                  </AreaChart>
                </ResponsiveContainer>
              )}
            </article>

            {/* Breakdown Donut */}
            <article className="cf-card p-6 flex flex-col justify-between">
              <div>
                <h2 className="mb-4 text-base font-semibold" style={{ color: "var(--text-primary)" }}>
                  Desglose de ingresos
                </h2>
                {(report?.breakdown.every((slice) => slice.amount === 0) ?? true) ? (
                  <div className="flex h-52 items-center justify-center text-sm" style={{ color: "var(--text-tertiary)" }}>
                    Sin datos en este período
                  </div>
                ) : (
                  <ResponsiveContainer width="100%" height={210}>
                    <PieChart>
                      <Pie
                        data={(report?.breakdown ?? []).filter((slice) => slice.amount > 0)}
                        dataKey="amount"
                        nameKey="label"
                        innerRadius={54}
                        outerRadius={80}
                        paddingAngle={3}
                      >
                        {(report?.breakdown ?? [])
                          .filter((slice) => slice.amount > 0)
                          .map((slice) => (
                            <Cell
                              key={slice.key}
                              fill={COLORS[channelColor(slice.key)]}
                              stroke="var(--bg-elevated)"
                              strokeWidth={2}
                            />
                          ))}
                      </Pie>
                      <Tooltip
                        contentStyle={{
                          backgroundColor: "var(--bg-elevated)",
                          borderColor: "var(--border)",
                          borderRadius: "0.75rem",
                          color: "var(--text-primary)",
                        }}
                        formatter={(val) => formatMoney(Number(val))}
                      />
                    </PieChart>
                  </ResponsiveContainer>
                )}
              </div>
              <BreakdownLegend slices={report?.breakdown ?? []} showEmpty />
            </article>
          </div>

          {/* Comparison section */}
          <CompareCreators
            models={filteredModels}
            query={query}
            selected={selected}
            report={report}
            compareChart={compareChart}
            onQuery={setQuery}
            onSelected={setSelected}
          />
        </>
      ) : (
        /* CREATORS COMPARISON TAB ONLY */
        <CompareCreators
          models={filteredModels}
          query={query}
          selected={selected}
          report={report}
          compareChart={compareChart}
          onQuery={setQuery}
          onSelected={setSelected}
        />
      )}
    </section>
  );
}

/* ── CompareCreators Component ────────────────────────────────────── */
function CompareCreators({
  models,
  query,
  selected,
  report,
  compareChart,
  onQuery,
  onSelected,
}: {
  models: Model[];
  query: string;
  selected: number[];
  report: AnalyticsReport | null;
  compareChart: Record<string, string | number>[];
  onQuery: (value: string) => void;
  onSelected: (update: (current: number[]) => number[]) => void;
}) {
  return (
    <div className="grid gap-5 lg:grid-cols-[17rem_1fr]">
      {/* Sidebar selection */}
      <aside className="cf-card p-5 flex flex-col gap-3">
        <div className="flex items-center gap-2">
          <Users size={16} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
          <h2 className="text-sm font-semibold" style={{ color: "var(--text-primary)" }}>
            Comparar creadoras
          </h2>
        </div>

        <div className="relative">
          <Search
            size={14}
            strokeWidth={2}
            className="absolute left-3 top-1/2 -translate-y-1/2"
            style={{ color: "var(--text-tertiary)" }}
          />
          <input
            value={query}
            placeholder="Buscar modelo…"
            className="cf-input"
            style={{ paddingLeft: "2rem", fontSize: "0.8125rem" }}
            onChange={(event) => onQuery(event.target.value)}
          />
        </div>

        <ul className="flex flex-col gap-1.5 overflow-y-auto max-h-72 pt-1">
          {models.length === 0 ? (
            <li className="text-xs py-2" style={{ color: "var(--text-tertiary)" }}>
              No se encontraron creadoras
            </li>
          ) : null}
          {models.map((model) => {
            const isChecked = selected.includes(model.id);
            return (
              <li key={model.id}>
                <label
                  className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 cursor-pointer text-sm font-medium transition-colors select-none"
                  style={{
                    background: isChecked ? "var(--bg-subtle)" : "transparent",
                    color: isChecked ? "var(--text-primary)" : "var(--text-secondary)",
                  }}
                >
                  <input
                    type="checkbox"
                    checked={isChecked}
                    className="rounded"
                    style={{ accentColor: "var(--brand-600)" }}
                    onChange={(event) =>
                      onSelected((current) =>
                        event.target.checked
                          ? [...current, model.id]
                          : current.filter((id) => id !== model.id),
                      )
                    }
                  />
                  <span
                    className="flex h-5 w-5 items-center justify-center rounded-full text-[9px] font-bold text-white shrink-0"
                    style={{ backgroundColor: avatarColor(model.id) }}
                  >
                    {initials(modelLabel(model))}
                  </span>
                  <span className="truncate">{modelLabel(model)}</span>
                </label>
              </li>
            );
          })}
        </ul>
      </aside>

      {/* Comparison visual cards */}
      <div className="flex flex-col gap-5">
        <article className="cf-card p-6">
          <h3 className="text-base font-semibold mb-4" style={{ color: "var(--text-primary)" }}>
            Comparativa directa de ingresos
          </h3>
          {compareChart.length === 0 ? (
            <div className="flex h-64 items-center justify-center text-sm" style={{ color: "var(--text-tertiary)" }}>
              Selecciona dos o más creadoras para visualizar la comparativa
            </div>
          ) : (
            <ResponsiveContainer width="100%" height={280}>
              <BarChart data={compareChart}>
                <CartesianGrid vertical={false} stroke="var(--border-muted)" strokeDasharray="3 3" />
                <XAxis
                  dataKey="label"
                  axisLine={false}
                  tickLine={false}
                  tick={{ fill: "var(--text-tertiary)", fontSize: 12 }}
                />
                <YAxis
                  axisLine={false}
                  tickLine={false}
                  tick={{ fill: "var(--text-tertiary)", fontSize: 12 }}
                  tickFormatter={(val) => `$${val}`}
                />
                <Tooltip
                  contentStyle={{
                    backgroundColor: "var(--bg-elevated)",
                    borderColor: "var(--border)",
                    borderRadius: "0.75rem",
                    color: "var(--text-primary)",
                  }}
                  formatter={(val) => formatMoney(Number(val))}
                />
                <Legend wrapperStyle={{ paddingTop: 10, fontSize: 12 }} />
                {(report?.comparisonSeries ?? []).map((series, index) => (
                  <Bar
                    key={series.id}
                    dataKey={`c${series.id}`}
                    name={series.name}
                    fill={COLORS[index % COLORS.length]}
                    radius={[4, 4, 0, 0]}
                  />
                ))}
              </BarChart>
            </ResponsiveContainer>
          )}
        </article>

        {/* Comparison Donuts */}
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {(report?.comparisonDonuts ?? []).map((donut) => (
            <article key={donut.id} className="cf-card p-4 flex flex-col justify-between">
              <div>
                <h4 className="text-sm font-semibold truncate" style={{ color: "var(--text-primary)" }}>
                  {donut.name}
                </h4>
                <p className="text-xs mt-0.5" style={{ color: "var(--text-tertiary)" }}>
                  Desglose por canal
                </p>
                {donut.slices.every((slice) => slice.amount === 0) ? (
                  <p className="mt-6 text-center text-xs" style={{ color: "var(--text-disabled)" }}>
                    Sin datos registrados
                  </p>
                ) : (
                  <ResponsiveContainer width="100%" height={150}>
                    <PieChart>
                      <Pie
                        data={donut.slices.filter((slice) => slice.amount > 0)}
                        dataKey="amount"
                        nameKey="label"
                        innerRadius={34}
                        outerRadius={54}
                        paddingAngle={2}
                      >
                        {donut.slices
                          .filter((slice) => slice.amount > 0)
                          .map((slice) => (
                            <Cell
                              key={slice.key}
                              fill={COLORS[channelColor(slice.key)]}
                              stroke="var(--bg-elevated)"
                              strokeWidth={2}
                            />
                          ))}
                      </Pie>
                      <Tooltip
                        contentStyle={{
                          backgroundColor: "var(--bg-elevated)",
                          borderColor: "var(--border)",
                          borderRadius: "0.5rem",
                          fontSize: 12,
                          color: "var(--text-primary)",
                        }}
                        formatter={(val) => formatMoney(Number(val))}
                      />
                    </PieChart>
                  </ResponsiveContainer>
                )}
              </div>
              <BreakdownLegend slices={donut.slices} showEmpty />
            </article>
          ))}
        </div>
      </div>
    </div>
  );
}

function channelColor(key: string): number {
  const order = ["subscriptions", "tips", "posts", "messages", "referrals", "streams"];
  const index = order.indexOf(key);
  return index >= 0 ? index : 0;
}

function BreakdownLegend({
  slices,
  showEmpty = false,
}: {
  slices: { key: string; label: string; amount: number; percent: number }[];
  showEmpty?: boolean;
}) {
  const visible = showEmpty ? slices : slices.filter((slice) => slice.amount > 0);
  if (visible.length === 0) return null;
  return (
    <ul className="mt-3 flex flex-col gap-1.5 text-xs">
      {visible.map((slice) => (
        <li key={slice.key} className="flex items-center justify-between gap-2">
          <span className="flex items-center gap-2 truncate" style={{ color: "var(--text-secondary)" }}>
            <span
              className="h-2 w-2 rounded-full shrink-0"
              style={{ backgroundColor: COLORS[channelColor(slice.key)] }}
            />
            {slice.label}
          </span>
          <span className="tabular-nums font-semibold shrink-0" style={{ color: "var(--text-primary)" }}>
            {formatMoney(slice.amount)} · {slice.percent.toFixed(1)}%
          </span>
        </li>
      ))}
    </ul>
  );
}
