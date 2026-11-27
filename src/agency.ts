import type { EarningsBasis, FanCounts } from "./api";
import type { ScheduleDay, Weekday } from "./types";
import { WEEKDAYS } from "./types";
import { supabase, usernameToEmail } from "./supabase";

export type AgencyRole = "owner" | "admin" | "chatter" | "dropper";

export type AgencyProfile = {
  id: string;
  username: string;
  full_name: string;
  role: AgencyRole;
  is_active: boolean;
};

export type AgencyEmployee = {
  id: string;
  username: string;
  name: string;
  role: AgencyRole;
  is_active: boolean;
  is_clocked_in: boolean;
  last_clock_in: string | null;
  days: ScheduleDay[];
  modelIds: string[];
};

export type ActiveShift = {
  id: string;
  userId: string;
  name: string;
  role: string;
  clockIn: string;
};

export type EmployeeSaleTotal = {
  employeeId: string;
  name: string;
  amount: number;
};

export type DashboardChannelPoint = {
  label: string;
  subs: number;
  tips: number;
  posts: number;
  ppv: number;
};

export type DashboardSnapshot = {
  revenue: PeriodRevenue;
  shifts: ShiftSummary[];
  clockedIn: ActiveShift[];
  onShift: boolean;
  salesByBasis: Record<EarningsBasis, EmployeeSaleTotal[]>;
  channelsByBasis: Record<EarningsBasis, DashboardChannelPoint[]>;
  fans: FanCounts | null;
};

const WEEKDAY_BY_INDEX: Weekday[] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const PROFILE_CACHE_TTL_MS = 60_000;

let cachedProfile: AgencyProfile | null = null;
let cachedProfileUserId: string | null = null;
let cachedProfileAt = 0;
let inFlightProfile: Promise<AgencyProfile | null> | null = null;

supabase.auth.onAuthStateChange((event, session) => {
  const nextId = session?.user?.id ?? null;
  if (event === "SIGNED_OUT" || nextId !== cachedProfileUserId) {
    cachedProfile = null;
    cachedProfileUserId = null;
    cachedProfileAt = 0;
    inFlightProfile = null;
  }
});

export function isManager(role: AgencyRole | null | undefined): boolean {
  return role === "owner" || role === "admin";
}

export function modelExternalKey(model: { partition_id: string }): string {
  return `partition:${model.partition_id}`;
}

export function roleLabel(role: AgencyRole): string {
  return role.charAt(0).toUpperCase() + role.slice(1);
}

export async function signInWithUsername(username: string, password: string): Promise<AgencyProfile> {
  const { error } = await supabase.auth.signInWithPassword({
    email: usernameToEmail(username),
    password,
  });
  if (error) {
    throw new Error("Usuario o contraseña incorrectos");
  }
  const profile = await loadMyProfile(true);
  if (!profile?.is_active) {
    await supabase.auth.signOut();
    throw new Error("Esta cuenta está desactivada");
  }
  return profile;
}

export async function signOut(): Promise<void> {
  cachedProfile = null;
  cachedProfileUserId = null;
  cachedProfileAt = 0;
  const { error } = await supabase.auth.signOut();
  if (error) throw error;
}

export async function loadMyProfile(force = false): Promise<AgencyProfile | null> {
  const { data: sessionData, error: sessionError } = await supabase.auth.getSession();
  if (sessionError) throw sessionError;
  const userId = sessionData.session?.user?.id ?? null;
  if (!userId) {
    cachedProfile = null;
    cachedProfileUserId = null;
    cachedProfileAt = 0;
    return null;
  }
  if (
    !force &&
    cachedProfile &&
    cachedProfileUserId === userId &&
    Date.now() - cachedProfileAt < PROFILE_CACHE_TTL_MS
  ) {
    return cachedProfile;
  }
  if (!force && inFlightProfile) {
    return inFlightProfile;
  }
  const request = (async () => {
    const { data, error } = await supabase
      .from("profiles")
      .select("id,username,full_name,role,is_active")
      .eq("id", userId)
      .maybeSingle();
    if (error) throw error;
    const resolved = (data as AgencyProfile | null) ?? null;
    cachedProfile = resolved;
    cachedProfileUserId = userId;
    cachedProfileAt = Date.now();
    return resolved;
  })();
  inFlightProfile = request;
  try {
    return await request;
  } finally {
    if (inFlightProfile === request) {
      inFlightProfile = null;
    }
  }
}

export async function listEmployees(): Promise<AgencyEmployee[]> {
  const [
    { data: profiles, error },
    { data: schedules, error: scheduleError },
    { data: shifts, error: shiftError },
    { data: assignments, error: assignmentError },
  ] = await Promise.all([
    supabase.from("profiles").select("id,username,full_name,role,is_active").order("full_name"),
    supabase.from("schedules").select("user_id,day_of_week,start_time,end_time"),
    supabase.from("shifts").select("user_id,clock_in").eq("status", "active"),
    supabase.from("model_assignments").select("user_id,model_id"),
  ]);
  if (error) throw error;
  if (scheduleError) throw scheduleError;
  if (shiftError) throw shiftError;
  if (assignmentError) throw assignmentError;

  const activeByUser = new Map((shifts ?? []).map((row) => [row.user_id as string, row.clock_in as string]));
  const daysByUser = new Map<string, ScheduleDay[]>();
  for (const row of schedules ?? []) {
    const userId = row.user_id as string;
    const days = daysByUser.get(userId) ?? blankDays();
    const index = Number(row.day_of_week);
    if (index >= 0 && index < days.length) {
      days[index] = {
        day: WEEKDAY_BY_INDEX[index],
        enabled: true,
        start: String(row.start_time).slice(0, 5),
        end: String(row.end_time).slice(0, 5),
      };
    }
    daysByUser.set(userId, days);
  }
  const modelsByUser = new Map<string, string[]>();
  for (const row of assignments ?? []) {
    const modelId = row.model_id as string;
    if (!modelId) continue;
    const list = modelsByUser.get(row.user_id as string) ?? [];
    list.push(modelId);
    modelsByUser.set(row.user_id as string, list);
  }

  return (profiles ?? []).map((profile) => ({
    id: profile.id as string,
    username: profile.username as string,
    name: (profile.full_name as string) || (profile.username as string),
    role: profile.role as AgencyRole,
    is_active: Boolean(profile.is_active),
    is_clocked_in: activeByUser.has(profile.id as string),
    last_clock_in: activeByUser.get(profile.id as string) ?? null,
    days: daysByUser.get(profile.id as string) ?? blankDays(),
    modelIds: modelsByUser.get(profile.id as string) ?? [],
  }));
}

function blankDays(): ScheduleDay[] {
  return WEEKDAYS.map(({ day }) => ({ day, enabled: false, start: "", end: "" }));
}

export async function saveEmployee(input: {
  id?: string;
  username: string;
  password?: string;
  fullName: string;
  role: AgencyRole;
  isActive: boolean;
  days: ScheduleDay[];
  modelIds: string[];
}): Promise<void> {
  const body = input.id
    ? {
        action: "update",
        userId: input.id,
        fullName: input.fullName,
        role: input.role,
        isActive: input.isActive,
      }
    : {
        action: "create",
        username: input.username,
        password: input.password ?? "",
        fullName: input.fullName,
        role: input.role,
      };
  const { data, error } = await supabase.functions.invoke("manage-employees", { body });
  if (error) {
    throw new Error(await functionError(error, data));
  }
  const payload = data as { id?: string; error?: string } | null;
  if (payload?.error) throw new Error(payload.error);
  const userId = input.id ?? payload?.id;
  if (!userId) throw new Error("No se recibió el id del empleado");
  await Promise.all([
    replaceSchedule(userId, input.days),
    input.role === "chatter" || input.role === "dropper"
      ? replaceAssignments(userId, input.modelIds)
      : Promise.resolve(),
  ]);
  if (userId === cachedProfileUserId) {
    cachedProfileAt = 0;
  }
}

async function functionError(error: { message: string; context?: Response }, data: unknown): Promise<string> {
  const payload = data as { error?: string } | null;
  if (payload?.error) return payload.error;
  if (error.context) {
    try {
      const body = (await error.context.json()) as { error?: string };
      if (body.error) return body.error;
    } catch {
      // The response body was not JSON.
    }
  }
  return error.message || "No se pudo guardar el empleado";
}

async function replaceSchedule(userId: string, days: ScheduleDay[]): Promise<void> {
  const { error: deleteError } = await supabase.from("schedules").delete().eq("user_id", userId);
  if (deleteError) throw deleteError;
  const rows = days
    .map((day, index) => ({ day, index }))
    .filter(({ day }) => day.enabled && day.start && day.end)
    .map(({ day, index }) => ({
      user_id: userId,
      day_of_week: index,
      start_time: day.start,
      end_time: day.end,
    }));
  if (rows.length === 0) return;
  const { error } = await supabase.from("schedules").insert(rows);
  if (error) throw error;
}

async function replaceAssignments(userId: string, modelIds: string[]): Promise<void> {
  const { error: deleteError } = await supabase.from("model_assignments").delete().eq("user_id", userId);
  if (deleteError) throw deleteError;
  const uniqueIds = [...new Set(modelIds.filter(Boolean))];
  if (uniqueIds.length === 0) return;
  const me = await loadMyProfile();
  const rows = uniqueIds.map((modelId) => ({
    user_id: userId,
    model_id: modelId,
    assigned_by: me?.id ?? null,
  }));
  const { error: insertError } = await supabase.from("model_assignments").insert(rows);
  if (insertError) throw insertError;
}

export async function deactivateEmployee(id: string): Promise<void> {
  const current = await listEmployees();
  const employee = current.find((item) => item.id === id);
  if (!employee) return;
  await saveEmployee({
    id,
    username: employee.username,
    fullName: employee.name,
    role: employee.role,
    isActive: false,
    days: employee.days,
    modelIds: employee.modelIds,
  });
}

export async function clockIn(): Promise<void> {
  const { error } = await supabase.rpc("clock_in");
  if (error) throw new Error(error.message);
}

export async function clockOut(): Promise<void> {
  const { error } = await supabase.rpc("clock_out");
  if (error) throw new Error(error.message);
}

export async function deleteShift(id: string): Promise<void> {
  const { data, error } = await supabase.from("shifts").delete().eq("id", id).select("id");
  if (error) throw new Error(error.message);
  if (!data?.length) throw new Error("No se pudo eliminar el turno");
}

export async function listActiveShifts(): Promise<ActiveShift[]> {
  const { data, error } = await supabase
    .from("shifts")
    .select("id,user_id,clock_in,profiles(full_name,username,role)")
    .eq("status", "active")
    .order("clock_in", { ascending: true });
  if (error) throw error;
  return (data ?? []).map((row) => {
    const profile = firstRelation(row.profiles);
    return {
      id: row.id as string,
      userId: row.user_id as string,
      name: profile?.full_name || profile?.username || "Empleado",
      role: profile?.role ? roleLabel(profile.role as AgencyRole) : "",
      clockIn: row.clock_in as string,
    };
  });
}

export function subscribeActiveShifts(onChange: () => void): () => void {
  const channel = supabase
    .channel(`shifts-live-${Math.random().toString(36).slice(2, 8)}`)
    .on("postgres_changes", { event: "*", schema: "public", table: "shifts" }, () => onChange())
    .subscribe();
  return () => {
    void supabase.removeChannel(channel);
  };
}

export function subscribeDropperTables(onChange: () => void): () => void {
  const channel = supabase
    .channel(`dropper-live-${Math.random().toString(36).slice(2, 8)}`)
    .on("postgres_changes", { event: "*", schema: "public", table: "shifts" }, () => onChange())
    .on("postgres_changes", { event: "*", schema: "public", table: "publications" }, () => onChange())
    .on("postgres_changes", { event: "*", schema: "public", table: "fan_arrivals" }, () => onChange())
    .on("postgres_changes", { event: "*", schema: "public", table: "dropper_shift_reports" }, () => onChange())
    .subscribe();
  return () => {
    void supabase.removeChannel(channel);
  };
}

export type ShiftSummary = {
  id: string;
  employeeName: string;
  startTime: string;
  endTime: string | null;
};

async function listMyShiftsForProfile(profile: AgencyProfile | null, from: string, to: string): Promise<ShiftSummary[]> {
  if (!profile) return [];
  const { data: shifts, error } = await supabase
    .from("shifts")
    .select("id,clock_in,clock_out")
    .eq("user_id", profile.id)
    .gte("clock_in", from)
    .lt("clock_in", to)
    .order("clock_in", { ascending: false });
  if (error) throw error;
  const name = profile.full_name || profile.username;
  return (shifts ?? []).map((shift) => ({
    id: shift.id as string,
    employeeName: name,
    startTime: shift.clock_in as string,
    endTime: (shift.clock_out as string | null) ?? null,
  }));
}

type SaleRow = { employeeId: string; name: string; role: string | null; gross: number; net: number; at: number };

async function listSales(from: string, to: string): Promise<SaleRow[]> {
  const { data, error } = await supabase
    .from("sales")
    .select("employee_id,amount_gross,amount_net,recorded_at,profiles(full_name,username,role)")
    .gte("recorded_at", from)
    .lt("recorded_at", to);
  if (error) throw error;
  return (data ?? []).map((row) => {
    const profile = firstRelation(row.profiles);
    return {
      employeeId: row.employee_id as string,
      name: profile?.full_name || profile?.username || "Empleado",
      role: typeof profile?.role === "string" ? profile.role : null,
      gross: Number(row.amount_gross),
      net: Number(row.amount_net),
      at: new Date(row.recorded_at as string).getTime(),
    };
  });
}

export type RevenueBar = {
  label: string;
  gross: number;
  net: number;
};

export type PeriodRevenue = {
  gross: number;
  net: number;
  dayCount: number;
  averageDaily: number;
  bars: RevenueBar[];
};

function computeSalesMetrics(
  sales: SaleRow[],
  from: string,
  to: string,
): {
  revenue: PeriodRevenue;
  salesByBasis: Record<EarningsBasis, EmployeeSaleTotal[]>;
} {
  const days = eachLocalDay(from, to);
  const buckets = new Map(days.map((day) => [day.key, { gross: 0, net: 0 }]));
  const totalsByEmployee = new Map<string, { name: string; gross: number; net: number }>();
  let gross = 0;
  let net = 0;

  for (const sale of sales) {
    const saleGross = Number.isFinite(sale.gross) ? sale.gross : 0;
    const saleNet = Number.isFinite(sale.net) ? sale.net : 0;
    gross += saleGross;
    net += saleNet;
    const bucket = buckets.get(localDayKey(sale.at));
    if (bucket) {
      bucket.gross += saleGross;
      bucket.net += saleNet;
    }
    if (sale.role === "chatter") {
      const emp = totalsByEmployee.get(sale.employeeId) ?? { name: sale.name, gross: 0, net: 0 };
      emp.gross += saleGross;
      emp.net += saleNet;
      totalsByEmployee.set(sale.employeeId, emp);
    }
  }

  const dayCount = Math.max(days.length, 1);
  const netRanking: EmployeeSaleTotal[] = [];
  const grossRanking: EmployeeSaleTotal[] = [];
  for (const [employeeId, emp] of totalsByEmployee.entries()) {
    netRanking.push({ employeeId, name: emp.name, amount: emp.net });
    grossRanking.push({ employeeId, name: emp.name, amount: emp.gross });
  }
  netRanking.sort((a, b) => b.amount - a.amount);
  grossRanking.sort((a, b) => b.amount - a.amount);

  return {
    revenue: {
      gross,
      net,
      dayCount,
      averageDaily: gross / dayCount,
      bars: days.map((day) => {
        const bucket = buckets.get(day.key) ?? { gross: 0, net: 0 };
        return { label: day.label, gross: bucket.gross, net: bucket.net };
      }),
    },
    salesByBasis: {
      net: netRanking,
      gross: grossRanking,
    },
  };
}

export async function getFanCounts(): Promise<FanCounts> {
  const { data, error } = await supabase
    .from("models")
    .select("id,name,handle,fans_count")
    .order("name", { ascending: true })
    .order("handle", { ascending: true })
    .order("id", { ascending: true });
  if (error) throw error;
  const accounts = (data ?? []).map((row) => ({
    modelId: row.id as string,
    name: String(row.name || row.handle || "Cuenta").trim() || "Cuenta",
    fans: row.fans_count == null ? null : Number(row.fans_count),
  }));
  const known = accounts.filter((account) => account.fans != null);
  const total = known.length > 0 ? known.reduce((sum, account) => sum + (account.fans ?? 0), 0) : null;
  return { total, accounts };
}

async function dashboardChannelsBoth(
  from: string,
  to: string,
  hourly: boolean,
): Promise<Record<EarningsBasis, DashboardChannelPoint[]>> {
  const slots = hourly ? eachLocalHour(from) : eachLocalDay(from, to);
  const netMap = new Map<string, DashboardChannelPoint>(
    slots.map((slot) => [slot.key, { label: slot.label, subs: 0, tips: 0, posts: 0, ppv: 0 }]),
  );
  const grossMap = new Map<string, DashboardChannelPoint>(
    slots.map((slot) => [slot.key, { label: slot.label, subs: 0, tips: 0, posts: 0, ppv: 0 }]),
  );

  const { data, error } = await supabase
    .from("model_earnings")
    .select("type,gross_amount,net_amount,created_at")
    .gte("created_at", from)
    .lt("created_at", to);
  if (error) throw error;

  for (const row of data ?? []) {
    const ms = new Date(row.created_at as string).getTime();
    if (!Number.isFinite(ms)) continue;
    const key = hourly ? localHourKey(ms) : localDayKey(ms);
    const netBucket = netMap.get(key);
    const grossBucket = grossMap.get(key);
    if (!netBucket || !grossBucket) continue;
    const g = Number(row.gross_amount);
    const n = Number(row.net_amount);
    const gClean = Number.isFinite(g) ? g : 0;
    const nClean = Number.isFinite(n) ? n : 0;
    switch (row.type) {
      case "sub":
        netBucket.subs += nClean;
        grossBucket.subs += gClean;
        break;
      case "tip":
        netBucket.tips += nClean;
        grossBucket.tips += gClean;
        break;
      case "post":
        netBucket.posts += nClean;
        grossBucket.posts += gClean;
        break;
      case "message":
        netBucket.ppv += nClean;
        grossBucket.ppv += gClean;
        break;
      default:
        break;
    }
  }

  return {
    net: slots.map((slot) => netMap.get(slot.key)!),
    gross: slots.map((slot) => grossMap.get(slot.key)!),
  };
}

/**
 * Fetches the entire Dashboard dataset in a single parallel wave (4 queries total, zero duplicates),
 * returning both Net and Gross series so toggling Neto/Bruto in the UI is instantaneous (0ms).
 */
export async function loadDashboardSnapshot(range: DashboardRange): Promise<DashboardSnapshot> {
  const profile = await loadMyProfile();
  const { from, to } = rangeBounds(range);
  const hourly = range === "today" || range === "yesterday";

  const [sales, shifts, activeShifts, channelsByBasis, fans] = await Promise.all([
    listSales(from, to),
    listMyShiftsForProfile(profile, from, to),
    listActiveShifts(),
    dashboardChannelsBoth(from, to, hourly).catch(
      (): Record<EarningsBasis, DashboardChannelPoint[]> => ({ net: [], gross: [] }),
    ),
    getFanCounts().catch(() => null),
  ]);

  const { revenue, salesByBasis } = computeSalesMetrics(sales, from, to);
  const onShift = Boolean(profile && activeShifts.some((item) => item.userId === profile.id));

  return {
    revenue,
    shifts,
    clockedIn: activeShifts,
    onShift,
    salesByBasis,
    channelsByBasis,
    fans,
  };
}

function eachLocalHour(fromIso: string): { key: string; label: string }[] {
  const start = new Date(fromIso);
  start.setMinutes(0, 0, 0);
  const slots: { key: string; label: string }[] = [];
  for (let hour = 0; hour < 24; hour += 1) {
    const cursor = new Date(start.getTime() + hour * 3_600_000);
    slots.push({
      key: localHourKey(cursor.getTime()),
      label: `${String(cursor.getHours()).padStart(2, "0")}:00`,
    });
  }
  return slots;
}

function localHourKey(ms: number): string {
  const date = new Date(ms);
  return `${localDayKey(ms)}-${String(date.getHours()).padStart(2, "0")}`;
}

function eachLocalDay(fromIso: string, toIso: string): { key: string; label: string }[] {
  const end = new Date(toIso).getTime();
  const cursor = new Date(fromIso);
  cursor.setHours(0, 0, 0, 0);
  const days: { key: string; label: string }[] = [];
  while (cursor.getTime() < end && days.length < 4000) {
    const key = localDayKey(cursor.getTime());
    days.push({ key, label: dayLabel(key) });
    cursor.setDate(cursor.getDate() + 1);
  }
  if (days.length === 0) {
    const key = localDayKey(new Date(fromIso).getTime());
    days.push({ key, label: dayLabel(key) });
  }
  return days;
}

function localDayKey(ms: number): string {
  const date = new Date(ms);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

function dayLabel(key: string): string {
  const [year, month, day] = key.split("-").map(Number);
  return new Intl.DateTimeFormat("es", { day: "numeric", month: "short" }).format(
    new Date(year, (month || 1) - 1, day || 1),
  );
}

export async function recordSale(input: {
  externalKey: string;
  fanPlatformId: string;
  amountGross: number;
  amountNet: number;
  category: "ppv" | "tip";
  recordedAt: string;
  sourceId: string;
}): Promise<void> {
  const timeZone =
    typeof localStorage !== "undefined"
      ? localStorage.getItem("cf-report-tz") || "America/Caracas"
      : "America/Caracas";
  const { error } = await supabase.rpc("record_sale", {
    p_external_key: input.externalKey,
    p_fan_platform_id: input.fanPlatformId,
    p_amount_gross: input.amountGross,
    p_amount_net: input.amountNet,
    p_category: input.category,
    p_recorded_at: input.recordedAt,
    p_source_id: input.sourceId,
    p_timezone: timeZone,
  });
  if (
    error &&
    !/turno activo|fuera del horario del turno|no asignado|not allowed/i.test(error.message)
  ) {
    throw new Error(error.message);
  }
}

export type FanSpendBreakdown = {
  ppvSpent: number;
  tipSpent: number;
  subSpent: number;
  otherSpent: number;
  totalSpendGross: number;
  totalEarnedNet: number;
  lastSpendAt: string | null;
  highestSpend: number;
};

export async function loadFanSpend(
  modelId: string,
  fanPlatformId: string,
): Promise<FanSpendBreakdown | null> {
  const { data, error } = await supabase.rpc("fan_spend_breakdown", {
    p_model_id: modelId,
    p_of_fan_id: fanPlatformId,
  });
  if (error) {
    if (/could not find the function|PGRST202/i.test(error.message)) return null;
    throw error;
  }
  const row = Array.isArray(data) ? data[0] : data;
  if (!row || typeof row !== "object") return null;
  const record = row as Record<string, unknown>;
  const number = (key: string) => {
    const value = Number(record[key]);
    return Number.isFinite(value) ? value : 0;
  };
  return {
    ppvSpent: number("ppv_spent"),
    tipSpent: number("tip_spent"),
    subSpent: number("sub_spent"),
    otherSpent: number("other_spent"),
    totalSpendGross: number("total_spend_gross"),
    totalEarnedNet: number("total_earned_net"),
    lastSpendAt: typeof record.last_spend_at === "string" ? record.last_spend_at : null,
    highestSpend: number("highest_spend"),
  };
}

export async function loadFanNote(modelId: string, fanPlatformId: string): Promise<string> {
  const { data, error } = await supabase
    .from("fan_notes")
    .select("notes")
    .eq("model_id", modelId)
    .eq("fan_platform_id", fanPlatformId)
    .maybeSingle();
  if (error) throw error;
  return (data?.notes as string | undefined) ?? "";
}

export async function saveFanNote(input: {
  modelId: string;
  fanPlatformId: string;
  fanUsername: string;
  notes: string;
}): Promise<void> {
  const profile = await loadMyProfile();
  if (!profile) throw new Error("Inicia sesión para guardar notas");
  const { error } = await supabase.from("fan_notes").upsert(
    {
      model_id: input.modelId,
      fan_platform_id: input.fanPlatformId,
      fan_username: input.fanUsername,
      notes: input.notes,
      updated_by: profile.id,
      updated_at: new Date().toISOString(),
    },
    { onConflict: "model_id,fan_platform_id" },
  );
  if (error) throw error;
}

export function subscribeFanNote(
  modelId: string,
  fanPlatformId: string,
  onNotes: (notes: string) => void,
): () => void {
  const channel = supabase
    .channel(`fan-note-${modelId}-${fanPlatformId}`)
    .on(
      "postgres_changes",
      { event: "*", schema: "public", table: "fan_notes", filter: `model_id=eq.${modelId}` },
      (payload) => {
        const next = payload.new as { fan_platform_id?: string; notes?: string } | null;
        if (next?.fan_platform_id === fanPlatformId) onNotes(next.notes ?? "");
      },
    )
    .subscribe();
  return () => {
    void supabase.removeChannel(channel);
  };
}

export type DashboardRange = "yesterday" | "today" | "thisWeek" | "thisMonth";

function rangeBounds(range: DashboardRange): { from: string; to: string } {
  const start = new Date();
  start.setHours(0, 0, 0, 0);
  if (range === "yesterday") start.setDate(start.getDate() - 1);
  if (range === "thisWeek") start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
  if (range === "thisMonth") start.setDate(1);
  const end = new Date(start);
  if (range === "yesterday") end.setDate(end.getDate() + 1);
  else if (range === "today") end.setDate(end.getDate() + 1);
  else end.setTime(Date.now() + 1000);
  return { from: start.toISOString(), to: end.toISOString() };
}

function firstRelation<T>(value: T | T[] | null): T | null {
  if (Array.isArray(value)) return value[0] ?? null;
  return value ?? null;
}
