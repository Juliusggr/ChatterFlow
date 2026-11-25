import type { Model, ScheduleDay, Weekday } from "./types";
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
  modelKeys: string[];
};

export type ActiveShift = {
  id: string;
  name: string;
  role: string;
  clockIn: string;
};

export type EmployeeSaleTotal = {
  employeeId: string;
  name: string;
  amount: number;
};

const WEEKDAY_BY_INDEX: Weekday[] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

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
  const profile = await loadMyProfile();
  if (!profile?.is_active) {
    await supabase.auth.signOut();
    throw new Error("Esta cuenta está desactivada");
  }
  return profile;
}

export async function signOut(): Promise<void> {
  const { error } = await supabase.auth.signOut();
  if (error) throw error;
}

export async function loadMyProfile(): Promise<AgencyProfile | null> {
  const { data: userData, error: userError } = await supabase.auth.getUser();
  if (userError) throw userError;
  if (!userData.user) return null;
  const { data, error } = await supabase
    .from("profiles")
    .select("id,username,full_name,role,is_active")
    .eq("id", userData.user.id)
    .maybeSingle();
  if (error) throw error;
  return (data as AgencyProfile | null) ?? null;
}

export async function syncAgencyModels(models: Model[]): Promise<void> {
  if (models.length === 0) return;
  const rows = models.map((model) => ({
    external_key: modelExternalKey(model),
    name: model.display_name || model.username || "Modelo",
    handle: model.username || "",
    avatar_url: model.avatar_url,
    session_data: {
      sqlite_id: model.id,
      partition_id: model.partition_id,
      of_user_id: model.of_user_id,
    },
  }));
  const { error } = await supabase.from("models").upsert(rows, { onConflict: "external_key" });
  if (error) throw error;
}

export async function assignedModelKeys(): Promise<Set<string>> {
  const profile = await loadMyProfile();
  if (!profile) return new Set();
  const { data: assignments, error } = await supabase
    .from("model_assignments")
    .select("model_id")
    .eq("user_id", profile.id);
  if (error) throw error;
  const ids = (assignments ?? []).map((row) => row.model_id as string);
  if (ids.length === 0) return new Set();
  const { data: models, error: modelError } = await supabase
    .from("models")
    .select("external_key")
    .in("id", ids);
  if (modelError) throw modelError;
  return new Set((models ?? []).map((row) => row.external_key as string));
}

async function modelIdByKey(externalKey: string): Promise<string | null> {
  const { data, error } = await supabase
    .from("models")
    .select("id")
    .eq("external_key", externalKey)
    .maybeSingle();
  if (error) throw error;
  return (data?.id as string | undefined) ?? null;
}

export async function listEmployees(): Promise<AgencyEmployee[]> {
  const [{ data: profiles, error }, { data: schedules, error: scheduleError }, { data: shifts, error: shiftError }, { data: assignments, error: assignmentError }, { data: models, error: modelError }] =
    await Promise.all([
      supabase.from("profiles").select("id,username,full_name,role,is_active").order("full_name"),
      supabase.from("schedules").select("user_id,day_of_week,start_time,end_time"),
      supabase.from("shifts").select("user_id,clock_in,status").eq("status", "active"),
      supabase.from("model_assignments").select("user_id,model_id"),
      supabase.from("models").select("id,external_key"),
    ]);
  if (error) throw error;
  if (scheduleError) throw scheduleError;
  if (shiftError) throw shiftError;
  if (assignmentError) throw assignmentError;
  if (modelError) throw modelError;

  const keysByModel = new Map((models ?? []).map((row) => [row.id as string, row.external_key as string]));
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
    const key = keysByModel.get(row.model_id as string);
    if (!key) continue;
    const list = modelsByUser.get(row.user_id as string) ?? [];
    list.push(key);
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
    modelKeys: modelsByUser.get(profile.id as string) ?? [],
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
  modelKeys: string[];
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
  await replaceSchedule(userId, input.days);
  if (input.role === "chatter" || input.role === "dropper") {
    await replaceAssignments(userId, input.modelKeys);
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

async function replaceAssignments(userId: string, modelKeys: string[]): Promise<void> {
  const { error: deleteError } = await supabase.from("model_assignments").delete().eq("user_id", userId);
  if (deleteError) throw deleteError;
  if (modelKeys.length === 0) return;
  const { data: models, error } = await supabase.from("models").select("id,external_key").in("external_key", modelKeys);
  if (error) throw error;
  const me = await loadMyProfile();
  const rows = (models ?? []).map((model) => ({
    user_id: userId,
    model_id: model.id as string,
    assigned_by: me?.id ?? null,
  }));
  if (rows.length === 0) return;
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
    modelKeys: employee.modelKeys,
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
    .select("id,clock_in,profiles(full_name,username,role)")
    .eq("status", "active")
    .order("clock_in", { ascending: true });
  if (error) throw error;
  return (data ?? []).map((row) => {
    const profile = firstRelation(row.profiles);
    return {
      id: row.id as string,
      name: profile?.full_name || profile?.username || "Empleado",
      role: profile?.role ? roleLabel(profile.role as AgencyRole) : "",
      clockIn: row.clock_in as string,
    };
  });
}

export function subscribeActiveShifts(onChange: () => void): () => void {
  const channel = supabase
    .channel("shifts-live")
    .on("postgres_changes", { event: "*", schema: "public", table: "shifts" }, () => onChange())
    .subscribe();
  return () => {
    void supabase.removeChannel(channel);
  };
}

export async function myShiftOpen(): Promise<boolean> {
  const profile = await loadMyProfile();
  if (!profile) return false;
  const { data, error } = await supabase
    .from("shifts")
    .select("id")
    .eq("user_id", profile.id)
    .eq("status", "active")
    .maybeSingle();
  if (error) throw error;
  return Boolean(data);
}

export type ShiftSummary = {
  id: string;
  employeeName: string;
  startTime: string;
  endTime: string | null;
};

export type CompletedShift = {
  id: string;
  employeeName: string;
  role: AgencyRole | null;
  startTime: string;
  endTime: string;
  amount: number | null;
};

export async function listMyShifts(range: DashboardRange): Promise<ShiftSummary[]> {
  const profile = await loadMyProfile();
  if (!profile) return [];
  const { from, to } = rangeBounds(range);
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

export async function salesByEmployee(range: DashboardRange, basis: "net" | "gross"): Promise<EmployeeSaleTotal[]> {
  const { from, to } = rangeBounds(range);
  const sales = await listSales(from, to);
  const totals = new Map<string, EmployeeSaleTotal>();
  for (const sale of sales) {
    if (sale.role !== "chatter") continue;
    const current = totals.get(sale.employeeId) ?? { employeeId: sale.employeeId, name: sale.name, amount: 0 };
    current.amount += basis === "net" ? sale.net : sale.gross;
    totals.set(sale.employeeId, current);
  }
  return [...totals.values()].sort((a, b) => b.amount - a.amount);
}

export function analyticsBounds(range: string): { from: string | null; to: string } {
  const now = new Date();
  const clean = range.trim().toLowerCase();
  const tomorrow = nextLocalMidnight(now);
  if (clean === "24h" || clean === "24" || clean === "1" || clean === "day" || clean === "today") {
    return { from: new Date(now.getTime() - 86_400_000).toISOString(), to: now.toISOString() };
  }
  const span = daySpan(clean);
  if (span != null) {
    const start = localMidnight(now);
    start.setDate(start.getDate() - (span - 1));
    return { from: start.toISOString(), to: tomorrow.toISOString() };
  }
  if (clean === "all" || clean === "historic" || clean === "historico" || clean === "all_time" || clean === "todo") {
    return { from: null, to: tomorrow.toISOString() };
  }
  const yearMatch = /^(?:year:)?(\d{4})$/.exec(clean);
  if (yearMatch && (clean.startsWith("year:") || clean.length === 4)) {
    const year = Number(yearMatch[1]);
    return {
      from: localDate(year, 1, 1).toISOString(),
      to: localDate(year + 1, 1, 1).toISOString(),
    };
  }
  const monthMatch = /^(?:month:)?(\d{4})-(\d{2})$/.exec(clean);
  if (monthMatch) {
    const year = Number(monthMatch[1]);
    const month = Number(monthMatch[2]);
    if (month >= 1 && month <= 12) {
      const nextYear = month === 12 ? year + 1 : year;
      const nextMonth = month === 12 ? 1 : month + 1;
      return {
        from: localDate(year, month, 1).toISOString(),
        to: localDate(nextYear, nextMonth, 1).toISOString(),
      };
    }
  }
  const start = localMidnight(now);
  start.setDate(start.getDate() - 6);
  return { from: start.toISOString(), to: tomorrow.toISOString() };
}

export async function listCompletedShifts(
  from: string | null,
  to: string,
  basis: "net" | "gross",
): Promise<CompletedShift[]> {
  let query = supabase
    .from("shifts")
    .select("id,user_id,clock_in,clock_out,profiles(full_name,username,role)")
    .not("clock_out", "is", null)
    .lt("clock_in", to)
    .order("clock_in", { ascending: false });
  if (from) query = query.gte("clock_in", from);
  const { data, error } = await query;
  if (error) throw error;
  const shifts = data ?? [];
  if (shifts.length === 0) return [];

  let fromMs = Number.POSITIVE_INFINITY;
  let toMs = new Date(to).getTime();
  for (const shift of shifts) {
    fromMs = Math.min(fromMs, new Date(shift.clock_in as string).getTime());
    toMs = Math.max(toMs, new Date(shift.clock_out as string).getTime());
  }
  const sales = await listSales(new Date(fromMs).toISOString(), new Date(toMs + 1).toISOString());

  return shifts.map((shift) => {
    const profile = firstRelation(shift.profiles);
    const rawRole = profile?.role;
    const role = isAgencyRole(rawRole) ? rawRole : null;
    const start = new Date(shift.clock_in as string).getTime();
    const end = new Date(shift.clock_out as string).getTime();
    const amount =
      role === "chatter"
        ? sales
            .filter((sale) => sale.employeeId === shift.user_id && sale.at >= start && sale.at <= end)
            .reduce((sum, sale) => sum + (basis === "net" ? sale.net : sale.gross), 0)
        : null;
    return {
      id: shift.id as string,
      employeeName: profile?.full_name || profile?.username || "Empleado",
      role,
      startTime: shift.clock_in as string,
      endTime: shift.clock_out as string,
      amount,
    };
  });
}

export async function chatterSales(
  from: string | null,
  to: string,
  basis: "net" | "gross",
): Promise<EmployeeSaleTotal[]> {
  const salesFrom = from ?? "1970-01-01T00:00:00.000Z";
  const [{ data: profiles, error }, sales] = await Promise.all([
    supabase.from("profiles").select("id,full_name,username").eq("role", "chatter").eq("is_active", true),
    listSales(salesFrom, to),
  ]);
  if (error) throw error;
  const totals = new Map<string, EmployeeSaleTotal>();
  for (const profile of profiles ?? []) {
    totals.set(profile.id as string, {
      employeeId: profile.id as string,
      name: (profile.full_name as string) || (profile.username as string) || "Empleado",
      amount: 0,
    });
  }
  for (const sale of sales) {
    const current = totals.get(sale.employeeId);
    if (!current || sale.role !== "chatter") continue;
    current.amount += basis === "net" ? sale.net : sale.gross;
  }
  return [...totals.values()].sort((a, b) => b.amount - a.amount || a.name.localeCompare(b.name, "es"));
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

function isAgencyRole(value: unknown): value is AgencyRole {
  return value === "owner" || value === "admin" || value === "chatter" || value === "dropper";
}

function daySpan(clean: string): number | null {
  const spans: Record<string, number> = {
    "7": 7,
    "7d": 7,
    "14": 14,
    "14d": 14,
    "30": 30,
    "30d": 30,
    "90": 90,
    "90d": 90,
    "180": 180,
    "180d": 180,
  };
  return spans[clean] ?? null;
}

function localMidnight(date: Date): Date {
  const copy = new Date(date);
  copy.setHours(0, 0, 0, 0);
  return copy;
}

function nextLocalMidnight(date: Date): Date {
  const start = localMidnight(date);
  start.setDate(start.getDate() + 1);
  return start;
}

function localDate(year: number, month: number, day: number): Date {
  return new Date(year, month - 1, day, 0, 0, 0, 0);
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
  const { error } = await supabase.rpc("record_sale", {
    p_external_key: input.externalKey,
    p_fan_platform_id: input.fanPlatformId,
    p_amount_gross: input.amountGross,
    p_amount_net: input.amountNet,
    p_category: input.category,
    p_recorded_at: input.recordedAt,
    p_source_id: input.sourceId,
  });
  if (error && !/turno activo|no asignado|not allowed/i.test(error.message)) {
    throw new Error(error.message);
  }
}

export async function loadFanNote(externalKey: string, fanPlatformId: string): Promise<string> {
  const modelId = await modelIdByKey(externalKey);
  if (!modelId) return "";
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
  externalKey: string;
  fanPlatformId: string;
  fanUsername: string;
  notes: string;
}): Promise<void> {
  const [modelId, profile] = await Promise.all([modelIdByKey(input.externalKey), loadMyProfile()]);
  if (!modelId) throw new Error("La modelo todavía no está en Supabase. Un owner o admin debe abrir la app para sincronizarla.");
  if (!profile) throw new Error("Inicia sesión para guardar notas");
  const { error } = await supabase.from("fan_notes").upsert(
    {
      model_id: modelId,
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
  externalKey: string,
  fanPlatformId: string,
  onNotes: (notes: string) => void,
): () => void {
  let active = true;
  let channel: ReturnType<typeof supabase.channel> | null = null;
  void modelIdByKey(externalKey).then((modelId) => {
    if (!active || !modelId) return;
    channel = supabase
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
  });
  return () => {
    active = false;
    if (channel) void supabase.removeChannel(channel);
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
