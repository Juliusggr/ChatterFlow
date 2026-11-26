export type ShiftPreset = {
  start: string;
  end: string;
  label: string;
  group: "Droppers" | "Chatters";
};

export const DROPPER_SHIFTS: ShiftPreset[] = [
  { start: "09:00", end: "17:00", label: "9:00 a. m. – 5:00 p. m.", group: "Droppers" },
  { start: "18:00", end: "02:00", label: "6:00 p. m. – 2:00 a. m.", group: "Droppers" },
];

export const CHATTER_SHIFTS: ShiftPreset[] = [
  { start: "08:00", end: "14:00", label: "8:00 a. m. – 2:00 p. m.", group: "Chatters" },
  { start: "14:00", end: "21:00", label: "2:00 p. m. – 9:00 p. m.", group: "Chatters" },
  { start: "21:00", end: "04:00", label: "9:00 p. m. – 4:00 a. m.", group: "Chatters" },
];

const PRESETS = [...DROPPER_SHIFTS, ...CHATTER_SHIFTS];

export function shiftsForRole(role: string): ShiftPreset[] | null {
  if (role === "dropper") return DROPPER_SHIFTS;
  if (role === "chatter") return CHATTER_SHIFTS;
  if (role === "owner" || role === "admin") return PRESETS;
  return null;
}

export function shiftLabel(start: string, end: string): string | null {
  const match = PRESETS.find((shift) => shift.start === start.slice(0, 5) && shift.end === end.slice(0, 5));
  return match?.label ?? null;
}
