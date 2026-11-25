import { listen } from "@tauri-apps/api/event";
import { CalendarDays, CircleDollarSign, TrendingUp, Users, type LucideIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import {
  errorMessage,
  getClockedInEmployees,
  getEarningsOverview,
  getEmployeeSales,
  getMyShifts,
  type ClockedInEmployee,
  type EarningsBasis,
  type EarningsOverview,
  type EmployeeSale,
  type ShiftRow,
  type TimeRange,
} from "./api";
import { formatMoney, formatStamp } from "./types";

const RANGES: { id: TimeRange; label: string }[] = [
  { id: "yesterday", label: "Ayer" },
  { id: "today", label: "Hoy" },
  { id: "thisWeek", label: "Esta semana" },
  { id: "thisMonth", label: "Este mes" },
];

const CHANNELS_META: Record<string, { color: string; bg: string }> = {
  subscriptions: { color: "#3a5be0", bg: "#e0e9ff" },
  posts:         { color: "#0f766e", bg: "#ccfbf1" },
  messages:      { color: "#7c3aed", bg: "#ede9fe" },
  tips:          { color: "#f97316", bg: "#fff7ed" },
  referrals:     { color: "#0369a1", bg: "#e0f2fe" },
  streams:       { color: "#be185d", bg: "#fce7f3" },
};

export function DashboardView() {
  const [range, setRange] = useState<TimeRange>("today");
  const [basis, setBasis] = useState<EarningsBasis>("net");
  const [overview, setOverview] = useState<EarningsOverview | null>(null);
  const [shifts, setShifts] = useState<ShiftRow[]>([]);
  const [clockedIn, setClockedIn] = useState<ClockedInEmployee[]>([]);
  const [sales, setSales] = useState<EmployeeSale[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    let alive = true;
    let stop = () => {};
    void listen("models-changed", () => setRefresh((current) => current + 1)).then((unlisten) => {
      if (alive) stop = unlisten;
      else unlisten();
    });
    return () => { alive = false; stop(); };
  }, []);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    Promise.all([
      getEarningsOverview(range, basis),
      getMyShifts(range, basis),
      getClockedInEmployees(),
      getEmployeeSales(range, basis),
    ])
      .then(([nextOverview, nextShifts, nextClocked, nextSales]) => {
        if (!alive) return;
        setOverview(nextOverview);
        setShifts(nextShifts);
        setClockedIn(nextClocked);
        setSales(nextSales);
        setError(null);
      })
      .catch((caught) => {
        if (!alive) return;
        setError(errorMessage(caught));
      })
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, [range, basis, refresh]);

  const total = overview?.total ?? 0;

  return (
    <section className="flex flex-col gap-6">
      {/* Header */}
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
            Creator earnings overview
          </h1>
          <p className="mt-1 text-sm" style={{ color: "var(--text-secondary)" }}>
            Resumen de ingresos por canal y período.
          </p>
        </div>

        {/* Controls row */}
        <div className="flex flex-wrap items-center gap-3">
          {/* Basis toggle */}
          <div
            className="inline-flex rounded-lg p-1 gap-0.5"
            style={{ background: "var(--bg-subtle)" }}
            role="group"
            aria-label="Earnings basis"
          >
            <BasisButton active={basis === "net"} onClick={() => setBasis("net")}>
              Neto
            </BasisButton>
            <BasisButton active={basis === "gross"} onClick={() => setBasis("gross")}>
              Bruto
            </BasisButton>
          </div>

          {/* Range buttons */}
          <div
            className="inline-flex rounded-lg p-1 gap-0.5"
            style={{ background: "var(--bg-subtle)" }}
            role="group"
            aria-label="Time range"
          >
            {RANGES.map((item) => (
              <button
                key={item.id}
                type="button"
                aria-pressed={range === item.id}
                className="rounded-md px-3 py-1.5 text-sm font-medium transition-all"
                style={
                  range === item.id
                    ? {
                        background: "var(--bg-elevated)",
                        color: "var(--text-primary)",
                        boxShadow: "var(--shadow-sm)",
                      }
                    : { color: "var(--text-tertiary)" }
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
        <p role="alert" className="text-sm" style={{ color: "#dc2626" }}>
          {error}
        </p>
      ) : null}

      {/* Total earnings card */}
      <div className="cf-card p-6">
        <div className="flex items-center gap-2 mb-1">
          <TrendingUp size={16} strokeWidth={1.75} style={{ color: "var(--brand-500)" }} />
          <span className="text-xs font-medium uppercase tracking-wide" style={{ color: "var(--text-tertiary)" }}>
            Total earnings
          </span>
        </div>
        <p className="text-4xl font-semibold tabular-nums tracking-tight" style={{ color: "var(--text-primary)" }}>
          {loading && !overview ? (
            <span className="cf-skeleton inline-block h-10 w-40 rounded-lg" />
          ) : (
            formatMoney(total)
          )}
        </p>

        <div className="mt-6 grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
          {(overview?.channels ?? EMPTY_CHANNELS).map((channel) => {
            const share = total > 0 ? (channel.amount / total) * 100 : 0;
            const meta = CHANNELS_META[channel.key] ?? { color: "var(--brand-500)", bg: "var(--bg-subtle)" };
            return (
              <div key={channel.key} className="flex flex-col gap-1.5">
                <p className="text-xs font-medium" style={{ color: "var(--text-tertiary)" }}>
                  {channel.label}
                </p>
                <p className="text-sm font-semibold tabular-nums" style={{ color: "var(--text-primary)" }}>
                  {loading && !overview ? "…" : formatMoney(channel.amount)}
                </p>
                <div
                  className="h-1.5 overflow-hidden rounded-full"
                  style={{ background: "var(--bg-muted)" }}
                >
                  <div
                    className="h-full rounded-full transition-all duration-500"
                    style={{ width: `${share}%`, background: meta.color }}
                  />
                </div>
                <p className="text-[11px]" style={{ color: "var(--text-disabled)" }}>
                  {share.toFixed(1)}%
                </p>
              </div>
            );
          })}
        </div>
      </div>

      {/* Bottom panels */}
      <div className="grid gap-4 lg:grid-cols-3">
        <Panel title="Mis turnos" icon={CalendarDays}>
          {shifts.length === 0 ? (
            <EmptyState icon={CalendarDays} label="Sin turnos en este período" />
          ) : (
            <ul className="flex flex-col divide-y" style={{ "--divide-color": "var(--border-muted)" } as React.CSSProperties}>
              {shifts.map((shift) => (
                <li key={shift.id} className="flex items-start justify-between gap-3 py-3 text-sm">
                  <span>
                    <span className="block font-medium" style={{ color: "var(--text-primary)" }}>
                      {shift.employeeName}
                    </span>
                    <span className="block text-xs mt-0.5" style={{ color: "var(--text-tertiary)" }}>
                      {shift.role} · {formatStamp(shift.startTime)}
                      {shift.endTime ? ` – ${formatStamp(shift.endTime)}` : " · Abierto"}
                    </span>
                  </span>
                  <span className="tabular-nums font-semibold text-sm shrink-0" style={{ color: "var(--text-primary)" }}>
                    {formatMoney(shift.amount)}
                  </span>
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
                    className="relative flex h-2 w-2 rounded-full shrink-0"
                    aria-hidden="true"
                    style={{ background: "var(--live)" }}
                  >
                    <span
                      className="absolute inline-flex h-full w-full rounded-full opacity-75 animate-ping"
                      style={{ background: "var(--live)" }}
                    />
                  </span>
                  <span className="text-sm font-medium" style={{ color: "var(--text-primary)" }}>
                    {employee.name}
                  </span>
                  <span
                    className="cf-badge cf-badge-neutral ml-auto"
                  >
                    {employee.role}
                  </span>
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
                      className="flex h-6 w-6 items-center justify-center rounded-full text-[10px] font-bold text-white shrink-0"
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

const EMPTY_CHANNELS: EarningsOverview["channels"] = [
  { key: "subscriptions", label: "Subscriptions", amount: 0 },
  { key: "posts",         label: "Posts",          amount: 0 },
  { key: "messages",      label: "Messages",        amount: 0 },
  { key: "tips",          label: "Tips",            amount: 0 },
  { key: "referrals",     label: "Referrals",       amount: 0 },
  { key: "streams",       label: "Streams",         amount: 0 },
];

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
      className="rounded-md px-3 py-1.5 text-sm font-medium transition-all"
      style={
        active
          ? {
              background: "var(--bg-elevated)",
              color: "var(--text-primary)",
              boxShadow: "var(--shadow-sm)",
            }
          : { color: "var(--text-tertiary)" }
      }
      onClick={onClick}
    >
      {children}
    </button>
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
    <section
      className="cf-card flex min-h-56 flex-col"
      style={{ padding: "1.25rem" }}
    >
      <div className="flex items-center gap-2 mb-4">
        <Icon size={15} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
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
