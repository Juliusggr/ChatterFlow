import type { EarningsBasis } from "../api";
import {
  deleteShift,
  isManager,
  loadMyProfile,
  readLocalDismissedShifts,
  type LocalDismissedShift,
} from "../agency";
import { shiftLabel } from "../shifts";
import { supabase } from "../supabase";
import { roundMoney } from "./format";
import { addZonedDays, DEFAULT_TIME_ZONE, zonedDate, zonedParts, type ZonedParts } from "./timeZone";

export type ReportRangeId = "today" | "month" | "days90" | "year" | "all";

export const REPORT_RANGES: { id: ReportRangeId; label: string }[] = [
  { id: "today", label: "Hoy" },
  { id: "month", label: "This month" },
  { id: "days90", label: "90 days" },
  { id: "year", label: "This year" },
  { id: "all", label: "All" },
];

export type EmployeeReport = {
  labels: string[];
  series: { id: string; name: string; values: number[] }[];
  ranking: { id: string; name: string; amount: number }[];
  totalSales: number;
  unlockRate: null;
  averageReplySeconds: null;
  scheduledHours: null;
  rows: {
    id: string;
    name: string;
    sales: number;
    unlockRatio: null;
  }[];
};

export type CreatorReport = {
  total: number;
  totalChange: number | null;
  subscriptions: number;
  subscriptionsChange: number | null;
  messages: number;
  messagesChange: number | null;
  tips: number;
  tipsChange: number | null;
  trends: { label: string; amount: number }[];
  messagesAmount: number;
  otherAmount: number;
  pages: {
    id: string;
    name: string;
    avatarUrl: string | null;
    messageEarnings: number;
    totalEarnings: number;
    engagementRatio: null;
  }[];
};

type SalePoint = {
  employeeId: string;
  name: string;
  role: string | null;
  gross: number;
  net: number;
  at: string;
};

type ModelEarningRow = {
  modelId: string;
  type: string;
  gross: number;
  net: number;
  at: string;
};

type Bucket = { key: string; label: string };

const REPORT_CACHE_TTL_MS = 8_000;

/**
 * Hard launch cutoffs:
 * - Droppers: nothing on or before 2026-11-25 (only >= 2026-11-26 00:00:00 in the report timezone).
 * - Chatters: nothing on or before 2026-11-26 (only >= 2026-11-27 00:00:00 in the report timezone).
 */
function dropperMinDate(timeZone: string): Date {
  return zonedDate(2026, 11, 26, 0, 0, 0, timeZone);
}

function chatterMinDate(timeZone: string): Date {
  return zonedDate(2026, 11, 27, 0, 0, 0, timeZone);
}

function clampMinIso(from: string | null, minDate: Date): string {
  if (!from) return minDate.toISOString();
  const fromMs = new Date(from).getTime();
  if (!Number.isFinite(fromMs) || fromMs < minDate.getTime()) {
    return minDate.toISOString();
  }
  return from;
}

function isShiftUuid(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.trim());
}

let employeeCache: {
  key: string;
  at: number;
  chatters: { id: string; name: string }[];
  sales: SalePoint[];
} | null = null;

let creatorCache: {
  key: string;
  at: number;
  byBasis: Record<EarningsBasis, CreatorReport>;
} | null = null;

const lastSavedShiftFingerprints = new Map<string, string>();

export async function loadEmployeeReport(range: ReportRangeId, basis: EarningsBasis, timeZone: string): Promise<EmployeeReport> {
  const profile = await loadMyProfile();
  if (!profile) throw new Error("No hay una sesión activa");
  const cacheKey = `${profile.id}:${range}:${timeZone}`;
  if (employeeCache && employeeCache.key === cacheKey && Date.now() - employeeCache.at < REPORT_CACHE_TTL_MS) {
    return buildEmployeeReport({
      range,
      basis,
      timeZone,
      chatters: employeeCache.chatters,
      sales: employeeCache.sales,
    });
  }

  const manager = isManager(profile.role);
  const [chatters, sales] = await Promise.all([
    manager ? listActiveChatters() : Promise.resolve([]),
    listChatterSales(range, timeZone, manager ? null : profile.id),
  ]);
  employeeCache = { key: cacheKey, at: Date.now(), chatters, sales };
  return buildEmployeeReport({ range, basis, timeZone, chatters, sales });
}

export function buildEmployeeReport(input: {
  range: ReportRangeId;
  basis: EarningsBasis;
  timeZone: string;
  chatters: { id: string; name: string }[];
  sales: SalePoint[];
}): EmployeeReport {
  const minChatterMs = chatterMinDate(input.timeZone).getTime();
  const salesByChatter = input.sales.filter((sale) => {
    if (sale.role !== "chatter") return false;
    const ms = new Date(sale.at).getTime();
    return Number.isFinite(ms) && ms >= minChatterMs;
  });
  const granularity = resolveBucketGranularity(
    input.range,
    salesByChatter.map((sale) => sale.at),
    input.timeZone,
  );
  const keys = salesByChatter
    .map((sale) => bucketKeyFor(granularity, sale.at, input.timeZone))
    .filter((key): key is string => Boolean(key));
  const buckets = collectBuckets(input.range, keys, input.timeZone, granularity);
  const byEmployee = new Map<string, { name: string; total: number; byBucket: Map<string, number> }>();

  for (const chatter of input.chatters) {
    byEmployee.set(chatter.id, { name: chatter.name, total: 0, byBucket: new Map() });
  }

  for (const sale of salesByChatter) {
    const raw = input.basis === "net" ? sale.net : sale.gross;
    if (!Number.isFinite(raw)) continue;
    const amount = roundMoney(raw);
    const key = bucketKeyFor(granularity, sale.at, input.timeZone);
    const current = byEmployee.get(sale.employeeId) ?? { name: sale.name, total: 0, byBucket: new Map() };
    current.name = sale.name || current.name;
    current.total = roundMoney(current.total + amount);
    if (key) current.byBucket.set(key, roundMoney((current.byBucket.get(key) ?? 0) + amount));
    byEmployee.set(sale.employeeId, current);
  }

  const ranked = [...byEmployee.entries()]
    .filter(([, row]) => row.total > 0)
    .sort((a, b) => b[1].total - a[1].total || a[1].name.localeCompare(b[1].name, "es"));

  const series = ranked.slice(0, 3).map(([id, row]) => ({
    id,
    name: row.name,
    values: buckets.map((bucket) => row.byBucket.get(bucket.key) ?? 0),
  }));

  const rows = [...byEmployee.entries()]
    .map(([id, row]) => ({
      id,
      name: row.name,
      sales: row.total,
      unlockRatio: null,
    }))
    .sort((a, b) => b.sales - a.sales || a.name.localeCompare(b.name, "es"));

  return {
    labels: buckets.map((bucket) => bucket.label),
    series,
    ranking: ranked.slice(0, 3).map(([id, row]) => ({ id, name: row.name, amount: row.total })),
    totalSales: roundMoney(rows.reduce((sum, row) => sum + row.sales, 0)),
    unlockRate: null,
    averageReplySeconds: null,
    scheduledHours: null,
    rows,
  };
}

type CreatorAggPayload = {
  cur: {
    total_gross: number;
    total_net: number;
    sub_gross: number;
    sub_net: number;
    msg_gross: number;
    msg_net: number;
    tip_gross: number;
    tip_net: number;
  };
  prev: {
    total_gross: number;
    total_net: number;
    sub_gross: number;
    sub_net: number;
    msg_gross: number;
    msg_net: number;
    tip_gross: number;
    tip_net: number;
  } | null;
  buckets: { bucket_key: string; gross: number; net: number }[];
  models: {
    model_id: string;
    msg_gross: number;
    msg_net: number;
    total_gross: number;
    total_net: number;
  }[];
};

export async function loadCreatorReport(range: ReportRangeId, basis: EarningsBasis, timeZone: string): Promise<CreatorReport> {
  const profile = await loadMyProfile();
  const cacheKey = `${profile?.id ?? "anon"}:${range}:${timeZone}`;
  if (creatorCache && creatorCache.key === cacheKey && Date.now() - creatorCache.at < REPORT_CACHE_TTL_MS) {
    return creatorCache.byBasis[basis];
  }

  const currentWin = queryWindow(range, timeZone);
  const prevWin = previousQueryWindow(range, timeZone);
  const bucketKind = range === "today" || range === "month" ? "day" : "month";

  const modelsPromise = supabase
    .from("models")
    .select("id,name,handle,avatar_url")
    .order("name", { ascending: true })
    .order("handle", { ascending: true })
    .order("id", { ascending: true });

  // For "today", use row-level query so we can render hourly buckets immediately without requiring an RPC migration.
  const aggPromise =
    range === "today"
      ? Promise.resolve({ data: null, error: null })
      : supabase.rpc("creator_report_agg", {
          p_from: currentWin.from,
          p_to: currentWin.to,
          p_prev_from: prevWin?.from ?? null,
          p_prev_to: prevWin?.to ?? null,
          p_bucket: bucketKind,
          p_tz: timeZone,
        });

  const [modelsRes, aggRes] = await Promise.all([modelsPromise, aggPromise]);
  if (modelsRes.error) throw modelsRes.error;
  const modelsList = modelsRes.data ?? [];

  let byBasis: Record<EarningsBasis, CreatorReport>;
  if (!aggRes.error && aggRes.data) {
    const agg = aggRes.data as CreatorAggPayload;
    byBasis = {
      net: buildCreatorReportFromAgg(range, "net", timeZone, modelsList, agg),
      gross: buildCreatorReportFromAgg(range, "gross", timeZone, modelsList, agg),
    };
  } else {
    // Seamless fallback if 20261126180000_optimize_queries_and_indexes.sql has not been executed yet:
    // scans the combined [minFrom, maxTo) window in ONE query instead of two separate queries.
    const minFrom =
      currentWin.from && prevWin
        ? currentWin.from < prevWin.from
          ? currentWin.from
          : prevWin.from
        : currentWin.from;
    const maxTo = prevWin && prevWin.to > currentWin.to ? prevWin.to : currentWin.to;
    const allRows = await listModelEarnings(minFrom, maxTo);
    const curStart = currentWin.from ? new Date(currentWin.from).getTime() : null;
    const curEnd = new Date(currentWin.to).getTime();
    const prevStart = prevWin ? new Date(prevWin.from).getTime() : null;
    const prevEnd = prevWin ? new Date(prevWin.to).getTime() : null;

    const currentRows: ModelEarningRow[] = [];
    const prevRows: ModelEarningRow[] = [];
    for (const row of allRows) {
      const ms = new Date(row.at).getTime();
      if (!Number.isFinite(ms)) continue;
      if ((curStart === null || ms >= curStart) && ms < curEnd) {
        currentRows.push(row);
      }
      if (prevStart !== null && prevEnd !== null && ms >= prevStart && ms < prevEnd) {
        prevRows.push(row);
      }
    }
    byBasis = {
      net: buildCreatorReportFromRows(range, "net", timeZone, modelsList, currentRows, prevWin ? prevRows : null),
      gross: buildCreatorReportFromRows(range, "gross", timeZone, modelsList, currentRows, prevWin ? prevRows : null),
    };
  }

  creatorCache = { key: cacheKey, at: Date.now(), byBasis };
  return byBasis[basis];
}

function buildCreatorReportFromAgg(
  range: ReportRangeId,
  basis: EarningsBasis,
  timeZone: string,
  modelsList: { id: unknown; name: unknown; handle: unknown; avatar_url: unknown }[],
  agg: CreatorAggPayload,
): CreatorReport {
  const pick = (net: number, gross: number) => roundMoney(Number(basis === "net" ? net : gross) || 0);
  const total = pick(agg.cur.total_net, agg.cur.total_gross);
  const subscriptions = pick(agg.cur.sub_net, agg.cur.sub_gross);
  const messages = pick(agg.cur.msg_net, agg.cur.msg_gross);
  const tips = pick(agg.cur.tip_net, agg.cur.tip_gross);

  const prev = agg.prev
    ? {
        total: pick(agg.prev.total_net, agg.prev.total_gross),
        subscriptions: pick(agg.prev.sub_net, agg.prev.sub_gross),
        messages: pick(agg.prev.msg_net, agg.prev.msg_gross),
        tips: pick(agg.prev.tip_net, agg.prev.tip_gross),
      }
    : null;

  const byBucket = new Map<string, number>();
  const bucketKeys: string[] = [];
  for (const b of agg.buckets ?? []) {
    const val = pick(b.net, b.gross);
    if (b.bucket_key) {
      if (val !== 0) bucketKeys.push(b.bucket_key);
      byBucket.set(b.bucket_key, val);
    }
  }
  const buckets = collectBuckets(range, bucketKeys, timeZone);
  const trends = buckets.map((bucket) => ({
    label: bucket.label,
    amount: roundMoney(byBucket.get(bucket.key) ?? 0),
  }));

  const perModel = new Map<string, { messageEarnings: number; totalEarnings: number }>();
  for (const m of agg.models ?? []) {
    perModel.set(String(m.model_id), {
      messageEarnings: pick(m.msg_net, m.msg_gross),
      totalEarnings: pick(m.total_net, m.total_gross),
    });
  }

  const pages = modelsList
    .map((model) => {
      const id = String(model.id);
      const totals = perModel.get(id) ?? { messageEarnings: 0, totalEarnings: 0 };
      const name = String(model.name || model.handle || "Sin nombre").trim() || "Sin nombre";
      return {
        id,
        name,
        avatarUrl: (model.avatar_url as string | null) ?? null,
        messageEarnings: totals.messageEarnings,
        totalEarnings: totals.totalEarnings,
        engagementRatio: null,
      };
    })
    .sort((a, b) => b.totalEarnings - a.totalEarnings || a.name.localeCompare(b.name, "es"));

  return {
    total,
    totalChange: prev ? percentChange(total, prev.total) : null,
    subscriptions,
    subscriptionsChange: prev ? percentChange(subscriptions, prev.subscriptions) : null,
    messages,
    messagesChange: prev ? percentChange(messages, prev.messages) : null,
    tips,
    tipsChange: prev ? percentChange(tips, prev.tips) : null,
    trends,
    messagesAmount: messages,
    otherAmount: roundMoney(Math.max(0, total - messages)),
    pages,
  };
}

function buildCreatorReportFromRows(
  range: ReportRangeId,
  basis: EarningsBasis,
  timeZone: string,
  modelsList: { id: unknown; name: unknown; handle: unknown; avatar_url: unknown }[],
  currentRows: ModelEarningRow[],
  prevRows: ModelEarningRow[] | null,
): CreatorReport {
  const amountOf = (row: ModelEarningRow) => {
    const raw = basis === "net" ? row.net : row.gross;
    return Number.isFinite(raw) ? raw : 0;
  };
  const sumPeriod = (rows: ModelEarningRow[]) => {
    let total = 0;
    let subscriptions = 0;
    let messages = 0;
    let tips = 0;
    for (const row of rows) {
      const value = amountOf(row);
      total += value;
      if (row.type === "sub") subscriptions += value;
      else if (row.type === "message") messages += value;
      else if (row.type === "tip") tips += value;
    }
    return {
      total: roundMoney(total),
      subscriptions: roundMoney(subscriptions),
      messages: roundMoney(messages),
      tips: roundMoney(tips),
    };
  };

  const cur = sumPeriod(currentRows);
  const prev = prevRows ? sumPeriod(prevRows) : null;
  const granularity = resolveBucketGranularity(
    range,
    currentRows.map((row) => row.at),
    timeZone,
  );
  const byBucket = new Map<string, number>();
  const bucketKeys: string[] = [];
  const perModel = new Map<string, { messageEarnings: number; totalEarnings: number }>();

  for (const row of currentRows) {
    const value = amountOf(row);
    const key = bucketKeyFor(granularity, row.at, timeZone);
    if (key) {
      bucketKeys.push(key);
      byBucket.set(key, roundMoney((byBucket.get(key) ?? 0) + value));
    }
    const modelTotals = perModel.get(row.modelId) ?? { messageEarnings: 0, totalEarnings: 0 };
    modelTotals.totalEarnings = roundMoney(modelTotals.totalEarnings + value);
    if (row.type === "message") {
      modelTotals.messageEarnings = roundMoney(modelTotals.messageEarnings + value);
    }
    perModel.set(row.modelId, modelTotals);
  }

  const buckets = collectBuckets(range, bucketKeys, timeZone, granularity);
  const trends = buckets.map((bucket) => ({
    label: bucket.label,
    amount: roundMoney(byBucket.get(bucket.key) ?? 0),
  }));

  const pages = modelsList
    .map((model) => {
      const id = String(model.id);
      const totals = perModel.get(id) ?? { messageEarnings: 0, totalEarnings: 0 };
      const name = String(model.name || model.handle || "Sin nombre").trim() || "Sin nombre";
      return {
        id,
        name,
        avatarUrl: (model.avatar_url as string | null) ?? null,
        messageEarnings: totals.messageEarnings,
        totalEarnings: totals.totalEarnings,
        engagementRatio: null,
      };
    })
    .sort((a, b) => b.totalEarnings - a.totalEarnings || a.name.localeCompare(b.name, "es"));

  return {
    total: cur.total,
    totalChange: prev ? percentChange(cur.total, prev.total) : null,
    subscriptions: cur.subscriptions,
    subscriptionsChange: prev ? percentChange(cur.subscriptions, prev.subscriptions) : null,
    messages: cur.messages,
    messagesChange: prev ? percentChange(cur.messages, prev.messages) : null,
    tips: cur.tips,
    tipsChange: prev ? percentChange(cur.tips, prev.tips) : null,
    trends,
    messagesAmount: cur.messages,
    otherAmount: roundMoney(Math.max(0, cur.total - cur.messages)),
    pages,
  };
}

function percentChange(current: number, previous: number): number | null {
  if (previous <= 0) return current > 0 ? 100 : null;
  return roundMoney(((current - previous) / previous) * 100);
}

async function listModelEarnings(from: string | null, to: string): Promise<ModelEarningRow[]> {
  const pageSize = 1000;
  const rows: ModelEarningRow[] = [];
  for (let page = 0; page < 50; page += 1) {
    let query = supabase
      .from("model_earnings")
      .select("id,model_id,type,gross_amount,net_amount,created_at")
      .order("created_at", { ascending: true })
      .order("id", { ascending: true })
      .range(page * pageSize, page * pageSize + pageSize - 1);
    if (from) query = query.gte("created_at", from);
    if (to) query = query.lt("created_at", to);
    const { data, error } = await query;
    if (error) throw error;
    const batch = data ?? [];
    for (const row of batch) {
      rows.push({
        modelId: row.model_id as string,
        type: String(row.type ?? ""),
        gross: Number(row.gross_amount ?? 0),
        net: Number(row.net_amount ?? 0),
        at: row.created_at as string,
      });
    }
    if (batch.length < pageSize) return rows;
  }
  return rows;
}

export type DropperShiftRecord = {
  id: string;
  shiftId: string;
  dropperUserId: string;
  dropperName: string;
  modelId: string;
  accountName: string;
  shiftStart: string;
  shiftEnd: string | null;
  scheduleLabel: string;
  status: "active" | "completed";
  publicationsCount: number;
  subscriptionsCount: number;
  updatedAt?: string;
};

export type DropperShiftEntry = {
  id: string;
  shiftId: string;
  dropperUserId: string;
  dropperName: string;
  modelId: string;
  accountName: string;
  shiftStart: string;
  shiftEnd: string | null;
  scheduleLabel: string;
  status: "active" | "completed";
  publicationsCount: number;
  subscriptionsCount: number;
  isActualShift: boolean;
};

export type DropperModelBreakdown = {
  id: string;
  modelId: string;
  accountName: string;
  publicationsCount: number;
  subscriptionsCount: number;
};

export type DropperSummaryRow = {
  id: string;
  name: string;
  schedule: string;
  posts: number;
  fans: number;
  shifts: number;
  modelsCount: number;
  models: DropperModelBreakdown[];
};

export type DropperShiftGroup = {
  id: string;
  shiftId: string;
  dropperUserId: string;
  dropperName: string;
  shiftStart: string;
  shiftEnd: string | null;
  scheduleLabel: string;
  status: "active" | "completed";
  publicationsCount: number;
  subscriptionsCount: number;
  accountsCount: number;
  isActualShift: boolean;
  models: DropperModelBreakdown[];
};

export type DropperRankingEntry = {
  id: string;
  name: string;
  fans: number;
  posts: number;
  shifts: number;
};

export type DropperReport = {
  labels: string[];
  series: { id: string; name: string; values: number[] }[];
  ranking: DropperRankingEntry[];
  totalPosts: number;
  totalFans: number;
  totalShifts: number;
  rows: DropperSummaryRow[];
  shiftGroups: DropperShiftGroup[];
  shiftRecords: DropperShiftEntry[];
};

type AssignedAccount = {
  modelId: string;
  name: string;
};

type ScheduleSlot = { day: number; start: number; end: number };

type DropperShiftRow = {
  id: string;
  userId: string;
  clockIn: string;
  clockOut: string | null;
  status: "active" | "completed";
};

type ActivityEvent = {
  kind: "post" | "fan";
  at: string;
  authorUserId: string | null;
  modelId: string;
};

export async function loadDropperReport(range: ReportRangeId, timeZone: string): Promise<DropperReport> {
  const profile = await loadMyProfile();
  if (!profile) throw new Error("No hay una sesión activa");
  const people = isManager(profile.role)
    ? await listActiveByRole("dropper")
    : profile.role === "dropper"
      ? [{ id: profile.id, name: displayName(profile.full_name, profile.username) }]
      : [];
  const ids = people.map((person) => person.id);
  if (ids.length === 0) {
    return buildDropperReport({
      range,
      timeZone,
      people: [],
      slotsByPerson: new Map(),
      accountsByPerson: new Map(),
      shifts: [],
      storedRecords: [],
      activity: [],
    });
  }

  const [slotsByPerson, accountsByPerson, shifts, activity, storedRemote, dismissedShifts] = await Promise.all([
    listScheduleSlots(ids),
    listAssignedAccounts(ids),
    listDropperShifts(ids, range, timeZone),
    listDropperActivity(range, timeZone),
    listRemoteDropperShiftRecords(ids, range, timeZone),
    listDismissedDropperShifts(ids),
  ]);
  const report = buildDropperReport({
    range,
    timeZone,
    people,
    slotsByPerson,
    accountsByPerson,
    shifts,
    storedRecords: storedRemote,
    dismissedShifts,
    activity,
  });
  void persistDropperShiftEntriesIfChanged(report.shiftRecords, storedRemote);
  return report;
}

export async function deleteDropperShiftGroup(group: DropperShiftGroup): Promise<void> {
  await deleteShift(group.shiftId, {
    dropperUserId: group.dropperUserId,
    shiftStart: group.shiftStart,
    shiftEnd: group.shiftEnd,
  });
  const prefix = `${group.shiftId}:`;
  for (const key of [...lastSavedShiftFingerprints.keys()]) {
    if (key.startsWith(prefix)) {
      lastSavedShiftFingerprints.delete(key);
    }
  }
}

export async function syncDropperShiftRecords(
  timeZone = localStorage.getItem("cf-report-tz") || DEFAULT_TIME_ZONE,
): Promise<void> {
  try {
    await loadDropperReport("today", timeZone);
  } catch {
    // Background shift record sync should never block UI interactions.
  }
}

export function buildDropperReport(input: {
  range: ReportRangeId;
  timeZone: string;
  people: { id: string; name: string }[];
  slotsByPerson: Map<string, ScheduleSlot[]>;
  accountsByPerson: Map<string, AssignedAccount[]>;
  shifts?: DropperShiftRow[];
  storedRecords?: DropperShiftRecord[];
  dismissedShifts?: LocalDismissedShift[];
  activity: ActivityEvent[];
  now?: Date;
}): DropperReport {
  const now = input.now ?? new Date();
  const nowMs = now.getTime();
  const minDropperMs = dropperMinDate(input.timeZone).getTime();
  const { from, to } = queryWindow(input.range, input.timeZone, now);
  const rawRangeStartMs = from ? new Date(from).getTime() : null;
  const rangeStartMs =
    rawRangeStartMs === null ? minDropperMs : Math.max(rawRangeStartMs, minDropperMs);
  const rangeEndMs = new Date(to).getTime();

  const dismissedKeySet = new Set<string>();
  for (const dismissed of input.dismissedShifts ?? []) {
    if (!dismissed.dropperUserId || !dismissed.shiftKey) continue;
    dismissedKeySet.add(`${dismissed.dropperUserId}:${dismissed.shiftKey}`);
  }
  const isShiftDismissed = (dropperUserId: string, shiftId: string): boolean =>
    dismissedKeySet.has(`${dropperUserId}:${shiftId}`);

  const peopleById = new Map(input.people.map((person) => [person.id, person]));
  const counted = input.activity.filter((event) => {
    const ms = new Date(event.at).getTime();
    return Number.isFinite(ms) && ms >= minDropperMs;
  });

  const dropperSummaries = new Map<
    string,
    {
      id: string;
      name: string;
      schedule: string;
      posts: number;
      fans: number;
      shiftIds: Set<string>;
      byBucket: Map<string, number>;
      models: Map<string, { modelId: string; accountName: string; posts: number; fans: number }>;
    }
  >();

  const ensureDropper = (personId: string, personName: string) => {
    const existing = dropperSummaries.get(personId);
    if (existing) {
      if (existing.name === "Sin nombre" && personName && personName !== "Sin nombre") {
        existing.name = personName;
      }
      return existing;
    }
    const created = {
      id: personId,
      name: personName,
      schedule: summarizeSlots(input.slotsByPerson.get(personId) ?? []),
      posts: 0,
      fans: 0,
      shiftIds: new Set<string>(),
      byBucket: new Map<string, number>(),
      models: new Map<string, { modelId: string; accountName: string; posts: number; fans: number }>(),
    };
    dropperSummaries.set(personId, created);
    return created;
  };

  const ensureDropperAccount = (personId: string, personName: string, account: AssignedAccount) => {
    const dropper = ensureDropper(personId, personName);
    if (account.modelId) {
      const existingModel = dropper.models.get(account.modelId);
      if (existingModel) {
        if (existingModel.accountName === "Cuenta" && account.name !== "Cuenta") {
          existingModel.accountName = account.name;
        }
      } else {
        dropper.models.set(account.modelId, {
          modelId: account.modelId,
          accountName: account.name,
          posts: 0,
          fans: 0,
        });
      }
    }
    return dropper;
  };

  for (const person of input.people) {
    ensureDropper(person.id, person.name);
    for (const account of input.accountsByPerson.get(person.id) ?? []) {
      ensureDropperAccount(person.id, person.name, account);
    }
  }

  type MutableShiftEntry = DropperShiftEntry & {
    startMs: number;
    endMs: number;
    slotStartMs: number | null;
    slotEndMs: number | null;
    fanSlotEndMs: number | null;
    livePosts: number;
    liveFans: number;
  };

  const shiftEntries = new Map<string, MutableShiftEntry>();
  const actualShiftsByPerson = new Map<string, MutableShiftEntry[]>();

  const registerShiftEntry = (entry: MutableShiftEntry) => {
    const existing = shiftEntries.get(entry.id);
    if (existing) return existing;
    shiftEntries.set(entry.id, entry);
    if (entry.isActualShift) {
      const list = actualShiftsByPerson.get(entry.dropperUserId) ?? [];
      list.push(entry);
      actualShiftsByPerson.set(entry.dropperUserId, list);
    }
    return entry;
  };

  // 1. Seed one record per actual check-in shift × assigned account (never invent past unclocked shifts).
  for (const shift of input.shifts ?? []) {
    const person = peopleById.get(shift.userId);
    if (!person) continue;
    const startMs = new Date(shift.clockIn).getTime();
    if (!Number.isFinite(startMs) || startMs < minDropperMs) continue;
    const rawEndMs = shift.clockOut ? new Date(shift.clockOut).getTime() : null;
    const endMs = rawEndMs && Number.isFinite(rawEndMs) ? rawEndMs : Math.max(nowMs, startMs);
    if (startMs >= rangeEndMs) continue;
    if (shift.status !== "active" && endMs < rangeStartMs) continue;
    if (isShiftDismissed(person.id, shift.id)) continue;
    const slots = input.slotsByPerson.get(person.id) ?? [];
    const slotMatch = findShiftScheduleWindow(startMs, endMs, slots, input.timeZone);
    const scheduleLabel = slotMatch?.label || summarizeSlots(slots);
    const assigned = input.accountsByPerson.get(person.id) ?? [];
    const targets: AssignedAccount[] =
      assigned.length > 0 ? assigned : [{ modelId: "", name: "Sin cuenta" }];
    for (const account of targets) {
      ensureDropperAccount(person.id, person.name, account);
      const id = `${shift.id}:${account.modelId || "unassigned"}`;
      registerShiftEntry({
        id,
        shiftId: shift.id,
        dropperUserId: person.id,
        dropperName: person.name,
        modelId: account.modelId,
        accountName: account.name,
        shiftStart: shift.clockIn,
        shiftEnd: shift.clockOut,
        scheduleLabel,
        status: shift.status === "completed" || shift.clockOut ? "completed" : "active",
        publicationsCount: 0,
        subscriptionsCount: 0,
        isActualShift: true,
        startMs,
        endMs,
        slotStartMs: slotMatch?.start ?? null,
        slotEndMs: slotMatch?.end ?? null,
        fanSlotEndMs: slotMatch?.fanEnd ?? null,
        livePosts: 0,
        liveFans: 0,
      });
    }
  }

  const resolveAccountForEvent = (personId: string, modelId: string): AssignedAccount => {
    const accounts = input.accountsByPerson.get(personId) ?? [];
    const exact = accounts.find((account) => account.modelId === modelId);
    if (exact) return exact;
    if (accounts.length === 1) return accounts[0];
    return { modelId, name: "Cuenta" };
  };

  const findMatchingActualShift = (
    personId: string,
    account: AssignedAccount,
    whenMs: number,
  ): MutableShiftEntry | null => {
    const candidates = (actualShiftsByPerson.get(personId) ?? []).filter(
      (entry) =>
        entry.modelId === account.modelId &&
        whenMs >= entry.startMs - 60_000 &&
        whenMs <= entry.endMs + 60_000,
    );
    if (candidates.length === 0) return null;
    candidates.sort((a, b) => b.startMs - a.startMs);
    return candidates[0];
  };

  const findMatchingShiftForFan = (
    personId: string,
    account: AssignedAccount,
    whenMs: number,
  ): MutableShiftEntry | null => {
    const direct = findMatchingActualShift(personId, account, whenMs);
    if (direct) return direct;
    const bySchedule = (actualShiftsByPerson.get(personId) ?? []).filter(
      (entry) =>
        entry.modelId === account.modelId &&
        entry.slotStartMs !== null &&
        entry.fanSlotEndMs !== null &&
        whenMs >= entry.slotStartMs &&
        whenMs < entry.fanSlotEndMs,
    );
    if (bySchedule.length === 0) return null;
    bySchedule.sort((a, b) => a.startMs - b.startMs);
    return bySchedule[0];
  };

  const findMatchingShiftForPost = (
    personId: string,
    account: AssignedAccount,
    whenMs: number,
  ): MutableShiftEntry | null => {
    const direct = findMatchingActualShift(personId, account, whenMs);
    if (direct) return direct;
    const bySchedule = (actualShiftsByPerson.get(personId) ?? []).filter(
      (entry) =>
        entry.modelId === account.modelId &&
        entry.slotStartMs !== null &&
        entry.slotEndMs !== null &&
        whenMs >= entry.slotStartMs &&
        whenMs <= Math.max(entry.slotEndMs, entry.endMs) + 60_000,
    );
    if (bySchedule.length === 0) return null;
    bySchedule.sort((a, b) => a.startMs - b.startMs);
    return bySchedule[0];
  };

  // 2. Attribute publications ONLY to real check-in shifts (never synthesize unclocked shifts).
  for (const event of counted) {
    if (event.kind !== "post") continue;
    const when = new Date(event.at);
    const whenMs = when.getTime();
    const authors =
      event.authorUserId && peopleById.has(event.authorUserId)
        ? [event.authorUserId]
        : droppersForPublication(
            event.modelId,
            when,
            input.timeZone,
            input.people,
            input.accountsByPerson,
            input.slotsByPerson,
            actualShiftsByPerson,
          );
    for (const authorId of authors) {
      const person = peopleById.get(authorId);
      if (!person) continue;
      const account = resolveAccountForEvent(authorId, event.modelId);
      ensureDropperAccount(authorId, person.name, account);
      const actualShift = findMatchingShiftForPost(authorId, account, whenMs);
      if (actualShift) {
        actualShift.livePosts += 1;
        actualShift.publicationsCount = actualShift.livePosts;
      }
    }
  }

  // 3. Attribute fan subscriptions ONLY to real check-in shifts & chart timestamps.
  const chartFanEvents: { dropperId: string; at: string; count: number }[] = [];
  for (const event of counted) {
    if (event.kind !== "fan") continue;
    const when = new Date(event.at);
    const whenMs = when.getTime();
    for (const person of input.people) {
      const accounts = input.accountsByPerson.get(person.id) ?? [];
      const account = accounts.find((candidate) => candidate.modelId === event.modelId);
      if (!account) continue;
      const actualShift = findMatchingShiftForFan(person.id, account, whenMs);
      if (actualShift) {
        actualShift.liveFans += 1;
        actualShift.subscriptionsCount = actualShift.liveFans;
        chartFanEvents.push({ dropperId: person.id, at: event.at, count: 1 });
      }
    }
  }

  // 4. Merge persisted real shift records from Supabase (>= 2026-11-26 only).
  for (const stored of input.storedRecords ?? []) {
    if (!peopleById.has(stored.dropperUserId)) continue;
    if (!isShiftUuid(stored.shiftId)) continue;
    const person = peopleById.get(stored.dropperUserId)!;
    const startMs = new Date(stored.shiftStart).getTime();
    if (!Number.isFinite(startMs) || startMs < minDropperMs) continue;
    const endMs = stored.shiftEnd ? new Date(stored.shiftEnd).getTime() : Math.max(nowMs, startMs);
    if (isShiftDismissed(person.id, stored.shiftId)) {
      continue;
    }
    const overlapsRange =
      startMs < rangeEndMs && (stored.status === "active" || endMs >= rangeStartMs);
    if (!overlapsRange) continue;

    const assignedAccounts = input.accountsByPerson.get(person.id) ?? [];
    const matchedAccount =
      assignedAccounts.find((account) => account.modelId === stored.modelId) ?? {
        modelId: stored.modelId,
        name: stored.accountName || "Cuenta",
      };
    const key = `${stored.shiftId}:${matchedAccount.modelId || "unassigned"}`;
    const existing = shiftEntries.get(key);
    if (existing) {
      existing.publicationsCount = Math.max(existing.publicationsCount, stored.publicationsCount);
      existing.subscriptionsCount = Math.max(existing.subscriptionsCount, stored.subscriptionsCount);
      if (!existing.scheduleLabel && stored.scheduleLabel) {
        existing.scheduleLabel = stored.scheduleLabel;
      }
    } else {
      ensureDropperAccount(person.id, person.name, matchedAccount);
      shiftEntries.set(key, {
        id: key,
        shiftId: stored.shiftId,
        dropperUserId: person.id,
        dropperName: stored.dropperName || person.name,
        modelId: matchedAccount.modelId,
        accountName: matchedAccount.name,
        shiftStart: stored.shiftStart,
        shiftEnd: stored.shiftEnd,
        scheduleLabel: stored.scheduleLabel || summarizeSlots(input.slotsByPerson.get(person.id) ?? []),
        status: stored.status === "completed" ? "completed" : "active",
        publicationsCount: Math.max(0, stored.publicationsCount),
        subscriptionsCount: Math.max(0, stored.subscriptionsCount),
        isActualShift: !stored.shiftId.startsWith("sched:") && !stored.shiftId.startsWith("day:"),
        startMs,
        endMs,
        slotStartMs: null,
        slotEndMs: null,
        fanSlotEndMs: null,
        livePosts: 0,
        liveFans: 0,
      });
    }
  }

  // 5. Roll up shift records into per-dropper summaries, grouped shift records, and chart buckets.
  const orderedShifts = [...shiftEntries.values()].sort(
    (left, right) =>
      right.startMs - left.startMs ||
      left.dropperName.localeCompare(right.dropperName, "es") ||
      left.accountName.localeCompare(right.accountName, "es"),
  );

  type MutableShiftGroup = {
    id: string;
    shiftId: string;
    dropperUserId: string;
    dropperName: string;
    shiftStart: string;
    shiftEnd: string | null;
    startMs: number;
    endMs: number;
    scheduleLabel: string;
    status: "active" | "completed";
    publicationsCount: number;
    subscriptionsCount: number;
    isActualShift: boolean;
    models: Map<string, DropperModelBreakdown>;
  };

  const shiftGroupMap = new Map<string, MutableShiftGroup>();

  for (const shift of orderedShifts) {
    const account: AssignedAccount = {
      modelId: shift.modelId,
      name: shift.accountName,
    };
    const dropper = ensureDropperAccount(shift.dropperUserId, shift.dropperName, account);
    dropper.posts += shift.publicationsCount;
    dropper.fans += shift.subscriptionsCount;
    dropper.shiftIds.add(shift.shiftId);
    if (shift.modelId) {
      const modelRow = dropper.models.get(shift.modelId);
      if (modelRow) {
        modelRow.posts += shift.publicationsCount;
        modelRow.fans += shift.subscriptionsCount;
      }
    }

    const extraPersistedFans = Math.max(0, shift.subscriptionsCount - shift.liveFans);
    if (extraPersistedFans > 0) {
      chartFanEvents.push({
        dropperId: shift.dropperUserId,
        at: shift.shiftStart,
        count: extraPersistedFans,
      });
    }

    const groupKey = `${shift.dropperUserId}:${shift.shiftId}`;
    let group = shiftGroupMap.get(groupKey);
    if (!group) {
      group = {
        id: groupKey,
        shiftId: shift.shiftId,
        dropperUserId: shift.dropperUserId,
        dropperName: shift.dropperName,
        shiftStart: shift.shiftStart,
        shiftEnd: shift.shiftEnd,
        startMs: shift.startMs,
        endMs: shift.endMs,
        scheduleLabel: shift.scheduleLabel,
        status: shift.status,
        publicationsCount: 0,
        subscriptionsCount: 0,
        isActualShift: shift.isActualShift,
        models: new Map(),
      };
      shiftGroupMap.set(groupKey, group);
    } else {
      if (shift.startMs < group.startMs) {
        group.startMs = shift.startMs;
        group.shiftStart = shift.shiftStart;
      }
      if (shift.status === "active") {
        group.status = "active";
        group.shiftEnd = null;
      } else if (group.status !== "active" && shift.shiftEnd && shift.endMs >= group.endMs) {
        group.endMs = shift.endMs;
        group.shiftEnd = shift.shiftEnd;
      }
      if ((!group.scheduleLabel || group.scheduleLabel === "Sin horario") && shift.scheduleLabel) {
        group.scheduleLabel = shift.scheduleLabel;
      }
      group.isActualShift = group.isActualShift || shift.isActualShift;
    }

    group.publicationsCount += shift.publicationsCount;
    group.subscriptionsCount += shift.subscriptionsCount;

    if (shift.modelId) {
      const existingModel = group.models.get(shift.modelId);
      if (existingModel) {
        existingModel.publicationsCount += shift.publicationsCount;
        existingModel.subscriptionsCount += shift.subscriptionsCount;
        if (existingModel.accountName === "Cuenta" && shift.accountName !== "Cuenta") {
          existingModel.accountName = shift.accountName;
        }
      } else {
        group.models.set(shift.modelId, {
          id: `${groupKey}:${shift.modelId}`,
          modelId: shift.modelId,
          accountName: shift.accountName,
          publicationsCount: shift.publicationsCount,
          subscriptionsCount: shift.subscriptionsCount,
        });
      }
    }
  }

  const sortModelBreakdowns = (models: DropperModelBreakdown[]) =>
    models.sort(
      (a, b) =>
        b.subscriptionsCount - a.subscriptionsCount ||
        b.publicationsCount - a.publicationsCount ||
        a.accountName.localeCompare(b.accountName, "es"),
    );

  const shiftGroups: DropperShiftGroup[] = [...shiftGroupMap.values()]
    .sort((left, right) => right.startMs - left.startMs || left.dropperName.localeCompare(right.dropperName, "es"))
    .map((group) => {
      const models = sortModelBreakdowns([...group.models.values()]);
      return {
        id: group.id,
        shiftId: group.shiftId,
        dropperUserId: group.dropperUserId,
        dropperName: group.dropperName,
        shiftStart: group.shiftStart,
        shiftEnd: group.shiftEnd,
        scheduleLabel: group.scheduleLabel,
        status: group.status,
        publicationsCount: group.publicationsCount,
        subscriptionsCount: group.subscriptionsCount,
        accountsCount: models.length,
        isActualShift: group.isActualShift,
        models,
      };
    });

  const granularity = resolveBucketGranularity(
    input.range,
    chartFanEvents.map((event) => event.at),
    input.timeZone,
  );
  const chartFanKeys: string[] = [];
  for (const event of chartFanEvents) {
    const key = bucketKeyFor(granularity, event.at, input.timeZone);
    if (!key) continue;
    chartFanKeys.push(key);
    const dropper = dropperSummaries.get(event.dropperId);
    if (dropper) {
      dropper.byBucket.set(key, (dropper.byBucket.get(key) ?? 0) + event.count);
    }
  }

  const buckets = collectBuckets(input.range, chartFanKeys, input.timeZone, granularity, now);
  const rankedDroppers = [...dropperSummaries.values()]
    .filter((row) => row.fans > 0 || row.posts > 0)
    .sort((a, b) => b.fans - a.fans || b.posts - a.posts || a.name.localeCompare(b.name, "es"));

  const series = rankedDroppers
    .filter((row) => row.fans > 0)
    .slice(0, 3)
    .map((row) => ({
      id: row.id,
      name: row.name,
      values: buckets.map((bucket) => row.byBucket.get(bucket.key) ?? 0),
    }));

  const ranking: DropperRankingEntry[] = rankedDroppers.map((row) => ({
    id: row.id,
    name: row.name,
    fans: row.fans,
    posts: row.posts,
    shifts: row.shiftIds.size,
  }));

  const rows: DropperSummaryRow[] = [...dropperSummaries.values()]
    .sort((a, b) => b.fans - a.fans || b.posts - a.posts || a.name.localeCompare(b.name, "es"))
    .map((row) => {
      const models = sortModelBreakdowns(
        [...row.models.values()].map((model) => ({
          id: `${row.id}:${model.modelId}`,
          modelId: model.modelId,
          accountName: model.accountName,
          publicationsCount: model.posts,
          subscriptionsCount: model.fans,
        })),
      );
      return {
        id: row.id,
        name: row.name,
        schedule: row.schedule,
        posts: row.posts,
        fans: row.fans,
        shifts: row.shiftIds.size,
        modelsCount: models.length,
        models,
      };
    });

  return {
    labels: buckets.map((bucket) => bucket.label),
    series,
    ranking,
    totalPosts: rows.reduce((sum, row) => sum + row.posts, 0),
    totalFans: rows.reduce((sum, row) => sum + row.fans, 0),
    totalShifts: shiftGroups.length,
    rows,
    shiftGroups,
    shiftRecords: orderedShifts.map(
      ({
        startMs: _s,
        endMs: _e,
        slotStartMs: _ss,
        slotEndMs: _se,
        fanSlotEndMs: _fse,
        livePosts: _lp,
        liveFans: _lf,
        ...entry
      }) => entry,
    ),
  };
}

function droppersForPublication(
  modelId: string,
  when: Date,
  timeZone: string,
  people: { id: string; name: string }[],
  accountsByPerson: Map<string, AssignedAccount[]>,
  slotsByPerson: Map<string, ScheduleSlot[]>,
  actualShiftsByPerson: Map<string, { startMs: number; endMs: number }[]>,
): string[] {
  const assigned = people
    .filter((person) => (accountsByPerson.get(person.id) ?? []).some((account) => account.modelId === modelId))
    .map((person) => person.id);
  const whenMs = when.getTime();
  const inActualShift = assigned.filter((id) =>
    (actualShiftsByPerson.get(id) ?? []).some(
      (shift) => whenMs >= shift.startMs - 60_000 && whenMs <= shift.endMs + 60_000,
    ),
  );
  if (inActualShift.length > 0) return inActualShift;
  const onShift = assigned.filter((id) => shiftWindow(when, slotsByPerson.get(id) ?? [], timeZone));
  if (onShift.length > 0) return onShift;
  return assigned.length === 1 ? assigned : [];
}

const NIGHT_DROPPER_START_MINUTES = 18 * 60;
const NIGHT_DROPPER_END_MINUTES = 2 * 60;
const NIGHT_DROPPER_FAN_END_MINUTES = 4 * 60;

function fanEndMinutesForSlot(slot: ScheduleSlot): number {
  if (slot.start === NIGHT_DROPPER_START_MINUTES && slot.end === NIGHT_DROPPER_END_MINUTES) {
    return NIGHT_DROPPER_FAN_END_MINUTES;
  }
  return slot.end;
}

function findShiftScheduleWindow(
  startMs: number,
  endMs: number,
  slots: ScheduleSlot[],
  timeZone: string,
): { start: number; end: number; fanEnd: number; label: string } | null {
  const atStart = shiftWindow(new Date(startMs), slots, timeZone);
  if (atStart) return atStart;
  const earlyGraceMs = 90 * 60_000;
  const upcoming = shiftWindow(new Date(startMs + earlyGraceMs), slots, timeZone);
  if (upcoming && upcoming.start >= startMs && upcoming.start - startMs <= earlyGraceMs) {
    return upcoming;
  }
  if (endMs > startMs) {
    const atMid = shiftWindow(new Date(Math.round((startMs + endMs) / 2)), slots, timeZone);
    if (atMid) return atMid;
    return shiftWindow(new Date(endMs), slots, timeZone);
  }
  return null;
}

function shiftWindow(
  when: Date,
  slots: ScheduleSlot[],
  timeZone: string,
  forFans = false,
): { start: number; end: number; fanEnd: number; label: string } | null {
  const parts = zonedParts(when, timeZone);
  const day = (new Date(Date.UTC(parts.year, parts.month - 1, parts.day)).getUTCDay() + 6) % 7;
  const minutes = parts.hour * 60 + parts.minute;
  const previous = (day + 6) % 7;
  for (const slot of slots) {
    if (slot.start === slot.end) continue;
    const label = formatSlotLabel(slot);
    const fanEndMinutes = fanEndMinutesForSlot(slot);
    const matchEndMinutes = forFans ? fanEndMinutes : slot.end;
    if (slot.start < slot.end && slot.day === day && minutes >= slot.start && minutes < matchEndMinutes) {
      const start = atMinutes(parts, slot.start, timeZone).getTime();
      const end = atMinutes(parts, slot.end, timeZone).getTime();
      const fanEnd = atMinutes(parts, fanEndMinutes, timeZone).getTime();
      return { start, end, fanEnd, label };
    }
    if (slot.start > slot.end && slot.day === day && minutes >= slot.start) {
      const start = atMinutes(parts, slot.start, timeZone);
      const nextDay = zonedParts(addZonedDays(parts, 1, timeZone), timeZone);
      const end = atMinutes(nextDay, slot.end, timeZone);
      const fanEnd = atMinutes(nextDay, fanEndMinutes, timeZone);
      return { start: start.getTime(), end: end.getTime(), fanEnd: fanEnd.getTime(), label };
    }
    if (slot.start > slot.end && slot.day === previous && minutes < matchEndMinutes) {
      const prevDay = zonedParts(addZonedDays(parts, -1, timeZone), timeZone);
      const start = atMinutes(prevDay, slot.start, timeZone);
      const end = atMinutes(parts, slot.end, timeZone);
      const fanEnd = atMinutes(parts, fanEndMinutes, timeZone);
      return { start: start.getTime(), end: end.getTime(), fanEnd: fanEnd.getTime(), label };
    }
  }
  return null;
}

function formatSlotLabel(slot: ScheduleSlot): string {
  const start = formatClockMinutes(slot.start);
  const end = formatClockMinutes(slot.end);
  return shiftLabel(start, end) ?? `${start} – ${end}`;
}

function summarizeSlots(slots: ScheduleSlot[]): string {
  if (slots.length === 0) return "Sin horario";
  const unique = [...new Set(slots.map((slot) => formatSlotLabel(slot)))];
  return unique.join(" · ");
}

function formatClockMinutes(totalMinutes: number): string {
  const hour = Math.floor(totalMinutes / 60) % 24;
  const minute = totalMinutes % 60;
  return `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
}

function atMinutes(parts: ZonedParts, minutes: number, timeZone: string): Date {
  return zonedDate(parts.year, parts.month, parts.day, Math.floor(minutes / 60), minutes % 60, 0, timeZone);
}

function minutesOf(value: string): number {
  const [hour, minute] = value.slice(0, 5).split(":").map(Number);
  if (!Number.isFinite(hour) || !Number.isFinite(minute)) return 0;
  return hour * 60 + minute;
}

async function listDropperActivity(range: ReportRangeId, timeZone: string): Promise<ActivityEvent[]> {
  const minDropper = dropperMinDate(timeZone);
  const { from, to } = queryWindow(range, timeZone);
  if (new Date(to).getTime() <= minDropper.getTime()) return [];
  const rawBuffered = from ? new Date(new Date(from).getTime() - 24 * 3_600_000).toISOString() : null;
  const bufferedFrom = clampMinIso(rawBuffered, minDropper);

  const pubQuery = supabase
    .from("publications")
    .select("model_id,created_at,author_user_id")
    .gte("created_at", bufferedFrom)
    .lt("created_at", to)
    .order("created_at", { ascending: false })
    .limit(5000);

  const fanQuery = supabase
    .from("fan_arrivals")
    .select("model_id,arrived_at")
    .gte("arrived_at", bufferedFrom)
    .lt("arrived_at", to)
    .order("arrived_at", { ascending: false })
    .limit(5000);

  const [pubRes, fanRes] = await Promise.all([pubQuery, fanQuery]);
  const events: ActivityEvent[] = [];

  if (!pubRes.error) {
    for (const row of pubRes.data ?? []) {
      events.push({
        kind: "post",
        at: row.created_at as string,
        authorUserId: (row.author_user_id as string | null) ?? null,
        modelId: row.model_id as string,
      });
    }
  }
  if (!fanRes.error) {
    for (const row of fanRes.data ?? []) {
      events.push({
        kind: "fan",
        at: row.arrived_at as string,
        authorUserId: null,
        modelId: row.model_id as string,
      });
    }
  }
  return events;
}

async function listDropperShifts(
  ids: string[],
  range: ReportRangeId,
  timeZone: string,
): Promise<DropperShiftRow[]> {
  if (ids.length === 0) return [];
  const minDropper = dropperMinDate(timeZone);
  const minIso = minDropper.toISOString();
  const { from, to } = queryWindow(range, timeZone);
  if (new Date(to).getTime() <= minDropper.getTime()) return [];
  const effectiveFrom = clampMinIso(from, minDropper);
  const query = supabase
    .from("shifts")
    .select("id,user_id,clock_in,clock_out,status")
    .in("user_id", ids)
    .gte("clock_in", minIso)
    .lt("clock_in", to)
    .or(`clock_out.gte.${effectiveFrom},status.eq.active,clock_in.gte.${effectiveFrom}`)
    .order("clock_in", { ascending: false });
  const { data, error } = await query;
  if (error) throw error;
  return (data ?? []).map((row) => ({
    id: row.id as string,
    userId: row.user_id as string,
    clockIn: row.clock_in as string,
    clockOut: (row.clock_out as string | null) ?? null,
    status: row.status === "completed" ? "completed" : "active",
  }));
}

async function listRemoteDropperShiftRecords(
  ids: string[],
  range: ReportRangeId,
  timeZone: string,
): Promise<DropperShiftRecord[]> {
  if (ids.length === 0) return [];
  try {
    const minDropper = dropperMinDate(timeZone);
    const minIso = minDropper.toISOString();
    const { from, to } = queryWindow(range, timeZone);
    if (new Date(to).getTime() <= minDropper.getTime()) return [];
    const effectiveFrom = clampMinIso(from, minDropper);
    const query = supabase
      .from("dropper_shift_reports")
      .select(
        "id,shift_id,dropper_user_id,model_id,shift_start,shift_end,schedule_label,status,publications_count,subscriptions_count,updated_at,models(name,handle),profiles(full_name,username)",
      )
      .in("dropper_user_id", ids)
      .gte("shift_start", minIso)
      .lt("shift_start", to)
      .or(`shift_end.gte.${effectiveFrom},status.eq.active,shift_start.gte.${effectiveFrom}`)
      .order("shift_start", { ascending: false });
    const { data, error } = await query;
    if (error || !data) return [];
    return data.map((row) => {
      const modelRel = firstRel<{ name?: string; handle?: string }>(row.models);
      const profileRel = relation(row.profiles);
      const modelId = String(row.model_id ?? "");
      return {
        id: `${row.shift_id as string}:${modelId || "unassigned"}`,
        shiftId: row.shift_id as string,
        dropperUserId: row.dropper_user_id as string,
        dropperName: displayName(profileRel?.full_name, profileRel?.username),
        modelId,
        accountName: String(modelRel?.name || modelRel?.handle || "Cuenta").trim() || "Cuenta",
        shiftStart: row.shift_start as string,
        shiftEnd: (row.shift_end as string | null) ?? null,
        scheduleLabel: String(row.schedule_label ?? ""),
        status: row.status === "completed" ? "completed" : "active",
        publicationsCount: Number(row.publications_count ?? 0),
        subscriptionsCount: Number(row.subscriptions_count ?? 0),
        updatedAt: (row.updated_at as string | undefined) ?? undefined,
      };
    });
  } catch {
    return [];
  }
}

async function listDismissedDropperShifts(ids: string[]): Promise<LocalDismissedShift[]> {
  const local = readLocalDismissedShifts().filter((item) => ids.includes(item.dropperUserId));
  if (ids.length === 0) return local;
  try {
    const { data, error } = await supabase
      .from("dismissed_dropper_shifts")
      .select("dropper_user_id,shift_key,window_start,window_end")
      .in("dropper_user_id", ids);
    if (error || !data) return local;
    const byKey = new Map<string, LocalDismissedShift>();
    for (const item of local) {
      byKey.set(`${item.dropperUserId}:${item.shiftKey}`, item);
    }
    for (const row of data) {
      const dropperUserId = String(row.dropper_user_id ?? "");
      const shiftKey = String(row.shift_key ?? "");
      if (!dropperUserId || !shiftKey) continue;
      byKey.set(`${dropperUserId}:${shiftKey}`, {
        dropperUserId,
        shiftKey,
        windowStart: (row.window_start as string | null) ?? null,
        windowEnd: (row.window_end as string | null) ?? null,
      });
    }
    return [...byKey.values()];
  } catch {
    return local;
  }
}

function shiftFingerprint(item: {
  status: string;
  publicationsCount: number;
  subscriptionsCount: number;
  shiftEnd: string | null;
  scheduleLabel: string;
}): string {
  return `${item.status}|${item.publicationsCount}|${item.subscriptionsCount}|${item.shiftEnd ?? ""}|${item.scheduleLabel}`;
}

/**
 * Only upserts shift records whose counts, status, or end time actually differ from Supabase,
 * eliminating redundant writes and Realtime feedback loops.
 */
async function persistDropperShiftEntriesIfChanged(
  entries: DropperShiftEntry[],
  storedRemote: DropperShiftRecord[],
): Promise<void> {
  const remoteByKey = new Map<string, string>();
  for (const stored of storedRemote) {
    if (!stored.modelId) continue;
    const key = `${stored.shiftId}:${stored.modelId}`;
    const fp = shiftFingerprint(stored);
    remoteByKey.set(key, fp);
    lastSavedShiftFingerprints.set(key, fp);
  }

  const changedEntries = entries.filter((entry) => {
    if (!entry.isActualShift || !entry.modelId) return false;
    const key = `${entry.shiftId}:${entry.modelId}`;
    const nextFp = shiftFingerprint(entry);
    const knownFp = remoteByKey.get(key) ?? lastSavedShiftFingerprints.get(key);
    return knownFp !== nextFp;
  });

  if (changedEntries.length === 0) return;

  const nowIso = new Date().toISOString();
  const remoteRows = changedEntries.map((entry) => ({
    shift_id: entry.shiftId,
    dropper_user_id: entry.dropperUserId,
    model_id: entry.modelId,
    shift_start: entry.shiftStart,
    shift_end: entry.shiftEnd,
    schedule_label: entry.scheduleLabel,
    status: entry.status,
    publications_count: entry.publicationsCount,
    subscriptions_count: entry.subscriptionsCount,
    updated_at: nowIso,
  }));

  try {
    const { error } = await supabase
      .from("dropper_shift_reports")
      .upsert(remoteRows, { onConflict: "shift_id,model_id" });
    if (!error) {
      for (const entry of changedEntries) {
        lastSavedShiftFingerprints.set(`${entry.shiftId}:${entry.modelId}`, shiftFingerprint(entry));
      }
    }
  } catch {
    // Ignore transient persistence errors.
  }
}

function firstRel<T>(value: T | T[] | null | undefined): T | null {
  if (Array.isArray(value)) return value[0] ?? null;
  return value ?? null;
}

async function listAssignedAccounts(ids: string[]): Promise<Map<string, AssignedAccount[]>> {
  const assigned = new Map<string, AssignedAccount[]>();
  if (ids.length === 0) return assigned;
  // Single FK-joined query instead of 2 sequential round-trips!
  const { data: links, error } = await supabase
    .from("model_assignments")
    .select("user_id,model_id,models(id,name,handle)")
    .in("user_id", ids);
  if (error) throw error;
  for (const link of links ?? []) {
    const remote = firstRel<{ id?: string; name?: string; handle?: string }>(link.models);
    const modelId = String(remote?.id || link.model_id || "");
    if (!modelId) continue;
    const userId = link.user_id as string;
    const list = assigned.get(userId) ?? [];
    list.push({
      modelId,
      name: String(remote?.name || remote?.handle || "Cuenta").trim() || "Cuenta",
    });
    assigned.set(userId, list);
  }
  return assigned;
}

async function listScheduleSlots(ids: string[]): Promise<Map<string, ScheduleSlot[]>> {
  const slots = new Map<string, ScheduleSlot[]>();
  if (ids.length === 0) return slots;
  const { data, error } = await supabase
    .from("schedules")
    .select("user_id,day_of_week,start_time,end_time")
    .in("user_id", ids);
  if (error) throw error;
  for (const row of data ?? []) {
    const id = row.user_id as string;
    const list = slots.get(id) ?? [];
    list.push({
      day: Number(row.day_of_week),
      start: minutesOf(String(row.start_time)),
      end: minutesOf(String(row.end_time)),
    });
    slots.set(id, list);
  }
  return slots;
}

async function listActiveChatters(): Promise<{ id: string; name: string }[]> {
  return listActiveByRole("chatter");
}

async function listActiveByRole(role: "chatter" | "dropper"): Promise<{ id: string; name: string }[]> {
  const { data, error } = await supabase
    .from("profiles")
    .select("id,full_name,username")
    .eq("role", role)
    .eq("is_active", true);
  if (error) throw error;
  return (data ?? []).map((row) => ({
    id: row.id as string,
    name: displayName(row.full_name as string | null, row.username as string | null),
  }));
}

async function listChatterSales(
  range: ReportRangeId,
  timeZone: string,
  onlyEmployeeId: string | null,
): Promise<SalePoint[]> {
  const minChatter = chatterMinDate(timeZone);
  const { from, to } = queryWindow(range, timeZone);
  const effectiveFrom = clampMinIso(from, minChatter);
  if (new Date(effectiveFrom).getTime() >= new Date(to).getTime()) return [];
  const pageSize = 1000;
  const rows: SalePoint[] = [];
  for (let page = 0; page < 50; page += 1) {
    let query = supabase
      .from("sales")
      .select("id,employee_id,amount_gross,amount_net,recorded_at,profiles(full_name,username,role)")
      .gte("recorded_at", effectiveFrom)
      .lt("recorded_at", to)
      .order("recorded_at", { ascending: true })
      .order("id", { ascending: true })
      .range(page * pageSize, page * pageSize + pageSize - 1);
    if (onlyEmployeeId) query = query.eq("employee_id", onlyEmployeeId);
    const { data, error } = await query;
    if (error) throw error;
    const batch = data ?? [];
    for (const row of batch) {
      if (!row.employee_id) continue;
      const profile = relation(row.profiles);
      rows.push({
        employeeId: row.employee_id as string,
        name: displayName(profile?.full_name, profile?.username),
        role: profile?.role ?? null,
        gross: Number(row.amount_gross),
        net: Number(row.amount_net),
        at: row.recorded_at as string,
      });
    }
    if (batch.length < pageSize) return rows;
  }
  throw new Error("Hay demasiadas ventas para sumarlas en una sola consulta");
}

function queryWindow(range: ReportRangeId, timeZone: string, now = new Date()): { from: string | null; to: string } {
  const today = zonedParts(now, timeZone);
  const tomorrow = addZonedDays(today, 1, timeZone).toISOString();
  if (range === "all") return { from: null, to: tomorrow };
  if (range === "today") {
    return { from: zonedDate(today.year, today.month, today.day, 0, 0, 0, timeZone).toISOString(), to: tomorrow };
  }
  if (range === "month") {
    return { from: zonedDate(today.year, today.month, 1, 0, 0, 0, timeZone).toISOString(), to: tomorrow };
  }
  if (range === "year") {
    return {
      from: zonedDate(today.year, 1, 1, 0, 0, 0, timeZone).toISOString(),
      to: zonedDate(today.year + 1, 1, 1, 0, 0, 0, timeZone).toISOString(),
    };
  }
  const start = addZonedDays(today, -89, timeZone);
  return { from: start.toISOString(), to: tomorrow };
}

function previousQueryWindow(range: ReportRangeId, timeZone: string): { from: string; to: string } | null {
  if (range === "all") return null;
  const today = zonedParts(new Date(), timeZone);
  if (range === "today") {
    const start = addZonedDays(today, -1, timeZone);
    const end = zonedDate(today.year, today.month, today.day, 0, 0, 0, timeZone);
    return { from: start.toISOString(), to: end.toISOString() };
  }
  if (range === "month") {
    const prevYear = today.month === 1 ? today.year - 1 : today.year;
    const prevMonth = today.month === 1 ? 12 : today.month - 1;
    const start = zonedDate(prevYear, prevMonth, 1, 0, 0, 0, timeZone);
    const end = zonedDate(today.year, today.month, 1, 0, 0, 0, timeZone);
    return { from: start.toISOString(), to: end.toISOString() };
  }
  if (range === "year") {
    const start = zonedDate(today.year - 1, 1, 1, 0, 0, 0, timeZone);
    const end = zonedDate(today.year, 1, 1, 0, 0, 0, timeZone);
    return { from: start.toISOString(), to: end.toISOString() };
  }
  const start = addZonedDays(today, -179, timeZone);
  const end = addZonedDays(today, -89, timeZone);
  return { from: start.toISOString(), to: end.toISOString() };
}

type BucketGranularity = "hour" | "day" | "month";

function resolveBucketGranularity(
  range: ReportRangeId,
  timestamps: string[],
  timeZone: string,
): BucketGranularity {
  if (range === "today") return "hour";
  if (range === "month") return "day";
  if (range === "days90" || range === "year") return "month";

  // For "all", pick a granularity that matches the actual span of data so charts never collapse into a single dot.
  let minMs = Number.POSITIVE_INFINITY;
  let maxMs = Number.NEGATIVE_INFINITY;
  const distinctDays = new Set<string>();
  for (const iso of timestamps) {
    const date = new Date(iso);
    const ms = date.getTime();
    if (!Number.isFinite(ms)) continue;
    if (ms < minMs) minMs = ms;
    if (ms > maxMs) maxMs = ms;
    distinctDays.add(dayKeyFromParts(zonedParts(date, timeZone)));
  }
  if (!Number.isFinite(minMs)) return "month";
  const spanHours = (maxMs - minMs) / 3_600_000;
  if (distinctDays.size <= 1 || spanHours <= 36) return "hour";
  if (spanHours <= 60 * 24) return "day";
  return "month";
}

function collectBuckets(
  range: ReportRangeId,
  saleKeys: string[],
  timeZone: string,
  granularity?: BucketGranularity,
  now = new Date(),
): Bucket[] {
  const effective: BucketGranularity =
    granularity ?? (range === "today" ? "hour" : range === "month" ? "day" : "month");
  const keys = new Set(baseKeys(range, timeZone, effective, now));
  for (const key of saleKeys) keys.add(key);
  const filled = fillBucketGaps([...keys].sort(), effective, timeZone);
  const withYear = new Set(filled.map((key) => key.slice(0, 4))).size > 1;
  return filled.map((key) => ({ key, label: labelForKey(key, withYear) }));
}

function fillBucketGaps(sorted: string[], granularity: BucketGranularity, timeZone: string): string[] {
  if (sorted.length === 0) return [];
  if (granularity === "hour") {
    const startMs = parseHourKeyMs(sorted[0], timeZone);
    const endMs = parseHourKeyMs(sorted[sorted.length - 1], timeZone);
    if (startMs === null || endMs === null) return sorted;
    const minEndMs = Math.max(endMs, startMs + 3_600_000);
    const result: string[] = [];
    const seen = new Set<string>();
    for (let ms = startMs, guard = 0; ms <= minEndMs && guard < 72; ms += 3_600_000, guard += 1) {
      const key = hourKeyFromParts(zonedParts(new Date(ms), timeZone));
      if (!seen.has(key)) {
        seen.add(key);
        result.push(key);
      }
    }
    return result;
  }
  if (granularity === "day") {
    const [sy, sm, sd] = sorted[0].split("-").map(Number);
    const [ey, em, ed] = sorted[sorted.length - 1].split("-").map(Number);
    if (!sy || !sm || !sd || !ey || !em || !ed) return sorted;
    const startParts: ZonedParts = { year: sy, month: sm, day: sd, hour: 0, minute: 0, second: 0 };
    const endKey = sorted[sorted.length - 1];
    const result: string[] = [];
    for (let offset = 0; offset < 90; offset += 1) {
      const next = zonedParts(addZonedDays(startParts, offset, timeZone), timeZone);
      const key = dayKeyFromParts(next);
      result.push(key);
      if (key >= endKey) break;
    }
    return result;
  }
  const [sy, sm] = sorted[0].split("-").map(Number);
  const [ey, em] = sorted[sorted.length - 1].split("-").map(Number);
  if (!sy || !sm || !ey || !em) return sorted;
  const result: string[] = [];
  let year = sy;
  let month = sm;
  while ((year < ey || (year === ey && month <= em)) && result.length < 48) {
    result.push(`${year}-${String(month).padStart(2, "0")}`);
    month += 1;
    if (month === 13) {
      month = 1;
      year += 1;
    }
  }
  return result;
}

function parseHourKeyMs(key: string, timeZone: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2})$/.exec(key);
  if (!match) return null;
  return zonedDate(
    Number(match[1]),
    Number(match[2]),
    Number(match[3]),
    Number(match[4]),
    0,
    0,
    timeZone,
  ).getTime();
}

function baseKeys(
  range: ReportRangeId,
  timeZone: string,
  granularity: BucketGranularity,
  now = new Date(),
): string[] {
  const today = zonedParts(now, timeZone);
  if (range === "all") return [];
  if (range === "today" || granularity === "hour") {
    const maxHour = Math.min(23, Math.max(today.hour, 1));
    const keys: string[] = [];
    for (let hour = 0; hour <= maxHour; hour += 1) {
      keys.push(hourKeyFromParts({ ...today, hour }));
    }
    return keys;
  }
  if (range === "month") {
    const keys: string[] = [];
    for (let day = 1; day <= today.day && keys.length < 40; day += 1) {
      keys.push(dayKeyFromParts({ ...today, day }));
    }
    return keys;
  }
  const keys: string[] = [];
  const startMonth = range === "year" ? 1 : zonedParts(addZonedDays(today, -89, timeZone), timeZone).month;
  const startYear = range === "year" ? today.year : zonedParts(addZonedDays(today, -89, timeZone), timeZone).year;
  let year = startYear;
  let month = startMonth;
  while ((year < today.year || (year === today.year && month <= today.month)) && keys.length < 48) {
    keys.push(`${year}-${String(month).padStart(2, "0")}`);
    month += 1;
    if (month === 13) {
      month = 1;
      year += 1;
    }
  }
  return keys;
}

function bucketKeyFor(granularity: BucketGranularity, iso: string, timeZone: string): string | null {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  const parts = zonedParts(date, timeZone);
  if (granularity === "hour") return hourKeyFromParts(parts);
  if (granularity === "day") return dayKeyFromParts(parts);
  return `${parts.year}-${String(parts.month).padStart(2, "0")}`;
}

function hourKeyFromParts(parts: ZonedParts): string {
  return `${dayKeyFromParts(parts)}T${String(parts.hour).padStart(2, "0")}`;
}

function dayKeyFromParts(parts: ZonedParts): string {
  return `${parts.year}-${String(parts.month).padStart(2, "0")}-${String(parts.day).padStart(2, "0")}`;
}

function labelForKey(key: string, withYear: boolean): string {
  const hourMatch = /^(\d{4})-(\d{2})-(\d{2})T(\d{2})$/.exec(key);
  if (hourMatch) {
    const hour = Number(hourMatch[4]);
    if (hour === 0) return "12 a.m.";
    if (hour < 12) return `${hour} a.m.`;
    if (hour === 12) return "12 p.m.";
    return `${hour - 12} p.m.`;
  }
  const [year, month, day] = key.split("-").map(Number);
  const date = new Date(year, (month || 1) - 1, day || 1);
  if (day) return new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" }).format(date);
  const name = new Intl.DateTimeFormat("en-US", { month: "short" }).format(date);
  return withYear ? `${name} ${year}` : name;
}

function displayName(fullName: string | null | undefined, username: string | null | undefined): string {
  const name = fullName?.trim();
  if (name) return name;
  const handle = username?.trim();
  return handle || "Sin nombre";
}

function relation(
  value: { full_name?: string | null; username?: string | null; role?: string | null } | { full_name?: string | null; username?: string | null; role?: string | null }[] | null,
): { full_name?: string | null; username?: string | null; role?: string | null } | null {
  if (Array.isArray(value)) return value[0] ?? null;
  return value;
}
