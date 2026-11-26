import type { ReactNode } from "react";
import { initials } from "../types";
import { formatReportMoney } from "./format";

export const INK = "var(--report-ink)";
export const MUTED = "var(--report-muted)";
export const LINE = "var(--report-line)";
export const HAIRLINE = "var(--report-hairline)";
export const ROW_LINE = "var(--report-row)";
export const CANVAS = "var(--report-canvas)";
export const CARD = "var(--report-card)";
export const PILL = "var(--report-pill)";
export const TRACK = "var(--report-track)";
export const DONUT = "var(--report-donut)";

export const PENDING_TITLE = "Sin fórmula confirmada con los datos actuales";

export function Hairline() {
  return <hr className="border-0 border-t" style={{ borderColor: HAIRLINE }} />;
}

export function Surface({ title, children, className = "" }: { title: string; children: ReactNode; className?: string }) {
  return (
    <section className={`rounded-[18px] px-5 py-4 ${className}`} style={{ background: CARD, border: `1px solid ${LINE}` }}>
      <h2 className="text-[15px] font-medium" style={{ color: INK }}>{title}</h2>
      {children}
    </section>
  );
}

export function KpiCard({ label, value, trend, pending = false }: { label: string; value: string; trend?: string | null; pending?: boolean }) {
  return (
    <article className="rounded-2xl px-[18px] py-4" style={{ background: CARD, border: `1px solid ${LINE}` }}>
      <span className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-[5px] text-[12.5px] font-medium" style={{ background: PILL, color: INK }}>
        <span className="h-[5px] w-[5px] rounded-full" style={{ background: INK }} aria-hidden="true" />
        {label}
      </span>
      <div className="mt-3 flex items-baseline justify-between gap-3">
        <p
          className="text-[22px] font-medium leading-none tabular-nums"
          style={{ color: pending ? MUTED : INK }}
          title={pending ? PENDING_TITLE : undefined}
        >
          {value}
        </p>
        {trend ? <p className="text-[13px] font-normal tabular-nums" style={{ color: INK }}>{trend}</p> : null}
      </div>
    </article>
  );
}

export function PersonAvatar({ name, url, size }: { name: string; url?: string | null; size: number }) {
  const label = initials(name);
  return (
    <span
      className="inline-flex shrink-0 items-center justify-center overflow-hidden rounded-full font-medium"
      style={{ width: size, height: size, background: PILL, color: INK, fontSize: size < 32 ? 11 : 12 }}
    >
      {url ? <img src={url} alt="" className="h-full w-full object-cover" /> : label}
    </span>
  );
}

export function ReportLoading() {
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4" aria-busy="true" aria-live="polite">
      {[0, 1, 2, 3].map((item) => (
        <div key={item} className="h-[92px] rounded-2xl" style={{ background: "var(--report-control)", border: `1px solid ${LINE}` }} />
      ))}
      <span className="sr-only">Cargando reportes</span>
    </div>
  );
}

export function ReportError({ message }: { message: string }) {
  return (
    <p role="alert" className="text-sm" style={{ color: INK }}>
      {message}
    </p>
  );
}

export function EmptyNote({ children }: { children: string }) {
  return (
    <p className="py-8 text-sm" style={{ color: MUTED }}>
      {children}
    </p>
  );
}

export function PendingCell() {
  return (
    <span style={{ color: MUTED }} title={PENDING_TITLE}>
      —
    </span>
  );
}

export function MoneyTooltip({
  active,
  payload,
  label,
  format = formatReportMoney,
}: {
  active?: boolean;
  payload?: { name?: string; value?: number | string; color?: string }[];
  label?: string | number;
  format?: (value: number) => string;
}) {
  if (!active || !payload?.length) return null;
  return (
    <div className="rounded-lg px-3 py-2 text-xs" style={{ background: CARD, border: `1px solid ${LINE}`, color: INK }}>
      <p className="mb-1" style={{ color: MUTED }}>{label}</p>
      {payload.map((entry, index) => (
        <p key={`${entry.name ?? "value"}-${index}`} className="tabular-nums" style={{ color: entry.color ?? INK }}>
          {entry.name}: {format(Number(entry.value ?? 0))}
        </p>
      ))}
    </div>
  );
}
