import type { EarningsBasis, FanCounts } from "./api";
import { addZonedDays, DEFAULT_TIME_ZONE, zonedDate, zonedParts } from "./reports/timeZone";
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

/** Hard start cutoffs (America/Caracas UTC-4): no dropper data <= 2026-11-25, no chatter data <= 2026-11-26. */
export const DROPPER_MIN_CARACAS_ISO = "2026-11-26T04:00:00.000Z";
export const CHATTER_MIN_CARACAS_ISO = "2026-11-27T04:00:00.000Z";
const CHATTER_MIN_CARACAS_MS = Date.parse(CHATTER_MIN_CARACAS_ISO);

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

export async function setEmployeeActive(id: string, isActive: boolean): Promise<void> {
  const { error } = await supabase.rpc("set_employee_active", {
    p_user_id: id,
    p_is_active: isActive,
  });
  if (!error) {
    if (id === cachedProfileUserId) cachedProfileAt = 0;
    return;
  }
  if (!isMissingRpcError(error.message)) {
    throw new Error(error.message);
  }
  const current = await listEmployees();
  const employee = current.find((item) => item.id === id);
  if (!employee) return;
  await saveEmployee({
    id,
    username: employee.username,
    fullName: employee.name,
    role: employee.role,
    isActive,
    days: employee.days,
    modelIds: employee.modelIds,
  });
}

export async function deactivateEmployee(id: string): Promise<void> {
  await setEmployeeActive(id, false);
}

export async function deleteEmployee(id: string): Promise<void> {
  const { error } = await supabase.rpc("delete_employee_account", {
    p_user_id: id,
  });
  if (!error) {
    if (id === cachedProfileUserId) cachedProfileAt = 0;
    return;
  }
  if (!isMissingRpcError(error.message)) {
    throw new Error(error.message);
  }
  const { data, error: fnErr } = await supabase.functions.invoke("manage-employees", {
    body: { action: "delete", userId: id },
  });
  if (!fnErr) {
    const payload = data as { error?: string } | null;
    if (!payload?.error) return;
    if (!/acción desconocida/i.test(payload.error)) {
      throw new Error(payload.error);
    }
  } else {
    const msg = await functionError(fnErr, data);
    if (!/acción desconocida/i.test(msg)) {
      throw new Error(msg);
    }
  }
  const { error: delProfileErr } = await supabase.from("profiles").delete().eq("id", id);
  if (delProfileErr) throw new Error(delProfileErr.message);
}

function isMissingRpcError(message: string): boolean {
  return /could not find the function|pgrst202|404/i.test(message);
}

function isUuidString(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.trim());
}

export type LocalDismissedShift = {
  dropperUserId: string;
  shiftKey: string;
  windowStart: string | null;
  windowEnd: string | null;
};

const DISMISSED_SHIFTS_STORAGE_KEY = "cf-dismissed-shifts-v1";

export function readLocalDismissedShifts(): LocalDismissedShift[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(DISMISSED_SHIFTS_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (item): item is LocalDismissedShift =>
        Boolean(item) &&
        typeof (item as LocalDismissedShift).dropperUserId === "string" &&
        typeof (item as LocalDismissedShift).shiftKey === "string",
    );
  } catch {
    return [];
  }
}

function rememberLocalDismissedShift(entry: LocalDismissedShift): void {
  if (typeof localStorage === "undefined") return;
  try {
    const current = readLocalDismissedShifts().filter(
      (item) => !(item.dropperUserId === entry.dropperUserId && item.shiftKey === entry.shiftKey),
    );
    current.push(entry);
    localStorage.setItem(DISMISSED_SHIFTS_STORAGE_KEY, JSON.stringify(current.slice(-500)));
  } catch {
    // Ignore storage quota errors.
  }
}

export async function clockIn(): Promise<void> {
  const { error } = await supabase.rpc("clock_in");
  if (error) throw new Error(error.message);
}

export async function clockOut(): Promise<void> {
  const { error } = await supabase.rpc("clock_out");
  if (error) throw new Error(error.message);
}

export async function deleteShift(
  id: string,
  opts?: {
    dropperUserId?: string;
    shiftStart?: string;
    shiftEnd?: string | null;
  },
): Promise<void> {
  const cleanId = id.trim();
  if (!cleanId) throw new Error("Falta el turno a eliminar");

  const { error: rpcError } = await supabase.rpc("owner_delete_shift", {
    p_shift_id: cleanId,
    p_dropper_user_id: opts?.dropperUserId ?? null,
    p_shift_start: opts?.shiftStart ?? null,
    p_shift_end: opts?.shiftEnd ?? null,
  });

  if (!rpcError) {
    if (opts?.dropperUserId) {
      rememberLocalDismissedShift({
        dropperUserId: opts.dropperUserId,
        shiftKey: cleanId,
        windowStart: opts.shiftStart ?? null,
        windowEnd: opts.shiftEnd ?? new Date().toISOString(),
      });
    }
    return;
  }

  if (!isMissingRpcError(rpcError.message)) {
    throw new Error(rpcError.message);
  }

  if (isUuidString(cleanId)) {
    await supabase.from("dropper_shift_reports").delete().eq("shift_id", cleanId);
    const { data, error } = await supabase.from("shifts").delete().eq("id", cleanId).select("id");
    if (error) throw new Error(error.message);
    if (!data?.length && !opts?.dropperUserId) {
      throw new Error("No se pudo eliminar el turno");
    }
  } else if (!opts?.dropperUserId) {
    throw new Error("No se pudo eliminar el turno");
  }

  if (opts?.dropperUserId) {
    rememberLocalDismissedShift({
      dropperUserId: opts.dropperUserId,
      shiftKey: cleanId,
      windowStart: opts.shiftStart ?? null,
      windowEnd: opts.shiftEnd ?? new Date().toISOString(),
    });
  }
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

export function subscribeMyAssignments(onChange: () => void): () => void {
  const channel = supabase
    .channel(`assignments-live-${Math.random().toString(36).slice(2, 8)}`)
    .on(
      "postgres_changes",
      { event: "*", schema: "public", table: "model_assignments" },
      () => onChange(),
    )
    .subscribe();
  return () => {
    void supabase.removeChannel(channel);
  };
}

export function subscribeActiveShifts(onChange: () => void): () => void {
  const channel = supabase
    .channel(`shifts-live-${Math.random().toString(36).slice(2, 8)}`)
    .on("postgres_changes", { event: "*", schema: "public", table: "shifts" }, () => onChange())
    .on("postgres_changes", { event: "*", schema: "public", table: "sales" }, () => onChange())
    .on("postgres_changes", { event: "*", schema: "public", table: "model_transactions" }, () => onChange())
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
    .on("postgres_changes", { event: "*", schema: "public", table: "dismissed_dropper_shifts" }, () => onChange())
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
  const roleCutoff =
    profile.role === "chatter"
      ? CHATTER_MIN_CARACAS_ISO
      : profile.role === "dropper"
        ? DROPPER_MIN_CARACAS_ISO
        : null;
  const effectiveFrom =
    roleCutoff && new Date(from).getTime() < new Date(roleCutoff).getTime() ? roleCutoff : from;
  if (new Date(effectiveFrom).getTime() >= new Date(to).getTime()) return [];
  const { data: shifts, error } = await supabase
    .from("shifts")
    .select("id,clock_in,clock_out")
    .eq("user_id", profile.id)
    .gte("clock_in", effectiveFrom)
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

type SaleRow = {
  modelId: string;
  fanId: string;
  employeeId: string;
  name: string;
  role: string | null;
  category: string;
  gross: number;
  net: number;
  at: number;
};

function round2Money(value: number): number {
  return Math.round((Number.isFinite(value) ? value : 0) * 100) / 100;
}

function getDashboardTimeZone(): string {
  if (typeof localStorage !== "undefined") {
    const saved = localStorage.getItem("cf-report-tz");
    if (saved && saved.trim()) return saved.trim();
  }
  return DEFAULT_TIME_ZONE;
}

function normalizeFanId(raw: unknown): string {
  return String(raw ?? "")
    .trim()
    .replace(/^[uU]/, "");
}

async function listSales(from: string, to: string, timeZone: string): Promise<SaleRow[]> {
  const chatterMinMs = zonedDate(2026, 11, 27, 0, 0, 0, timeZone).getTime();
  const minIso = new Date(chatterMinMs).toISOString();
  const effectiveFrom = new Date(from).getTime() < chatterMinMs ? minIso : from;
  if (new Date(effectiveFrom).getTime() >= new Date(to).getTime()) return [];
  const { data, error } = await supabase
    .from("sales")
    .select("model_id,fan_platform_id,employee_id,category,amount_gross,amount_net,recorded_at,profiles(full_name,username,role)")
    .gte("recorded_at", effectiveFrom)
    .lt("recorded_at", to);
  if (error) throw error;
  return (data ?? [])
    .map((row) => {
      const profile = firstRelation(row.profiles);
      return {
        modelId: String(row.model_id ?? ""),
        fanId: normalizeFanId(row.fan_platform_id),
        employeeId: (row.employee_id as string | null) ?? "",
        name: profile?.full_name || profile?.username || "Empleado",
        role: typeof profile?.role === "string" ? profile.role : null,
        category: typeof row.category === "string" ? row.category : "",
        gross: Number(row.amount_gross),
        net: Number(row.amount_net),
        at: new Date(row.recorded_at as string).getTime(),
      };
    })
    .filter((row) => Number.isFinite(row.at) && row.at >= chatterMinMs);
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
  timeZone: string,
): {
  revenue: PeriodRevenue;
  salesByBasis: Record<EarningsBasis, EmployeeSaleTotal[]>;
} {
  const days = eachZonedDay(from, to, timeZone);
  const buckets = new Map(days.map((day) => [day.key, { gross: 0, net: 0 }]));
  const totalsByEmployee = new Map<string, { name: string; gross: number; net: number }>();
  let gross = 0;
  let net = 0;

  for (const sale of sales) {
    const saleGross = Number.isFinite(sale.gross) ? sale.gross : 0;
    const saleNet = Number.isFinite(sale.net) ? sale.net : 0;
    gross = round2Money(gross + saleGross);
    net = round2Money(net + saleNet);
    const bucket = buckets.get(zonedDayKey(sale.at, timeZone));
    if (bucket) {
      bucket.gross = round2Money(bucket.gross + saleGross);
      bucket.net = round2Money(bucket.net + saleNet);
    }
    if (sale.role === "chatter" && sale.employeeId) {
      const emp = totalsByEmployee.get(sale.employeeId) ?? { name: sale.name, gross: 0, net: 0 };
      emp.gross = round2Money(emp.gross + saleGross);
      emp.net = round2Money(emp.net + saleNet);
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
      averageDaily: round2Money(gross / dayCount),
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

type AgencyDay = { gross: number; net: number };

type RealTxMatchEntry = {
  modelId: string;
  fanId: string;
  type: string;
  gross: number;
  at: number;
  used: boolean;
};

async function fetchRangeModelTransactions(from: string, to: string) {
  const pageSize = 1000;
  const all: {
    model_id: unknown;
    of_transaction_id: unknown;
    of_fan_id: unknown;
    type: unknown;
    gross_amount: unknown;
    net_amount: unknown;
    created_at: unknown;
  }[] = [];
  for (let page = 0; page < 20; page += 1) {
    const { data, error } = await supabase
      .from("model_transactions")
      .select("model_id,of_transaction_id,of_fan_id,type,gross_amount,net_amount,created_at")
      .gte("created_at", from)
      .lt("created_at", to)
      .order("created_at", { ascending: true })
      .order("id", { ascending: true })
      .range(page * pageSize, page * pageSize + pageSize - 1);
    if (error) throw error;
    const batch = data ?? [];
    all.push(...batch);
    if (batch.length < pageSize) break;
  }
  return all;
}

async function dashboardChannelsAndRevenue(
  from: string,
  to: string,
  hourly: boolean,
  timeZone: string,
  sales: SaleRow[],
): Promise<{
  channels: Record<EarningsBasis, DashboardChannelPoint[]>;
  revenue: PeriodRevenue;
}> {
  const slots = hourly ? eachZonedHour(from, timeZone) : eachZonedDay(from, to, timeZone);
  const days = eachZonedDay(from, to, timeZone);
  const netMap = new Map<string, DashboardChannelPoint>(
    slots.map((slot) => [slot.key, { label: slot.label, subs: 0, tips: 0, posts: 0, ppv: 0 }]),
  );
  const grossMap = new Map<string, DashboardChannelPoint>(
    slots.map((slot) => [slot.key, { label: slot.label, subs: 0, tips: 0, posts: 0, ppv: 0 }]),
  );
  const daily = new Map<string, AgencyDay>(
    days.map((day) => [day.key, { gross: 0, net: 0 }]),
  );

  const nowParts = zonedParts(new Date(), timeZone);
  const todayKey = `${nowParts.year}-${String(nowParts.month).padStart(2, "0")}-${String(nowParts.day).padStart(2, "0")}`;
  const yestParts = zonedParts(addZonedDays(nowParts, -1, timeZone), timeZone);
  const yesterdayKey = `${yestParts.year}-${String(yestParts.month).padStart(2, "0")}-${String(yestParts.day).padStart(2, "0")}`;

  const rawTxRows = await fetchRangeModelTransactions(from, to).catch(() => null);

  const realTxPool: RealTxMatchEntry[] = [];
  const realModelTypeDays = new Set<string>();
  const chartFallbackModelTypeDays = new Set<string>();

  const applyTxToBuckets = (type: string, gClean: number, nClean: number, ms: number) => {
    if (gClean <= 0 && nClean <= 0) return;
    const dayKey = zonedDayKey(ms, timeZone);
    const day = daily.get(dayKey);
    if (day) {
      day.gross = round2Money(day.gross + gClean);
      day.net = round2Money(day.net + nClean);
    }
    const slotKey = hourly ? zonedHourKey(ms, timeZone) : dayKey;
    const netBucket = netMap.get(slotKey);
    const grossBucket = grossMap.get(slotKey);
    if (!netBucket || !grossBucket) return;
    switch (type) {
      case "sub":
        netBucket.subs = round2Money(netBucket.subs + nClean);
        grossBucket.subs = round2Money(grossBucket.subs + gClean);
        break;
      case "tip":
        netBucket.tips = round2Money(netBucket.tips + nClean);
        grossBucket.tips = round2Money(grossBucket.tips + gClean);
        break;
      case "post":
        netBucket.posts = round2Money(netBucket.posts + nClean);
        grossBucket.posts = round2Money(grossBucket.posts + gClean);
        break;
      case "message":
        netBucket.ppv = round2Money(netBucket.ppv + nClean);
        grossBucket.ppv = round2Money(grossBucket.ppv + gClean);
        break;
      default:
        break;
    }
  };

  if (rawTxRows) {
    const chartCandidates: {
      modelId: string;
      type: string;
      gross: number;
      net: number;
      ms: number;
      dayKey: string;
    }[] = [];

    for (const row of rawTxRows) {
      const txId = String(row.of_transaction_id ?? "");
      const modelId = String(row.model_id ?? "");
      const type = String(row.type ?? "");
      const ms = new Date(row.created_at as string).getTime();
      if (!Number.isFinite(ms)) continue;
      const g = Number(row.gross_amount);
      const n = Number(row.net_amount);
      const gClean = Number.isFinite(g) ? round2Money(g) : 0;
      const nClean = Number.isFinite(n) ? round2Money(n) : 0;
      const dayKey = zonedDayKey(ms, timeZone);

      if (txId.startsWith("chart:")) {
        chartCandidates.push({ modelId, type, gross: gClean, net: nClean, ms, dayKey });
        continue;
      }

      realModelTypeDays.add(`${modelId}:${type}:${dayKey}`);
      applyTxToBuckets(type, gClean, nClean, ms);
      if (type === "message" || type === "tip") {
        realTxPool.push({
          modelId,
          fanId: normalizeFanId(row.of_fan_id),
          type,
          gross: gClean,
          at: ms,
          used: false,
        });
      }
    }

    // Only use 'chart:' rows on older historical days where zero real transactions exist for that model+type+day.
    // Never use UTC-bucketed 'chart:' rows for today or yesterday, which require exact 00:00-23:59 local timestamps.
    for (const chart of chartCandidates) {
      if (hourly || chart.dayKey === todayKey || chart.dayKey === yesterdayKey) continue;
      const key = `${chart.modelId}:${chart.type}:${chart.dayKey}`;
      if (realModelTypeDays.has(key)) continue;
      chartFallbackModelTypeDays.add(key);
      applyTxToBuckets(chart.type, chart.gross, chart.net, chart.ms);
    }
  }

  // Fold in any recorded chatter sale not yet mirrored in model_transactions (1-to-1 per-transaction dedup).
  for (const sale of sales) {
    const saleType = sale.category === "tip" ? "tip" : sale.category === "ppv" ? "message" : null;
    if (!saleType) continue;
    const saleGross = Number.isFinite(sale.gross) ? round2Money(sale.gross) : 0;
    const saleNet = Number.isFinite(sale.net) ? round2Money(sale.net) : 0;
    if (saleGross <= 0) continue;

    const matchedTx = realTxPool.find(
      (tx) =>
        !tx.used &&
        tx.modelId === sale.modelId &&
        tx.type === saleType &&
        Math.abs(tx.gross - saleGross) < 0.02 &&
        (!sale.fanId || !tx.fanId || tx.fanId === sale.fanId) &&
        Math.abs(tx.at - sale.at) <= 30 * 60_000,
    );
    if (matchedTx) {
      matchedTx.used = true;
      continue;
    }

    const dayKey = zonedDayKey(sale.at, timeZone);
    if (chartFallbackModelTypeDays.has(`${sale.modelId}:${saleType}:${dayKey}`)) {
      continue;
    }
    applyTxToBuckets(saleType, saleGross, saleNet, sale.at);
  }

  let totalGross = 0;
  let totalNet = 0;
  const bars = days.map((day) => {
    const bucket = daily.get(day.key) ?? { gross: 0, net: 0 };
    totalGross = round2Money(totalGross + bucket.gross);
    totalNet = round2Money(totalNet + bucket.net);
    return { label: day.label, gross: bucket.gross, net: bucket.net };
  });
  const dayCount = Math.max(days.length, 1);

  return {
    channels: {
      net: slots.map((slot) => netMap.get(slot.key)!),
      gross: slots.map((slot) => grossMap.get(slot.key)!),
    },
    revenue: {
      gross: totalGross,
      net: totalNet,
      dayCount,
      averageDaily: round2Money(totalGross / dayCount),
      bars,
    },
  };
}

/**
 * Fetches the entire Dashboard dataset in a single parallel wave (exact 00:00–23:59 day bounds),
 * returning both Net and Gross series so toggling Neto/Bruto in the UI is instantaneous (0ms).
 */
export async function loadDashboardSnapshot(
  range: DashboardRange,
  timeZone = getDashboardTimeZone(),
): Promise<DashboardSnapshot> {
  const profile = await loadMyProfile();
  const { from, to } = rangeBounds(range, timeZone);
  const hourly = range === "today" || range === "yesterday";

  const [sales, shifts, activeShifts, fans] = await Promise.all([
    listSales(from, to, timeZone),
    listMyShiftsForProfile(profile, from, to),
    listActiveShifts(),
    getFanCounts().catch(() => null),
  ]);

  const [{ salesByBasis }, { channels, revenue }] = await Promise.all([
    Promise.resolve(computeSalesMetrics(sales, from, to, timeZone)),
    dashboardChannelsAndRevenue(from, to, hourly, timeZone, sales),
  ]);
  const onShift = Boolean(profile && activeShifts.some((item) => item.userId === profile.id));

  return {
    revenue,
    shifts,
    clockedIn: activeShifts,
    onShift,
    salesByBasis,
    channelsByBasis: channels,
    fans,
  };
}

function eachZonedHour(fromIso: string, timeZone: string): { key: string; label: string }[] {
  const parts = zonedParts(new Date(fromIso), timeZone);
  const dayKey = `${parts.year}-${String(parts.month).padStart(2, "0")}-${String(parts.day).padStart(2, "0")}`;
  const slots: { key: string; label: string }[] = [];
  for (let hour = 0; hour < 24; hour += 1) {
    const hh = String(hour).padStart(2, "0");
    slots.push({
      key: `${dayKey}-${hh}`,
      label: `${hh}:00`,
    });
  }
  return slots;
}

function zonedHourKey(ms: number, timeZone: string): string {
  const parts = zonedParts(new Date(ms), timeZone);
  const month = String(parts.month).padStart(2, "0");
  const day = String(parts.day).padStart(2, "0");
  const hour = String(parts.hour).padStart(2, "0");
  return `${parts.year}-${month}-${day}-${hour}`;
}

function eachZonedDay(fromIso: string, toIso: string, timeZone: string): { key: string; label: string }[] {
  const endMs = new Date(toIso).getTime();
  let cursorParts = zonedParts(new Date(fromIso), timeZone);
  let cursorDate = zonedDate(cursorParts.year, cursorParts.month, cursorParts.day, 0, 0, 0, timeZone);
  const days: { key: string; label: string }[] = [];
  while (cursorDate.getTime() < endMs && days.length < 4000) {
    const key = `${cursorParts.year}-${String(cursorParts.month).padStart(2, "0")}-${String(cursorParts.day).padStart(2, "0")}`;
    days.push({ key, label: dayLabel(key) });
    cursorDate = addZonedDays(cursorParts, 1, timeZone);
    cursorParts = zonedParts(cursorDate, timeZone);
  }
  if (days.length === 0) {
    const key = zonedDayKey(new Date(fromIso).getTime(), timeZone);
    days.push({ key, label: dayLabel(key) });
  }
  return days;
}

function zonedDayKey(ms: number, timeZone: string): string {
  const parts = zonedParts(new Date(ms), timeZone);
  const month = String(parts.month).padStart(2, "0");
  const day = String(parts.day).padStart(2, "0");
  return `${parts.year}-${month}-${day}`;
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
  const recordedMs = Date.parse(input.recordedAt);
  if (Number.isFinite(recordedMs) && recordedMs < CHATTER_MIN_CARACAS_MS) {
    return;
  }
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

function rangeBounds(range: DashboardRange, timeZone: string): { from: string; to: string } {
  const nowParts = zonedParts(new Date(), timeZone);
  const todayStart = zonedDate(nowParts.year, nowParts.month, nowParts.day, 0, 0, 0, timeZone);
  const tomorrowStart = addZonedDays(nowParts, 1, timeZone);

  if (range === "today") {
    // Full current day from 00:00:00.000 through 23:59:59.999 (< tomorrow 00:00:00.000)
    return { from: todayStart.toISOString(), to: tomorrowStart.toISOString() };
  }
  if (range === "yesterday") {
    const yesterdayStart = addZonedDays(nowParts, -1, timeZone);
    return { from: yesterdayStart.toISOString(), to: todayStart.toISOString() };
  }
  if (range === "thisWeek") {
    const weekdayOffset =
      (new Date(Date.UTC(nowParts.year, nowParts.month - 1, nowParts.day)).getUTCDay() + 6) % 7;
    const weekStart = addZonedDays(nowParts, -weekdayOffset, timeZone);
    return { from: weekStart.toISOString(), to: tomorrowStart.toISOString() };
  }
  const monthStart = zonedDate(nowParts.year, nowParts.month, 1, 0, 0, 0, timeZone);
  return { from: monthStart.toISOString(), to: tomorrowStart.toISOString() };
}

function firstRelation<T>(value: T | T[] | null): T | null {
  if (Array.isArray(value)) return value[0] ?? null;
  return value ?? null;
}
