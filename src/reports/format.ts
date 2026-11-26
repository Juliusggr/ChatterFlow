export function roundMoney(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.round(value * 100) / 100;
}

export function formatCount(value: number): string {
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 }).format(Number.isFinite(value) ? value : 0);
}

export function formatReportMoney(value: number): string {
  const cents = Math.round(value * 100);
  const whole = cents % 100 === 0;
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: whole ? 0 : 2,
    maximumFractionDigits: whole ? 0 : 2,
  }).format(Number.isFinite(value) ? value : 0);
}

export function formatPercent(value: number): string {
  if (!Number.isFinite(value)) return "0%";
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? `${rounded}%` : `${rounded.toFixed(1)}%`;
}

export function formatTrend(change: number | null): string | null {
  if (change === null || !Number.isFinite(change)) return null;
  const rounded = Math.round(change);
  const arrow = rounded < 0 ? "↘" : "↗";
  return `${arrow} ${Math.abs(rounded)}%`;
}

export function moneyAxis(max: number): { ticks: number[]; top: number } {
  if (!Number.isFinite(max) || max <= 0) return { ticks: [0], top: 0 };
  const rough = max / 2;
  const exponent = Math.floor(Math.log10(rough));
  const base = 10 ** exponent;
  const fraction = rough / base;
  const nice = fraction <= 1 ? 1 : fraction <= 2 ? 2 : fraction <= 5 ? 5 : 10;
  const step = nice * base;
  const top = Math.ceil((max - 1e-9) / step) * step;
  const mid = roundMoney(top / 2);
  const ticks = mid > 0 && mid < top ? [0, mid, top] : [0, top];
  return { ticks, top };
}

export function prettyBucketLabel(label: string, labels: string[]): string {
  const month = /^(\d{4})-(\d{2})$/.exec(label);
  if (month) {
    const years = new Set(labels.map((item) => /^(\d{4})-/.exec(item)?.[1]).filter((year) => year));
    const date = new Date(Number(month[1]), Number(month[2]) - 1, 1);
    const name = new Intl.DateTimeFormat("en-US", { month: "short" }).format(date);
    return years.size > 1 ? `${name} ${month[1]}` : name;
  }
  const day = /^(\d{2})\/(\d{2})$/.exec(label);
  if (day) {
    const date = new Date(2000, Number(day[2]) - 1, Number(day[1]));
    if (Number.isNaN(date.getTime())) return label;
    return new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" }).format(date);
  }
  return label;
}
