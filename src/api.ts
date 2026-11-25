import { invoke } from "@tauri-apps/api/core";
import type { Employee, EmployeeRole, Model, ScheduleDay } from "./types";

export type SyncReport = {
  model: Model;
  transactions_upserted: number;
};

export function errorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) {
    return error;
  }
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }
  return "No se pudo completar la operación";
}

export type TimeRange = "yesterday" | "today" | "thisWeek" | "thisMonth";
export type EarningsBasis = "net" | "gross";

export type ChannelTotal = {
  key: string;
  label: string;
  amount: number;
};

export type EarningsOverview = {
  total: number;
  channels: ChannelTotal[];
};

export type ShiftRow = {
  id: number;
  employeeName: string;
  role: string;
  startTime: string;
  endTime: string | null;
  amount: number;
};

export type ClockedInEmployee = {
  id: number;
  name: string;
  role: string;
  lastClockIn: string | null;
};

export type EmployeeSale = {
  employeeId: number;
  name: string;
  amount: number;
};

export function getEarningsOverview(range: TimeRange, basis: EarningsBasis): Promise<EarningsOverview> {
  return invoke("get_earnings_overview", { range, basis });
}

export function getMyShifts(range: TimeRange, basis: EarningsBasis): Promise<ShiftRow[]> {
  return invoke("get_my_shifts", { range, basis });
}

export function getClockedInEmployees(): Promise<ClockedInEmployee[]> {
  return invoke("get_clocked_in_employees");
}

export function getEmployeeSales(range: TimeRange, basis: EarningsBasis): Promise<EmployeeSale[]> {
  return invoke("get_employee_sales", { range, basis });
}

export type AnalyticsRange =
  | "24h"
  | "7"
  | "14"
  | "30"
  | "90"
  | "180"
  | "all"
  | `year:${number}`
  | `month:${string}`
  | string;
export type TrendBucket = "hour" | "day" | "week" | "month";
export type MetricCard = { key: string; label: string; amount: number; changePercent: number | null };
export type CreatorCard = {
  id: number;
  name: string;
  username: string;
  avatarUrl: string | null;
  currentBalance: number;
  pendingBalance: number;
  lastSyncedAt: string | null;
  total: number;
  channels: { key: string; label: string; amount: number }[];
};
export type AnalyticsReport = {
  cards: MetricCard[];
  creators: CreatorCard[];
  trends: { label: string; amount: number }[];
  channelsOverTime: {
    label: string;
    subs: number;
    tips: number;
    posts: number;
    ppv: number;
    referrals: number;
    streams: number;
  }[];
  breakdown: { key: string; label: string; amount: number; percent: number }[];
  comparisonLabels: string[];
  comparisonSeries: { id: number; name: string; values: number[] }[];
  comparisonDonuts: { id: number; name: string; slices: { key: string; label: string; amount: number; percent: number }[] }[];
};

export function getAnalytics(
  range: AnalyticsRange,
  modelIds: number[],
  compareIds: number[],
  net: boolean,
  bucket: TrendBucket,
): Promise<AnalyticsReport> {
  return invoke("get_analytics", { range, modelIds, compareIds, net, bucket });
}

export function exportAnalytics(
  range: AnalyticsRange,
  modelIds: number[],
  format: "csv" | "xlsx" | "pdf",
): Promise<{ saved: boolean; path: string | null }> {
  return invoke("export_analytics", { range, modelIds, format });
}

export function syncAllModels(): Promise<{ synced: number; skipped: number; errors: string[] }> {
  return invoke("sync_all_models");
}

export function getEmployees(): Promise<Employee[]> {
  return invoke("get_employees");
}

export function getModels(): Promise<Model[]> {
  return invoke("get_models");
}

export function addEmployee(input: {
  name: string;
  role: EmployeeRole;
  scheduleJson: { days: ScheduleDay[] };
}): Promise<Employee> {
  return invoke("add_employee", input);
}

export function updateEmployeeRoleSchedule(input: {
  id: number;
  role: EmployeeRole;
  scheduleJson: { days: ScheduleDay[] };
}): Promise<Employee> {
  return invoke("update_employee_role_schedule", input);
}

export function deleteEmployee(id: number): Promise<void> {
  return invoke("delete_employee", { id });
}

export function toggleClockIn(id: number): Promise<Employee> {
  return invoke("toggle_clock_in", { id });
}

export function openModelSession(modelId: number | null): Promise<Model> {
  return invoke("open_model_session", { modelId });
}

export function deleteModel(id: number): Promise<void> {
  return invoke("delete_model", { id });
}

export function syncModelData(id: number): Promise<SyncReport> {
  return invoke("sync_model_data", { id });
}

export type FanListItem = { name: string; checked: boolean };
export type FanLists = { fans: FanListItem[]; following: FanListItem[] };
export type FanSale = {
  ofTransactionId: string;
  kind: string;
  grossAmount: number;
  netAmount: number;
  createdAt: string;
};
export type FanInsights = {
  modelId: number;
  ofFanId: string;
  totalSpend: number;
  lastSpend: string | null;
  ppvTotal: number;
  ppvCount: number;
  tipTotal: number;
  tipCount: number;
  highestSpend: number | null;
  spendingPower: string;
  chargebackRisk: string;
  subStatus: string;
  subCost: number | null;
  subDuration: string;
  autoRenewal: string;
  spendSource: string;
  nickname: string;
  birthday: string;
  location: string;
  country: string;
  lists: FanLists;
  notes: string;
  sales: FanSale[];
};

export type MessageBarPlacement = "top" | "sidebar";

export type MessageTabsState = {
  openModelIds: number[];
  activeModelId: number | null;
  barPlacement: MessageBarPlacement;
  unreadCounts: Record<number, number>;
};

export type NotificationKind = "message" | "ppv" | "tip";

export type AppNotification = {
  id: string;
  kind: NotificationKind;
  modelId: number;
  modelName: string;
  modelUsername: string;
  modelAvatar: string | null;
  ofFanId: string;
  fanName: string;
  amount: number | null;
  netAmount: number | null;
  preview: string;
  createdAt: string;
};

export function openMessages(modelId: number): Promise<void> {
  return invoke("open_messages", { modelId });
}

export function getMessageTabs(): Promise<MessageTabsState> {
  return invoke("get_message_tabs");
}

export function setMessageBarPlacement(placement: MessageBarPlacement): Promise<MessageTabsState> {
  return invoke("set_message_bar_placement", { placement });
}

export function selectMessageTab(modelId: number): Promise<void> {
  return invoke("select_message_tab", { modelId });
}

export function closeMessageTab(modelId: number): Promise<void> {
  return invoke("close_message_tab", { modelId });
}

export function getRecentNotifications(): Promise<AppNotification[]> {
  return invoke("get_recent_notifications");
}

export function clearNotificationsHistory(): Promise<void> {
  return invoke("clear_notifications");
}

export function dismissNotificationItem(id: string): Promise<void> {
  return invoke("dismiss_notification", { id });
}

export function openIncomingChat(modelId: number, ofFanId: string): Promise<void> {
  return invoke("open_incoming_chat", { modelId, ofFanId });
}

export function setNotificationDelivery(
  messagesEnabled: boolean,
  salesEnabled: boolean,
  osPushEnabled: boolean,
): Promise<void> {
  return invoke("set_notification_delivery", { messagesEnabled, salesEnabled, osPushEnabled });
}

export function getFanInsights(modelId: number, ofFanId: string): Promise<FanInsights> {
  return invoke("get_fan_insights", { modelId, ofFanId });
}

export function updateFanProfile(input: {
  modelId: number;
  ofFanId: string;
  nickname?: string;
  birthday?: string;
  location?: string;
  country?: string;
}): Promise<void> {
  return invoke("update_fan_profile", input);
}

export function fanSelected(modelId: number, ofFanId: string): Promise<void> {
  return invoke("fan_selected", { modelId, ofFanId });
}

export type InboxChat = {
  ofFanId: string;
  username: string;
  nickname: string;
  lastBody: string | null;
  lastAt: string | null;
};

export type ScriptInfo = {
  scriptId: number;
  messageId: number;
};

export type VaultMedia = {
  id?: string;
  type?: string;
  url?: string;
  previewUrl?: string;
  isDeleted?: boolean;
  [key: string]: unknown;
};

export type ChatMessage = {
  id: number;
  body: string;
  direction: "in" | "out" | string;
  createdAt: string;
  mediaPrice: number | null;
  lockedText: boolean;
  media: VaultMedia[];
  releaseForms: unknown[];
  scriptInfos: ScriptInfo[];
};

export type ScriptRecord = {
  id: number;
  scriptName: string;
  scriptContent: string;
  scriptTextContent: string;
  mediaPrice: number | null;
  lockedText: boolean;
  media: VaultMedia[];
  releaseForms: unknown[];
  tagId: string;
};

export type ScriptPage = {
  scriptList: ScriptRecord[];
  tabList: string[];
  page: number;
  size: number;
  hasNext: boolean;
  itemsEmpty: boolean;
  mediaPriceMin: number;
  mediaPriceMax: number;
};

export type PpvPoint = {
  id: number;
  price: number | null;
  purchased: boolean;
  createdAt: string;
  messageId: number | null;
};

export type PpvTimelineState = {
  ppvData: PpvPoint[];
  ppvPrice: number | null;
  ppvAvgPrice: number | null;
  page: number;
  size: number;
  hasNext: boolean;
  ppvLoading: boolean;
  firstLoading: boolean;
  itemsEmpty: boolean;
  payCount: number;
  allPayCount: number;
  payPercent: number;
};

export type OutboundDraft = {
  body: string;
  mediaPrice: number | null;
  media: VaultMedia[];
  releaseForms: unknown[];
  lockedText: boolean;
  scriptId: number | null;
};

export function listInbox(modelId: number): Promise<InboxChat[]> {
  return invoke("list_inbox", { modelId });
}

export function listThread(modelId: number, ofFanId: string): Promise<ChatMessage[]> {
  return invoke("list_thread", { modelId, ofFanId });
}

export function listScripts(input: {
  modelId: number;
  page: number;
  text: string;
  tagId: string;
}): Promise<ScriptPage> {
  return invoke("list_scripts", input);
}

export function getPpvTimeline(modelId: number, ofFanId: string, page: number): Promise<PpvTimelineState> {
  return invoke("get_ppv_timeline", { modelId, ofFanId, page });
}

export function sendChatMessage(modelId: number, ofFanId: string, draft: OutboundDraft): Promise<ChatMessage> {
  return invoke("send_message", { modelId, ofFanId, draft });
}

export function insertChatText(modelId: number, text: string): Promise<string> {
  return invoke("insert_chat_text", { modelId, text });
}

export type VaultItem = {
  id: string;
  kind: string;
  createdAt: string;
  width: number | null;
  height: number | null;
  downloadUrl: string | null;
};

export type VaultPage = {
  items: VaultItem[];
  hasMore: boolean;
  nextOffset: number;
};

export type VaultDownloadReport = {
  directory: string;
  saved: number;
  failed: string[];
};

export function listVaultMedia(modelId: number, offset: number, kind: "all" | "photo" | "video"): Promise<VaultPage> {
  return invoke("list_vault_media", { modelId, offset, kind });
}

export function downloadVaultMedia(
  modelId: number,
  files: { id: string; kind: string; url: string }[],
): Promise<VaultDownloadReport> {
  return invoke("download_vault_media", { modelId, files });
}

export function saveFanNotes(modelId: number, ofFanId: string, notesText: string): Promise<void> {
  return invoke("save_fan_notes", { modelId, ofFanId, notesText });
}

export function updateFanLists(modelId: number, ofFanId: string, lists: FanLists): Promise<void> {
  return invoke("update_fan_lists", { modelId, ofFanId, lists });
}
