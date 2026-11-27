import { listen } from "@tauri-apps/api/event";
import { ChevronDown } from "lucide-react";
import { Fragment, useEffect, useState } from "react";
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { subscribeDropperTables } from "../agency";
import { errorMessage } from "../api";
import {
  CARD,
  EmptyNote,
  Hairline,
  INK,
  KpiCard,
  LINE,
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
import {
  loadDropperReport,
  type DropperModelBreakdown,
  type DropperReport,
  type DropperShiftGroup,
  type DropperSummaryRow,
  type ReportRangeId,
} from "./data";
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
  const [expandedDroppers, setExpandedDroppers] = useState<Set<string>>(() => new Set());
  const [expandedShifts, setExpandedShifts] = useState<Set<string>>(() => new Set());

  const toggleDropper = (id: string) => {
    setExpandedDroppers((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleShift = (id: string) => {
    setExpandedShifts((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const chartRows = report.labels.map((label, index) => {
    const row: Record<string, string | number> = { label };
    report.series.forEach((series, seriesIndex) => {
      row[`s${seriesIndex}`] = series.values[index] ?? 0;
    });
    return row;
  });
  const max = Math.max(0, ...report.series.flatMap((series) => series.values));
  const axis = moneyAxis(max);
  const singlePoint = chartRows.length === 1;

  return (
    <div className="flex flex-col gap-7">
      <Hairline />
      <div className="grid grid-cols-1 items-stretch gap-6 lg:grid-cols-12">
        <Surface title="Suscripciones" className="lg:col-span-7">
          {report.series.length === 0 ? (
            <EmptyNote>No hay suscripciones en este rango.</EmptyNote>
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
                      dot={singlePoint ? { r: 3.5, strokeWidth: 2, fill: CARD } : false}
                      activeDot={{ r: 3, strokeWidth: 0 }}
                      isAnimationActive={false}
                    />
                  ))}
                </LineChart>
              </ResponsiveContainer>
            </div>
          )}
        </Surface>
        <div className="flex h-full flex-col justify-center lg:col-span-5">
          {report.ranking.length === 0 ? (
            <EmptyNote>No hay actividad de droppers en este rango.</EmptyNote>
          ) : (
            <ol
              className={`flex max-h-[310px] flex-col gap-3 overflow-y-auto py-2 pr-1 ${report.ranking.length <= 3 ? "h-full justify-evenly" : "justify-start"}`}
              aria-label="Top droppers"
            >
              {report.ranking.map((person, index) => {
                const rank = RANKS[index] ?? { background: PILL, color: MUTED };
                const perShift = person.shifts > 1 ? formatAvgPerShift(person.fans / person.shifts) : null;
                return (
                  <li key={person.id} className="flex items-center gap-3">
                    <PersonAvatar name={person.name} size={36} />
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[15px] font-medium" style={{ color: INK }}>
                        {person.name}
                      </p>
                      {person.shifts > 0 ? (
                        <p className="text-xs tabular-nums" style={{ color: MUTED }}>
                          {formatPlural(person.shifts, "turno", "turnos")}
                          {perShift ? ` · ${perShift} / turno` : ""}
                        </p>
                      ) : null}
                    </div>
                    <span
                      className="shrink-0 rounded-lg px-2.5 py-1 text-xs font-medium tabular-nums"
                      style={{ background: PILL, color: INK }}
                    >
                      {formatPlural(person.fans, "suscriptor", "suscriptores")}
                    </span>
                    <span
                      className="flex h-[34px] w-7 shrink-0 items-center justify-center rounded-lg text-sm font-medium tabular-nums"
                      style={{ background: rank.background, color: rank.color }}
                    >
                      {index + 1}
                    </span>
                  </li>
                );
              })}
            </ol>
          )}
        </div>
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
            <table className="w-full min-w-[700px] table-fixed border-collapse text-left">
              <thead>
                <tr className="text-[13px] font-normal" style={{ color: MUTED }}>
                  <th className="w-[22%] pb-3 font-normal">Dropper</th>
                  <th className="w-[16%] pb-3 font-normal">Modelos utilizadas</th>
                  <th className="w-[22%] pb-3 font-normal">Horario</th>
                  <th className="w-[12%] pb-3 font-normal">Publicaciones</th>
                  <th className="w-[12%] pb-3 font-normal">Suscripciones</th>
                  <th className="w-[8%] pb-3 font-normal">Turnos</th>
                  <th className="w-[8%] pb-3 text-right font-normal">
                    <span className="sr-only">Detalles</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {report.rows.map((row) => (
                  <DropperSummaryTableRow
                    key={row.id}
                    row={row}
                    expanded={expandedDroppers.has(row.id)}
                    onToggle={() => toggleDropper(row.id)}
                  />
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
        {report.shiftGroups.length === 0 ? (
          <EmptyNote>No hay turnos registrados en este rango.</EmptyNote>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[780px] table-fixed border-collapse text-left">
              <thead>
                <tr className="text-[13px] font-normal" style={{ color: MUTED }}>
                  <th className="w-[17%] pb-3 font-normal">Dropper</th>
                  <th className="w-[14%] pb-3 font-normal">Cuentas utilizadas</th>
                  <th className="w-[12%] pb-3 font-normal">Fecha</th>
                  <th className="w-[23%] pb-3 font-normal">Turno</th>
                  <th className="w-[11%] pb-3 font-normal">Publicaciones</th>
                  <th className="w-[11%] pb-3 font-normal">Suscripciones</th>
                  <th className="w-[12%] pb-3 text-right font-normal">
                    <span className="sr-only">Modelos</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {report.shiftGroups.map((group) => (
                  <ShiftGroupRow
                    key={group.id}
                    group={group}
                    timeZone={timeZone}
                    expanded={expandedShifts.has(group.id)}
                    onToggle={() => toggleShift(group.id)}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}

function DropperSummaryTableRow({
  row,
  expanded,
  onToggle,
}: {
  row: DropperSummaryRow;
  expanded: boolean;
  onToggle: () => void;
}) {
  const canExpand = row.models.length > 0;
  return (
    <Fragment>
      <tr
        className={`text-sm ${expanded && canExpand ? "" : "border-b"} ${canExpand ? "cursor-pointer" : ""}`}
        style={{ borderColor: ROW_LINE, color: INK }}
        onClick={canExpand ? onToggle : undefined}
      >
        <td className="py-3 pr-2">
          <span className="flex items-center gap-3">
            <PersonAvatar name={row.name} size={30} />
            <span className="truncate font-medium">{row.name}</span>
          </span>
        </td>
        <td className="py-3 pr-2">
          <span
            className="inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium tabular-nums"
            style={{ background: PILL, color: row.modelsCount > 0 ? INK : MUTED }}
          >
            {row.modelsCount > 0 ? formatPlural(row.modelsCount, "modelo", "modelos") : "Sin modelos"}
          </span>
        </td>
        <td className="py-3 pr-2 text-xs" style={{ color: MUTED }}>
          {row.schedule}
        </td>
        <td className="py-3 tabular-nums font-medium">{formatCount(row.posts)}</td>
        <td className="py-3 tabular-nums font-medium">{formatCount(row.fans)}</td>
        <td className="py-3 tabular-nums">{formatCount(row.shifts)}</td>
        <td className="py-3 text-right">
          {canExpand ? (
            <ExpandModelsButton
              expanded={expanded}
              label={expanded ? `Ocultar modelos de ${row.name}` : `Mostrar modelos de ${row.name}`}
              onClick={(event) => {
                event.stopPropagation();
                onToggle();
              }}
            />
          ) : null}
        </td>
      </tr>
      {expanded && canExpand ? (
        <tr className="border-b" style={{ borderColor: ROW_LINE }}>
          <td colSpan={7} className="pb-4 pt-1">
            <ModelBreakdownPanel models={row.models} />
          </td>
        </tr>
      ) : null}
    </Fragment>
  );
}

function ShiftGroupRow({
  group,
  timeZone,
  expanded,
  onToggle,
}: {
  group: DropperShiftGroup;
  timeZone: string;
  expanded: boolean;
  onToggle: () => void;
}) {
  const dateLabel = formatZonedDate(group.shiftStart, timeZone);
  const startLabel = formatZonedTime(group.shiftStart, timeZone);
  const endLabel = group.shiftEnd ? formatZonedTime(group.shiftEnd, timeZone) : "En curso";
  const active = group.status === "active";
  const canExpand = group.models.length > 0;

  return (
    <Fragment>
      <tr
        className={`text-sm ${expanded && canExpand ? "" : "border-b"} ${canExpand ? "cursor-pointer" : ""}`}
        style={{ borderColor: ROW_LINE, color: INK }}
        onClick={canExpand ? onToggle : undefined}
      >
        <td className="py-3 pr-2">
          <span className="flex items-center gap-3">
            <PersonAvatar name={group.dropperName} size={28} />
            <span className="truncate font-medium">{group.dropperName}</span>
          </span>
        </td>
        <td className="py-3 pr-2">
          <span
            className="inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium tabular-nums"
            style={{ background: PILL, color: group.accountsCount > 0 ? INK : MUTED }}
          >
            {group.accountsCount > 0 ? formatPlural(group.accountsCount, "cuenta", "cuentas") : "Sin cuentas"}
          </span>
        </td>
        <td className="py-3 pr-2 text-xs tabular-nums" style={{ color: MUTED }}>
          {dateLabel}
        </td>
        <td className="py-3 pr-2">
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
          {group.scheduleLabel && group.scheduleLabel !== "Sin horario" ? (
            <p className="mt-0.5 text-[11px]" style={{ color: MUTED }}>
              Horario: {group.scheduleLabel}
            </p>
          ) : null}
        </td>
        <td className="py-3 tabular-nums font-medium">{formatCount(group.publicationsCount)}</td>
        <td className="py-3 tabular-nums font-medium">{formatCount(group.subscriptionsCount)}</td>
        <td className="py-3 text-right">
          {canExpand ? (
            <ExpandModelsButton
              expanded={expanded}
              label={
                expanded
                  ? `Ocultar modelos del turno de ${group.dropperName}`
                  : `Expandir modelos del turno de ${group.dropperName}`
              }
              withText
              onClick={(event) => {
                event.stopPropagation();
                onToggle();
              }}
            />
          ) : null}
        </td>
      </tr>
      {expanded && canExpand ? (
        <tr className="border-b" style={{ borderColor: ROW_LINE }}>
          <td colSpan={7} className="pb-4 pt-1">
            <ModelBreakdownPanel models={group.models} />
          </td>
        </tr>
      ) : null}
    </Fragment>
  );
}

function ExpandModelsButton({
  expanded,
  label,
  withText = false,
  onClick,
}: {
  expanded: boolean;
  label: string;
  withText?: boolean;
  onClick: (event: React.MouseEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-expanded={expanded}
      aria-label={label}
      className="inline-flex cursor-pointer items-center gap-1 whitespace-nowrap rounded-lg px-2.5 py-1 text-xs font-medium transition-colors"
      style={{
        background: expanded ? INK : "var(--report-control)",
        color: expanded ? "var(--report-canvas)" : INK,
        border: `1px solid ${expanded ? INK : LINE}`,
      }}
    >
      {withText ? <span>{expanded ? "Ocultar" : "Modelos"}</span> : null}
      <ChevronDown
        className="h-3.5 w-3.5 shrink-0 transition-transform duration-150"
        style={{ transform: expanded ? "rotate(180deg)" : "rotate(0deg)" }}
        aria-hidden="true"
      />
    </button>
  );
}

function ModelBreakdownPanel({ models }: { models: DropperModelBreakdown[] }) {
  return (
    <div
      className="rounded-xl px-4 py-2.5"
      style={{ background: "var(--report-control)", border: `1px solid ${LINE}` }}
    >
      <table className="w-full table-fixed border-collapse text-left">
        <thead>
          <tr className="text-[12px] font-normal" style={{ color: MUTED }}>
            <th className="w-[56%] pb-2 font-normal">Modelo</th>
            <th className="w-[22%] pb-2 font-normal">Publicaciones</th>
            <th className="w-[22%] pb-2 font-normal">Suscripciones</th>
          </tr>
        </thead>
        <tbody>
          {models.map((model, index) => (
            <tr
              key={model.id}
              className={index < models.length - 1 ? "border-b text-[13px]" : "text-[13px]"}
              style={{ borderColor: LINE, color: INK }}
            >
              <td className="py-2 pr-3 truncate font-medium">{model.accountName}</td>
              <td className="py-2 tabular-nums">{formatCount(model.publicationsCount)}</td>
              <td className="py-2 tabular-nums font-medium">{formatCount(model.subscriptionsCount)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function formatPlural(count: number, singular: string, plural: string): string {
  return `${formatCount(count)} ${count === 1 ? singular : plural}`;
}

function formatAvgPerShift(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? formatCount(rounded) : rounded.toFixed(1);
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
