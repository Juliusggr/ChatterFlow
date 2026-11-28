import { useState } from "react";
import type { EarningsBasis } from "./api";
import { CreatorReports } from "./reports/CreatorReports";
import { DropperReports } from "./reports/DropperReports";
import { EmployeeReports } from "./reports/EmployeeReports";
import { REPORT_RANGES, type ReportRangeId } from "./reports/data";
import { DEFAULT_TIME_ZONE, TIME_ZONES } from "./reports/timeZone";

type ReportPage = "chatters" | "droppers" | "creators";

const PAGES: { id: ReportPage; label: string; title: string }[] = [
  { id: "chatters", label: "Chatters", title: "Employee reports" },
  { id: "droppers", label: "Droppers", title: "Dropper reports" },
  { id: "creators", label: "Creators", title: "Creator reports" },
];

export function AnalyticsView({ canDeleteShifts = false }: { canDeleteShifts?: boolean }) {
  const [page, setPage] = useState<ReportPage>("chatters");
  const [range, setRange] = useState<ReportRangeId>("today");
  const [timeZone, setTimeZone] = useState(() => localStorage.getItem("cf-report-tz") || DEFAULT_TIME_ZONE);
  const [basis, setBasis] = useState<EarningsBasis>("gross");
  const title = PAGES.find((item) => item.id === page)?.title ?? "Employee reports";

  return (
    <div className="cf-report min-h-full">
      <div className="mx-auto flex w-full max-w-6xl flex-col px-8 py-8">
        <header className="mb-6 flex flex-wrap items-center justify-between gap-4">
          <h1 className="text-[24px] font-semibold tracking-[-0.02em]">
            {title}
          </h1>
          <div className="flex flex-wrap items-center gap-3">
            <div className="flex rounded-[10px] p-1" style={{ background: "var(--report-control)" }} role="group" aria-label="Report">
              {PAGES.map((item) => {
                const active = page === item.id;
                return (
                  <button
                    key={item.id}
                    type="button"
                    aria-pressed={active}
                    className="rounded-lg px-3 py-1.5 text-[13px] font-medium"
                    style={{ background: active ? "var(--report-card)" : "transparent", color: "var(--report-ink)" }}
                    onClick={() => setPage(item.id)}
                  >
                    {item.label}
                  </button>
                );
              })}
            </div>
            <select
              aria-label="Zona horaria"
              className="rounded-lg px-2.5 py-1.5 text-[13px]"
              style={{ background: "var(--report-card)", border: "1px solid var(--report-line)", color: "var(--report-ink)" }}
              value={timeZone}
              onChange={(event) => {
                setTimeZone(event.target.value);
                localStorage.setItem("cf-report-tz", event.target.value);
              }}
            >
              {TIME_ZONES.map((zone) => (
                <option key={zone.id} value={zone.id}>{zone.label}</option>
              ))}
            </select>
            <select
              aria-label="Date range"
              className="rounded-lg px-2.5 py-1.5 text-[13px]"
              style={{ background: "var(--report-card)", border: "1px solid var(--report-line)", color: "var(--report-ink)" }}
              value={range}
              onChange={(event) => setRange(event.target.value as ReportRangeId)}
            >
              {REPORT_RANGES.map((item) => (
                <option key={item.id} value={item.id}>{item.label}</option>
              ))}
            </select>
            {page === "droppers" ? null : (
            <div className="flex rounded-lg p-0.5" style={{ background: "var(--report-pill)" }} role="group" aria-label="Earnings basis">
              {(["gross", "net"] as const).map((item) => {
                const active = basis === item;
                return (
                  <button
                    key={item}
                    type="button"
                    aria-pressed={active}
                    className="rounded-md px-2.5 py-1 text-[13px]"
                    style={{ background: active ? "var(--report-card)" : "transparent", color: "var(--report-ink)" }}
                    onClick={() => setBasis(item)}
                  >
                    {item === "gross" ? "Bruto" : "Neto"}
                  </button>
                );
              })}
            </div>
            )}
          </div>
        </header>
        {page === "chatters" ? <EmployeeReports range={range} basis={basis} timeZone={timeZone} /> : null}
        {page === "droppers" ? <DropperReports range={range} timeZone={timeZone} canDeleteShifts={canDeleteShifts} /> : null}
        {page === "creators" ? <CreatorReports range={range} basis={basis} timeZone={timeZone} /> : null}
      </div>
    </div>
  );
}
