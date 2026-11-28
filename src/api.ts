import { invoke } from "@tauri-apps/api/core";
import type { Model } from "./types";

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
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) {
      return message;
    }
  }
  return "No se pudo completar la operación";
}

export type TimeRange = "yesterday" | "today" | "thisWeek" | "thisMonth";
export type EarningsBasis = "net" | "gross";

export type FanAccountCount = {
  modelId: string;
  name: string;
  fans: number | null;
};

export type FanCounts = {
  total: number | null;
  accounts: FanAccountCount[];
};

export function setSupabaseSession(
  accessToken: string | null,
  userId: string | null,
  role: string | null,
): Promise<void> {
  return invoke("set_supabase_session", { accessToken, userId, role });
}

export function getModels(): Promise<Model[]> {
  return invoke("get_models");
}

export function refreshModels(): Promise<Model[]> {
  return invoke("refresh_models");
}

export function setAppTheme(dark: boolean): Promise<void> {
  return invoke("set_app_theme", { dark });
}

export function openModelSession(modelId: string | null): Promise<Model> {
  return invoke("open_model_session", { modelId });
}

export function deleteModel(id: string): Promise<void> {
  return invoke("delete_model", { id });
}

export function syncModelData(id: string): Promise<SyncReport> {
  return invoke("sync_model_data", { id });
}

export type MessageBarPlacement = "top" | "sidebar";

export type MessageTabsState = {
  openModelIds: string[];
  activeModelId: string | null;
  barPlacement: MessageBarPlacement;
  insightsOpen: boolean;
  unreadCounts: Record<string, number>;
};

export type NotificationKind = "message" | "ppv" | "tip";

export type AppNotification = {
  id: string;
  kind: NotificationKind;
  modelId: string;
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

export function openMessages(modelId: string): Promise<void> {
  return invoke("open_messages", { modelId });
}

export function getMessageTabs(): Promise<MessageTabsState> {
  return invoke("get_message_tabs");
}

export function setMessageBarPlacement(placement: MessageBarPlacement): Promise<MessageTabsState> {
  return invoke("set_message_bar_placement", { placement });
}

export function setInsightsOpen(open: boolean): Promise<MessageTabsState> {
  return invoke("set_insights_open", { open });
}

export function selectMessageTab(modelId: string): Promise<void> {
  return invoke("select_message_tab", { modelId });
}

export function closeMessageTab(modelId: string): Promise<void> {
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

export function openIncomingChat(modelId: string, ofFanId: string): Promise<void> {
  return invoke("open_incoming_chat", { modelId, ofFanId });
}

export function setNotificationDelivery(
  messagesEnabled: boolean,
  salesEnabled: boolean,
  osPushEnabled: boolean,
): Promise<void> {
  return invoke("set_notification_delivery", { messagesEnabled, salesEnabled, osPushEnabled });
}

export function fanSelected(modelId: string, ofFanId: string): Promise<void> {
  return invoke("fan_selected", { modelId, ofFanId });
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

export function listVaultMedia(modelId: string, offset: number, kind: "all" | "photo" | "video"): Promise<VaultPage> {
  return invoke("list_vault_media", { modelId, offset, kind });
}

export function downloadVaultMedia(
  modelId: string,
  files: { id: string; kind: string; url: string }[],
): Promise<VaultDownloadReport> {
  return invoke("download_vault_media", { modelId, files });
}
