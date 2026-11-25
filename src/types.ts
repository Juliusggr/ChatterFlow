export type EmployeeRole = "Owner" | "Admin" | "Chatter" | "Dropper";

export type Weekday = "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun";

export type ScheduleDay = {
  day: Weekday;
  enabled: boolean;
  start: string;
  end: string;
};

export type Employee = {
  id: number;
  name: string;
  role: EmployeeRole;
  schedule_json: unknown;
  is_clocked_in: boolean;
  last_clock_in: string | null;
};

export type Model = {
  id: number;
  of_user_id: string | null;
  username: string;
  display_name: string;
  avatar_url: string | null;
  current_balance: number;
  pending_balance: number;
  last_synced_at: string | null;
  partition_id: string;
  window_open: boolean;
};

export const ROLES: EmployeeRole[] = ["Owner", "Admin", "Chatter", "Dropper"];

export const WEEKDAYS: { day: Weekday; label: string }[] = [
  { day: "mon", label: "Lunes" },
  { day: "tue", label: "Martes" },
  { day: "wed", label: "Miércoles" },
  { day: "thu", label: "Jueves" },
  { day: "fri", label: "Viernes" },
  { day: "sat", label: "Sábado" },
  { day: "sun", label: "Domingo" },
];

export function blankSchedule(): ScheduleDay[] {
  return WEEKDAYS.map(({ day }) => ({ day, enabled: false, start: "", end: "" }));
}

export function scheduleFromJson(value: unknown): ScheduleDay[] {
  const days = blankSchedule();
  if (!value || typeof value !== "object" || !("days" in value)) {
    return days;
  }
  const stored = (value as { days?: unknown }).days;
  if (!Array.isArray(stored)) {
    return days;
  }
  for (const entry of stored) {
    if (!entry || typeof entry !== "object") {
      continue;
    }
    const record = entry as Record<string, unknown>;
    const index = days.findIndex((day) => day.day === record.day);
    if (index < 0) {
      continue;
    }
    days[index] = {
      day: days[index].day,
      enabled: record.enabled === true,
      start: typeof record.start === "string" ? record.start : "",
      end: typeof record.end === "string" ? record.end : "",
    };
  }
  return days;
}

export function summarizeSchedule(days: ScheduleDay[]): string {
  const active = days.filter((day) => day.enabled && day.start && day.end);
  if (active.length === 0) {
    return "No data";
  }
  return active
    .map((day) => {
      const label = WEEKDAYS.find((item) => item.day === day.day)?.label ?? day.day;
      return `${label} ${day.start}–${day.end}`;
    })
    .join(", ");
}

export function modelLabel(model: Model): string {
  const name = model.display_name.trim() || model.username.trim();
  return name || "Unknown";
}

export function initials(label: string): string {
  if (label === "Unknown") {
    return "";
  }
  const parts = label.split(/\s+/).filter(Boolean);
  if (parts.length === 1) {
    return parts[0].slice(0, 2).toUpperCase();
  }
  return `${parts[0][0] ?? ""}${parts[1][0] ?? ""}`.toUpperCase();
}

const AVATAR_COLORS = ["#1d4ed8", "#c2410c", "#334155", "#1e3a8a", "#9a3412"];

export function avatarColor(id: number): string {
  return AVATAR_COLORS[Math.abs(id) % AVATAR_COLORS.length];
}

export function formatStamp(value: string | null): string {
  if (!value) {
    return "No data";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("es", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

export function formatShiftDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat("es", { dateStyle: "medium" }).format(date);
}

export function formatShiftTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat("es", { timeStyle: "short" }).format(date);
}

export function sameLocalDay(a: string, b: string): boolean {
  const left = new Date(a);
  const right = new Date(b);
  if (Number.isNaN(left.getTime()) || Number.isNaN(right.getTime())) return false;
  return (
    left.getFullYear() === right.getFullYear() &&
    left.getMonth() === right.getMonth() &&
    left.getDate() === right.getDate()
  );
}

export function formatCheckout(start: string, end: string | null): string {
  if (!end) return "Abierto";
  const time = formatShiftTime(end);
  return sameLocalDay(start, end) ? time : `${formatShiftDate(end)} · ${time}`;
}

export function formatMoney(value: number): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
  }).format(value);
}
