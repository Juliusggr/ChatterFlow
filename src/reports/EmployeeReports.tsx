import { useEffect, useState } from "react";
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { errorMessage, type EarningsBasis } from "../api";
import {
  CARD,
  EmptyNote,
  Hairline,
  INK,
  KpiCard,
  MUTED,
  MoneyTooltip,
  PendingCell,
  PersonAvatar,
  ROW_LINE,
  ReportError,
  ReportLoading,
  Surface,
  TRACK,
} from "./chrome";
import { loadEmployeeReport, type EmployeeReport, type ReportRangeId } from "./data";
import { formatReportMoney, moneyAxis } from "./format";

const SERIES = ["var(--report-series-1)", "var(--report-series-2)", "var(--report-series-3)"] as const;
const RANKS = [
  { background: "var(--report-series-1)", color: "var(--report-rank-ink)" },
  { background: "var(--report-series-2)", color: "var(--report-rank-ink)" },
  { background: "var(--report-rank-3)", color: "var(--report-rank-3-ink)" },
] as const;

export function EmployeeReports({ range, basis, timeZone }: { range: ReportRangeId; basis: EarningsBasis; timeZone: string }) {
  const [report, setReport] = useState<EmployeeReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    loadEmployeeReport(range, basis, timeZone)
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
  }, [range, basis, timeZone]);

  if (loading) return <ReportLoading />;
  if (error || !report) return <ReportError message={error ?? "No se pudo cargar el reporte"} />;
  return <EmployeeReportView report={report} />;
}

export function EmployeeReportView({ report }: { report: EmployeeReport }) {
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
        <Surface title="Employee comparison" className="lg:col-span-7">
          {report.series.length === 0 ? (
            <EmptyNote>No hay ventas en este rango.</EmptyNote>
          ) : (
            <div className="mt-4 h-[250px] w-full">
              <ResponsiveContainer width="100%" height="100%">
                <LineChart data={chartRows} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                  <CartesianGrid vertical={false} stroke={TRACK} />
                  <XAxis
                    dataKey="label"
                    axisLine={false}
                    tickLine={false}
                    minTickGap={24}
                    tick={{ fontSize: 12, fill: MUTED }}
                  />
                  <YAxis
                    ticks={axis.ticks}
                    domain={[0, axis.top > 0 ? axis.top : 1]}
                    axisLine={false}
                    tickLine={false}
                    width={56}
                    tick={{ fontSize: 12, fill: MUTED }}
                    tickFormatter={(value: number) => formatReportMoney(value)}
                  />
                  <Tooltip content={<MoneyTooltip />} cursor={{ stroke: TRACK }} />
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
        <ol className="flex h-full flex-col justify-evenly gap-3 py-2 lg:col-span-5" aria-label="Top employees">
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

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <KpiCard label="Total employee sales" value={formatReportMoney(report.totalSales)} />
        <KpiCard label="Unlock rate" value="—" pending />
        <KpiCard label="Average reply time" value="—" pending />
        <KpiCard label="Scheduled hours" value="—" pending />
      </div>

      <Hairline />
      {report.rows.length === 0 ? (
        <EmptyNote>No hay empleados en este rango.</EmptyNote>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] table-fixed border-collapse text-left">
            <thead>
              <tr className="text-[13px] font-normal" style={{ color: MUTED }}>
                <th className="w-[40%] pb-3 font-normal">Employee</th>
                <th className="w-[30%] pb-3 font-normal">Sales</th>
                <th className="pb-3 font-normal">Unlock ratio</th>
              </tr>
            </thead>
            <tbody>
              {report.rows.map((row) => (
                <tr key={row.id} className="border-b text-sm" style={{ borderColor: ROW_LINE, color: INK }}>
                  <td className="py-3">
                    <span className="flex items-center gap-3">
                      <PersonAvatar name={row.name} size={30} />
                      {row.name}
                    </span>
                  </td>
                  <td className="py-3 tabular-nums">{formatReportMoney(row.sales)}</td>
                  <td className="py-3 tabular-nums"><PendingCell /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
