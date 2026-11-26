import { useEffect, useState } from "react";
import { Bar, BarChart, CartesianGrid, Cell, Pie, PieChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { errorMessage, type EarningsBasis } from "../api";
import {
  CARD,
  DONUT,
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
import { loadCreatorReport, type CreatorReport, type ReportRangeId } from "./data";
import { formatPercent, formatReportMoney, formatTrend, moneyAxis } from "./format";

export function CreatorReports({ range, basis, timeZone }: { range: ReportRangeId; basis: EarningsBasis; timeZone: string }) {
  const [report, setReport] = useState<CreatorReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    loadCreatorReport(range, basis, timeZone)
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
  return <CreatorReportView report={report} />;
}

export function CreatorReportView({ report }: { report: CreatorReport }) {
  const hasTrend = report.trends.some((point) => point.amount > 0);
  const max = Math.max(0, ...report.trends.map((point) => point.amount));
  const axis = moneyAxis(max);
  const breakdownTotal = report.messagesAmount + report.otherAmount;
  const slices = [
    { name: "Messages", amount: report.messagesAmount, fill: DONUT },
    { name: "Other", amount: report.otherAmount, fill: INK },
  ];

  return (
    <div className="flex flex-col gap-7">
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <KpiCard label="Total Earnings" value={formatReportMoney(report.total)} trend={formatTrend(report.totalChange)} />
        <KpiCard label="Subscriptions" value={formatReportMoney(report.subscriptions)} trend={formatTrend(report.subscriptionsChange)} />
        <KpiCard label="Messages" value={formatReportMoney(report.messages)} trend={formatTrend(report.messagesChange)} />
        <KpiCard label="Tips" value={formatReportMoney(report.tips)} trend={formatTrend(report.tipsChange)} />
      </div>

      <Hairline />

      <div className="grid grid-cols-1 items-stretch gap-6 lg:grid-cols-12">
        <Surface title="Earning Trends" className="lg:col-span-7">
          {hasTrend ? (
            <div className="mt-4 h-[250px] w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={report.trends} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                  <CartesianGrid vertical={false} stroke={TRACK} />
                  <XAxis dataKey="label" axisLine={false} tickLine={false} minTickGap={20} tick={{ fontSize: 12, fill: MUTED }} />
                  <YAxis
                    ticks={axis.ticks}
                    domain={[0, axis.top > 0 ? axis.top : 1]}
                    axisLine={false}
                    tickLine={false}
                    width={56}
                    tick={{ fontSize: 12, fill: MUTED }}
                    tickFormatter={(value: number) => formatReportMoney(value)}
                  />
                  <Tooltip content={<MoneyTooltip />} cursor={false} />
                  <Bar dataKey="amount" name="Earnings" fill={INK} barSize={12} radius={[3, 3, 0, 0]} isAnimationActive={false} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          ) : (
            <EmptyNote>No hay ingresos en este rango.</EmptyNote>
          )}
        </Surface>

        <Surface title="Earnings breakdown" className="lg:col-span-5">
          <div className="mt-2 flex flex-wrap items-center gap-6">
            <div className="relative h-[200px] w-[200px] shrink-0">
              {breakdownTotal > 0 ? (
                <ResponsiveContainer width="100%" height="100%">
                  <PieChart>
                    <Pie
                      data={slices}
                      dataKey="amount"
                      nameKey="name"
                      innerRadius={58}
                      outerRadius={78}
                      startAngle={90}
                      endAngle={-270}
                      stroke="none"
                      isAnimationActive={false}
                    >
                      {slices.map((slice) => (
                        <Cell key={slice.name} fill={slice.fill} />
                      ))}
                    </Pie>
                  </PieChart>
                </ResponsiveContainer>
              ) : (
                <div
                  className="absolute top-1/2 left-1/2 h-[156px] w-[156px] -translate-x-1/2 -translate-y-1/2 rounded-full"
                  style={{ border: `20px solid ${DONUT}`, background: CARD }}
                />
              )}
              <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
                <span className="text-[22px] font-medium tabular-nums" style={{ color: INK }}>
                  {formatReportMoney(report.total)}
                </span>
              </div>
            </div>
            <ul className="flex flex-col gap-3 text-sm" style={{ color: INK }}>
              <li className="flex items-center gap-2">
                <Dot color={DONUT} />
                <span>Messages {formatPercent(share(report.messagesAmount, breakdownTotal))}</span>
              </li>
              <li className="flex items-center gap-2">
                <Dot color={INK} />
                <span>Other {formatPercent(share(report.otherAmount, breakdownTotal))}</span>
              </li>
            </ul>
          </div>
        </Surface>
      </div>

      <Hairline />

      <section>
        <h2 className="text-[15px] font-medium" style={{ color: INK }}>Statistics</h2>
        {report.pages.length === 0 ? (
          <EmptyNote>No hay creadores sincronizados.</EmptyNote>
        ) : (
          <div className="mt-4 overflow-x-auto">
            <table className="w-full min-w-[720px] table-fixed border-collapse text-left">
              <thead>
                <tr className="text-[13px] font-normal" style={{ color: MUTED }}>
                  <th className="w-[30%] pb-3 font-normal">Pages</th>
                  <th className="w-[24%] pb-3 font-normal">Message earnings</th>
                  <th className="w-[24%] pb-3 font-normal">Total earnings</th>
                  <th className="pb-3 font-normal">Engagement ratio</th>
                </tr>
              </thead>
              <tbody>
                {report.pages.map((page) => (
                  <tr key={page.id} className="border-b text-sm" style={{ borderColor: ROW_LINE, color: INK }}>
                    <td className="py-3">
                      <span className="flex items-center gap-3">
                        <PersonAvatar name={page.name} url={page.avatarUrl} size={30} />
                        {page.name}
                      </span>
                    </td>
                    <td className="py-3 tabular-nums">{formatReportMoney(page.messageEarnings)}</td>
                    <td className="py-3 tabular-nums">{formatReportMoney(page.totalEarnings)}</td>
                    <td className="py-3 tabular-nums"><PendingCell /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}

function share(part: number, total: number): number {
  if (total <= 0) return 0;
  return (part / total) * 100;
}

function Dot({ color }: { color: string }) {
  return <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: color }} aria-hidden="true" />;
}
