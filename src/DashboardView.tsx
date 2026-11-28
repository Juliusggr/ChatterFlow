import { listen } from "@tauri-apps/api/event";
import {
  CalendarDays,
  CircleDollarSign,
  HandCoins,
  Image as ImageIcon,
  MessageCircle,
  Trash2,
  Users,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  errorMessage,
  type EarningsBasis,
  type TimeRange,
} from "./api";
import {
  clockIn,
  clockOut,
  deleteShift,
  loadDashboardSnapshot,
  subscribeActiveShifts,
  type DashboardSnapshot,
} from "./agency";
import { syncDropperShiftRecords } from "./reports/data";
import { formatCheckout, formatMoney, formatShiftDate, formatShiftTime } from "./types";

const RANGES: { id: TimeRange; label: string }[] = [
  { id: "yesterday", label: "Ayer" },
  { id: "today", label: "Hoy" },
  { id: "thisWeek", label: "Esta semana" },
  { id: "thisMonth", label: "Este mes" },
];

const CHANNEL_CARDS: { field: "subs" | "ppv" | "tips" | "posts"; label: string; icon: LucideIcon }[] = [
  { field: "subs", label: "Suscripciones", icon: Users },
  { field: "ppv", label: "Mensajes", icon: MessageCircle },
  { field: "tips", label: "Tips", icon: HandCoins },
  { field: "posts", label: "Posts", icon: ImageIcon },
];

const CARD_RADIUS = 20;

export function DashboardView({ canDeleteShifts = false }: { canDeleteShifts?: boolean }) {
  const [range, setRange] = useState<TimeRange>("today");
  const [basis, setBasis] = useState<EarningsBasis>("gross");
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null);
  const [clockBusy, setClockBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [subsMode, setSubsMode] = useState<"income" | "fans">("income");

  const revenue = snapshot?.revenue ?? null;
  const shifts = snapshot?.shifts ?? [];
  const clockedIn = snapshot?.clockedIn ?? [];
  const onShift = snapshot?.onShift ?? false;
  const sales = snapshot?.salesByBasis[basis] ?? [];
  const channels = snapshot?.channelsByBasis[basis] ?? [];
  const fans = snapshot?.fans ?? null;

  useEffect(() => {
    let alive = true;
    let stop = () => {};
    let timer: number | undefined;
    const scheduleRefresh = () => {
      if (timer !== undefined) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (alive) setRefresh((current) => current + 1);
      }, 200);
    };
    void listen("models-changed", scheduleRefresh).then((unlisten) => {
      if (alive) stop = unlisten;
      else unlisten();
    });
    const stopShifts = subscribeActiveShifts(scheduleRefresh);
    return () => {
      alive = false;
      if (timer !== undefined) window.clearTimeout(timer);
      stop();
      stopShifts();
    };
  }, []);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    loadDashboardSnapshot(range)
      .then((next) => {
        if (!alive) return;
        setSnapshot(next);
        setError(null);
      })
      .catch((caught) => {
        if (!alive) return;
        setError(errorMessage(caught));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [range, refresh]);

  async function removeShift(id: string) {
    setDeletingId(id);
    try {
      await deleteShift(id);
      setPendingDeleteId(null);
      setRefresh((current) => current + 1);
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setDeletingId(null);
    }
  }

  const rangeLabel = RANGES.find((item) => item.id === range)?.label ?? "";

  return (
    <section className="flex flex-col gap-5">
      <header className="flex flex-wrap items-center justify-between gap-4">
        <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
          Dashboard
        </h1>

        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            disabled={clockBusy || onShift}
            className="cf-btn-live"
            style={{ background: "#ff6a00" }}
            onClick={() => {
              setClockBusy(true);
              void clockIn()
                .then(() => {
                  setRefresh((current) => current + 1);
                  void syncDropperShiftRecords();
                })
                .catch((caught) => setError(errorMessage(caught)))
                .finally(() => setClockBusy(false));
            }}
          >
            Check In
          </button>
          <button
            type="button"
            disabled={clockBusy || !onShift}
            className="cf-btn-secondary"
            style={{ background: "var(--bg-elevated)", color: "var(--header-chip-text)", borderColor: "var(--header-chip)" }}
            onClick={() => {
              setClockBusy(true);
              void clockOut()
                .then(() => {
                  setRefresh((current) => current + 1);
                  void syncDropperShiftRecords();
                })
                .catch((caught) => setError(errorMessage(caught)))
                .finally(() => setClockBusy(false));
            }}
          >
            Check Out
          </button>
          <div
            className="inline-flex gap-0.5 rounded-full p-1"
            style={{ background: "var(--header-chip)" }}
            role="group"
            aria-label="Earnings basis"
          >
            <BasisButton active={basis === "gross"} onClick={() => setBasis("gross")}>
              Bruto
            </BasisButton>
            <BasisButton active={basis === "net"} onClick={() => setBasis("net")}>
              Neto
            </BasisButton>
          </div>
          <div
            className="inline-flex gap-0.5 rounded-full p-1"
            style={{ background: "var(--header-chip)" }}
            role="group"
            aria-label="Time range"
          >
            {RANGES.map((item) => (
              <button
                key={item.id}
                type="button"
                aria-pressed={range === item.id}
                className="rounded-full px-3.5 py-1.5 text-sm font-medium transition-colors"
                style={
                  range === item.id
                    ? { background: "var(--text-primary)", color: "var(--bg)" }
                    : { color: "var(--header-chip-text)", background: "transparent" }
                }
                onClick={() => setRange(item.id)}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>
      </header>

      {error ? (
        <p role="alert" className="text-sm" style={{ color: "var(--danger)" }}>
          {error}
        </p>
      ) : null}

      <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1.7fr)_minmax(17rem,0.85fr)]">
        <article className="cf-card min-w-0 p-5 sm:p-6" style={{ borderRadius: CARD_RADIUS }}>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <h2 className="text-base font-semibold" style={{ color: "var(--text-primary)" }}>
              Ingresos
            </h2>
            <span
              className="rounded-full px-2.5 py-1 text-xs font-medium"
              style={{ background: "var(--bg-subtle)", color: "var(--text-secondary)" }}
            >
              {rangeLabel}
            </span>
          </div>
          <div className="mt-5 grid gap-4 sm:grid-cols-3">
            <RevenueFigure
              label="Ingreso bruto"
              value={loading && !revenue ? null : formatMoney(revenue?.gross ?? 0)}
            />
            <RevenueFigure
              label="Ingreso neto"
              value={loading && !revenue ? null : formatMoney(revenue?.net ?? 0)}
            />
            <RevenueFigure
              label="Promedio diario"
              value={loading && !revenue ? null : formatMoney(revenue?.averageDaily ?? 0)}
            />
          </div>
          <div className="mt-4 flex items-center justify-end gap-4 text-xs" style={{ color: "var(--text-tertiary)" }}>
            <span className="inline-flex items-center gap-1.5">
              <i className="h-2.5 w-2.5 rounded-sm" style={{ background: "var(--bar-gross)" }} />
              Bruto
            </span>
            <span className="inline-flex items-center gap-1.5">
              <i className="h-2.5 w-2.5 rounded-sm" style={{ background: "var(--bar-net)" }} />
              Neto
            </span>
          </div>
          <div
            className="mt-2 h-60"
            role="img"
            aria-label={`Ingresos por día. Bruto ${formatMoney(revenue?.gross ?? 0)}, neto ${formatMoney(revenue?.net ?? 0)}.`}
          >
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={revenue?.bars ?? []} barGap={3} barCategoryGap="28%" margin={{ top: 8, right: 4, left: 0, bottom: 0 }}>
                <CartesianGrid vertical={false} stroke="var(--border-muted)" />
                <XAxis
                  dataKey="label"
                  axisLine={false}
                  tickLine={false}
                  tick={{ fill: "var(--text-tertiary)", fontSize: 11 }}
                />
                <YAxis
                  axisLine={false}
                  tickLine={false}
                  width={48}
                  tick={{ fill: "var(--text-tertiary)", fontSize: 11 }}
                  tickFormatter={(value) => axisMoney(Number(value))}
                />
                <Tooltip
                  cursor={{ fill: "var(--bg-subtle)" }}
                  contentStyle={{
                    backgroundColor: "var(--bg-elevated)",
                    borderColor: "var(--border)",
                    borderRadius: 12,
                    color: "var(--text-primary)",
                  }}
                  formatter={(value) => formatMoney(Number(value))}
                />
                <Bar dataKey="gross" name="Bruto" fill="var(--bar-gross)" radius={[4, 4, 0, 0]} maxBarSize={18} />
                <Bar dataKey="net" name="Neto" fill="var(--bar-net)" radius={[4, 4, 0, 0]} maxBarSize={18} />
              </BarChart>
            </ResponsiveContainer>
          </div>
        </article>

        <article className="cf-card flex min-h-72 flex-col p-5" style={{ borderRadius: CARD_RADIUS }}>
          <h2 className="text-base font-semibold" style={{ color: "var(--text-primary)" }}>
            Empleados
          </h2>
          <p className="mt-1 text-xs" style={{ color: "var(--text-tertiary)" }}>
            {basis === "net" ? "Neto del período" : "Bruto del período"}
          </p>
          {sales.length === 0 ? (
            <EmptyState icon={CircleDollarSign} label="Sin ventas en este período" />
          ) : (
            <ol className="mt-3 flex max-h-[28rem] flex-col gap-1 overflow-auto">
              {sales.map((sale, index) => (
                <li
                  key={sale.employeeId}
                  className="flex items-center gap-3 py-2.5 text-sm"
                  style={index < sales.length - 1 ? { borderBottom: "1px solid var(--border-muted)" } : undefined}
                >
                  <span className="w-8 shrink-0 text-xs font-semibold tabular-nums" style={{ color: "var(--text-tertiary)" }}>
                    #{index + 1}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-medium" style={{ color: "var(--text-primary)" }}>
                    {sale.name}
                  </span>
                  <span className="shrink-0 tabular-nums font-semibold" style={{ color: "var(--text-primary)" }}>
                    {formatMoney(sale.amount)}
                  </span>
                </li>
              ))}
            </ol>
          )}
        </article>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        {CHANNEL_CARDS.map((card) => {
          const points = channels.map((point) => ({ label: point.label, value: point[card.field] }));
          const total = points.reduce((sum, point) => sum + (Number.isFinite(point.value) ? point.value : 0), 0);
          const Icon = card.icon;
          const showingFans = card.field === "subs" && subsMode === "fans";
          const line = sparkline(points);
          return (
            <article key={card.field} className="cf-card flex flex-col p-5" style={{ borderRadius: CARD_RADIUS }}>
              <div className="flex items-center gap-2">
                <span
                  className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full"
                  style={{ background: "var(--bg-subtle)" }}
                >
                  <Icon size={15} strokeWidth={1.75} style={{ color: "var(--text-secondary)" }} />
                </span>
                <h2 className="text-sm font-medium" style={{ color: "var(--text-secondary)" }}>
                  {card.label}
                </h2>
              </div>
              {card.field === "subs" ? (
                <div
                  className="mt-3 grid grid-cols-2 gap-0.5 rounded-full p-0.5"
                  style={{ background: "var(--header-chip)" }}
                  role="group"
                  aria-label="Suscripciones o fans"
                >
                  <button
                    type="button"
                    aria-pressed={subsMode === "income"}
                    className="rounded-full px-2 py-1 text-xs font-medium"
                    style={
                      subsMode === "income"
                        ? { background: "var(--bg-elevated)", color: "var(--text-primary)" }
                        : { color: "var(--header-chip-text)", background: "transparent" }
                    }
                    onClick={() => setSubsMode("income")}
                  >
                    Ingresos
                  </button>
                  <button
                    type="button"
                    aria-pressed={subsMode === "fans"}
                    className="rounded-full px-2 py-1 text-xs font-medium"
                    style={
                      subsMode === "fans"
                        ? { background: "var(--bg-elevated)", color: "var(--text-primary)" }
                        : { color: "var(--header-chip-text)", background: "transparent" }
                    }
                    onClick={() => setSubsMode("fans")}
                  >
                    Fans
                  </button>
                </div>
              ) : null}
              <p className="mt-4 text-3xl font-semibold tabular-nums tracking-tight" style={{ color: "var(--text-primary)" }}>
                {showingFans ? (
                  loading && fans == null ? (
                    <span className="cf-skeleton inline-block h-8 w-28 rounded-lg" />
                  ) : fans?.total == null ? (
                    "—"
                  ) : (
                    fans.total.toLocaleString("es")
                  )
                ) : loading && channels.length === 0 ? (
                  <span className="cf-skeleton inline-block h-8 w-28 rounded-lg" />
                ) : (
                  formatMoney(total)
                )}
              </p>
              {showingFans ? (
                <ul className="mt-3 flex max-h-14 flex-col gap-1 overflow-auto text-xs" aria-label="Fans por cuenta">
                  {(fans?.accounts ?? []).length === 0 ? (
                    <li style={{ color: "var(--text-tertiary)" }}>Sin cuentas</li>
                  ) : (
                    fans?.accounts.map((account) => (
                      <li key={account.modelId} className="flex items-center justify-between gap-3">
                        <span className="min-w-0 truncate" style={{ color: "var(--text-secondary)" }}>
                          {account.name}
                        </span>
                        <span className="tabular-nums font-medium" style={{ color: "var(--text-primary)" }}>
                          {account.fans == null ? "—" : account.fans.toLocaleString("es")}
                        </span>
                      </li>
                    ))
                  )}
                </ul>
              ) : (
                <div className="mt-3 h-14" role="img" aria-label={`${card.label} por día`}>
                  <ResponsiveContainer width="100%" height="100%">
                    <AreaChart data={line} margin={{ top: 6, right: 0, left: 0, bottom: 0 }}>
                      <Area
                        type="monotone"
                        dataKey="value"
                        stroke="#ff5a1f"
                        fill="#ff5a1f"
                        fillOpacity={0.16}
                        strokeWidth={2}
                        dot={false}
                        activeDot={false}
                        isAnimationActive={false}
                      />
                    </AreaChart>
                  </ResponsiveContainer>
                </div>
              )}
            </article>
          );
        })}
      </div>

      <div className="grid gap-4 lg:grid-cols-3">
        <Panel title="Mis turnos" icon={CalendarDays}>
          {shifts.length === 0 ? (
            <EmptyState icon={CalendarDays} label="Sin turnos en este período" />
          ) : (
            <ul className="flex flex-col">
              {shifts.map((shift, index) => (
                <li
                  key={shift.id}
                  className="grid grid-cols-2 gap-x-4 gap-y-2 py-3 text-sm"
                  style={index > 0 ? { borderTop: "1px solid var(--border-muted)" } : undefined}
                >
                  <ShiftField label="Usuario" value={shift.employeeName} />
                  <ShiftField label="Fecha" value={formatShiftDate(shift.startTime)} />
                  <ShiftField label="Hora del check-in" value={formatShiftTime(shift.startTime)} />
                  <ShiftField label="Hora del check-out" value={formatCheckout(shift.startTime, shift.endTime)} />
                  {canDeleteShifts ? (
                    <ShiftDelete
                      pending={pendingDeleteId === shift.id}
                      busy={deletingId === shift.id}
                      onAsk={() => setPendingDeleteId(shift.id)}
                      onCancel={() => setPendingDeleteId(null)}
                      onConfirm={() => void removeShift(shift.id)}
                    />
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel title="Empleados activos" icon={Users}>
          {clockedIn.length === 0 ? (
            <EmptyState icon={Users} label="No hay empleados con turno activo." />
          ) : (
            <ul className="flex flex-col gap-2">
              {clockedIn.map((employee) => (
                <li key={employee.id} className="flex items-center gap-2.5 py-1.5">
                  <span
                    className="relative flex h-2 w-2 shrink-0 rounded-full"
                    aria-hidden="true"
                    style={{ background: "var(--live)" }}
                  >
                    <span
                      className="absolute inline-flex h-full w-full animate-ping rounded-full opacity-75"
                      style={{ background: "var(--live)" }}
                    />
                  </span>
                  <span className="text-sm font-medium" style={{ color: "var(--text-primary)" }}>
                    {employee.name}
                  </span>
                  <span className="cf-badge cf-badge-neutral ml-auto">
                    {employee.role}
                  </span>
                  {canDeleteShifts ? (
                    <ShiftDelete
                      pending={pendingDeleteId === employee.id}
                      busy={deletingId === employee.id}
                      onAsk={() => setPendingDeleteId(employee.id)}
                      onCancel={() => setPendingDeleteId(null)}
                      onConfirm={() => void removeShift(employee.id)}
                    />
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel title="Ventas por empleado" icon={CircleDollarSign}>
          {sales.length === 0 ? (
            <EmptyState icon={CircleDollarSign} label="Sin ventas en este período" />
          ) : (
            <ul className="flex flex-col gap-1">
              {sales.map((sale, index) => (
                <li
                  key={sale.employeeId}
                  className="flex items-center justify-between gap-3 py-2 text-sm"
                  style={index < sales.length - 1 ? { borderBottom: "1px solid var(--border-muted)" } : {}}
                >
                  <span className="flex items-center gap-2">
                    <span
                      className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-[10px] font-bold text-white"
                      style={{ background: `hsl(${(index * 67) % 360} 55% 45%)` }}
                    >
                      {sale.name[0]?.toUpperCase()}
                    </span>
                    <span className="font-medium" style={{ color: "var(--text-primary)" }}>
                      {sale.name}
                    </span>
                  </span>
                  <span className="tabular-nums font-semibold" style={{ color: "var(--text-primary)" }}>
                    {formatMoney(sale.amount)}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </section>
  );
}

function sparkline(points: { label: string; value: number }[]) {
  if (points.length >= 2) return points;
  const value = points[0]?.value ?? 0;
  const label = points[0]?.label ?? "";
  return [
    { label, value },
    { label: `${label} `, value },
  ];
}

function axisMoney(value: number): string {
  if (!Number.isFinite(value)) return "$0";
  if (Math.abs(value) >= 1000) {
    const thousands = value / 1000;
    return `$${thousands >= 10 ? Math.round(thousands) : thousands.toFixed(1)}K`;
  }
  return `$${Math.round(value)}`;
}

function RevenueFigure({ label, value }: { label: string; value: string | null }) {
  return (
    <div>
      <p className="text-xs font-medium" style={{ color: "var(--text-tertiary)" }}>
        {label}
      </p>
      <p className="mt-1 text-2xl font-semibold tabular-nums tracking-tight sm:text-3xl" style={{ color: "var(--text-primary)" }}>
        {value == null ? <span className="cf-skeleton inline-block h-8 w-28 rounded-lg" /> : value}
      </p>
    </div>
  );
}

function BasisButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: string;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      className="rounded-full px-3 py-1.5 text-sm font-medium transition-colors"
      style={
        active
          ? { background: "var(--bg-elevated)", color: "var(--text-primary)", boxShadow: "var(--shadow-sm)" }
          : { color: "var(--header-chip-text)", background: "transparent" }
      }
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function ShiftDelete({
  pending,
  busy,
  onAsk,
  onCancel,
  onConfirm,
}: {
  pending: boolean;
  busy: boolean;
  onAsk: () => void;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  if (!pending) {
    return (
      <button
        type="button"
        className="cf-btn-secondary col-span-2 w-fit"
        style={{ padding: "0.3rem 0.65rem", color: "var(--text-tertiary)" }}
        onClick={onAsk}
      >
        <Trash2 size={13} strokeWidth={2} />
        Eliminar
      </button>
    );
  }
  return (
    <span className="col-span-2 flex flex-wrap gap-2">
      <button
        type="button"
        className="cf-btn-danger"
        style={{ padding: "0.3rem 0.65rem" }}
        disabled={busy}
        onClick={onConfirm}
      >
        {busy ? "Eliminando…" : "Confirmar"}
      </button>
      <button
        type="button"
        className="cf-btn-secondary"
        style={{ padding: "0.3rem 0.65rem" }}
        disabled={busy}
        onClick={onCancel}
      >
        Cancelar
      </button>
    </span>
  );
}

function ShiftField({ label, value }: { label: string; value: string }) {
  return (
    <span>
      <span className="block text-[11px] font-medium" style={{ color: "var(--text-tertiary)" }}>
        {label}
      </span>
      <span className="block font-medium" style={{ color: "var(--text-primary)" }}>
        {value}
      </span>
    </span>
  );
}

function Panel({
  title,
  icon: Icon,
  children,
}: {
  title: string;
  icon: LucideIcon;
  children: ReactNode;
}) {
  return (
    <section className="cf-card flex min-h-56 flex-col" style={{ padding: "1.25rem", borderRadius: CARD_RADIUS }}>
      <div className="mb-4 flex items-center gap-2">
        <Icon size={16} strokeWidth={1.75} style={{ color: "var(--text-secondary)" }} />
        <h2 className="text-sm font-semibold" style={{ color: "var(--text-primary)" }}>
          {title}
        </h2>
      </div>
      <div className="flex flex-1 flex-col">{children}</div>
    </section>
  );
}

function EmptyState({ icon: Icon, label }: { icon: LucideIcon; label: string }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 py-6 text-center">
      <div
        className="flex h-10 w-10 items-center justify-center rounded-full"
        style={{ background: "var(--bg-subtle)" }}
      >
        <Icon size={18} strokeWidth={1.5} style={{ color: "var(--text-disabled)" }} aria-hidden="true" />
      </div>
      <p className="text-sm" style={{ color: "var(--text-tertiary)" }}>
        {label}
      </p>
    </div>
  );
}
