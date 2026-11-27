export const DEFAULT_TIME_ZONE = "America/Caracas";

export const TIME_ZONES: { id: string; label: string }[] = [
  { id: "America/Caracas", label: "Venezuela" },
  { id: "America/Bogota", label: "Colombia" },
  { id: "America/Lima", label: "Perú" },
  { id: "America/Panama", label: "Panamá" },
  { id: "America/Mexico_City", label: "Ciudad de México" },
  { id: "America/New_York", label: "Nueva York" },
  { id: "America/Chicago", label: "Chicago" },
  { id: "America/Los_Angeles", label: "Los Ángeles" },
  { id: "Europe/Madrid", label: "España" },
  { id: "UTC", label: "UTC" },
];

export type ZonedParts = {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
};

export function zonedParts(date: Date, timeZone: string): ZonedParts {
  const formatted = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hourCycle: "h23",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).formatToParts(date);
  const bag = Object.fromEntries(formatted.filter((part) => part.type !== "literal").map((part) => [part.type, part.value]));
  const hour = bag.hour === "24" ? 0 : Number(bag.hour);
  return {
    year: Number(bag.year),
    month: Number(bag.month),
    day: Number(bag.day),
    hour,
    minute: Number(bag.minute),
    second: Number(bag.second),
  };
}

export function zonedDate(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute: number,
  second: number,
  timeZone: string,
): Date {
  const target = Date.UTC(year, month - 1, day, hour, minute, second);
  let utc = target;
  for (let pass = 0; pass < 3; pass += 1) {
    const shown = zonedParts(new Date(utc), timeZone);
    const shownUtc = Date.UTC(shown.year, shown.month - 1, shown.day, shown.hour, shown.minute, shown.second);
    const delta = shownUtc - target;
    if (delta === 0) break;
    utc -= delta;
  }
  return new Date(utc);
}

export function timeZoneOffsetSeconds(timeZone: string, at = new Date()): number {
  const parts = zonedParts(at, timeZone);
  const asUtc = Date.UTC(parts.year, parts.month - 1, parts.day, parts.hour, parts.minute, parts.second);
  return Math.round((asUtc - at.getTime()) / 1000);
}

export function addZonedDays(parts: ZonedParts, days: number, timeZone: string): Date {
  const noon = zonedDate(parts.year, parts.month, parts.day, 12, 0, 0, timeZone);
  const shifted = zonedParts(new Date(noon.getTime() + days * 86_400_000), timeZone);
  return zonedDate(shifted.year, shifted.month, shifted.day, 0, 0, 0, timeZone);
}
