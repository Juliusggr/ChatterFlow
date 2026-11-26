import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { subscribeDropperTables } from "../agency";
import { errorMessage } from "../api";
import {
  EmptyNote,
  Hairline,
  INK,
  KpiCard,
  MUTED,
  MoneyTooltip,
  PILL,
  PersonAvatar,
  ROW_LINE,
  ReportError,
  ReportLoading,
  Surface,
  TRACK,
} from "./chrome";
import { loadDropperReport, type DropperReport, type DropperShiftEntry, type ReportRangeId } from "./data";
import { formatCount, moneyAxis } from "./format";

const SERIES = ["var(--report-series-1)", "var(--report-series-2)", "var(--report-series-3)"] as const;
const RANKS = [
  { background: "var(--report-series-1)", color: "var(--report-rank-ink)" },
  { background: "var(--report-series-2)", color: "var(--report-rank-ink)" },
  { background: "var(--report-rank-3)", color: "var(--report-rank-3-ink)" },
] as const;

export function DropperReports({ range, timeZone }: { range: ReportRangeId; timeZone: string }) {
  const [report, setReport] = useState<DropperReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    let alive = true;
    let timer: number | undefined;
    const stops: Array<() => void> = [];
    const bump = () => {
      if (!alive) return;
      if (timer !== undefined) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (alive) setRefresh((current) => current + 1);
      }, 220);
    };
    void listen("dropper-activity-changed", bump)
      .then((stop) => {
        if (alive) stops.push(stop);
        else stop();
      })
      .catch(() => undefined);
    stops.push(subscribeDropperTables(bump));
    return () => {
      alive = false;
      if (timer !== undefined) window.clearTimeout(timer);
      stops.forEach((stop) => stop());
    };
  }, []);

  useEffect(() => {
    let alive = true;
    setLoading((current) => (report === null ? true : current));
    loadDropperReport(range, timeZone)
      .then((next) => {
        if (!alive) return;
        setReport(next);
        setError(null);
      })
      .catch((caught) => {
        if (!alive) return;
        setReport(null);
        setError(errorMessage(caught));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [range, timeZone, refresh]);

  if (loading) return <ReportLoading />;
  if (error || !report) return <ReportError message={error ?? "No se pudo cargar el reporte"} />;
  return <DropperReportView report={report} timeZone={timeZone} />;
}

function DropperReportView({ report, timeZone }: { report: DropperReport; timeZone: string }) {
  const chartRows = report.labels.map((label, index) => {
    const row: Record<string, string | number> = { label };
    report.series.forEach((series, seriesIndex) => {
      row[`s${seriesIndex}`] = series.values[index] ?? 0;
    });
    return row;
  });
  const max = Math.max(0, ...report.series.flatMap((series) => series.values));
  const axis = moneyAxis(max);

  return (
    <div className="flex flex-col gap-7">
      <Hairline />
      <div className="grid grid-cols-1 items-stretch gap-6 lg:grid-cols-12">
        <Surface title="Publicaciones" className="lg:col-span-7">
          {report.series.length === 0 ? (
            <EmptyNote>No hay publicaciones en este rango.</EmptyNote>
          ) : (
            <div className="mt-4 h-[250px] w-full">
              <ResponsiveContainer width="100%" height="100%">
                <LineChart data={chartRows} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                  <CartesianGrid vertical={false} stroke={TRACK} />
                  <XAxis dataKey="label" axisLine={false} tickLine={false} minTickGap={24} tick={{ fontSize: 12, fill: MUTED }} />
                  <YAxis
                    ticks={axis.ticks}
                    domain={[0, axis.top > 0 ? axis.top : 1]}
                    axisLine={false}
                    tickLine={false}
                    width={48}
                    allowDecimals={false}
                    tick={{ fontSize: 12, fill: MUTED }}
                    tickFormatter={(value: number) => formatCount(value)}
                  />
                  <Tooltip content={<MoneyTooltip format={formatCount} />} cursor={{ stroke: TRACK }} />
                  {report.series.map((series, index) => (
                    <Line
                      key={series.id}
                      type="monotone"
                      dataKey={`s${index}`}
                      name={series.name}
                      stroke={SERIES[index] ?? INK}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 3, strokeWidth: 0 }}
                      isAnimationActive={false}
                    />
                  ))}
                </LineChart>
              </ResponsiveContainer>
            </div>
          )}
        </Surface>
        <ol className="flex h-full flex-col justify-evenly gap-3 py-2 lg:col-span-5" aria-label="Top droppers">
          {report.ranking.map((person, index) => {
            const rank = RANKS[index] ?? RANKS[2];
            return (
              <li key={person.id} className="flex items-center gap-3">
                <PersonAvatar name={person.name} size={36} />
                <span className="text-[15px] font-medium" style={{ color: INK }}>{person.name}</span>
                <span
                  className="ml-auto flex h-[34px] w-7 items-center justify-center rounded-lg text-sm font-medium tabular-nums"
                  style={{ background: rank.background, color: rank.color }}
                >
                  {index + 1}
                </span>
              </li>
            );
          })}
        </ol>
      </div>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <KpiCard label="Publicaciones" value={formatCount(report.totalPosts)} />
        <KpiCard label="Suscripciones" value={formatCount(report.totalFans)} />
        <KpiCard label="Turnos registrados" value={formatCount(report.totalShifts)} />
      </div>

      <Hairline />
      <section>
        <h2 className="mb-4 text-[15px] font-medium" style={{ color: INK }}>
          Resumen por dropper
        </h2>
        {report.rows.length === 0 ? (
          <EmptyNote>No hay droppers en este rango.</EmptyNote>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[640px] table-fixed border-collapse text-left">
              <thead>
                <tr className="text-[13px] font-normal" style={{ color: MUTED }}>
                  <th className="w-[22%] pb-3 font-normal">Dropper</th>
                  <th className="w-[20%] pb-3 font-normal">Cuenta</th>
                  <th className="w-[24%] pb-3 font-normal">Horario</th>
                  <th className="w-[12%] pb-3 font-normal">Publicaciones</th>
                  <th className="w-[12%] pb-3 font-normal">Suscripciones</th>
                  <th className="pb-3 font-normal">Turnos</th>
                </tr>
              </thead>
              <tbody>
                {report.rows.map((row) => (
                  <tr key={row.id} className="border-b text-sm" style={{ borderColor: ROW_LINE, color: INK }}>
                    <td className="py-3">
                      <span className="flex items-center gap-3">
                        <PersonAvatar name={row.name} size={30} />
                        <span className="truncate">{row.name}</span>
                      </span>
                    </td>
                    <td className="py-3 truncate">{row.account}</td>
                    <td className="py-3 text-xs" style={{ color: MUTED }}>{row.schedule}</td>
                    <td className="py-3 tabular-nums font-medium">{formatCount(row.posts)}</td>
                    <td className="py-3 tabular-nums font-medium">{formatCount(row.fans)}</td>
                    <td className="py-3 tabular-nums">{formatCount(row.shifts)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <Hairline />
      <section>
        <div className="mb-4 flex flex-wrap items-baseline justify-between gap-2">
          <h2 className="text-[15px] font-medium" style={{ color: INK }}>
            Registro de turnos
          </h2>
          <span className="text-xs" style={{ color: MUTED }}>
            Historial guardado por cada turno
          </span>
        </div>
        {report.shiftRecords.length === 0 ? (
          <EmptyNote>No hay turnos registrados en este rango.</EmptyNote>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[700px] table-fixed border-collapse text-left">
              <thead>
                <tr className="text-[13px] font-normal" style={{ color: MUTED }}>
                  <th className="w-[20%] pb-3 font-normal">Dropper</th>
                  <th className="w-[18%] pb-3 font-normal">Cuenta</th>
                  <th className="w-[14%] pb-3 font-normal">Fecha</th>
                  <th className="w-[24%] pb-3 font-normal">Turno</th>
                  <th className="w-[12%] pb-3 font-normal">Publicaciones</th>
                  <th className="pb-3 font-normal">Suscripciones</th>
                </tr>
              </thead>
              <tbody>
                {report.shiftRecords.map((entry) => (
                  <ShiftRecordRow key={entry.id} entry={entry} timeZone={timeZone} />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}

function ShiftRecordRow({ entry, timeZone }: { entry: DropperShiftEntry; timeZone: string }) {
  const dateLabel = formatZonedDate(entry.shiftStart, timeZone);
  const startLabel = formatZonedTime(entry.shiftStart, timeZone);
  const endLabel = entry.shiftEnd ? formatZonedTime(entry.shiftEnd, timeZone) : "En curso";
  const active = entry.status === "active";

  return (
    <tr className="border-b text-sm" style={{ borderColor: ROW_LINE, color: INK }}>
      <td className="py-3">
        <span className="flex items-center gap-3">
          <PersonAvatar name={entry.dropperName} size={28} />
          <span className="truncate">{entry.dropperName}</span>
        </span>
      </td>
      <td className="py-3 truncate">{entry.accountName}</td>
      <td className="py-3 text-xs tabular-nums" style={{ color: MUTED }}>
        {dateLabel}
      </td>
      <td className="py-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="tabular-nums text-xs font-medium" style={{ color: INK }}>
            {startLabel} – {endLabel}
          </span>
          <span
            className="rounded-md px-1.5 py-0.5 text-[11px] font-medium"
            style={{
              background: PILL,
              color: active ? "var(--live, #16a34a)" : MUTED,
            }}
          >
            {active ? "En turno" : "Finalizado"}
          </span>
        </div>
        {entry.scheduleLabel && entry.scheduleLabel !== "Sin horario" ? (
          <p className="mt-0.5 text-[11px]" style={{ color: MUTED }}>
            Horario: {entry.scheduleLabel}
          </p>
        ) : null}
      </td>
      <td className="py-3 tabular-nums font-medium">{formatCount(entry.publicationsCount)}</td>
      <td className="py-3 tabular-nums font-medium">{formatCount(entry.subscriptionsCount)}</td>
    </tr>
  );
}

function formatZonedDate(iso: string, timeZone: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return new Intl.DateTimeFormat("es", {
    timeZone,
    day: "numeric",
    month: "short",
    year: "numeric",
  }).format(date);
}

function formatZonedTime(iso: string, timeZone: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return new Intl.DateTimeFormat("es", {
    timeZone,
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}
